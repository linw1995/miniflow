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

const SUPPORT_PACKAGES: [(&str, &str); 6] = [
    ("mf-telemetry", "crates/mf-telemetry"),
    ("mf-runtime-derive", "crates/mf-runtime-derive"),
    ("mf-runtime", "crates/mf-runtime"),
    ("mfn-core", "crates/builtin-nodes/core"),
    ("mfn-code", "crates/builtin-nodes/code"),
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

fn unpack_package(archive: &Path, vendor: &Path) -> PathBuf {
    tar::Archive::new(GzDecoder::new(File::open(archive).unwrap()))
        .unpack(vendor)
        .unwrap();
    let directory = vendor.join(archive.file_stem().unwrap());
    let mut files = BTreeMap::new();
    file_checksums(&directory, &directory, &mut files);
    fs::write(
        directory.join(".cargo-checksum.json"),
        serde_json::to_vec(&json!({"files": files, "package": checksum(archive)})).unwrap(),
    )
    .unwrap();
    directory
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
        let target = root.join("setup-target");
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
        if let Some(seed) = env::var_os("MF_TEST_TARGET_DIR") {
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
            unpack_package(
                &target
                    .join("package")
                    .join(format!("{name}-{version}.crate")),
                &vendor,
            );
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

        let fixture_manifest =
            workspace.join("crates/mf-compiler/tests/fixtures/multi-nodes/Cargo.toml");
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
                    .arg(&fixture_manifest),
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
        let fixture_source = unpack_package(
            &target
                .join("package")
                .join(format!("fixture-multi-nodes-{fixture_version}.crate")),
            &vendor,
        );
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
            .env("MF_TEST_TARGET_DIR", self.root().join("setup-target"));
        if locked {
            command.arg("--locked");
        }
        checked(&mut command)
    }
}
