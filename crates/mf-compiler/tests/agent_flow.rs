#![cfg(unix)]
extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;
mod common;

use mf_compiler::{NodeRegistry, compile_definition, instantiate_stream, load_workflow};
use mf_runtime::{FlowRuntime, StreamOptions, WorkflowArguments};
use mfn_llm::test_support::{Reply, Server};
use serde_json::{Value, json};

fn tool_reply() -> Reply {
    Reply::stream_tool_calls(vec![
        ("first", "multiply", json!({"amount":21})),
        ("other", "add", json!({"a":3,"b":4})),
        ("second", "multiply", json!({"amount":7})),
    ])
}

fn definition() -> mf_compiler::WorkflowDefinition {
    load_workflow(
        &common::crates_dir()
            .parent()
            .unwrap()
            .join("examples/chat-tools-flow.json"),
    )
    .unwrap()
    .definition
}
fn execute(server: &Server, max_rounds: i64) -> Vec<Value> {
    execute_document(&definition(), server, max_rounds, None)
}

fn execute_document(
    document: &mf_compiler::WorkflowDefinition,
    server: &Server,
    max_rounds: i64,
    snapshots: Option<mf_runtime::SnapshotRecorder>,
) -> Vec<Value> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(document, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let arguments=WorkflowArguments::from_json(json!({"params.prompt":"Calculate 21","params.model":"example-model","params.base_url":server.base_url,"params.api_key":"synthetic-key","params.factor":2,"params.max_rounds":max_rounds}).to_string().as_bytes()).unwrap();
    let instance = FlowRuntime::default()
        .start_stream(
            prepared,
            StreamOptions {
                arguments,
                snapshots,
                ..Default::default()
            },
        )
        .unwrap();
    let mut results = vec![];
    while let Some(output) = instance.recv().unwrap() {
        results.push(serde_json::to_value(output.outputs).unwrap());
    }
    results
}

fn terminal(records: &[Value]) -> &Value {
    let terminal: Vec<_> = records
        .iter()
        .filter(|record| record.get("status").is_some())
        .collect();
    assert_eq!(terminal.len(), 1, "{records:#?}");
    terminal[0]
}

#[test]
fn agent_argument_descriptions_survive_freezing_without_becoming_execution_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    let agent = description
        .nodes
        .iter()
        .find(|node| node.id == "agent")
        .unwrap();
    let tools = &agent.subgraph_arguments["tools"];
    assert_eq!(
        tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.kind.as_str()))
            .collect::<Vec<_>>(),
        [("add", "builtin.code"), ("multiply", "workflow.call")]
    );
    assert_eq!(description.nodes.len(), 3);
    assert_eq!(description.static_node_count().unwrap().get(), 34);
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(manifest.description().unwrap(), description);
    let replanned = mf_compiler::plan_definition(&manifest.definition).unwrap();
    assert_eq!(
        mf_compiler::describe_compiled(&replanned).unwrap(),
        description
    );
    let encoded = serde_json::to_value(&agent.subgraph_arguments).unwrap();
    assert!(!encoded.to_string().contains("api_key"));
    assert!(!encoded.to_string().contains("factor"));
}

#[test]
fn builtin_agent_routes_mixed_and_repeated_calls_through_one_iteration() {
    let server = Server::start(vec![tool_reply(), Reply::stream_text("done")]);
    let results = execute(&server, 4);
    let result = terminal(&results);
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["text"], "done");
    assert_eq!(result["usages"].as_array().unwrap().len(), 2);
    assert!(
        results
            .iter()
            .any(|value| value["answer"]["text"] == "done")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let tools = requests[0].body["tools"].as_array().unwrap();
    let multiply = tools
        .iter()
        .find(|tool| tool["function"]["name"] == "multiply")
        .unwrap();
    let properties = &multiply["function"]["parameters"]["properties"];
    assert!(properties.get("amount").is_some());
    assert!(properties.get("factor").is_none());
    let tool_results: Vec<_> = requests[1].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect();
    assert_eq!(tool_results.len(), 3);
    for (result, id, expected) in [
        (tool_results[0], "first", "The product is 42"),
        (tool_results[1], "other", "The sum is 7"),
        (tool_results[2], "second", "The product is 14"),
    ] {
        assert_eq!(result["tool_call_id"], id);
        assert_eq!(
            serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap(),
            json!({"content":expected})
        );
    }
}

