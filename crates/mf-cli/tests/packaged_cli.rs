use std::{path::Path, process::Command};

fn run_acceptance_script(script: &str) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let status = Command::new("python3")
        .arg(workspace.join("scripts").join(script))
        .current_dir(workspace)
        .status()
        .unwrap_or_else(|error| {
            panic!("could not start {script}: {error}; run tests through nix develop")
        });
    assert!(
        status.success(),
        "{script} failed with {status}; see captured output"
    );
}

#[test]
fn packaged_cli_acceptance() {
    run_acceptance_script("test-packaged-cli.py");
}

#[test]
fn release_support_prerequisites() {
    run_acceptance_script("test-release-support.py");
}
