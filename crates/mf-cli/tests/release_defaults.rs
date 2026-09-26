#![cfg(not(feature = "development-support"))]

use std::{fs, path::Path, process::Command};

#[test]
fn default_cli_ignores_development_override_and_never_discovers_checkout_sources() {
    let root = std::env::temp_dir().join(format!(
        "mf-release-default-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let definition = root.join("flow.json");
    fs::write(
        &definition,
        r#"{"version":"2026-09-26","dependencies":{},"nodes":[]}"#,
    )
    .unwrap();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mf"))
        .current_dir(crates.parent().unwrap())
        .arg("compile")
        .arg(&definition)
        .arg("--output")
        .arg(root.join("flow"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .env("MF_DEV_SUPPORT_ROOT", crates)
        .env("CARGO", root.join("missing-cargo"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    let manifest = fs::read_to_string(root.join("build/Cargo.toml")).unwrap();
    for name in ["mf-runtime", "mf-compiler"] {
        assert!(manifest.contains(&format!(
            "{name} = {{ version = \"={}\" }}",
            env!("CARGO_PKG_VERSION")
        )));
    }
    assert!(!manifest.contains("path ="));
    assert!(!manifest.contains("mf-bundle"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("dependency resolution"));
    fs::remove_dir_all(root).unwrap();
}
