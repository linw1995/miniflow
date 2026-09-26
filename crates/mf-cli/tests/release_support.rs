#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

struct ReleaseProbe {
    directory: TempDir,
}

impl ReleaseProbe {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("mf release probe ")
            .tempdir()
            .unwrap();
        let root = directory.path();
        fs::create_dir(root.join("tmp")).unwrap();
        fs::write(
            root.join("workspace.json"),
            json!({"packages":[{"name":"mf-cli","version":"0.1.0"}]}).to_string(),
        )
        .unwrap();
        let packages = ["mf-runtime", "mf-compiler", "mfn-constant", "mfn-identity"]
            .map(|name| json!({"name":name,"version":"0.1.0","source":"registry+fixture"}));
        fs::write(
            root.join("resolved.json"),
            json!({"packages": packages}).to_string(),
        )
        .unwrap();
        fs::write(root.join("owners"), "maintainer (Name)\n").unwrap();
        fs::write(root.join("calls"), "").unwrap();
        let bash = Command::new("bash")
            .args(["-c", "command -v bash"])
            .output()
            .unwrap();
        assert!(bash.status.success());
        let interpreter = String::from_utf8(bash.stdout).unwrap();
        let stub = format!(
            "#!{}\n{}",
            interpreter.trim(),
            r#"
set -euo pipefail
printf '%s\n' "$1" >> "$MF_PROBE_ROOT/calls"
case "$1" in
  owner)
    if [[ "${MF_FAIL_STAGE:-}" == owner ]]; then
      printf 'package not found\n' >&2
      exit 101
    fi
    cat "$MF_PROBE_ROOT/owners"
    ;;
  metadata)
    if [[ "${2:-}" == --no-deps ]]; then
      cat "$MF_PROBE_ROOT/workspace.json"
    elif [[ "${MF_FAIL_STAGE:-}" == resolution ]]; then
      printf 'package not found\n' >&2
      exit 101
    else
      cat "$MF_PROBE_ROOT/resolved.json"
    fi
    ;;
  *) printf 'unexpected Cargo command\n' >&2; exit 97 ;;
esac
"#
        );
        fs::write(root.join("cargo"), stub).unwrap();
        fs::set_permissions(root.join("cargo"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { directory }
    }

    fn command(&self) -> Command {
        let root = self.directory.path();
        let mut command = Command::new("bash");
        command
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../scripts/check-release-support.sh"),
            )
            .current_dir(root)
            .env("CARGO", root.join("cargo"))
            .env("TMPDIR", root.join("tmp"))
            .env("MF_PROBE_ROOT", root);
        command
    }

    fn run(&self, owner: &str) -> Output {
        self.command().args(["--owner", owner]).output().unwrap()
    }

    fn update_resolution(&self, update: impl FnOnce(&mut Value)) {
        let path = self.directory.path().join("resolved.json");
        let mut value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        update(&mut value);
        fs::write(path, value.to_string()).unwrap();
    }

    fn assert_cleaned_up(&self) {
        assert_eq!(
            fs::read_dir(self.directory.path().join("tmp"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn accepts_owned_available_versions_without_publishing() {
    let probe = ReleaseProbe::new();
    let output = probe.run("maintainer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("CLI 0.1.0"));
    let calls = fs::read_to_string(probe.directory.path().join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|line| *line == "owner").count(), 4);
    assert_eq!(calls.lines().filter(|line| *line == "metadata").count(), 2);
    assert!(
        calls
            .lines()
            .all(|line| matches!(line, "owner" | "metadata"))
    );
    probe.assert_cleaned_up();
}

#[test]
fn rejects_wrong_owner_with_exact_login_matching() {
    let probe = ReleaseProbe::new();
    let output = probe.run("maint");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("required owner maint is absent"));
    probe.assert_cleaned_up();
}

#[test]
fn rejects_unavailable_support_versions() {
    let probe = ReleaseProbe::new();
    probe.update_resolution(|value| value["packages"][1]["version"] = json!("0.2.0"));
    let output = probe.run("maintainer");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mf-compiler at 0.1.0"));
    probe.assert_cleaned_up();
}

#[test]
fn rejects_local_packages_as_registry_release_evidence() {
    let probe = ReleaseProbe::new();
    probe.update_resolution(|value| value["packages"][0]["source"] = Value::Null);
    let output = probe.run("maintainer");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mf-runtime at 0.1.0"));
    probe.assert_cleaned_up();
}

#[test]
fn preserves_cargo_failure_context_and_cleans_up() {
    for stage in ["owner", "resolution"] {
        let probe = ReleaseProbe::new();
        let output = probe
            .command()
            .args(["--owner", "maintainer"])
            .env("MF_FAIL_STAGE", stage)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("package not found"), "{stderr}");
        assert!(
            stderr.contains("support package prerequisite failed"),
            "{stderr}"
        );
        probe.assert_cleaned_up();
    }
}

#[test]
fn rejects_invalid_arguments_before_running_cargo() {
    let probe = ReleaseProbe::new();
    for args in [
        vec![],
        vec!["--owner"],
        vec!["--owner", ""],
        vec!["--unknown"],
    ] {
        let result = probe.command().args(args).output().unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("Usage:"));
    }
    assert!(
        fs::read(probe.directory.path().join("calls"))
            .unwrap()
            .is_empty()
    );
    probe.assert_cleaned_up();
}
