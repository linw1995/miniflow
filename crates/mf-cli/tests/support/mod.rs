use flate2::read::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const SUPPORT_PACKAGES: [(&str, &str); 7] = [
    ("mf-telemetry", "crates/mf-telemetry"),
    ("mf-runtime-derive", "crates/mf-runtime-derive"),
    ("mf-runtime", "crates/mf-runtime"),
    ("mfn-core", "crates/builtin-nodes/core"),
    ("mfn-code", "crates/builtin-nodes/code"),
    ("mfn-llm", "crates/builtin-nodes/llm"),
    ("mf-compiler", "crates/mf-compiler"),
];

pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

pub fn checked(command: &mut Command) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("could not start {command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?} failed with {}:\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn cargo_binary() -> OsString {
    // Fixture builds must use their own target instead of the nextest wrapper's shared target.
    env::var_os("MF_TEST_REAL_CARGO")
        .or_else(|| env::var_os("CARGO"))
        .unwrap_or_else(|| "cargo".into())
}

fn checksum(path: &Path) -> String {
    Sha256::digest(fs::read(path).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn file_checksums(root: &Path, directory: &Path, files: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            file_checksums(root, &path, files);
        } else if entry.file_name() != ".cargo-checksum.json" {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            files.insert(relative, checksum(&path));
        }
    }
}

