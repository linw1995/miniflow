mod common;
use mf_compiler::{SupportPackages, resolve_project, write_dependency_project};
use std::fs;

#[test]
fn reuses_authoritative_lock_and_rejects_missing_or_incompatible_locked_inputs() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let lock = root.path().join("flow.lock");
    let plan = common::fixture_plan();
    write_dependency_project(
        &project,
        &plan,
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    assert!(resolve_project(&project, &lock, true).is_err());
    let metadata = resolve_project(&project, &lock, false).unwrap();
    assert!(
        metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "fixture-multi-nodes")
    );
    mf_compiler::validate_runtime_identity(&metadata).unwrap();
    fs::copy(project.join("Cargo.lock"), &lock).unwrap();
    let original = fs::read(&lock).unwrap();
    fs::write(project.join("Cargo.lock"), "stale invalid working lock").unwrap();
    resolve_project(&project, &lock, true).unwrap();
    assert_eq!(fs::read(&lock).unwrap(), original);
    assert_eq!(fs::read(project.join("Cargo.lock")).unwrap(), original);
    let manifest = project.join("Cargo.toml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)
            .unwrap()
            .replace("version = \"0.1.0\"", "version = \"0.2.0\""),
    )
    .unwrap();
    assert!(resolve_project(&project, &lock, true).is_err());
    assert_eq!(fs::read(&lock).unwrap(), original);
    resolve_project(&project, &lock, false).unwrap();
    assert_ne!(fs::read(project.join("Cargo.lock")).unwrap(), original);
    assert_eq!(fs::read(&lock).unwrap(), original);
}
