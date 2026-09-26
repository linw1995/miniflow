mod common;

use mf_compiler::{SupportPackages, write_dependency_project};
use std::fs;

#[test]
fn generated_runner_has_no_ui_and_only_opt_in_export_dependencies() {
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
    let dependency_names = || {
        let output = mf_compiler::cargo_command(&project)
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
            .collect::<std::collections::BTreeSet<_>>()
    };
    let minimal = dependency_names();
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
    let manifest = project.join("Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    let path = serde_json::to_string(&common::crates_dir().join("mf-telemetry")).unwrap();
    fs::write(
        &manifest,
        format!("{source}\nmf-telemetry = {{ path = {path}, features = [\"otlp\"] }}\n"),
    )
    .unwrap();
    let exporting = dependency_names();
    assert!(exporting.contains("opentelemetry_sdk"));
    assert!(exporting.contains("opentelemetry-otlp"));
    for name in [
        "mf-tui",
        "ratatui",
        "ratatui-core",
        "ratatui-crossterm",
        "crossterm",
        "termion",
    ] {
        assert!(
            !minimal.contains(name) && !exporting.contains(name),
            "runner contains {name}"
        );
    }
}
