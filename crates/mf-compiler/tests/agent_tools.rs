#![cfg(unix)]

mod common;
#[path = "common/snapshots.rs"]
mod snapshots;
#[path = "common/tool_values.rs"]
mod tool_values;

extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
    resolve_flows,
};
use mf_runtime::{
    CompiledGraph, ExecutionContext, FlowRuntime, SnapshotOutcome, StreamOptions, WorkflowArguments,
};
use mfn_llm::{
    flows::{agent_definition, agent_turn_definition},
    test_support::{Reply, Server, messages},
};
use serde_json::{Value, json};
use snapshots::Snapshots;
use std::collections::BTreeMap;
use tool_values::{FlowTool, configured_agent};

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
        "operation":{"description":"The arithmetic operation."},
        "left":{"description":"First operand."},
        "right":{"description":"Second operand."}
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

fn assert_catalog_once_outside_loop(recording: &Snapshots, tool_count: usize) {
    let history = recording.history();
    let started = history
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry
                .snapshot
                .node(&entry.scope, &entry.node)
                .unwrap()
                .outcome
                == SnapshotOutcome::Started
        })
        .collect::<Vec<_>>();
    let catalogs = started
        .iter()
        .filter(|(_, entry)| {
            entry.node.as_ref() == "catalog"
                && entry
                    .scope
                    .iter()
                    .map(|scope| scope.node_id())
                    .eq(["agent"])
        })
        .collect::<Vec<_>>();
    assert_eq!(catalogs.len(), 1);
    let (repeat_index, repeat) = started
        .iter()
        .find(|(_, entry)| entry.node.as_ref() == "repeat")
        .unwrap();
    assert!(catalogs[0].0 < *repeat_index);
    let projections = started
        .iter()
        .filter(|(_, entry)| entry.node.as_ref() == "definition")
        .collect::<Vec<_>>();
    assert_eq!(projections.len(), tool_count);
    for (index, projection) in projections {
        assert!(*index < *repeat_index);
        assert!(
            projection
                .scope
                .iter()
                .any(|scope| scope.node_id() == "catalog")
        );
        assert!(
            projection
                .scope
                .iter()
                .all(|scope| scope.node_id() != "repeat")
        );
    }
    // Runtime capabilities stay native through the caller map and Loop state;
    // snapshots expose neither executable bodies nor the chat credential.
    let snapshot = repeat.snapshot.node(&repeat.scope, &repeat.node).unwrap();
    let tools = snapshot.inputs.get("tools").unwrap().as_object().unwrap();
    assert_eq!(tools.len(), tool_count);
    // Each capability stays a native Flow; the callable properties travel beside it.
    assert!(tools.values().all(|tool| tool.as_str() == Some("<Flow>")));
    let properties = snapshot
        .inputs
        .get("tool_properties")
        .unwrap()
        .as_object()
        .unwrap();
    assert_eq!(properties.len(), tool_count);
    for property in properties.values() {
        let record = property.as_object().unwrap();
        assert_eq!(record.len(), 3);
        assert!(
            record["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty() && description != "[REDACTED]")
        );
        assert!(record["parameters"].is_object());
        assert_eq!(record["interface"]["callable"].as_bool(), Some(true));
    }
    for entry in history {
        let snapshot = entry.snapshot.node(&entry.scope, &entry.node).unwrap();
        let serialized = serde_json::to_string(snapshot).unwrap();
        assert!(
            !serialized.contains("synthetic-agent-key"),
            "{}: {serialized}",
            entry.node
        );
        assert!(
            !serialized.contains("flow_0"),
            "{}: {serialized}",
            entry.node
        );
    }
}

