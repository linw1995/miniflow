#![cfg(unix)]
mod common;

extern crate mfn_core as _;
extern crate mfn_llm as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{FlowRuntime, StreamOptions, WorkflowArguments};
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn arguments(value: &Value) -> WorkflowArguments {
    WorkflowArguments::from_json(value.to_string().as_bytes()).unwrap()
}

fn memory(definition: &WorkflowDefinition, inputs: &Value) -> Vec<Value> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry).unwrap();
    if definition.execution.is_some() {
        let prepared = instantiate_stream(&plan, &registry).unwrap();
        let instance = FlowRuntime::default()
            .start_stream(
                prepared,
                StreamOptions {
                    arguments: arguments(inputs),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut results = vec![];
        while let Some(output) = instance.recv().unwrap() {
            results.push(serde_json::to_value(output.outputs).unwrap());
        }
        results
    } else {
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        vec![serde_json::to_value(flow.execute_with_inputs(arguments(inputs)).unwrap()).unwrap()]
    }
}

#[test]
#[ignore = "invoked as an isolated credential subprocess"]
fn memory_probe() {
    let definition =
        WorkflowDefinition::from_json(&std::env::var("MINIFLOW_LLM_DEFINITION").unwrap()).unwrap();
    let inputs: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_LLM_INPUTS").unwrap()).unwrap();
    println!("MEMORY_RESULT:{}", json!(memory(&definition, &inputs)));
}

fn run_memory(value: &Value, inputs: &Value) -> Vec<Value> {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "memory_probe", "--ignored", "--nocapture"])
        .env("MINIFLOW_LLM_DEFINITION", value.to_string())
        .env("MINIFLOW_LLM_INPUTS", inputs.to_string())
        .env("MODEL_API_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(
        output
            .lines()
            .find_map(|line| line.strip_prefix("MEMORY_RESULT:"))
            .unwrap(),
    )
    .unwrap()
}

fn configure(value: &mut Value, server: &Server) {
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            if node["kind"] == "builtin.llm" {
                node["config"] = json!({"model": "example-model", "base_url": server.base_url, "api_key_env": "MODEL_API_KEY"});
            }
            if let Some(body) = node.get_mut("loop").and_then(|value| value.get_mut("body")) {
                configure(body, server);
            }
            if let Some(body) = node
                .get_mut("config")
                .and_then(|value| value.get_mut("body"))
            {
                configure(body, server);
            }
        }
    }
}

fn dependencies() -> Value {
    json!({
        "core": {"package": "mfn-core", "path": common::crates_dir().join("builtin-nodes/core")},
        "llm": {"package": "mfn-llm", "path": common::crates_dir().join("builtin-nodes/llm")}
    })
}

fn compile(value: &Value, root: &Path) -> std::path::PathBuf {
    let definition = root.join("flow.json");
    let output = root.join("flow");
    fs::write(&definition, value.to_string()).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &definition,
            output: &output,
            locked: false,
            build_dir: Some(&root.join("build")),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    output
}

