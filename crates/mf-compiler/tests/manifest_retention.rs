#![cfg(all(feature = "codegen", any(target_os = "linux", target_os = "macos")))]
mod common;

use mf_compiler::{
    RunnerOptions, SupportPackages, plan_definition, resolve_project,
    write_dependency_project_with_options,
};
use mf_runtime::{
    MANIFEST_HEADER_BYTES, MANIFEST_MAGIC, MAX_MANIFEST_PAYLOAD_BYTES, WorkflowInterface,
    WorkflowManifest,
};
use mf_telemetry::description::WorkflowDescription;
use std::{fs, path::Path, process::Command};

fn retained_manifest(runner: &Path) -> WorkflowManifest {
    #[cfg(target_os = "macos")]
    let (tool, arguments, section) = ("otool", vec!["-l"], "sectname __mf_manifest");
    #[cfg(target_os = "linux")]
    let (tool, arguments, section) = ("readelf", vec!["-SW"], ".mf_manifest");
    let output = Command::new(tool)
        .args(arguments)
        .arg(runner)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches(section)
            .count(),
        1
    );
    let bytes = fs::read(runner).unwrap();
    // Inspection tests also verify framing; the container reader is exercised in the TUI suite.
    let records: Vec<_> = bytes
        .windows(MANIFEST_MAGIC.len())
        .enumerate()
        .filter(|(_, bytes)| *bytes == MANIFEST_MAGIC)
        .filter_map(|(offset, _)| {
            let header = bytes.get(offset..offset + MANIFEST_HEADER_BYTES)?;
            let length = u64::from_le_bytes(header[12..20].try_into().ok()?);
            if length > MAX_MANIFEST_PAYLOAD_BYTES as u64 {
                return None;
            }
            let end = offset
                .checked_add(MANIFEST_HEADER_BYTES)?
                .checked_add(length as usize)?;
            WorkflowManifest::from_bytes(bytes.get(offset..end)?).ok()
        })
        .collect();
    assert_eq!(records.len(), 1);
    records.into_iter().next().unwrap()
}

#[test]
fn release_lto_and_strip_preserve_standalone_factory_free_manifest() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let mut definition = common::fixture_definition();
    definition.nodes[0].config = serde_json::json!({"print":true});
    let plan = plan_definition(&definition).unwrap();
    for telemetry in [false, true] {
        write_dependency_project_with_options(
            &project,
            &plan,
            &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
            &RunnerOptions { telemetry },
        )
        .unwrap();
        let manifest_path = project.join("Cargo.toml");
        let mut cargo_manifest = fs::read_to_string(&manifest_path).unwrap();
        cargo_manifest.push_str("\n[profile.release]\nlto = true\ncodegen-units = 1\n");
        fs::write(manifest_path, cargo_manifest).unwrap();
        resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let output = mf_compiler::cargo_command(&project)
            .args(["build", "--offline", "--release", "--locked"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let runner = root.path().join("standalone");
        fs::copy(common::runner_executable(&project, "release"), &runner).unwrap();
        let before = retained_manifest(&runner);
        let output = Command::new("strip").arg(&runner).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(retained_manifest(&runner), before);
        for flag in ["--describe", "--describe-interface"] {
            let output = Command::new(&runner)
                .arg(flag)
                .env("PATH", "")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            if flag == "--describe" {
                assert_eq!(
                    WorkflowDescription::from_json(&output.stdout).unwrap(),
                    before.description
                );
            } else {
                assert_eq!(
                    WorkflowInterface::from_json(&output.stdout).unwrap(),
                    before.interface
                );
            }
        }
    }
}
