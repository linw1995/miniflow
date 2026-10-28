mod support;

use mf_runtime::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    process::Command,
    sync::{Arc, Mutex},
};
use support::{Reply, Server};

type Invoke =
    dyn Fn(Inputs, &mut ExecutionContext) -> Result<NodeResult, NodeExecutionError> + Send + Sync;
struct Task(Box<Invoke>);
impl TaskNode for Task {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        (self.0)(inputs, context)
    }
}
struct Sink(Arc<Mutex<Vec<Value>>>);
impl EmissionSink for Sink {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::to_value(result.outputs).unwrap());
        Ok(())
    }
}
fn port(name: &'static str) -> PortSpec {
    PortSpec::new(name, ValueType::String, true)
}
fn probe(server: &Server, options: Value) -> Value {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tool_probe", "--ignored", "--nocapture"])
        .env("MINIFLOW_TOOL_OPTIONS", options.to_string())
        .env("MINIFLOW_TOOL_URL", &server.base_url)
        .env("MINIFLOW_TOOL_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("TOOL_RESULT:"))
            .unwrap(),
    )
    .unwrap()
}

#[test]
#[ignore = "invoked as an isolated credential subprocess"]
fn tool_probe() {
    let options: Value =
        serde_json::from_str(&std::env::var("MINIFLOW_TOOL_OPTIONS").unwrap()).unwrap();
    let mut config = json!({"model":"example-model","api_key_env":"MINIFLOW_TOOL_KEY","base_url":std::env::var("MINIFLOW_TOOL_URL").unwrap(),"call_outputs":{"query":"string"}});
    if let Some(patch) = options["config"].as_object() {
        config.as_object_mut().unwrap().extend(patch.clone());
    }
    let prepared = mfn_llm::prepare_llm(config).unwrap();
    let mut metadata = prepared.metadata;
    metadata.ports.inputs.clear();
    metadata.input_schemas.clear();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected task stream")
    };
    let results = Arc::new(Mutex::new(None));
    let recorded = results.clone();
    let deltas = Arc::new(Mutex::new(Vec::new()));
    let emitted = deltas.clone();
    let streaming = options["stream"] == true;
    let history = options.get("history").cloned();
    let rounds = options["rounds"].as_u64().unwrap_or(1);
    let steps = Arc::new(Mutex::new(Vec::new()));
    let completed = steps.clone();
    let parent: TaskFlowNode = FlowNode {
        definition_id: "parent".into(),
        metadata,
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(move |_, context| {
            let mut inputs = Inputs::from([("prompt".into(), "question".into())]);
            if let Some(history) = &history {
                inputs.insert("history".into(), history.clone().into());
            }
            let mut result = NodeResult::default();
            // The harness owns continuation scheduling, just as a Flow loop does in production.
            for _ in 0..rounds {
                result = if streaming {
                    executor.execute_with_sink(inputs.clone(), context, &mut Sink(emitted.clone()))
                } else {
                    executor.execute(inputs.clone(), context)
                }?;
                completed
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(&result.outputs).unwrap());
                if result.outputs["status"] != ValueRef::from("tool_calls") {
                    break;
                }
                assert!(result.outputs["continuation"].is_secret());
                inputs.insert(
                    "continuation".into(),
                    result.outputs["continuation"].clone(),
                );
                inputs.insert("history".into(), result.outputs["history"].clone());
            }
            *recorded.lock().unwrap() = Some(serde_json::to_value(&result.outputs).unwrap());
            Ok(result)
        }))),
    };
    let supplied: TaskFlowNode = FlowNode {
        definition_id: "config".into(),
        metadata: NodeMetadata::new(NodePorts {
            inputs: vec![],
            outputs: vec![port("token")],
        }),
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(|_, _| {
            Ok(Outputs::from([(
                "token".into(),
                ValueRef::from("private-token").into_secret(),
            )])
            .into())
        }))),
    };
    let collision = options["collision"] == true;
    let default_only = options["default_only"] == true;
    let mut tool_metadata = NodeMetadata::new(NodePorts {
        inputs: vec![port("text"), port("token")],
        outputs: vec![port("reply")],
    });
    if collision {
        tool_metadata.ports.inputs.push(port("query"));
    }
    tool_metadata.input_bindings.insert(
        "token".into(),
        InputBinding {
            secret: true,
            ..Default::default()
        },
    );
    let schema = if options["opaque"] == true {
        Some(ValueSchema::Opaque {
            value_type: ValueType::String,
            reason: SchemaOpacity::Codec,
        })
    } else if options["enum"] == true {
        Some(ValueSchema::Enum(vec!["allowed".into()]))
    } else {
        None
    };
    if let Some(schema) = &schema {
        tool_metadata
            .input_schemas
            .insert("text".into(), schema.clone());
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let invoked = seen.clone();
    let tool: TaskFlowNode = FlowNode {
        definition_id: "tool".into(),
        metadata: tool_metadata,
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(move |inputs, context| {
            assert!(inputs["token"].is_secret());
            assert_eq!(inputs["token"], ValueRef::from("private-token"));
            assert_eq!(context.invocation_path().len(), 1);
            invoked
                .lock()
                .unwrap()
                .push(serde_json::to_value(&inputs).unwrap());
            let text = inputs["text"].as_str().unwrap();
            if text == "control" {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    WorkflowRunError::StepBudget {
                        definition_id: "tool".into(),
                    },
                )
                .into());
            }
            if text == "fail" {
                return Err(NodeExecutionError::ExecutionFailed {
                    message: "private-tool-failure".into(),
                });
            }
            if text == "skip" {
                return Ok(NodeResult {
                    skipped: BTreeSet::from(["reply".into()]),
                    ..Default::default()
                });
            }
            let reply = match text {
                "secret" => ValueRef::from("private-result").into_secret(),
                "large" => "x".repeat(70_000).into(),
                _ => inputs["text"].clone(),
            };
            Ok(Outputs::from([("reply".into(), reply)]).into())
        }))),
    };
    let mut inputs = vec![
        ResolvedCallInput {
            port: port("text"),
            source: if default_only {
                CallInputSource::Default
            } else {
                CallInputSource::Output(port("query"))
            },
            secret: false,
            schema,
        },
        ResolvedCallInput {
            port: port("token"),
            source: CallInputSource::External,
            secret: true,
            schema: None,
        },
    ];
    if collision {
        inputs.push(ResolvedCallInput {
            port: port("query"),
            source: CallInputSource::Default,
            secret: false,
            schema: None,
        });
    }
    let call = PreparedCall {
        target: NodeIdentity {
            id: "tool".into(),
            kind: "test.tool".into(),
            path: vec![],
        },
        binding: ResolvedCallBinding {
            id: CallBindingId::new(0),
            name: "search".into(),
            description: Some("Search the configured service".into()),
            caller: NodeId::new(1),
            target: NodeId::new(2),
            inputs,
            dependencies: vec![FlowDependency {
                exclusive: false,
                input: Some("token".into()),
                source_node: "config".into(),
                source_output: "token".into(),
                source_key: "config.token".into(),
            }],
            success_dependencies: vec![],
            prerequisites: vec![],
        },
        visibility: OutputVisibility::Borrowed(&["config.token"]),
        guard: Arc::new(|_| Ok(BTreeSet::new())),
    };
    let domains = (0..2)
        .map(|id| ExecutionDomain {
            id,
            nodes: vec![NodeId::new(id)].into(),
            positions: vec![id].into(),
            predecessors: if id == 1 { vec![0] } else { vec![] }.into(),
            successors: if id == 0 { vec![1] } else { vec![] }.into(),
            first_position: id,
            snapshot_visibility: OutputVisibility::Borrowed(&["config.token"]),
        })
        .collect::<Vec<_>>();
    let flow = Flow::from_plan(
        vec![supplied, parent, tool],
        FlowPlan {
            connections: vec![].into(),
            dependencies: vec![vec![].into(); 3].into(),
            execution_order: vec![NodeId::new(0), NodeId::new(1), NodeId::new(2)].into(),
            outputs: vec![].into(),
            execution_domains: ExecutionDomains::from_parts(
                domains.into(),
                vec![vec![].into(), vec![0].into()].into(),
            ),
        },
        WorkflowInputSchema::default(),
    )
    .with_prepared_calls(vec![call])
    .unwrap();
    let outcome = flow.execute_in_context(&mut ExecutionContext::default());
    println!(
        "TOOL_RESULT:{}",
        json!({"ok":outcome.is_ok(),"error":outcome.err().map(|error| error.to_string()),"outputs":*results.lock().unwrap(),"calls":*seen.lock().unwrap(),"deltas":*deltas.lock().unwrap(),"steps":*steps.lock().unwrap()})
    );
}

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})
}
fn reply(calls: Vec<Value>) -> Reply {
    Reply::json(
        json!({"id":"reply","object":"chat.completion","created":0,"model":"example-model","choices":[{"index":0,"message":{"role":"assistant","content":"checking","tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
    )
}

#[test]
fn repeated_calls_compose_external_inputs_and_preserve_protocol_and_usage() {
    let server = Server::start(vec![
        reply(vec![
            call("a", "search", json!({"query":"first"})),
            call("b", "search", json!({"query":"second"})),
        ]),
        reply(vec![call("c", "search", json!({"query":"third"}))]),
        Reply::text("done"),
    ]);
    let result = probe(&server, json!({"rounds":3}));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["calls"].as_array().unwrap().len(), 3);
    assert_eq!(result["outputs"]["text"], "done");
    assert_eq!(
        result["outputs"]["usage"],
        json!({"input_tokens":2,"output_tokens":1,"total_tokens":3})
    );
    let usages: Vec<_> = result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| &step["usages"][0])
        .collect();
    assert_eq!(usages.len(), 3);
    assert_ne!(usages[0]["call_id"], usages[1]["call_id"]);
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let definition = &requests[0].body["tools"][0]["function"];
    assert_eq!(definition["description"], "Search the configured service");
    assert_eq!(
        definition["parameters"]["properties"],
        json!({"query":{"type":"string"}})
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.body.to_string().contains("private-token"))
    );
    let replay = &requests[1].body["messages"];
    assert_eq!(replay[1]["tool_calls"][0]["id"], "a");
    assert_eq!(replay[2]["tool_call_id"], "a");
    assert_eq!(replay[3]["tool_call_id"], "b");
    let history_server = Server::start(vec![Reply::text("again")]);
    let replayed = probe(
        &history_server,
        json!({"history":result["outputs"]["history"]}),
    );
    assert_eq!(replayed["ok"], true, "{replayed}");
    assert!(replayed["calls"].as_array().unwrap().is_empty());
}

#[test]
fn defaults_and_colliding_names_have_unambiguous_model_arguments() {
    for (options, arguments, expected) in [
        (
            json!({"default_only":true}),
            json!({"text":"first"}),
            json!({"text":{"type":"string"}}),
        ),
        (
            json!({"collision":true}),
            json!({"outputs":{"query":"first"},"defaults":{"query":"second"}}),
            Value::Null,
        ),
    ] {
        let server = Server::start(vec![
            reply(vec![call("a", "search", arguments)]),
            Reply::text("done"),
        ]);
        let result = probe(&server, options);
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["calls"][0]["text"], "first");
        let properties =
            server.requests()[0].body["tools"][0]["function"]["parameters"]["properties"].clone();
        if expected.is_null() {
            assert_eq!(result["calls"][0]["query"], "second");
            assert!(properties.get("outputs").is_some());
            assert!(properties.get("defaults").is_some());
        } else {
            assert_eq!(properties, expected);
        }
    }
}

#[test]
fn invalid_calls_receive_feedback_without_dispatch_and_domain_failures_are_sanitized() {
    let server = Server::start(vec![
        reply(vec![
            call("a", "missing", json!({})),
            call("b", "search", json!({"query":2})),
            call("c", "search", json!({"query":"fail"})),
            call("d", "search", json!({"query":"secret"})),
            call("e", "search", json!({"query":"large"})),
        ]),
        Reply::text("done"),
    ]);
    let result = probe(&server, json!({"rounds":2}));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["calls"].as_array().unwrap().len(), 3);
    let request = server.requests()[1].body.to_string();
    for private in ["private-tool-failure", "private-result", "private-token"] {
        assert!(!request.contains(private));
    }
    for feedback in [
        "unknown tool name",
        "arguments do not match",
        "tool execution failed",
        "size limit",
    ] {
        assert!(request.contains(feedback), "{request}");
    }
}

