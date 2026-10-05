mod common;
use mf_compiler::{SupportPackages, write_dependency_project};

#[test]
fn generates_one_target_with_safe_aliases_and_feature_selections() {
    let plan = common::fixture_plan();
    let directory = tempfile::tempdir().unwrap();
    write_dependency_project(
        directory.path(),
        &plan,
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let manifest = std::fs::read_to_string(directory.path().join("Cargo.toml")).unwrap();
    assert!(manifest.contains("node_0 = { package = \"fixture-multi-nodes\""));
    assert!(manifest.contains("features = [\"double\"]"));
    assert!(manifest.contains("default-features = false"));
    assert!(!manifest.contains("mf-bundle"));
    let main = std::fs::read_to_string(directory.path().join("src/main.rs")).unwrap();
    assert!(main.starts_with("extern crate node_0 as _;"));
    assert!(main.contains("mf_runtime::RunnerCommand::Validate"));
    assert!(directory.path().join("src/main.rs").is_file());
    assert!(!directory.path().join("src/bin").exists());
}

#[cfg(unix)]
#[test]
fn rejects_linked_project_directories_before_writing_outside_the_build() {
    use std::{fs, os::unix::fs::symlink};
    for name in ["src", "target"] {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("build");
        let outside = root.path().join("inputs");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&outside).unwrap();
        let plan = common::fixture_plan();
        let original = serde_json::to_string(&plan.definition).unwrap();
        fs::write(outside.join("workflow.rs"), &original).unwrap();
        symlink(&outside, project.join(name)).unwrap();
        assert!(
            write_dependency_project(
                &project,
                &plan,
                &SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                }
            )
            .is_err(),
            "linked {name} was accepted"
        );
        assert_eq!(
            fs::read_to_string(outside.join("workflow.rs")).unwrap(),
            original
        );
        assert!(!project.join("Cargo.toml").exists());
    }
}
