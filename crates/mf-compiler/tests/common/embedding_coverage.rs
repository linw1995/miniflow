//! Retain the exact executables and source aliases used by embedding fixtures.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

pub fn preserve(root: &Path, output: &Output, executable: &Path, fixture: &str, main_source: &str) {
    let Some(coverage) = std::env::var_os("MF_COVERAGE_BINARIES_DIR") else {
        return;
    };
    assert_success(output);
    let coverage = Path::new(&coverage);
    let name = format!("mf-{fixture}");
    let build_script = root.join("build.rs").canonicalize().unwrap();
    // Cargo identifies the exact build-script executable, including fresh artifacts.
    // Scanning the shared target directory could retain a stale script instead.
    let artifacts: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact"
                && message["target"]["kind"] == json!(["custom-build"])
                && message["target"]["src_path"]
                    .as_str()
                    .is_some_and(|source| root.join(source).canonicalize().unwrap() == build_script)
        })
        .collect();
    assert_eq!(artifacts.len(), 1, "missing host build-script artifact");
    let artifact = &artifacts[0];
    let script_binary = artifact["executable"].as_str().unwrap_or_else(|| {
        let filenames = artifact["filenames"].as_array().unwrap();
        assert_eq!(filenames.len(), 1, "ambiguous build-script artifact");
        filenames[0].as_str().unwrap()
    });
    let script_binary = Path::new(script_binary);
    let script_directory = script_binary.parent().unwrap();
    let launcher = script_directory.join(".kache-launch");
    // Ignore a stale record if this artifact is no longer a Kache launcher.
    let launcher_marker = b"KACHE_BUILD_SCRIPT_PATH=";
    let is_launcher = launcher.is_file()
        && fs::read(script_binary)
            .unwrap()
            .windows(launcher_marker.len())
            .any(|window| window == launcher_marker);
    let script_binary = if is_launcher {
        // Kache publishes a launcher at Cargo's artifact path. Its NUL-delimited
        // record names the preserved Rust executable that owns the coverage maps.
        let record = fs::read(launcher).unwrap();
        let preserved = record.split(|byte| *byte == 0).next().unwrap();
        script_directory.join(std::str::from_utf8(preserved).unwrap())
    } else {
        script_binary.to_owned()
    };
    let retained_script = coverage.join(format!("{name}-build-script"));
    fs::copy(script_binary, &retained_script).unwrap();
    fs::copy(executable, coverage.join(&name)).unwrap();

    let repository = super::crates_dir().parent().unwrap().to_owned();
    let fixture_directory = format!("crates/mf-compiler/tests/fixtures/{fixture}");
    let mappings = [
        ("src/main.rs", format!("{fixture_directory}/{main_source}")),
        ("build.rs", format!("{fixture_directory}/build.rs")),
    ]
    .map(|(source, fixture)| {
        let source = root.join(source).canonicalize().unwrap();
        // Remaps are only valid when the compiled and tracked sources share lines.
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(repository.join(&fixture)).unwrap()
        );
        (source.to_str().unwrap().to_owned(), fixture)
    });
    let mut mappings = BTreeMap::from(mappings);
    // A reused script can still name its original, now-deleted source root.
    // Ask its coverage map rather than guessing a cache-wide source alias.
    let output = Command::new("rustc")
        .args(["--print", "target-libdir"])
        .output()
        .unwrap();
    assert_success(&output);
    let libdir = String::from_utf8(output.stdout).unwrap();
    let llvm_cov = Path::new(libdir.trim())
        .parent()
        .unwrap()
        .join("bin")
        .join(format!("llvm-cov{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new(llvm_cov)
        .args(["export", "--empty-profile", "--summary-only"])
        .arg(retained_script)
        .output()
        .unwrap();
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let sources: Vec<&str> = report["data"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|data| data["files"].as_array().unwrap())
        .filter_map(|file| file["filename"].as_str())
        .filter(|source| {
            Path::new(source)
                .file_name()
                .is_some_and(|name| name == "build.rs")
        })
        .collect();
    assert_eq!(sources.len(), 1, "ambiguous build-script coverage source");
    mappings.insert(
        sources[0].to_owned(),
        format!("{fixture_directory}/build.rs"),
    );
    fs::write(
        coverage.join(format!("{name}.paths.json")),
        serde_json::to_vec(&mappings).unwrap(),
    )
    .unwrap();
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
