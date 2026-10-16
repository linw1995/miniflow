use mfn_llm::test_support as llm_support;
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
    let llm_server = llm_support::Server::start(vec![llm_support::Reply::text("packaged reply")]);
    let llm_flow = json!({
        "version": "2026-10-03",
        "dependencies": {"llm": {"package": "mfn-llm", "version": format!("={}", env!("CARGO_PKG_VERSION"))}},
        "nodes": [{"id": "chat", "kind": "builtin.llm", "config": {"model": "example-model", "base_url": llm_server.base_url, "api_key_env": "MINIFLOW_PACKAGED_LLM_KEY"}}],
        "outputs": [{"name": "text", "node": "chat", "port": "text"}, {"name": "history", "node": "chat", "port": "history"}]
    });
    let llm_definition = project.join("llm.json");
    let llm_executable = project.join("llm");
    fs::write(&llm_definition, llm_flow.to_string()).unwrap();
    fixture.compile(
        &llm_definition,
        &llm_executable,
        &fixture.root().join("llm-build"),
        false,
        false,
    );
    assert!(llm_server.requests().is_empty());
    fs::remove_file(&llm_definition).unwrap();
    fs::remove_dir_all(fixture.root().join("llm-build")).unwrap();
    let llm_inputs = json!({"chat.prompt":"Hello", "chat.history":[{"role": "user", "content": "prior"}, {"role": "assistant", "content": "earlier reply"}]});
    let llm_result = checked(
        Command::new(&llm_executable)
            .current_dir(&project)
            .env_clear()
            .env("PATH", "")
            .env("MINIFLOW_PACKAGED_LLM_KEY", "synthetic-key")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .args(["--inputs", &llm_inputs.to_string()]),
    );
    let llm_result: Value = serde_json::from_slice(&llm_result.stdout).unwrap();
    assert_eq!(llm_result["text"], "packaged reply");
    assert_eq!(llm_result["history"].as_array().unwrap().len(), 4);
    assert_eq!(llm_server.requests().len(), 1);
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
    assert!(!manifest.contains("\nlibc = "));
    for name in ["mf-runtime", "mf-compiler", "mf-telemetry"] {
        assert!(manifest.contains(&format!(
            "{name} = {{ version = \"={}\", default-features = false }}",
            env!("CARGO_PKG_VERSION")
        )));
    }
    assert!(manifest.contains("default = [\"telemetry\"]"));
    assert!(manifest.contains("telemetry = [\"mf-telemetry/otlp\"]"));
    assert!(!manifest.contains("mf-tui"));
    let typed_flow = json!({
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

    let name = "cel-list";
    let source = include_str!("../../../examples/cel-list.json");
    let expected = json!({"doubled": [2, 4]});
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

    let mut loop_flow: Value =
        serde_json::from_str(include_str!("../../../examples/loop.json")).unwrap();
    loop_flow["dependencies"] = json!({
        "core": {"package":"mfn-core","version":format!("={}", env!("CARGO_PKG_VERSION"))},
        "code": {"package":"mfn-code","version":format!("={}", env!("CARGO_PKG_VERSION"))}
    });
    let loop_definition = project.join("loop.json");
    let loop_output = project.join("loop");
    let loop_build = fixture.root().join("loop-build");
    fs::write(&loop_definition, loop_flow.to_string()).unwrap();
    fixture.compile(&loop_definition, &loop_output, &loop_build, false, false);
    let description = serde_json::to_value(
        mf_tui::manifest::read_manifest(&loop_output)
            .unwrap()
            .description()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(description["version"], "2026-09-29");
    assert_eq!(description["loop_bodies"].as_array().unwrap().len(), 1);
    let loop_result = checked(Command::new(&loop_output).env_clear().env("PATH", ""));
    assert_eq!(
        serde_json::from_slice::<Value>(&loop_result.stdout).unwrap(),
        json!({"count": 3})
    );
    let loop_runtime = fixture.root().join("loop-runtime");
    fs::create_dir(&loop_runtime).unwrap();
    let standalone_loop = loop_runtime.join("flow");
    fs::rename(&loop_output, &standalone_loop).unwrap();
    fs::remove_file(&loop_definition).unwrap();
    fs::remove_file(loop_definition.with_extension("lock")).unwrap();
    fs::remove_dir_all(&loop_build).unwrap();
    let portable_loop = checked(
        Command::new(&standalone_loop)
            .current_dir(&loop_runtime)
            .env_clear()
            .env("PATH", ""),
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&portable_loop.stdout).unwrap(),
        json!({"count": 3})
    );

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
    let invalid = command().arg("--unknown").output().unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("Usage:"), "{stderr}");
    assert!(stderr.contains("--unknown"), "{stderr}");
    let help = command().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));
    assert_eq!(
        serde_json::from_slice::<Value>(&checked(&mut command()).stdout).unwrap(),
        json!({"result":14})
    );
}
