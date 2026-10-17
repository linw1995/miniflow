#![cfg(feature = "development-support")]

use mfn_llm::test_support as llm_support;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn examples() -> [(&'static str, Option<&'static str>, Value); 11] {
    [
        ("cel-list.json", None, json!({"doubled": [2, 4]})),
        ("cel-scalar.json", None, json!({"doubled": 42})),
        ("else-if.json", None, json!({"medium": {"amount": 500}})),
        ("hello-workflow.json", None, json!({"answer": 42})),
        ("if-else.json", None, json!({"accepted": {"amount": 150}})),
        ("iteration.json", None, json!({"results": [2, 5, 8]})),
        ("loop.json", None, json!({"count": 3})),
        (
            "llm-terminal.json",
            None,
            json!({"text": "reply", "history": [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}]}),
        ),
        (
            "llm-parameters.json",
            None,
            json!({"text": "reply", "history": [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}]}),
        ),
        (
            "llm-loop.json",
            None,
            json!({"text": "reply", "history": [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}, {"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}]}),
        ),
        ("stream-batch.json", Some("1\n"), json!({"batch": [1]})),
    ]
}

#[test]
fn every_example_has_an_expected_result() {
    let mut actual: Vec<_> = fs::read_dir(workspace().join("examples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    let mut expected: Vec<_> = examples().into_iter().map(|(name, _, _)| name).collect();
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "update the example cases when adding, removing, or renaming examples/*.json"
    );
}

#[cfg(unix)]
#[test]
fn documented_examples_compile_and_produce_expected_outputs() {
    use std::{
        io::Write,
        os::unix::fs::symlink,
        process::{Command, Stdio},
    };

    let temporary = tempfile::Builder::new()
        .prefix("mf examples ")
        .tempdir()
        .unwrap();
    let definitions = temporary.path().join("examples");
    let runtime = temporary.path().join("runtime");
    fs::create_dir(&definitions).unwrap();
    fs::create_dir(&runtime).unwrap();
    // Preserve the examples' relative paths while keeping lock files out of the checkout.
    symlink(workspace().join("crates"), temporary.path().join("crates")).unwrap();

    for (name, input, expected) in examples() {
        let definition = definitions.join(name);
        let server = name
            .starts_with("llm-")
            .then(|| llm_support::Server::start(vec![llm_support::Reply::text("reply"); 2]));
        if let Some(server) = &server {
            let mut value: Value =
                serde_json::from_slice(&fs::read(workspace().join("examples").join(name)).unwrap())
                    .unwrap();
            let config = if name == "llm-loop.json" {
                &mut value["nodes"][0]["loop"]["body"]["nodes"][0]["config"]
            } else if name == "llm-parameters.json" {
                &mut value["nodes"][1]["config"]
            } else {
                &mut value["nodes"][0]["config"]
            };
            config["base_url"] = server.base_url.clone().into();
            config["model"] = "example-model".into();
            fs::write(&definition, value.to_string()).unwrap();
        } else {
            fs::copy(workspace().join("examples").join(name), &definition).unwrap();
        }
        let stem = Path::new(name).file_stem().unwrap();
        let executable = runtime.join(stem);
        let build = temporary.path().join("build").join(stem);
        let compiled = Command::new(env!("CARGO_BIN_EXE_mf"))
            .current_dir(&runtime)
            .env("MF_DEV_SUPPORT_ROOT", workspace().join("crates"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env_remove("MODEL_API_KEY")
            .arg("compile")
            .arg(&definition)
            .arg("--output")
            .arg(&executable)
            .arg("--build-dir")
            .arg(&build)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{name}: compilation failed with {}:\n{}\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr),
        );

        let stderr = String::from_utf8_lossy(&compiled.stderr);
        assert!(
            !stderr
                .lines()
                .any(|line| line.starts_with("warning:") || line.starts_with("warning[")),
            "{name}: compilation emitted warnings:\n{stderr}",
        );

        let mut command = Command::new(&executable);
        command
            .current_dir(&runtime)
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if server.is_some() {
            let inputs = if name == "llm-loop.json" {
                json!({"repeat.prompt":"Hello", "repeat.history":[], "repeat.text":""})
            } else if name == "llm-parameters.json" {
                json!({"params.prompt":"Hello"})
            } else {
                json!({"chat.prompt":"Hello"})
            };
            command
                .args(["--inputs", &inputs.to_string()])
                .env("MODEL_API_KEY", "synthetic-key")
                .env("NO_PROXY", "127.0.0.1,localhost")
                .env("no_proxy", "127.0.0.1,localhost");
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{name}: execution failed with {}:\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let actual: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{name}: invalid JSON output: {error}\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        assert_eq!(actual, expected, "{name}: unexpected workflow output");
    }
}
