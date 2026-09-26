#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
use tempfile::TempDir;

fn probe() -> (TempDir, Command) {
    let directory = tempfile::Builder::new()
        .prefix("mf release probe ")
        .tempdir()
        .unwrap();
    let root = directory.path();
    let stub = root.join("cargo");
    let bash = Command::new("bash")
        .args(["-c", "command -v bash"])
        .output()
        .unwrap();
    assert!(bash.status.success());
    let interpreter = String::from_utf8(bash.stdout).unwrap();
    fs::write(
        &stub,
        format!(
            "#!{}\n{}",
            interpreter.trim(),
            r#"set -euo pipefail
case "$1" in
  owner)
    printf '%s\n' "${@: -1}" >> "$MF_PROBE_ROOT/owners-checked"
    printf 'maintainer (Name)\n'
    ;;
  metadata)
    if [[ "${2:-}" == --no-deps ]]; then
      printf '{"packages":[{"name":"mf-cli","version":"1.2.3"}]}\n'
    else
      cp Cargo.toml "$MF_PROBE_ROOT/probe.toml"
      if [[ "${MF_RESOLUTION_FAILS:-}" == true ]]; then
        printf 'required version is unavailable\n' >&2
        exit 101
      fi
    fi
    ;;
  *) exit 97 ;;
esac
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new("bash");
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-release-support.sh"))
        .current_dir(root)
        .env("CARGO", stub)
        .env("MF_PROBE_ROOT", root)
        .env("TMPDIR", root);
    (directory, command)
}

#[test]
fn checks_owners_and_resolves_exact_workspace_versions() {
    let (directory, mut command) = probe();
    let output = command.args(["--owner", "maintainer"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = fs::read_to_string(directory.path().join("probe.toml")).unwrap();
    let owners = fs::read_to_string(directory.path().join("owners-checked")).unwrap();
    for name in ["mf-runtime", "mf-compiler", "mfn-constant", "mfn-identity"] {
        assert!(owners.lines().any(|line| line == name), "{name}");
        assert!(
            manifest.contains(&format!("{name} = \"=1.2.3\"")),
            "{manifest}"
        );
    }
}

#[test]
fn rejects_a_partial_owner_login_before_resolving_packages() {
    let (directory, mut command) = probe();
    let output = command.args(["--owner", "maint"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("required owner maint is absent"));
    assert!(!directory.path().join("probe.toml").exists());
}

#[test]
fn cargo_resolution_failure_blocks_release() {
    let (_directory, mut command) = probe();
    let output = command
        .args(["--owner", "maintainer"])
        .env("MF_RESOLUTION_FAILS", "true")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("required version is unavailable"));
}
