#![cfg(unix)]
#[path = "common/snapshots.rs"]
mod snapshots;
use snapshots::Snapshots;
mod common;

extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{ExecutionContext, FlowRuntime, StreamOptions, WorkflowArguments};
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
        let recording = Snapshots::memory();
        let mut context = ExecutionContext::default();
        context.set_snapshot_recorder(recording.recorder());
        let mut results = Vec::new();
        for inputs in inputs
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(inputs))
        {
            context.set_workflow_arguments(arguments(inputs));
            let output = flow.execute_in_context(&mut context).unwrap();
            if inputs.get("params.api_key").is_some() {
                let snapshot = recording.current();
                let params = snapshot.node(&[], "params").unwrap();
                assert_eq!(params.inputs["api_key"], json!("[REDACTED]"));
                assert_eq!(params.outputs["api_key"], json!("[REDACTED]"));
            }
            results.push(serde_json::to_value(output).unwrap());
        }
        results
    }
}

fn parameter_definition() -> Value {
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/llm-parameters.json")).unwrap();
    value["dependencies"] = dependencies();
    value
}

#[test]
fn parameter_templates_configure_memory_and_standalone_llm_requests() {
    fn stable(mut outputs: Vec<Value>) -> Vec<Value> {
        for output in &mut outputs {
            for usage in output["usages"].as_array_mut().unwrap() {
                let usage = usage.as_object_mut().unwrap();
                usage.remove("call_id");
                usage.remove("started_at");
            }
        }
        outputs
    }
    let first = Server::start(vec![Reply::text("first"); 6]);
    let second = Server::start(vec![Reply::text("second"); 3]);
    let mut definition = parameter_definition();
    definition["nodes"][0]["config"]["parameters"]["call_attribution"] =
        json!({"type":"object", "secret":false});
    definition["nodes"][1]["config"]["attribution"] = json!({"$ref":"params.call_attribution"});
    definition["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"usages", "node":"chat", "port":"usages"}));
    let root = tempfile::tempdir().unwrap();
    let executable = compile(&definition, root.path());
    assert!(first.requests().is_empty());
    let mut invocations = Vec::new();
    let mut expected = Vec::new();
    for (server, key, model, profile, temperature, max_tokens) in [
        (
            &first,
            "synthetic-first-key",
            "first-model",
            Some("quality"),
            0.1,
            16,
        ),
        (
            &second,
            "synthetic-second-key",
            "second-model",
            Some("economy"),
            0.8,
            32,
        ),
        (&first, "synthetic-first-key", "first-model", None, 0.1, 16),
    ] {
        let mut attribution = json!({"purpose":"conversation"});
        if let Some(profile) = profile {
            attribution["profile"] = profile.into();
        }
        let inputs = json!({"params.api_key":key, "params.base_url":server.base_url,
            "params.model":model, "params.prompt":"Hello", "params.temperature":temperature,
            "params.max_tokens":max_tokens, "params.call_attribution":attribution});
        let output = stable(run(&executable, &inputs));
        assert_eq!(output[0]["usages"][0]["attribution"], attribution);
        assert_eq!(output, stable(run_memory(&definition, &inputs)));
        expected.extend(output);
        invocations.push(inputs);
        for request in server.requests() {
            assert!(request.headers.contains(&format!("Bearer {key}")));
            assert_eq!(request.body["model"], model);
            assert_eq!(request.body["temperature"], temperature);
            assert_eq!(request.body["max_tokens"], max_tokens);
            assert!(request.body.get("attribution").is_none());
            if let Some(profile) = profile {
                assert!(!request.body.to_string().contains(profile));
            }
        }
    }
    assert_eq!(
        stable(run_memory(&definition, &json!(invocations))),
        expected
    );
    assert_eq!(first.requests().len(), 6);
    assert_eq!(second.requests().len(), 3);
    assert!(
        first.requests()[2]
            .headers
            .contains("Bearer synthetic-first-key")
    );
    assert!(
        second.requests()[2]
            .headers
            .contains("Bearer synthetic-second-key")
    );
    assert_eq!(second.requests()[2].body["model"], "second-model");
}

#[test]
fn config_references_require_known_explicit_predecessors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = parameter_definition();
    definition["nodes"][1]["config"]["model"] = json!({"$ref":"params.missing"});
    let error =
        compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("is unknown"), "{error}");
    let mut definition = parameter_definition();
    definition["edges"] = json!([]);
    definition["nodes"][1]
        .as_object_mut()
        .unwrap()
        .remove("inputs");
    let error =
        compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires an explicit dependency"),
        "{error}"
    );
}