fn run(executable: &Path, inputs: &Value) -> Vec<Value> {
    let output = Command::new(executable)
        .current_dir(executable.parent().unwrap())
        .args(["--inputs", &inputs.to_string()])
        .env_clear()
        .env("PATH", "")
        .env("MODEL_API_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

#[test]
fn llm_workflows_match_memory_and_standalone_runners() {
    let workspace = common::crates_dir().parent().unwrap().to_owned();
    let cases = [
        (
            serde_json::from_str::<Value>(
                &fs::read_to_string(workspace.join("examples/llm-terminal.json")).unwrap(),
            )
            .unwrap(),
            json!({"chat": {"prompt": "Hello", "system": "Answer briefly", "history": [{"role": "user", "content": "prior"}, {"role": "assistant", "content": "earlier reply"}]}}),
            1,
        ),
        (
            serde_json::from_str::<Value>(
                &fs::read_to_string(workspace.join("examples/llm-loop.json")).unwrap(),
            )
            .unwrap(),
            json!({"repeat": {"prompt": "Hello", "history": [], "text": ""}}),
            2,
        ),
        (
            json!({"version": "2026-10-03", "nodes": [{"id": "iteration", "kind": "builtin.iteration", "config": {"mode": "parallel", "on_error": "terminate", "body": {"nodes": [{"id": "chat", "kind": "builtin.llm"}], "edges": [{"from_node": "%iteration", "from_output": "item", "to_node": "chat", "to_input": "prompt"}], "result": {"node": "chat", "port": "text"}}}}], "outputs": [{"name": "results", "node": "iteration", "port": "results"}]}),
            json!({"iteration": {"items": ["one", "two"]}}),
            2,
        ),
    ];
    for (mut value, inputs, calls) in cases {
        let server = Server::start(vec![Reply::text("reply"); calls * 2]);
        value["dependencies"] = dependencies();
        configure(&mut value, &server);
        if value["nodes"][0]["kind"] == "builtin.llm" {
            value["outputs"].as_array_mut().unwrap().extend([
                json!({"name": "usage", "node": "chat", "port": "usage", "optional": true}),
                json!({"name": "finish_reason", "node": "chat", "port": "finish_reason", "optional": true}),
            ]);
        }
        let root = tempfile::tempdir().unwrap();
        let executable = compile(&value, root.path());
        assert!(
            server.requests().is_empty(),
            "compilation must not execute the model"
        );
        let expected = run_memory(&value, &inputs);
        fs::remove_file(root.path().join("flow.json")).unwrap();
        fs::remove_dir_all(root.path().join("build")).unwrap();
        let generated = run(&executable, &inputs);
        assert_eq!(generated, expected);
        assert_eq!(server.requests().len(), calls * 2);
        if value["nodes"][0]["kind"] == "builtin.llm" {
            assert_eq!(generated[0]["finish_reason"], "stop");
            assert_eq!(
                generated[0]["usage"],
                json!({"input_tokens": 2, "output_tokens": 1, "total_tokens": 3})
            );
            let expected_messages = json!([
                {"role": "system", "content": "Answer briefly"},
                {"role": "user", "content": "prior"},
                {"role": "assistant", "content": "earlier reply"},
                {"role": "user", "content": "Hello"}
            ]);
            for request in server.requests() {
                assert_eq!(json!(messages(&request)), expected_messages);
            }
        }
        if value["nodes"][0]["kind"] == "workflow.loop" {
            assert_eq!(generated[0]["history"].as_array().unwrap().len(), 4);
            assert_eq!(messages(&server.requests()[1]).len(), 3);
        }
    }
}

#[test]
fn llm_stream_tasks_and_skips_preserve_existing_execution_contracts() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("prompts.txt");
    fs::write(&input, "first\nsecond\n").unwrap();
    let server = Server::start(vec![Reply::text("reply"); 4]);
    let mut value = json!({
        "version": "2026-10-03", "execution": {"mode": "stream", "limits": {"workers": 1, "max_pending_messages": 2}},
        "dependencies": dependencies(),
        "nodes": [{"id": "read", "kind": "builtin.readline"}, {"id": "chat", "kind": "builtin.llm"}],
        "edges": [{"from_node": "read", "from_output": "line", "to_node": "chat", "to_input": "prompt"}],
        "outputs": [{"name": "text", "node": "chat", "port": "text"}, {"name": "history", "node": "chat", "port": "history"}]
    });
    configure(&mut value, &server);
    let inputs = json!({"read": {"path": input}});
    let executable = compile(&value, root.path());
    assert!(server.requests().is_empty());
    let expected = run_memory(&value, &inputs);
    assert_eq!(run(&executable, &inputs), expected);
    assert_eq!(expected.len(), 2);
    assert_eq!(server.requests().len(), 4);

    let root = tempfile::tempdir().unwrap();
    let mut skipped = json!({
        "version": "2026-10-03", "dependencies": dependencies(),
        "nodes": [
            {"id": "enabled", "kind": "builtin.constant", "config": {"value": false}},
            {"id": "prompt", "kind": "builtin.constant", "config": {"value": "Hello"}},
            {"id": "gate", "kind": "builtin.if_else", "config": {"branches": [{"id": "go", "condition": {"source": {"output": "enabled.value", "path": ""}, "operator": "eq", "value": true}}]}},
            {"id": "chat", "kind": "builtin.llm"}
        ],
        "control_edges": [{"from_node": "enabled", "from_output": "value", "to_node": "gate"}, {"from_node": "gate", "from_output": "go", "to_node": "chat"}],
        "edges": [{"from_node": "prompt", "from_output": "value", "to_node": "chat", "to_input": "prompt"}],
        "outputs": [{"name": "text", "node": "chat", "port": "text", "optional": true}]
    });
    configure(&mut skipped, &server);
    let executable = compile(&skipped, root.path());
    assert_eq!(run_memory(&skipped, &json!({})), vec![json!({})]);
    assert_eq!(run(&executable, &json!({})), vec![json!({})]);
    assert_eq!(server.requests().len(), 4);
}
