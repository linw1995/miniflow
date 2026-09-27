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
        "dependencies":{
            "core":{"package":"mfn-core","version":format!("={}", env!("CARGO_PKG_VERSION"))},
            "external":{"package":"fixture-multi-nodes","version":format!("={}", fixture.fixture_version),"features":["double"]}},
        "nodes":[
            {"id":"source","kind":"fixture.source"},
            {"id":"route","kind":"builtin.if_else","config":{"branches":[
                {"id":"accepted","condition":{"source":{"output":"source.value","path":""},"operator":"gt","value":0}}
            ]}},
            {"id":"echo","kind":"fixture.echo"},
            {"id":"inactive","kind":"fixture.context","config":{"ports":["value"],"fail":true}}
        ],
        "control_edges":[
            {"from_node":"source","from_output":"value","to_node":"route"},
            {"from_node":"route","from_output":"accepted","to_node":"echo"},
            {"from_node":"route","from_output":"else","to_node":"inactive"}
        ],
        "edges":[{"from_node":"source","from_output":"value","to_node":"echo","to_input":"input"}],
        "outputs":[{"name":"result","node":"echo","port":"value"},{"name":"inactive","node":"inactive","port":"value","optional":true}]
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
    for name in ["mf-runtime", "mf-compiler"] {
        assert!(manifest.contains(&format!(
            "{name} = {{ version = \"={}\" }}",
            env!("CARGO_PKG_VERSION")
        )));
    }
    let generated = fs::read_to_string(build.join("src/workflow.rs")).unwrap();
    assert!(generated.contains("mf_runtime::execute_node"));
    assert!(!generated.contains("Flow::new"));

    let mut typed_flow = json!({
        "version": "2026-09-26",
        "dependencies": {
            "external": {"package": "fixture-multi-nodes", "version": format!("={}", fixture.fixture_version)}
        },
        "nodes": [
            {"id": "source", "kind": "fixture.typed_source", "config": {"type": "any", "value": [1, 2]}},
            {"id": "sink", "kind": "fixture.typed_echo", "config": {"type": "list_int64"}}
        ],
        "edges": [{"from_node": "source", "from_output": "value", "to_node": "sink", "to_input": "input"}],
        "outputs": [{"name": "result", "node": "sink", "port": "value"}]
    });
    let typed_definition = project.join("typed.json");
    fs::write(&typed_definition, typed_flow.to_string()).unwrap();
    let typed_executable = project.join("typed");
    let typed_build = fixture.root().join("typed-build");
    fixture.compile(
        &typed_definition,
        &typed_executable,
        &typed_build,
        false,
        false,
    );
    let typed_result = checked(Command::new(&typed_executable).env_clear().env("PATH", ""));
    assert_eq!(
        serde_json::from_slice::<Value>(&typed_result.stdout).unwrap(),
        json!({"result": [1, 2]})
    );
    typed_flow["nodes"][0]["config"]["value"] = json!([1, "wrong"]);
    fs::write(&typed_definition, typed_flow.to_string()).unwrap();
    fixture.compile(
        &typed_definition,
        &typed_executable,
        &typed_build,
        false,
        false,
    );
    let typed_failure = Command::new(&typed_executable)
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(!typed_failure.status.success());
    let diagnostic = String::from_utf8_lossy(&typed_failure.stderr);
    assert!(diagnostic.contains("sink") && diagnostic.contains("/1"));

    for (name, source, expected) in [
        (
            "cel-scalar",
            include_str!("../../../examples/cel-scalar.json"),
            json!({"doubled": 42}),
        ),
        (
            "cel-list",
            include_str!("../../../examples/cel-list.json"),
            json!({"doubled": [2, 4]}),
        ),
    ] {
        let mut cel_flow: Value = serde_json::from_str(source).unwrap();
        cel_flow["dependencies"] = json!({
            "core": {"package":"mfn-core","version":format!("={}", env!("CARGO_PKG_VERSION"))},
            "code": {"package":"mfn-code","version":format!("={}", env!("CARGO_PKG_VERSION"))}
        });
        let cel_definition = project.join(format!("{name}.json"));
        let cel_output = project.join(name);
        let cel_build = fixture.root().join(format!("{name}-build"));
        fs::write(&cel_definition, cel_flow.to_string()).unwrap();
        fixture.compile(&cel_definition, &cel_output, &cel_build, false, false);
        let manifest = fs::read_to_string(cel_build.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(&format!(
            "package = \"mfn-code\", version = \"={}\"",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(!manifest.contains("path ="));
        let runtime = fixture.root().join(format!("{name}-runtime"));
        fs::create_dir(&runtime).unwrap();
        let standalone = runtime.join("flow");
        fs::rename(&cel_output, &standalone).unwrap();
        fs::remove_file(&cel_definition).unwrap();
        fs::remove_file(cel_definition.with_extension("lock")).unwrap();
        fs::remove_dir_all(&cel_build).unwrap();
        let result = checked(
            Command::new(&standalone)
                .current_dir(&runtime)
                .env_clear()
                .env("PATH", ""),
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            expected
        );
    }

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
        flow["dependencies"] = json!({name: dependency, "core":{"package":"mfn-core","version":format!("={}", env!("CARGO_PKG_VERSION"))}});
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
