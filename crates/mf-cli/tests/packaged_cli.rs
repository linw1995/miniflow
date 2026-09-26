mod support;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use support::{PackagedCli, checked, copy_directory};

fn git(directory: &Path, args: &[&str]) -> std::process::Output {
    checked(
        Command::new("git")
            .current_dir(directory)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1"),
    )
}

fn file_url(path: &Path) -> String {
    let absolute = fs::canonicalize(path).unwrap();
    let mut url = String::from("file://");
    for byte in absolute.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            url.push(char::from(*byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[test]
fn packaged_cli_acceptance() {
    let fixture = PackagedCli::setup();
    let project = fixture.root().join("user-project");
    fs::create_dir(&project).unwrap();
    let mut flow = json!({
        "version":"2026-09-26",
        "dependencies":{"external":{"package":"fixture-multi-nodes","version":format!("={}", fixture.fixture_version),"features":["double"]}},
        "nodes":[{"id":"source","kind":"fixture.source"},{"id":"echo","kind":"fixture.echo"}],
        "edges":[{"from_node":"source","from_output":"value","to_node":"echo","to_input":"input"}],
        "outputs":[{"name":"result","node":"echo","port":"value"}]
    });
    let definition = project.join("flow.json");
    fs::write(&definition, flow.to_string()).unwrap();
    let build = fixture.root().join("build");
    let output = project.join("flow");
    fixture.compile(&definition, &output, &build, false, false);
    let lock = fs::read(project.join("flow.lock")).unwrap();
    let repeated = fixture.compile(&definition, &output, &build, true, false);
    assert!(String::from_utf8_lossy(&repeated.stderr).contains("reused compiled runner"));
    assert_eq!(fs::read(project.join("flow.lock")).unwrap(), lock);
    let manifest = fs::read_to_string(build.join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("path =") && !manifest.contains("mf-bundle"));

    let local = project.join("local-nodes");
    let repository = fixture.root().join("git-nodes");
    copy_directory(&fixture.fixture_source, &local);
    copy_directory(&fixture.fixture_source, &repository);
    git(&repository, &["init", "--quiet"]);
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "fixture"]);
    let revision = String::from_utf8(git(&repository, &["rev-parse", "HEAD"]).stdout).unwrap();
    for (name, dependency, expected) in [
        (
            "local",
            json!({"package":"fixture-multi-nodes","path":"local-nodes"}),
            7,
        ),
        (
            "git",
            json!({"package":"fixture-multi-nodes","git":file_url(&repository),"rev":revision.trim(),"features":["double"]}),
            14,
        ),
    ] {
        flow["dependencies"] = json!({name: dependency});
        let source = project.join(format!("{name}.json"));
        fs::write(&source, flow.to_string()).unwrap();
        let executable = project.join(name);
        let directory = fixture.root().join(format!("{name}-build"));
        // Offline mode also prohibits the first checkout of a file:// repository.
        fixture.compile(&source, &executable, &directory, false, name == "git");
        let locked = fs::read(source.with_extension("lock")).unwrap();
        fixture.compile(&source, &executable, &directory, true, false);
        assert_eq!(fs::read(source.with_extension("lock")).unwrap(), locked);
        let result = checked(
            Command::new(&executable)
                .current_dir(fixture.root())
                .env_clear()
                .env("PATH", ""),
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!({"result":expected})
        );
    }
    assert_ne!(
        fs::read(project.join("local.lock")).unwrap(),
        fs::read(project.join("git.lock")).unwrap()
    );

    let runtime = fixture.root().join("runtime");
    fs::create_dir(&runtime).unwrap();
    let standalone = runtime.join("flow");
    fs::copy(&output, &standalone).unwrap();
    fs::remove_dir_all(project).unwrap();
    fs::remove_dir_all(build).unwrap();
    let command = || {
        let mut command = Command::new(&standalone);
        command.current_dir(&runtime).env_clear().env("PATH", "");
        command
    };
    assert!(checked(command().arg("--validate")).stdout.is_empty());
    let invalid = command().arg("--unknown").output().unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("usage:"));
    assert_eq!(
        serde_json::from_slice::<Value>(&checked(&mut command()).stdout).unwrap(),
        json!({"result":14})
    );
}