#[test]
fn agent_round_budget_is_owned_by_the_flow() {
    let server = Server::start(vec![tool_reply()]);
    let results = execute(&server, 1);
    let result = terminal(&results);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["error"], "Tool round limit reached");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn builtin_agent_generated_runner_keeps_tools_without_source_files() {
    use mf_compiler::{
        CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options,
    };
    use std::{fs, process::Command};
    let server = Server::start(vec![tool_reply(), Reply::stream_text("done")]);
    let definition = definition();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("flow.json");
    let build = root.path().join("build");
    let executable = root.path().join("agent");
    let encoded = serde_json::to_string(&definition).unwrap();
    assert!(!encoded.contains("flows/multiply-tool.json"));
    fs::write(&source, encoded).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &source,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: true },
    )
    .unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(&build).unwrap();
    let output = Command::new(&executable)
        .args([
            "--params.prompt=Calculate 21",
            "--params.model=example-model",
            "--params.api_key=synthetic-key",
            "--params.factor=2",
        ])
        .arg(format!("--params.base_url={}", server.base_url))
        .env_clear()
        .env("PATH", "")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(
        records
            .iter()
            .any(|value| value["answer"]["text"] == "done")
    );
    let result = terminal(&records);
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["text"], "done");
    assert_eq!(result["usages"].as_array().unwrap().len(), 2);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let results: Vec<_> = requests[1].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect();
    assert_eq!(
        results
            .iter()
            .map(|value| value["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "other", "second"]
    );
}

#[test]
fn unobserved_agent_stream_keeps_normal_unary_workflow_execution() {
    let tool = Reply::json(
        json!({"id":"tool","object":"chat.completion","created":0,"model":"example-model","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"one","type":"function","function":{"name":"multiply","arguments":"{\"amount\":21}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
    );
    let server = Server::start(vec![tool, Reply::text("done")]);
    let mut definition = definition();
    definition.outputs.retain(|output| output.name != "answer");
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_workflow_arguments(WorkflowArguments::from_json(json!({"params.prompt":"Calculate 21","params.model":"example-model","params.base_url":server.base_url,"params.api_key":"synthetic-key"}).to_string().as_bytes()).unwrap());
    let output = flow.execute_in_context(&mut context).unwrap();
    assert_eq!(output["text"], json!("done"));
    assert!(
        server
            .requests()
            .iter()
            .all(|request| request.body.get("stream").is_none_or(|value| value != true))
    );
}

fn failure(server: &Server, overrides: Value) -> (String, Vec<String>) {
    failure_in(&definition(), server, overrides)
}

fn failure_in(
    definition: &mf_compiler::WorkflowDefinition,
    server: &Server,
    overrides: Value,
) -> (String, Vec<String>) {
    use std::sync::{Arc, Mutex};
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let mut inputs = json!({"params.prompt":"Calculate 21","params.model":"example-model","params.base_url":server.base_url,"params.api_key":"synthetic-key"});
    inputs
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    let nodes = Arc::new(Mutex::new(vec![]));
    let recorded = nodes.clone();
    let recorder = mf_runtime::SnapshotRecorder::new(move |event| {
        recorded.lock().unwrap().push(event.node.to_string())
    });
    let arguments = WorkflowArguments::from_json(inputs.to_string().as_bytes()).unwrap();
    let instance = FlowRuntime::default()
        .start_stream(
            prepared,
            StreamOptions {
                arguments,
                snapshots: Some(recorder),
                ..Default::default()
            },
        )
        .unwrap();
    let error = loop {
        match instance.recv() {
            Err(error) => break error.to_string(),
            Ok(Some(_)) => {}
            Ok(None) => panic!("agent unexpectedly completed"),
        }
    };
    let recorded = nodes.lock().unwrap().clone();
    (error, recorded)
}

#[test]
fn agent_limits_and_malformed_calls_prevent_tool_side_effects() {
    let server = Server::start(vec![]);
    let (error, nodes) = failure(&server, json!({"params.max_rounds":0}));
    assert!(error.contains("Agent limits exceed"), "{error}");
    assert!(server.requests().is_empty());
    assert!(!nodes.iter().any(|node| node == "chat"));
    let server = Server::start(vec![tool_reply()]);
    let (error, nodes) = failure(&server, json!({"params.max_tool_calls":1}));
    assert!(error.contains("exceeds max_tool_calls"), "{error}");
    assert_eq!(server.requests().len(), 1);
    assert!(!nodes.iter().any(|node| node == "dispatch"));
    let server = Server::start(vec![Reply::stream_tool_calls(vec![(
        "bad",
        "multiply",
        json!({}),
    )])]);
    let (error, nodes) = failure(&server, json!({}));
    assert!(
        error.contains("required input was not provided") && error.contains("amount"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1);
    assert!(
        !nodes
            .iter()
            .any(|node| node == "multiply_flow" || node == "multiply")
    );
}

#[test]
fn caller_bindings_are_checked_before_model_io_and_cannot_be_overwritten() {
    let server = Server::start(vec![]);
    let mut document = definition();
    let params = document
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "params")
        .unwrap();
    params.config["parameters"]["factor"]["type"] = "any".into();
    let bindings = document
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "tool_inputs")
        .unwrap();
    bindings.config["inputs"]["factor"] = "any".into();
    let (error, nodes) = failure_in(&document, &server, json!({"params.factor":"invalid"}));
    assert!(
        error.contains("invalid bindings for tool `multiply`"),
        "{error}"
    );
    assert!(server.requests().is_empty());
    assert!(!nodes.iter().any(|node| node == "chat"));

    let server = Server::start(vec![Reply::stream_tool_calls(vec![(
        "bad",
        "multiply",
        json!({"amount":21,"factor":9}),
    )])]);
    let (error, nodes) = failure(&server, json!({}));
    assert!(error.contains("already bound by the caller"), "{error}");
    assert_eq!(server.requests().len(), 1);
    assert!(!nodes.iter().any(|node| node == "multiply_flow"));
}

#[cfg(feature = "codegen")]
#[test]
fn public_codegen_preserves_the_agent_contract_without_model_or_tool_execution() {
    let server = Server::start(vec![]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let expected = plan.manifest(&registry).unwrap();
    for standard in [false, true] {
        let artifacts = if standard {
            plan.generate_runner_execution_plans(&registry)
        } else {
            plan.generate_execution_plans(&registry)
        }
        .unwrap();
        let manifest = mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
        assert_eq!(manifest, expected);
        let frozen = mf_compiler::plan_definition(&manifest.definition).unwrap();
        let generated = frozen.generate_runner_artifacts().unwrap();
        assert_eq!(
            mf_compiler::CompiledWorkflow::from_json(&generated.plan_json).unwrap(),
            frozen
        );
        let targets: Vec<mf_runtime::NodeTarget> =
            serde_json::from_str(&artifacts.node_targets_json).unwrap();
        assert!(targets.iter().any(|target| target.id == "agent"));
    }
    assert!(server.requests().is_empty());
}

#[test]
fn private_caller_bindings_stay_out_of_model_messages_and_snapshots() {
    use std::sync::{Arc, Mutex};
    let examples = common::crates_dir().parent().unwrap().join("examples");
    let mut authored: Value =
        serde_json::from_slice(&std::fs::read(examples.join("chat-tools-flow.json")).unwrap())
            .unwrap();
    authored["nodes"][0]["config"]["parameters"]["token"] =
        json!({"type":"string","default":"private-value","secret":true});
    let nodes = authored["nodes"].as_array_mut().unwrap();
    let bindings = nodes
        .iter_mut()
        .find(|node| node["id"] == "tool_inputs")
        .unwrap();
    bindings["config"]["inputs"] = json!({"token":"string"});
    bindings["config"]["code"]["value"] = "{'private':{'token':token}}".into();
    bindings["inputs"] = json!({"token":{"from":{"node":"params","port":"token"}}});
    let agent = nodes.iter_mut().find(|node| node["id"] == "agent").unwrap();
    agent["subgraphs"]["tools"] = json!({"collection":{"private":{"node":{"id":"private","kind":"builtin.parameters","config":{"parameters":{"value":{"type":"string","secret":false},"token":{"type":"string","secret":true}}}}}}});
    let document =
        mf_compiler::resolve_flows(&serde_json::from_value(authored).unwrap(), Some(&examples))
            .unwrap()
            .definition;
    let server = Server::start(vec![
        Reply::stream_tool_calls(vec![(
            "private-call",
            "private",
            json!({"value":"visible"}),
        )]),
        Reply::stream_text("done"),
    ]);
    let captured = Arc::new(Mutex::new(Vec::new()));
    let saved = captured.clone();
    let recorder = mf_runtime::SnapshotRecorder::new(move |event| {
        saved
            .lock()
            .unwrap()
            .push(serde_json::to_string(&event).unwrap())
    });
    let outputs = execute_document(&document, &server, 4, Some(recorder));
    assert_eq!(terminal(&outputs)["text"], "done");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.body.to_string().contains("private-value"))
    );
    let properties = &requests[0].body["tools"][0]["function"]["parameters"]["properties"];
    assert!(properties.get("value").is_some() && properties.get("token").is_none());
    let snapshots = captured.lock().unwrap();
    assert!(!snapshots.is_empty());
    assert!(
        snapshots
            .iter()
            .all(|snapshot| !snapshot.contains("private-value"))
    );
}