#[test]
fn decoded_contract_is_validated_before_tool_execution() {
    let server = Server::start(vec![
        reply(vec![
            call("a", "search", json!({"query":"forbidden"})),
            call("b", "search", json!({"query":"allowed"})),
        ]),
        Reply::text("done"),
    ]);
    let result = probe(&server, json!({"enum":true}));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);
    assert_eq!(result["calls"][0]["text"], "allowed");
    assert!(
        server.requests()[0].body["tools"]
            .to_string()
            .contains("enum")
    );
}

#[test]
fn budgets_and_ambiguous_batches_stop_before_any_new_side_effect() {
    let mut duplicate_json = call("a", "search", json!({}));
    duplicate_json["function"]["arguments"] = "{\"query\":\"first\",\"query\":\"second\"}".into();
    for (batch, config) in [
        (
            vec![
                call("a", "search", json!({"query":"first"})),
                call("a", "search", json!({"query":"second"})),
            ],
            json!({}),
        ),
        (vec![duplicate_json], json!({})),
        (
            vec![
                call("a", "search", json!({"query":"first"})),
                call("b", "search", json!({"query":"second"})),
            ],
            json!({"max_tool_calls":1}),
        ),
    ] {
        let server = Server::start(vec![reply(batch)]);
        let result = probe(&server, json!({"config":config}));
        assert_eq!(result["ok"], false, "{result}");
        assert!(result["calls"].as_array().unwrap().is_empty());
        assert_eq!(server.requests().len(), 1);
    }
    let server = Server::start(vec![
        reply(vec![call("a", "search", json!({"query":"first"}))]),
        reply(vec![call("b", "search", json!({"query":"second"}))]),
    ]);
    let result = probe(&server, json!({}));
    assert_eq!(result["outputs"]["status"], "tool_calls", "{result}");
    assert!(!result["outputs"]["continuation"].is_null());
    assert_eq!(result["outputs"]["usages"].as_array().unwrap().len(), 1);
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);
    assert_eq!(server.requests().len(), 1);
}