#[test]
fn invalid_dynamic_configuration_retains_safe_typed_errors() {
    use std::error::Error;
    let registry = NodeRegistry::from_inventory().unwrap();
    for (field, value) in [
        ("temperature", json!("synthetic-secret-value")),
        ("base_url", json!("file:///synthetic-secret-value")),
        ("model", json!(" ")),
        ("api_key", json!("")),
        ("max_tokens", json!(0)),
        ("timeout_ms", json!(0)),
        ("attribution", json!({"profile":"synthetic-secret-value"})),
    ] {
        let mut definition = parameter_definition();
        definition["nodes"][0]["config"]["parameters"]["invalid"] = json!({"type":"any"});
        definition["nodes"][1]["config"]["base_url"] = json!("http://127.0.0.1:9/v1");
        definition["nodes"][1]["config"][field] = json!({"$ref":"params.invalid"});
        let plan =
            compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let recording = Snapshots::memory();
        let mut context = ExecutionContext::default();
        context.set_snapshot_recorder(recording.recorder());
        context.set_workflow_arguments(arguments(&json!({"params.api_key":"synthetic-key", "params.prompt":"Hello", "params.invalid":value})));
        let error = flow.execute_in_context(&mut context).unwrap_err();
        let snapshot = recording.current();
        assert_eq!(
            snapshot.node(&[], "params").unwrap().outputs["invalid"],
            json!("[REDACTED]")
        );
        let mut source: &dyn Error = &error;
        let mut typed = false;
        let mut configuration_error = false;
        loop {
            typed |= source.is::<mf_runtime::InputDecodeError>();
            configuration_error |= matches!(
                source.downcast_ref::<mfn_llm::LlmExecutionError>(),
                Some(mfn_llm::LlmExecutionError::Configuration { .. })
            );
            assert!(!source.to_string().contains("synthetic-secret-value"));
            assert!(!format!("{source:?}").contains("synthetic-secret-value"));
            match source.source() {
                Some(next) => source = next,
                None => break,
            }
        }
        assert!(
            configuration_error,
            "expected configuration validation for {field}: {error}"
        );
        if field == "temperature" {
            assert!(typed);
        }
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
            json!({"chat.prompt":"Hello", "chat.system":"Answer briefly", "chat.history":[{"role": "user", "content": "prior"}, {"role": "assistant", "content": "earlier reply"}]}),
            1,
        ),
        (
            serde_json::from_str::<Value>(
                &fs::read_to_string(workspace.join("examples/llm-loop.json")).unwrap(),
            )
            .unwrap(),
            json!({"repeat.prompt":"Hello", "repeat.history":[], "repeat.text":""}),
            2,
        ),
        (
            json!({"version": "2026-10-03", "nodes": [{"id": "iteration", "kind": "builtin.iteration", "config": {"mode": "parallel", "on_error": "terminate", "body": {"nodes": [{"id": "chat", "kind": "builtin.llm"}], "edges": [{"from_node": "%iteration", "from_output": "item", "to_node": "chat", "to_input": "prompt"}], "result": {"node": "chat", "port": "text"}}}}], "outputs": [{"name": "results", "node": "iteration", "port": "results"}]}),
            json!({"iteration.items":["one", "two"]}),
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
        "version": "2026-10-03", "execution": {"limits": {"workers": 1, "max_pending_messages": 2}},
        "dependencies": dependencies(),
        "nodes": [{"id": "read", "kind": "builtin.readline"}, {"id": "chat", "kind": "builtin.llm"}],
        "edges": [{"from_node": "read", "from_output": "line", "to_node": "chat", "to_input": "prompt"}],
        "outputs": [{"name": "text", "node": "chat", "port": "text"}, {"name": "history", "node": "chat", "port": "history"}]
    });
    configure(&mut value, &server);
    let inputs = json!({"read.path":input});
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

