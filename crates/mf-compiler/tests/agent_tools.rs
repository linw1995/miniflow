#![cfg(unix)]

mod common;
#[path = "common/snapshots.rs"]
mod snapshots;

extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::{
    ExecutionContext, FlowRuntime, SnapshotOutcome, StreamOptions, WorkflowArguments,
};
use mfn_llm::{
    flows::{FlowTool, agent_definition, agent_turn_definition},
    test_support::{Reply, Server, messages},
};
use serde_json::{Value, json};
use snapshots::Snapshots;
use std::collections::BTreeMap;

fn local_dependencies(definition: &mut WorkflowDefinition) {
    fn select_sources(value: &mut Value, dependencies: &Value) {
        match value {
            Value::Object(fields) => {
                if fields.contains_key("nodes") && fields.contains_key("version") {
                    fields.insert("dependencies".into(), dependencies.clone());
                }
                for value in fields.values_mut() {
                    select_sources(value, dependencies);
                }
            }
            Value::Array(values) => {
                for value in values {
                    select_sources(value, dependencies);
                }
            }
            _ => {}
        }
    }
    let dependencies = json!({
        "core": {"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
        "code": {"package":"mfn-code", "path":common::crates_dir().join("builtin-nodes/code")},
        "llm": {"package":"mfn-llm", "path":common::crates_dir().join("builtin-nodes/llm")}
    });
    let mut value = serde_json::to_value(&*definition).unwrap();
    select_sources(&mut value, &dependencies);
    *definition = serde_json::from_value(value).unwrap();
}

fn calculator_tools() -> BTreeMap<String, FlowTool> {
    let mut body = WorkflowDefinition::from_json(include_str!(
        "../../../examples/agent-calculator/calculator.json"
    ))
    .unwrap();
    // The included example's relative dependency belongs to its source directory,
    // not the compiler test's directory.
    local_dependencies(&mut body);
    let parameters = serde_json::from_value(json!({
        "calculate.operation":{"description":"The arithmetic operation."},
        "calculate.left":{"description":"First operand."},
        "calculate.right":{"description":"Second operand."}
    }))
    .unwrap();
    BTreeMap::from([(
        "calculator".into(),
        FlowTool {
            description: "Calculate with two numbers.".into(),
            parameters,
            body,
        },
    )])
}

fn agent(tools: BTreeMap<String, FlowTool>) -> WorkflowDefinition {
    let mut definition = agent_definition(tools).unwrap();
    local_dependencies(&mut definition);
    definition
}

fn inputs(server: &Server) -> Value {
    json!({
        "params.prompt":"Calculate two plus three and three times four.",
        "params.model":"example-model",
        "params.base_url":server.base_url,
        "params.api_key":"synthetic-agent-key"
    })
}

fn arguments(inputs: Value) -> WorkflowArguments {
    WorkflowArguments::try_from(inputs).unwrap()
}

fn memory(mut definition: WorkflowDefinition, inputs: Value) -> (Value, Snapshots) {
    // The same definition exposes a streaming answer. A synchronous consumer
    // deliberately subscribes only to its terminal outputs.
    definition.outputs.retain(|output| output.name != "answer");
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let recording = Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recording.recorder());
    context.set_workflow_arguments(arguments(inputs));
    let outputs = flow.execute_in_context(&mut context).unwrap();
    (serde_json::to_value(outputs).unwrap(), recording)
}

fn finite_stream(mut definition: WorkflowDefinition, inputs: Value) -> Vec<Value> {
    definition.execution = Some(Default::default());
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
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
    let mut outputs = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        outputs.push(serde_json::to_value(output.outputs).unwrap());
    }
    outputs
}

fn terminal(outputs: &[Value]) -> &Value {
    let terminals = outputs
        .iter()
        .filter(|output| output.get("status").is_some())
        .collect::<Vec<_>>();
    assert_eq!(terminals.len(), 1, "{outputs:?}");
    terminals[0]
}

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id, "type":"function", "function":{
        "name":name, "arguments":arguments.to_string()
    }})
}