fn stream_call(raw: &str, finish: bool) -> Reply {
    let mut reply = Reply::stream_text("");
    let start = json!({"id":"reply","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"content":"checking","tool_calls":[{"id":"a","type":"function","function":{"name":"search","arguments":""}}]},"finish_reason":null}]});
    reply.body = format!("data: {start}\n\n");
    for fragment in raw.as_bytes().chunks(3) {
        let chunk = json!({"id":"reply","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"tool_calls":[{"function":{"arguments":std::str::from_utf8(fragment).unwrap()}}]},"finish_reason":null}]});
        reply.body.push_str(&format!("data: {chunk}\n\n"));
    }
    if finish {
        let chunk = json!({"id":"reply","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"completion_tokens":1}});
        reply
            .body
            .push_str(&format!("data: {chunk}\n\ndata: [DONE]\n\n"));
    }
    reply
}

#[test]
fn streamed_arguments_are_buffered_until_complete_and_rounds_keep_distinct_usage() {
    let server = Server::start(vec![
        stream_call("{\"query\":\"first\"}", true),
        Reply::stream_text("done"),
    ]);
    let result = probe(&server, json!({"stream":true,"rounds":2}));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);
    assert_eq!(result["outputs"]["text"], "done");
    assert_eq!(result["steps"][0]["usage"], json!({"output_tokens":1}));
    assert_eq!(
        result["outputs"]["usage"],
        json!({"input_tokens":2,"output_tokens":1,"total_tokens":3})
    );
    let deltas = result["deltas"].as_array().unwrap();
    assert_eq!(
        deltas
            .iter()
            .map(|delta| delta["delta"]["text"].as_str().unwrap())
            .collect::<String>(),
        "checkingdone"
    );
    assert_ne!(
        deltas[0]["delta"]["model_call_id"],
        deltas[1]["delta"]["model_call_id"]
    );
    assert_eq!(server.requests().len(), 2);
    assert!(
        server
            .requests()
            .iter()
            .all(|request| request.body["stream"] == true)
    );
}

#[test]
fn incomplete_and_duplicate_stream_arguments_never_execute_a_tool() {
    for (raw, finish) in [
        ("{\"query\":", false),
        ("{\"query\":\"first\",\"query\":\"second\"}", true),
    ] {
        let server = Server::start(vec![stream_call(raw, finish)]);
        let result = probe(&server, json!({"stream":true}));
        assert_eq!(result["ok"], false, "{result}");
        assert!(result["calls"].as_array().unwrap().is_empty());
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn opaque_public_parameters_are_rejected_before_requesting_a_completion() {
    let server = Server::start(vec![]);
    let result = probe(&server, json!({"opaque":true}));
    assert_eq!(result["ok"], false, "{result}");
    assert!(result["error"].as_str().unwrap().contains("opaque"));
    assert!(result["calls"].as_array().unwrap().is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn runtime_control_failures_propagate_even_when_provider_errors_are_captured() {
    let server = Server::start(vec![reply(vec![call(
        "a",
        "search",
        json!({"query":"control"}),
    )])]);
    let result = probe(&server, json!({"config":{"capture_errors":true}}));
    assert_eq!(result["ok"], false, "{result}");
    assert!(result["outputs"].is_null());
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn rejected_tool_arguments_retain_reported_usage_without_executing_the_tool() {
    for usage in [
        json!({"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}),
        json!({"completion_tokens":1}),
    ] {
        let mut tool_call = call("a", "search", json!({}));
        tool_call["function"]["arguments"] = "{\"query\":\"first\",\"query\":\"second\"}".into();
        let response = reply(vec![tool_call]);
        let mut body: Value = serde_json::from_str(&response.body).unwrap();
        body["usage"] = usage;
        let server = Server::start(vec![Reply::json(body)]);
        let result = probe(&server, json!({"config":{"capture_errors":true}}));
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["outputs"]["status"], "failed");
        assert_eq!(result["outputs"]["usages"][0]["usage"]["output_tokens"], 1);
        assert_eq!(result["outputs"]["usage"]["output_tokens"], 1);
        assert!(result["calls"].as_array().unwrap().is_empty());
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn an_unreported_later_usage_never_reuses_the_previous_round_counters() {
    let mut final_reply: Value = serde_json::from_str(&Reply::text("done").body).unwrap();
    final_reply.as_object_mut().unwrap().remove("usage");
    let server = Server::start(vec![
        reply(vec![call("a", "search", json!({"query":"first"}))]),
        Reply::json(final_reply),
    ]);
    let result = probe(&server, json!({"rounds":2}));
    assert_eq!(result["ok"], true, "{result}");
    assert!(result["outputs"].get("usage").is_none());
    let usages: Vec<_> = result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| &step["usages"][0])
        .collect();
    assert_eq!(usages.len(), 2);
    assert_eq!(usages[0]["usage"]["output_tokens"], 1);
    assert!(usages[1].get("usage").is_none());
}