#[test]
fn composed_conversation_compiles_without_application_nodes() {
    let definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/chat-title-flow.json"))
            .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(
        registry
            .iter()
            .all(|registration| !registration.kind.starts_with("mf_chat."))
    );
    let plan = compile_definition(&definition, &registry).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    let arguments = mf_runtime::resolve_workflow_inputs(
        &manifest.input_schema,
        &arguments(&json!({
            "input.prompt":"Hello",
            "input.api_key":"synthetic-key",
            "input.context":{"title":{"source":"provisional","revision":0,"attempted":false,"enabled":true}}
        })),
        |_| None,
    ).unwrap();
    manifest.input_schema.validate(&arguments).unwrap();
    assert_eq!(arguments.0["input.system"].as_str(), Some(""));
    assert_eq!(
        serde_json::to_value(&arguments).unwrap()["input.history"],
        json!([])
    );
    instantiate_stream(&plan, &registry).unwrap();
}

#[test]
fn directed_llm_tools_match_memory_and_generated_finite_and_native_execution() {
    fn stable(mut values: Vec<Value>) -> Vec<Value> {
        for value in &mut values {
            for usage in value["usages"].as_array_mut().unwrap() {
                let usage = usage.as_object_mut().unwrap();
                usage.remove("call_id");
                usage.remove("started_at");
            }
        }
        values
    }
    for native in [false, true] {
        for limit in [1, 4] {
            let tool = Reply::json(
                json!({"id":"first","object":"chat.completion","created":0,"model":"example-model","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"multiply","arguments":"{\"amount\":21}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
            );
            let replies = if limit == 1 {
                vec![tool.clone(), tool]
            } else {
                vec![tool.clone(), Reply::text("42"), tool, Reply::text("42")]
            };
            let server = Server::start(replies);
            let mut definition: Value =
                serde_json::from_str(include_str!("../../../examples/chat-tools-flow.json"))
                    .unwrap();
            definition["dependencies"] = dependencies();
            definition["dependencies"]["code"] = json!({"package":"mfn-code","path":common::crates_dir().join("builtin-nodes/code")});
            definition["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["id"] == "repeat")
                .unwrap()["loop"]["max_iterations"] = limit.into();
            definition["outputs"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"history","node":"repeat","port":"history"}));
            if native {
                definition["execution"] = json!({"limits":{"workers":1,"max_pending_messages":2}});
            }
            let root = tempfile::tempdir().unwrap();
            let executable = compile(&definition, root.path());
            assert!(server.requests().is_empty());
            let inputs = json!({"params.prompt":"Use multiply to double 21","params.base_url":server.base_url,"params.model":"example-model"});
            let memory = stable(run_memory(&definition, &inputs));
            assert_eq!(stable(run(&executable, &inputs)), memory);
            assert_eq!(memory.len(), 1);
            let output = &memory[0];
            let calls = if limit == 1 { 1 } else { 2 };
            assert_eq!(output["usages"].as_array().unwrap().len(), calls);
            assert_eq!(output["history"].as_array().unwrap().len(), calls + 2);
            if limit == 1 {
                assert_eq!(output["status"], "failed");
                assert_eq!(output["error"], "Tool round limit reached");
            } else {
                assert_eq!(output["status"], "succeeded");
                assert_eq!(output["text"], "42");
            }
            let requests = server.requests();
            assert_eq!(requests.len(), calls * 2);
            for request in [&requests[0], &requests[calls]] {
                let properties = &request.body["tools"][0]["function"]["parameters"]["properties"];
                assert!(properties.get("amount").is_some());
                assert!(properties.get("factor").is_none());
            }
            if limit > 1 {
                for request in [&requests[1], &requests[3]] {
                    let messages = request.body["messages"].as_array().unwrap();
                    assert_eq!(
                        messages
                            .iter()
                            .filter(|message| message["role"] == "user")
                            .count(),
                        1
                    );
                    assert_eq!(messages[2]["tool_call_id"], "a");
                    let result: Value =
                        serde_json::from_str(messages[2]["content"].as_str().unwrap()).unwrap();
                    assert_eq!(result, json!({"result":42}));
                }
            }
        }
    }
}