fn tool_reply(calls: Value, stream: bool) -> Reply {
    if stream {
        let mut indexed = calls;
        for (index, call) in indexed.as_array_mut().unwrap().iter_mut().enumerate() {
            call["index"] = json!(index);
        }
        let chunk = json!({"id":"agent-tools", "object":"chat.completion.chunk", "created":0,
            "model":"example-model", "choices":[{"index":0, "delta":{"tool_calls":indexed}, "finish_reason":null}]});
        let finish = json!({"id":"agent-tools", "object":"chat.completion.chunk", "created":0,
            "model":"example-model", "choices":[{"index":0, "delta":{}, "finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
        let mut reply = Reply::stream_text("");
        reply.body = format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
        reply
    } else {
        Reply::json(
            json!({"id":"agent-tools", "object":"chat.completion", "created":0,
            "model":"example-model", "choices":[{"index":0,
                "message":{"role":"assistant", "content":null, "tool_calls":calls},
                "finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
        )
    }
}

fn assert_success(output: &Value, text: &str, calls: usize) {
    assert_eq!(output["status"], "succeeded", "{output}");
    assert_eq!(output["text"], text);
    assert!(output.get("error").is_none_or(|error| error == ""));
    let usages = output["usages"].as_array().unwrap();
    assert_eq!(usages.len(), calls);
    for usage in usages {
        assert_eq!(usage["node"]["kind"], "builtin.llm");
        assert_eq!(
            usage["usage"],
            json!({"input_tokens":2,"output_tokens":1,"total_tokens":3})
        );
    }
}

fn assert_calculator_schema(tools: &Value) {
    assert_eq!(
        tools,
        &json!([{"type":"function", "function":{
            "name":"calculator", "description":"Calculate with two numbers.",
            "parameters":{
                "type":"object", "properties":{
                    "left":{"type":"number", "description":"First operand."},
                    "operation":{"type":"string", "description":"The arithmetic operation."},
                    "right":{"type":"number", "description":"Second operand."}
                },
                "required":["left","operation","right"], "additionalProperties":false
            }
        }}])
    );
}

#[test]
fn calculator_parallel_calls_preserve_provider_order_schema_and_usage_in_both_modes() {
    for stream in [false, true] {
        let calls = json!([
            call(
                "addition",
                "calculator",
                json!({"operation":"add", "left":2.0, "right":3.0})
            ),
            call(
                "product",
                "calculator",
                json!({"operation":"multiply", "left":3.0, "right":4.0})
            )
        ]);
        let completion = if stream {
            Reply::stream_text("Five and twelve")
        } else {
            Reply::text("Five and twelve")
        };
        let server = Server::start(vec![tool_reply(calls.clone(), stream), completion]);
        let definition = agent(calculator_tools());
        let mut args = inputs(&server);
        args["params.system"] = json!("Use the calculator.");
        args["params.history"] = json!([
            {"role":"user", "content":"Earlier question"},
            {"role":"assistant", "content":"Earlier answer"}
        ]);
        if stream {
            let outputs = finite_stream(definition, args.clone());
            assert_success(terminal(&outputs), "Five and twelve", 2);
            assert_eq!(
                outputs
                    .iter()
                    .filter_map(|output| output["answer"]["text"].as_str())
                    .collect::<String>(),
                "Five and twelve"
            );
        } else {
            let (output, recording) = memory(definition, args.clone());
            assert_success(&output, "Five and twelve", 2);
            // Projection is per invocation, not per turn or parallel tool item.
            let projections = recording
                .history()
                .into_iter()
                .filter(|entry| {
                    entry.scope.is_empty()
                        && entry.node.as_ref() == "definition_0"
                        && entry
                            .snapshot
                            .node(&entry.scope, &entry.node)
                            .unwrap()
                            .outcome
                            == SnapshotOutcome::Started
                })
                .count();
            assert_eq!(projections, 1);
        }
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_calculator_schema(&requests[0].body["tools"]);
        assert_eq!(requests[1].body["tools"], requests[0].body["tools"]);
        let initial = json!([
            {"role":"system", "content":"Use the calculator."},
            {"role":"user", "content":"Earlier question"},
            {"role":"assistant", "content":"Earlier answer"},
            {"role":"user", "content":args["params.prompt"]}
        ]);
        assert_eq!(json!(messages(&requests[0])), initial);
        // Preserve the provider's multipart system/assistant content encoding.
        let mut continued = requests[0].body["messages"].as_array().unwrap().clone();
        continued.extend([
            json!({"role":"assistant", "tool_calls":calls}),
            json!({"role":"tool", "tool_call_id":"addition", "content":"5.0"}),
            json!({"role":"tool", "tool_call_id":"product", "content":"12.0"}),
        ]);
        // Compare the entire wire conversation, not just IDs or the last result.
        assert_eq!(requests[1].body["messages"], json!(continued));
        for request in requests {
            assert_eq!(request.body["model"], "example-model");
            assert_eq!(request.body["stream"].as_bool().unwrap_or(false), stream);
            assert!(request.headers.contains("Bearer synthetic-agent-key"));
        }
    }
}

#[test]
fn empty_agent_completes_a_direct_answer_without_advertising_tools() {
    let server = Server::start(vec![Reply::text("Direct answer")]);
    let (output, _) = memory(agent(BTreeMap::new()), inputs(&server));
    assert_success(&output, "Direct answer", 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .body
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(
        json!(messages(&requests[0])),
        json!([
            {"role":"user", "content":inputs(&server)["params.prompt"]}
        ])
    );
}

#[test]
fn agent_turn_forwards_finite_stream_answer_and_terminal_usage() {
    let server = Server::start(vec![Reply::stream_text("Direct streamed answer")]);
    let mut definition = agent_turn_definition().unwrap();
    local_dependencies(&mut definition);
    let mut args = inputs(&server);
    args.as_object_mut().unwrap().remove("params.prompt");
    args["params.messages"] = json!([{"role":"user", "content":"Hello"}]);
    let outputs = finite_stream(definition, args);
    let output = terminal(&outputs);
    assert_success(output, "Direct streamed answer", 1);
    assert_eq!(output["outcome"], "completed");
    assert_eq!(output["tool_calls"], json!([]));
    assert_eq!(
        outputs
            .iter()
            .filter_map(|output| output["answer"]["text"].as_str())
            .collect::<String>(),
        "Direct streamed answer"
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn provider_failure_is_a_failed_agent_result_without_another_turn() {
    let mut reply = Reply::json(json!({"error":{"message":"PRIVATE_PROVIDER_SENTINEL"}}));
    reply.status = 401;
    let server = Server::start(vec![reply]);
    let (output, _) = memory(agent(calculator_tools()), inputs(&server));
    assert_eq!(output["status"], "failed");
    assert_eq!(output["text"], "");
    let error = output["error"].as_str().unwrap();
    assert!(error.contains("401"), "{output}");
    assert!(!output.to_string().contains("PRIVATE_PROVIDER_SENTINEL"));
    assert!(!output.to_string().contains("synthetic-agent-key"));
    assert_eq!(output["usages"].as_array().unwrap().len(), 1);
    assert!(output["usages"][0]["error"].as_str().is_some());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn native_agent_loop_stops_after_eight_tool_turns() {
    let replies = (0..8)
        .map(|round| {
            tool_reply(
                json!([call(
                    &format!("round-{round}"),
                    "calculator",
                    json!({"operation":"add", "left":2.0, "right":3.0})
                )]),
                false,
            )
        })
        .collect();
    let server = Server::start(replies);
    let (output, recording) = memory(agent(calculator_tools()), inputs(&server));
    assert_eq!(output["status"], "failed");
    assert!(
        output["error"]
            .as_str()
            .unwrap()
            .contains("8-iteration limit"),
        "{output}"
    );
    assert_eq!(output["usages"].as_array().unwrap().len(), 8);
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    for (round, request) in requests.iter().enumerate() {
        assert_eq!(request.body["tools"], requests[0].body["tools"]);
        assert_eq!(
            request.body["messages"].as_array().unwrap().len(),
            1 + round * 2
        );
        if round > 0 {
            assert_eq!(
                request.body["messages"][round * 2],
                json!({
                    "role":"tool", "tool_call_id":format!("round-{}", round - 1), "content":"5.0"
                })
            );
        }
    }
    assert_eq!(
        recording
            .history()
            .into_iter()
            .filter(|entry| {
                entry.scope.is_empty()
                    && entry.node.as_ref() == "definition_0"
                    && entry
                        .snapshot
                        .node(&entry.scope, &entry.node)
                        .unwrap()
                        .outcome
                        == SnapshotOutcome::Started
            })
            .count(),
        1
    );
}

#[test]
fn unknown_and_invalid_tool_arguments_are_ordered_results_and_the_agent_can_recover() {
    let calls = json!([
        call("unknown", "missing", json!({})),
        call("array", "calculator", json!([])),
        call(
            "required",
            "calculator",
            json!({"operation":"add", "left":2.0})
        ),
        call(
            "wrong-type",
            "calculator",
            json!({"operation":"add", "left":"two", "right":3.0})
        ),
        call(
            "extra",
            "calculator",
            json!({"operation":"add", "left":2.0, "right":3.0, "extra":true})
        ),
        call(
            "valid",
            "calculator",
            json!({"operation":"add", "left":2.0, "right":3.0})
        )
    ]);
    let server = Server::start(vec![
        tool_reply(calls.clone(), false),
        Reply::text("Recovered"),
    ]);
    let (output, _) = memory(agent(calculator_tools()), inputs(&server));
    assert_success(&output, "Recovered", 2);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["tools"], requests[0].body["tools"]);
    let messages = requests[1].body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 8);
    assert_eq!(messages[1], json!({"role":"assistant", "tool_calls":calls}));
    for (message, id) in messages[2..].iter().zip([
        "unknown",
        "array",
        "required",
        "wrong-type",
        "extra",
        "valid",
    ]) {
        assert_eq!(message["role"], "tool");
        assert_eq!(message["tool_call_id"], id);
    }
    let errors = messages[2..7]
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["content"].as_str().unwrap()).unwrap()["error"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(errors[0], "unknown tool");
    assert!(errors[1].contains("arguments must be a JSON object"));
    assert!(errors[2].contains("required"));
    assert!(errors[3].contains("left"));
    assert!(errors[4].contains("unknown or secret argument `extra`"));
    assert_eq!(messages[7]["content"], "5.0");
}

#[test]
fn malformed_provider_calls_fail_without_tool_dispatch_or_retry() {
    for calls in [
        json!([call(
            " ",
            "calculator",
            json!({"operation":"add", "left":2.0, "right":3.0})
        )]),
        json!([call("blank-name", " ", json!({}))]),
        json!([{"id":"invalid-json", "type":"function", "function":{"name":"calculator", "arguments":"{"}}]),
    ] {
        let server = Server::start(vec![tool_reply(calls, false)]);
        let (output, recording) = memory(agent(calculator_tools()), inputs(&server));
        assert_eq!(output["status"], "failed", "{output}");
        assert!(!output["error"].as_str().unwrap().is_empty());
        assert_eq!(output["usages"].as_array().unwrap().len(), 1);
        assert_eq!(server.requests().len(), 1);
        assert!(
            recording
                .history()
                .iter()
                .all(|entry| entry.node.as_ref() != "calculate")
        );
    }
}
