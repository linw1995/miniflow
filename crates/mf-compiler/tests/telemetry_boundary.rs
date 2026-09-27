mod common;

use mf_compiler::{SupportPackages, write_dependency_project};
use std::{collections::BTreeSet, fs, path::Path};

fn dependency_names(project: &Path) -> BTreeSet<String> {
    let output = mf_compiler::cargo_command(project)
        .args([
            "tree",
            "--offline",
            "--edges",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split_whitespace().next().unwrap().to_owned())
        .collect()
}

#[test]
fn generated_runner_exports_otel_without_terminal_dependencies() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("runner");
    write_dependency_project(
        &project,
        &common::fixture_plan(),
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let minimal_project = root.path().join("minimal");
    fs::create_dir_all(minimal_project.join("src")).unwrap();
    fs::write(minimal_project.join("src/lib.rs"), "pub fn minimal() {}\n").unwrap();
    let path = serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap();
    fs::write(minimal_project.join("Cargo.toml"), format!(
        "[package]\nname = \"mf-minimal-runtime-check\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nmf-runtime = {{ path = {path} }}\n"
    )).unwrap();
    let minimal = dependency_names(&minimal_project);
    assert!(minimal.contains("mf-telemetry"));
    assert!(minimal.contains("opentelemetry"));
    for name in [
        "opentelemetry_sdk",
        "opentelemetry-otlp",
        "reqwest",
        "tokio",
    ] {
        assert!(!minimal.contains(name), "minimal runner contains {name}");
    }
    let exporting = dependency_names(&project);
    assert!(exporting.contains("opentelemetry_sdk"));
    assert!(exporting.contains("opentelemetry-otlp"));
    assert!(exporting.contains("reqwest"));
    for name in [
        "mf-tui",
        "ratatui",
        "ratatui-core",
        "ratatui-crossterm",
        "crossterm",
        "termion",
        "process-wrap",
    ] {
        assert!(
            !minimal.contains(name) && !exporting.contains(name),
            "runner contains {name}"
        );
    }
}
