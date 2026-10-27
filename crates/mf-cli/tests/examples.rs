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

fn chat_usage(path: Value) -> Value {
    json!({"started_at":0,"call_id":"generated", "node":{"id":"chat", "kind":"builtin.llm", "path":path}, "model":"example-model", "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}})
}

fn title_usage(node: &str, purpose: &str) -> Value {
    let mut usage = chat_usage(json!([]));
    usage["node"]["id"] = node.into();
    usage["attribution"] = json!({"purpose":purpose});
    usage
}

fn nested_title_usage() -> Value {
    let mut usage = title_usage("title_model", "title");
    usage["node"]["path"] = json!([{"node":"title","index":0}]);
    usage
}

fn examples() -> [(&'static str, Option<&'static str>, Value); 18] {
    [
        ("cel-list.json", None, json!({"doubled": [2, 4]})),
        ("cel-scalar.json", None, json!({"doubled": 42})),
        ("clustered-workflow.json", None, json!({"doubled": 42})),
        (
            "chat-flow.json",
            None,
            json!({"status":"succeeded", "text":"reply", "usages":[chat_usage(json!([]))], "finish_reason":"stop"}),
        ),
        (
            "chat-loop-flow.json",
            None,
            json!({"status":"succeeded", "text":"reply", "error":"", "metadata":{"strategy":"iterative"}, "usages":[chat_usage(json!([{"node":"repeat","index":0}])), chat_usage(json!([{"node":"repeat","index":1}]))]}),
        ),
        (
            "chat-title-flow.json",
            None,
            json!({"status":"succeeded","text":"reply","title":"reply","finish_reason":"stop",
                "usages":[title_usage("model", "conversation"), title_usage("title_model", "title")]}),
        ),
        (
            "chat-nested-title-flow.json",
            None,
            json!({"status":"succeeded","text":"reply","title":"reply","finish_reason":"stop",
                "usages":[title_usage("model", "conversation"), nested_title_usage()]}),
        ),
        ("else-if.json", None, json!({"medium": {"amount": 500}})),
        (
            "exclusive-inputs.json",
            None,
            json!({"result":{"mode":"local"}}),
        ),
        ("hello-workflow.json", None, json!({"answer": 42})),
        ("if-else.json", None, json!({"accepted": {"amount": 150}})),
        ("iteration.json", None, json!({"results": [2, 5, 8]})),
        ("loop.json", None, json!({"count": 3})),
        (
            "llm-terminal.json",
            None,
            json!({"text":"reply", "message":{"role":"assistant","content":"reply"}}),
        ),
        (
            "llm-parameters.json",
            None,
            json!({"text":"reply", "message":{"role":"assistant","content":"reply"}}),
        ),
        (
            "llm-loop.json",
            None,
            json!({"text": "reply", "history": [{"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}, {"role": "user", "content": "Hello"}, {"role": "assistant", "content": "reply"}]}),
        ),
        (
            "llm-tools.json",
            None,
            json!({"outcome":"completed", "text":"reply", "message":{"role":"assistant","content":"reply"}, "tool_calls":[], "usages":[{
                "started_at":0, "call_id":"generated", "node":{"id":"model","kind":"builtin.llm","path":[]}, "model":"example-model", "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
            }]}),
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
        let server = (name.starts_with("llm-") || name.starts_with("chat-")).then(|| {
            llm_support::Server::start(
                if matches!(name, "chat-title-flow.json" | "chat-nested-title-flow.json") {
                    vec![
                        llm_support::Reply::stream_text("reply"),
                        llm_support::Reply::text("reply"),
                    ]
                } else {
                    vec![llm_support::Reply::text("reply"); 2]
                },
            )
        });
        if let Some(server) = &server
            && !name.starts_with("chat-")
        {
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
        if let Some(server) = &server {
            let inputs = if name == "llm-loop.json" {
                json!({"repeat.prompt":"Hello", "repeat.history":[], "repeat.text":""})
            } else if matches!(name, "chat-title-flow.json" | "chat-nested-title-flow.json") {
                json!({"input.prompt":"Hello", "input.model":"example-model", "input.base_url":server.base_url})
            } else if name.starts_with("chat-") {
                json!({"params.prompt":"Hello", "params.model":"example-model", "params.base_url":server.base_url})
            } else if name == "llm-tools.json" {
                json!({"model.messages":[{"role":"user","content":"Hello"}]})
            } else if name == "llm-parameters.json" {
                json!({"params.messages":[{"role":"user","content":"Hello"}]})
            } else {
                json!({"chat.messages":[{"role":"user","content":"Hello"}]})
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
        let mut actual: Value =
            if matches!(name, "chat-title-flow.json" | "chat-nested-title-flow.json") {
                let records: Vec<Value> = serde_json::Deserializer::from_slice(&output.stdout)
                    .into_iter()
                    .collect::<Result<_, _>>()
                    .unwrap();
                let answer: String = records
                    .iter()
                    .filter_map(|record| record.get("answer"))
                    .map(|delta| delta["text"].as_str().unwrap())
                    .collect();
                assert_eq!(answer, "reply");
                assert_eq!(server.as_ref().unwrap().requests()[0].body["stream"], true);
                records
                    .into_iter()
                    .find(|record| record.get("status").is_some())
                    .unwrap()
            } else {
                serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                    panic!(
                        "{name}: invalid JSON output: {error}\n{}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                })
            };
        if name.starts_with("chat-") || name == "llm-tools.json" {
            let mut identities = std::collections::BTreeSet::new();
            for usage in actual["usages"].as_array_mut().unwrap() {
                let id = usage["call_id"].as_str().unwrap().to_owned();
                assert!(!id.is_empty() && identities.insert(id));
                usage["call_id"] = "generated".into();
                assert!(usage["started_at"].as_i64().unwrap() > 0);
                usage["started_at"] = 0.into();
                assert!(usage.get("timing").is_none_or(Value::is_null));
            }
        }
        assert_eq!(actual, expected, "{name}: unexpected workflow output");
        if name == "exclusive-inputs.json" {
            for ci in [false, true] {
                let output = Command::new(env!("CARGO_BIN_EXE_mf"))
                    .args([
                        "run",
                        executable.to_str().unwrap(),
                        "--inputs",
                        &json!({"params.ci":ci}).to_string(),
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    json!({"result":{"mode":if ci {"ci"} else {"local"}}})
                );
            }
        }
    }
}
