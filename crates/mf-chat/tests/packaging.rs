use mfn_llm::test_support::{Reply, Server};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let path = destination.join(name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &path);
        } else {
            fs::copy(entry.path(), path).unwrap();
        }
    }
}

fn copy_chat_workspace(root: &Path, destination: &Path) {
    let mut manifest: toml::Table = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let workspace = manifest["workspace"].as_table_mut().unwrap();
    for key in ["members", "default-members"] {
        workspace.insert(
            key.into(),
            toml::Value::Array(vec!["crates/mf-chat".into()]),
        );
    }
    // Only the chat build inputs are deleted by this fixture. Stable dependency
    // paths let Cargo reuse the same artifacts as the generated-runner tests.
    for (_, dependency) in workspace["dependencies"].as_table_mut().unwrap().iter_mut() {
        if let Some(path) = dependency.get_mut("path") {
            *path = root.join(path.as_str().unwrap()).to_str().unwrap().into();
        }
    }
    fs::create_dir_all(destination).unwrap();
    fs::write(
        destination.join("Cargo.toml"),
        toml::to_string(&manifest).unwrap(),
    )
    .unwrap();
    fs::copy(root.join("Cargo.lock"), destination.join("Cargo.lock")).unwrap();
    copy_tree(
        &root.join("crates/mf-chat"),
        &destination.join("crates/mf-chat"),
    );
}

#[test]
fn packaged_inputs_include_workflows_migrations_and_host_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = Command::new("cargo")
        .args([
            "package",
            "-p",
            "mf-chat",
            "--list",
            "--allow-dirty",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = String::from_utf8(output.stdout).unwrap();
    for required in [
        "workflows/conversation.json",
        "migrations/00000000000001_chat_storage/up.sql",
        "migrations/00000000000001_chat_storage/down.sql",
        "tests/fixtures/host/src/main.rs",
    ] {
        assert!(
            files.lines().any(|line| line == required),
            "missing {required}"
        );
    }
}

#[test]
fn source_free_host_preserves_multi_session_retry_title_and_statistics() {
    let directory = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let copy = directory.path().join("workspace");
    copy_chat_workspace(root, &copy);
    let host = copy.join("crates/mf-chat/tests/fixtures/host");
    fs::rename(host.join("Cargo.toml.in"), host.join("Cargo.toml")).unwrap();
    fs::copy(root.join("Cargo.lock"), host.join("Cargo.lock")).unwrap();
    let target = std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/chat-host-fixture"));
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--release", "--offline", "--manifest-path"])
        .arg(host.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let binary = directory
        .path()
        .join(format!("chat-host{}", std::env::consts::EXE_SUFFIX));
    fs::copy(
        target
            .join("release")
            .join(format!("chat-host-fixture{}", std::env::consts::EXE_SUFFIX)),
        &binary,
    )
    .unwrap();
    // Preserve the executed binary's maps after deleting the source-free fixture.
    if let Some(coverage) = std::env::var_os("MF_COVERAGE_BINARIES_DIR") {
        let coverage = Path::new(&coverage);
        fs::copy(&binary, coverage.join("mf-chat-host-fixture")).unwrap();
        fs::write(
            coverage.join("mf-chat-host-fixture.paths.json"),
            serde_json::to_vec(&std::collections::BTreeMap::from([(
                host.join("src/main.rs")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                "crates/mf-chat/tests/fixtures/host/src/main.rs",
            )]))
            .unwrap(),
        )
        .unwrap();
    }
    fs::remove_dir_all(&copy).unwrap();
    let mut failed = Reply::json(json!({"error":{"message":"synthetic failure"}}));
    failed.status = 500;
    let server = Server::start(vec![
        Reply::text("answer A"),
        Reply::text("Summary A"),
        failed,
        Reply::text("answer B"),
        Reply::text("Summary B"),
    ]);
    let mut command = Command::new(&binary);
    command
        .arg(directory.path().join("chat.sqlite"))
        .current_dir(directory.path())
        .env("PATH", "")
        .env("MF_CHAT_FIXTURE_URL", &server.base_url)
        .env("MF_CHAT_FIXTURE_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["usage"]["overall"]["attempted_calls"], 5);
    assert_eq!(result["usage"]["overall"]["statuses"]["failed"], 1);
    assert_eq!(
        result["usage"]["overall"]["total_tokens"]["known_subtotal"],
        12
    );
    assert_eq!(
        result["usage"]["overall"]["total_tokens"]["missing_calls"],
        1
    );
    assert_eq!(result["title_a"], "Manual A");
    assert_eq!(result["title_b"], "Summary B");
    assert_eq!(result["messages_a"], 2);
    assert_eq!(result["messages_b"], 3);
    assert_eq!(result["history_b"], 2);
    assert_eq!(server.requests().len(), 5);
}
