mod common;
use mf_compiler::{SupportPackages, dependency_project_files, write_dependency_project};

#[test]
fn generates_one_target_with_safe_aliases_and_feature_selections() {
    let plan = common::fixture_plan();
    let files = dependency_project_files(
        &plan.definition,
        &plan.generate_artifacts().unwrap(),
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let manifest = &files[std::path::Path::new("Cargo.toml")];
    assert!(manifest.contains("node_0 = { package = \"fixture-multi-nodes\""));
    assert!(manifest.contains("features = [\"double\"]"));
    assert!(manifest.contains("default-features = false"));
    assert!(!manifest.contains("mf-bundle"));
    let main = &files[std::path::Path::new("src/main.rs")];
    assert!(main.starts_with("extern crate node_0 as _;"));
    assert!(main.contains("--validate"));
    let directory = common::Directory::new();
    write_dependency_project(&directory.0, &files).unwrap();
    assert!(directory.0.join("src/main.rs").is_file());
    assert!(!directory.0.join("src/bin").exists());
}

#[test]
fn rejects_incomplete_artifacts_before_mutating_the_project() {
    let root = common::Directory::new();
    let plan = common::fixture_plan();
    let mut files = dependency_project_files(
        &plan.definition,
        &plan.generate_artifacts().unwrap(),
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    write_dependency_project(&root.0, &files).unwrap();
    let original = std::fs::read(root.0.join("src/main.rs")).unwrap();
    files.remove(std::path::Path::new("src/main.rs"));
    files.insert("src/workflow.rs".into(), "incomplete attempt".into());
    assert!(write_dependency_project(&root.0, &files).is_err());
    assert_eq!(std::fs::read(root.0.join("src/main.rs")).unwrap(), original);
}