// Compare authored/lowered definitions recursively, not observation descriptions:
// runtime origins legitimately include the caller's differing root workflow ID.
fn author_graph(graph: &CompiledGraph) -> Value {
    let children = graph
        .children
        .iter()
        .map(|(owner, bindings)| {
            let bindings = bindings
                .iter()
                .map(|(binding, child)| (binding.clone(), author_graph(child)))
                .collect::<BTreeMap<_, _>>();
            (owner.as_str(), bindings)
        })
        .collect::<BTreeMap<_, _>>();
    json!({"definition":graph.definition,"children":children})
}

#[test]
fn zero_one_and_three_runtime_tools_use_the_same_fixed_agent_child_and_ports() {
    let source = agent_definition().unwrap();
    let value = serde_json::to_value(&source).unwrap();
    let node = |id: &str| {
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap()
    };
    let parameters = node("params")["config"]["parameters"].as_object().unwrap();
    assert_eq!(parameters.len(), 8);
    assert_eq!(parameters["tools"]["type"], json!({"map":"flow"}));
    assert_eq!(
        parameters["tool_properties"]["type"],
        json!({"map":"object"})
    );
    assert!(parameters["tools"].get("default").is_none());
    assert!(parameters.get("descriptions").is_none());
    assert!(!value.to_string().contains("flow_0"));
    assert_eq!(node("catalog")["kind"], "workflow.call");
    let catalog_flow = node("catalog")["config"]["flow"].as_str().unwrap();
    assert_eq!(
        value["flows"][catalog_flow]["builtin"],
        "mfn-llm.tool-catalog"
    );
    let repeat = node("repeat");
    let shared_inputs = repeat["config"]["inputs"].as_object().unwrap();
    assert_eq!(shared_inputs.len(), 6);
    assert_eq!(shared_inputs["tools"], json!({"map":"flow"}));
    assert_eq!(shared_inputs["tool_properties"], json!({"map":"object"}));
    assert_eq!(shared_inputs["tool_definitions"], json!({"list":"object"}));
    let variables = repeat["config"]["variables"].as_array().unwrap();
    assert_eq!(variables.len(), 5);
    assert!(variables.iter().all(|variable| variable["type"] != "flow"));
    let body = &repeat["config"]["body"];
    let dispatch = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "dispatch")
        .unwrap();
    let dispatch_flow = dispatch["config"]["flow"].as_str().unwrap();
    assert_eq!(body["flows"][dispatch_flow]["builtin"], "mfn-llm.tool-uses");
    assert_eq!(
        dispatch["inputs"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["params.calls", "params.tool_properties", "params.tools"]
    );
    assert!(dispatch["config"].get("body").is_none());

    let registry = NodeRegistry::from_inventory().unwrap();
    let calculator = calculator_tools().remove("calculator").unwrap();
    let mut resolved_baseline = None;
    let mut compiled_baseline = None;
    let mut caller_baseline = None;
    for names in [
        vec![],
        vec!["calculator"],
        vec!["add", "multiply", "subtract"],
    ] {
        let tools = names
            .iter()
            .map(|name| ((*name).to_owned(), calculator.clone()))
            .collect();
        let definition = configured_agent(tools);
        let caller = definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "agent")
            .unwrap();
        if let Some(baseline) = &caller_baseline {
            assert_eq!(caller, baseline);
        } else {
            caller_baseline = Some(caller.clone());
        }
        let resolved = resolve_flows(&definition, None).unwrap();
        let resolved_child = &resolved.children[&"agent".into()]["body"];
        if let Some(baseline) = &resolved_baseline {
            assert_eq!(&resolved_child.definition, baseline);
        } else {
            resolved_baseline = Some(resolved_child.definition.clone());
        }
        let plan = compile_definition(&definition, &registry).unwrap();
        let child = &plan.graph.children[&"agent".into()]["body"];
        let shape = author_graph(child);
        if let Some(baseline) = &compiled_baseline {
            assert_eq!(&shape, baseline);
        } else {
            compiled_baseline = Some(shape);
        }
        assert_eq!(
            child.children[&"catalog".into()]["body"]
                .definition
                .nodes
                .iter()
                .filter(|node| node.kind == mf_runtime::ITERATION_KIND)
                .count(),
            1
        );

        // Exercise the actual native map, catalog and dispatch, not just example JSON.
        let replies = if let Some(name) = names.last() {
            vec![
                tool_reply(
                    json!([call(
                        "selected",
                        name,
                        json!({"operation":"add", "left":2.0, "right":3.0})
                    )]),
                    false,
                ),
                Reply::text("Runtime tools work"),
            ]
        } else {
            vec![Reply::text("Runtime tools work")]
        };
        let server = Server::start(replies);
        let (output, recording) = memory(definition, inputs(&server));
        assert_success(
            &output,
            "Runtime tools work",
            if names.is_empty() { 1 } else { 2 },
        );
        assert_catalog_once_outside_loop(&recording, names.len());
        let requests = server.requests();
        let advertised = requests[0]
            .body
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(advertised, names);
        if !names.is_empty() {
            assert_eq!(
                requests[1].body["messages"]
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap(),
                &json!({"role":"tool", "tool_call_id":"selected", "content":"5.0"})
            );
        }
        assert_eq!(agent_definition().unwrap(), source);
    }
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
        let definition = configured_agent(calculator_tools());
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
            assert_catalog_once_outside_loop(&recording, 1);
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
    let (output, recording) = memory(configured_agent(BTreeMap::new()), inputs(&server));
    assert_success(&output, "Direct answer", 1);
    assert_catalog_once_outside_loop(&recording, 0);
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
    let (output, _) = memory(configured_agent(calculator_tools()), inputs(&server));
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
    let (output, recording) = memory(configured_agent(calculator_tools()), inputs(&server));
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
    assert_catalog_once_outside_loop(&recording, 1);
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
    let (output, _) = memory(configured_agent(calculator_tools()), inputs(&server));
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
    // A dynamic map caller cannot enumerate unexpected keys, so the report is
    // deliberately coarser than a compiled adapter's single-key message.
    assert!(errors[4].contains("unknown or secret argument"));
    assert_eq!(messages[7]["content"], "5.0");
}