fn unpack_package(archive: &Path, vendor: &Path) -> (PathBuf, String) {
    let directory = vendor.join(archive.file_stem().unwrap());
    if directory.exists() {
        fs::remove_dir_all(&directory).unwrap();
    }
    tar::Archive::new(GzDecoder::new(File::open(archive).unwrap()))
        .unpack(vendor)
        .unwrap();
    let mut files = BTreeMap::new();
    file_checksums(&directory, &directory, &mut files);
    fs::write(
        directory.join(".cargo-checksum.json"),
        serde_json::to_vec(&json!({"files": &files, "package": checksum(archive)})).unwrap(),
    )
    .unwrap();
    // Cargo adds the checkout revision even when all build inputs are unchanged.
    // Our support crates do not consume this generated metadata.
    files.remove(".cargo_vcs_info.json");
    // Dependency builds use the consumer's lockfile. This packaged lockfile
    // also changes when only transitive package revision metadata changes.
    files.remove("Cargo.lock");
    let source_checksum = Sha256::digest(serde_json::to_vec(&files).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (directory, source_checksum)
}

#[test]
fn packaged_source_identity_ignores_revision_but_tracks_build_inputs() {
    use flate2::{Compression, write::GzEncoder};

    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("fixture-0.1.0.crate");
    let vendor = root.path().join("vendor");
    fs::create_dir(&vendor).unwrap();
    let write_archive = |revision: &str, source: &str, manifest: &str| {
        let encoder = GzEncoder::new(File::create(&archive).unwrap(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let vcs = json!({"git": {"sha1": revision}}).to_string();
        let lock = format!("version = 4\n# dependency checksum: {revision}\n");
        for (name, contents) in [
            ("src/lib.rs", source),
            ("Cargo.toml", manifest),
            ("Cargo.lock", lock.as_str()),
            (".cargo_vcs_info.json", vcs.as_str()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("fixture-0.1.0/{name}"),
                    contents.as_bytes(),
                )
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    };
    write_archive("first", "pub fn value() -> u32 { 1 }", "original manifest");
    let archive_checksum = checksum(&archive);
    let (directory, first) = unpack_package(&archive, &vendor);
    fs::write(directory.join("obsolete.rs"), "obsolete input").unwrap();
    write_archive("second", "pub fn value() -> u32 { 1 }", "original manifest");
    assert_ne!(checksum(&archive), archive_checksum);
    let (_, second) = unpack_package(&archive, &vendor);
    assert_eq!(first, second);
    assert!(!directory.join("obsolete.rs").exists());
    let checksums: Value =
        serde_json::from_slice(&fs::read(directory.join(".cargo-checksum.json")).unwrap()).unwrap();
    assert_eq!(checksums["package"], checksum(&archive));
    assert_eq!(
        checksums["files"][".cargo_vcs_info.json"],
        checksum(&directory.join(".cargo_vcs_info.json"))
    );
    assert_eq!(
        checksums["files"]["Cargo.lock"],
        checksum(&directory.join("Cargo.lock"))
    );
    write_archive("second", "pub fn value() -> u32 { 2 }", "original manifest");
    assert_ne!(unpack_package(&archive, &vendor).1, second);
    write_archive("second", "pub fn value() -> u32 { 1 }", "changed manifest");
    assert_ne!(unpack_package(&archive, &vendor).1, second);
}

pub fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

pub struct PackagedCli {
    directory: TempDir,
    cli: PathBuf,
    cargo_home: PathBuf,
    target: PathBuf,
    pub fixture_source: PathBuf,
    pub fixture_version: String,
}

impl PackagedCli {
    pub fn setup() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("mf packaged cli ")
            .tempdir()
            .unwrap();
        let root = directory.path();
        let workspace = workspace();
        let target = env::var_os("MF_TEST_PACKAGE_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("setup-target"));
        let vendor = env::var_os("MF_TEST_VENDOR_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("vendor"));
        let config = root.join("source.toml");
        let real_cargo = cargo_binary();
        let cargo = || {
            let mut command = Command::new(&real_cargo);
            command
                .current_dir(&workspace)
                .env("CARGO_TARGET_DIR", &target);
            command
        };

        // A filtered test run may not have fetched dependencies for other targets.
        if env::var_os("MF_TEST_VENDOR_DIR").is_none() {
            checked(cargo().args(["fetch", "--locked"]));
            checked(
                cargo()
                    .args([
                        "vendor",
                        "--offline",
                        "--locked",
                        "--respect-source-config",
                        "--versioned-dirs",
                    ])
                    .arg(&vendor),
            );
        }
        if !target.join("release").exists()
            && let Some(seed) = env::var_os("MF_TEST_TARGET_DIR")
        {
            let seed = PathBuf::from(seed).join("release");
            // Copy dependency artifacts while keeping fixture writes and executables isolated.
            for name in ["deps", ".fingerprint", "build"] {
                copy_directory(&seed.join(name), &target.join("release").join(name));
            }
        }
        fs::write(&config, format!(
            "[source.crates-io]\nreplace-with = \"mf-package-fixture\"\n[source.mf-package-fixture]\ndirectory = {}\n",
            serde_json::to_string(&vendor).unwrap(),
        )).unwrap();

        let version = env!("CARGO_PKG_VERSION");
        let mut packages = BTreeMap::new();
        for (name, path) in SUPPORT_PACKAGES {
            checked(
                cargo()
                    .args([
                        "package",
                        "--offline",
                        "--allow-dirty",
                        "--no-verify",
                        "--manifest-path",
                    ])
                    .arg(workspace.join(path).join("Cargo.toml"))
                    .arg("--config")
                    .arg(&config),
            );
            let archive = target
                .join("package")
                .join(format!("{name}-{version}.crate"));
            let (_, source_checksum) = unpack_package(&archive, &vendor);
            packages.insert(name.to_owned(), (version.to_owned(), source_checksum));
        }
        let fixture_original = workspace.join("crates/mf-compiler/tests/fixtures/multi-nodes");
        let fixture_project = root.join("fixture-package");
        fs::create_dir(&fixture_project).unwrap();
        copy_directory(&fixture_original.join("src"), &fixture_project.join("src"));
        let fixture_manifest = fixture_project.join("Cargo.toml");
        let fixture_text = fs::read_to_string(fixture_original.join("Cargo.toml"))
            .unwrap()
            .replace(
                "mf-runtime = { path = \"../../../../mf-runtime\", version = \"0.1.0\" }",
                &format!("mf-runtime = {{ version = \"={version}\" }}"),
            );
        fs::write(&fixture_manifest, fixture_text).unwrap();
        // A standalone fixture lock may contain versions absent from the exact vendor graph.
        fs::copy(
            workspace.join("Cargo.lock"),
            fixture_project.join("Cargo.lock"),
        )
        .unwrap();
        let metadata: Value = serde_json::from_slice(
            &checked(
                cargo()
                    .args([
                        "metadata",
                        "--offline",
                        "--no-deps",
                        "--format-version",
                        "1",
                        "--manifest-path",
                    ])
                    .arg(&fixture_manifest)
                    .arg("--config")
                    .arg(&config),
            )
            .stdout,
        )
        .unwrap();
        let fixture_version = metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "fixture-multi-nodes")
            .unwrap()["version"]
            .as_str()
            .unwrap()
            .to_owned();
        checked(
            cargo()
                .args([
                    "package",
                    "--offline",
                    "--allow-dirty",
                    "--no-verify",
                    "--manifest-path",
                ])
                .arg(&fixture_manifest)
                .arg("--config")
                .arg(&config),
        );
        let fixture_archive = target
            .join("package")
            .join(format!("fixture-multi-nodes-{fixture_version}.crate"));
        let (fixture_source, source_checksum) = unpack_package(&fixture_archive, &vendor);
        packages.insert(
            "fixture-multi-nodes".to_owned(),
            (fixture_version.clone(), source_checksum),
        );
        let package_marker = target.join("mf-packaged-sources.json");
        let package_identity = serde_json::to_vec(&packages).unwrap();
        if fs::read(&package_marker).ok().as_deref() != Some(package_identity.as_slice()) {
            // Workspace cleanup only knows path packages. Resolve the registry
            // identities too, so changed build inputs cannot reuse same-version code.
            let cleanup = root.join("package-cleanup");
            fs::create_dir_all(cleanup.join("src")).unwrap();
            fs::write(cleanup.join("src/lib.rs"), "").unwrap();
            let mut manifest = String::from(
                "[package]\nname = \"mf-package-cleanup\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n",
            );
            for (name, (version, _)) in &packages {
                manifest.push_str(&format!(
                    "{name} = {{ version = \"={version}\", default-features = false }}\n"
                ));
            }
            fs::write(cleanup.join("Cargo.toml"), manifest).unwrap();
            let mut clean = cargo();
            clean
                .args([
                    "clean",
                    "--offline",
                    "--profile",
                    "release",
                    "--manifest-path",
                ])
                .arg(cleanup.join("Cargo.toml"))
                .arg("--config")
                .arg(&config);
            for (name, (version, _)) in &packages {
                clean.args(["-p", &format!("{name}@{version}")]);
            }
            checked(&mut clean);
            fs::write(package_marker, package_identity).unwrap();
        }
        checked(
            cargo()
                .args([
                    "build",
                    "--release",
                    "--locked",
                    "--offline",
                    "-p",
                    "mf-cli",
                    "--no-default-features",
                ])
                .arg("--config")
                .arg(&config),
        );
        let cli = root
            .join("installed")
            .join(format!("mf{}", env::consts::EXE_SUFFIX));
        fs::create_dir_all(cli.parent().unwrap()).unwrap();
        fs::copy(target.join("release").join(cli.file_name().unwrap()), &cli).unwrap();

        let cargo_home = root.join("cargo-home");
        fs::create_dir(&cargo_home).unwrap();
        fs::copy(&config, cargo_home.join("config.toml")).unwrap();
        #[cfg(unix)]
        {
            let path = root.join("cargo-wrapper");
            fs::copy(workspace.join("scripts/nextest-cargo.sh"), &path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            directory,
            cli,
            cargo_home,
            target,
            fixture_source,
            fixture_version,
        }
    }

    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    pub fn compile(
        &self,
        definition: &Path,
        output: &Path,
        build: &Path,
        locked: bool,
        fetch_local_git: bool,
    ) -> Output {
        let mut command = Command::new(&self.cli);
        command
            .current_dir(self.cli.parent().unwrap())
            .arg("compile")
            .arg(definition)
            .arg("--output")
            .arg(output)
            .arg("--build-dir")
            .arg(build)
            .env("CARGO_HOME", &self.cargo_home)
            .env_remove("MF_TEST_SOURCE_CONFIG")
            .env(
                "MF_DEV_SUPPORT_ROOT",
                self.root().join("unavailable-checkout"),
            )
            .env(
                "CARGO_NET_OFFLINE",
                if fetch_local_git { "false" } else { "true" },
            );
        #[cfg(unix)]
        // The compiler sets a per-project target; the test wrapper redirects it to the shared fixture target.
        command
            .env("CARGO", self.root().join("cargo-wrapper"))
            .env("MF_TEST_REAL_CARGO", cargo_binary())
            .env("MF_TEST_TARGET_DIR", &self.target);
        if locked {
            command.arg("--locked");
        }
        checked(&mut command)
    }
}