#[test]
fn secret_tool_results_reach_the_model_but_not_agent_snapshots_or_catalog() {
    const SECRET: &str = "synthetic-private-agent-tool-result";
    let tool = FlowTool {
        description: "Return a privately bound value.".into(),
        parameters: BTreeMap::new(),
        body: WorkflowDefinition::from_json(
            &json!({
                "version":"2026-10-28", "nodes":[{
                    "id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                        "content":{"type":"string", "default":SECRET, "secret":true}
                    }}
                }], "outputs":[{"name":"content", "node":"params", "port":"content"}]
            })
            .to_string(),
        )
        .unwrap(),
    };
    let server = Server::start(vec![
        tool_reply(json!([call("private-result", "private", json!({}))]), false),
        Reply::text("Private result received"),
    ]);
    let (output, recording) = memory(
        configured_agent(BTreeMap::from([("private".into(), tool)])),
        inputs(&server),
    );
    assert_success(&output, "Private result received", 2);
    assert_catalog_once_outside_loop(&recording, 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].body["tools"].to_string().contains(SECRET));
    assert_eq!(
        requests[0].body["tools"][0]["function"]["parameters"]["properties"],
        json!({})
    );
    assert_eq!(
        requests[1].body["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        &json!({"role":"tool", "tool_call_id":"private-result", "content":SECRET})
    );
    assert!(!output.to_string().contains(SECRET));
    let history = recording.history();
    assert!(
        history
            .iter()
            .any(|entry| entry.node.as_ref() == "dispatch")
    );
    for entry in history {
        let snapshot = entry.snapshot.node(&entry.scope, &entry.node).unwrap();
        let serialized = serde_json::to_string(snapshot).unwrap();
        assert!(!serialized.contains(SECRET), "{}: {serialized}", entry.node);
    }
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
        let (output, recording) = memory(configured_agent(calculator_tools()), inputs(&server));
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
