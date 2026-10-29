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
struct Sink(Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>);
impl EmissionSink for Sink {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.1.lock().unwrap().push(
            serde_json::to_value(
                ValueRef::object(
                    result
                        .outputs
                        .iter()
                        .map(|(name, value)| (name.clone().into(), value.clone())),
                )
                .redacted(),
            )
            .unwrap(),
        );
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
    let mut config = json!({"model":"example-model","api_key_env":"MINIFLOW_TOOL_KEY","base_url":std::env::var("MINIFLOW_TOOL_URL").unwrap()});
    if let Some(patch) = options["config"].as_object() {
        config.as_object_mut().unwrap().extend(patch.clone());
    }
    let prepared = mfn_llm::prepare_llm(config).unwrap();
    let mut metadata = prepared.metadata;
    metadata.ports.inputs = vec![PortSpec::new("tools", ValueType::Array, true)];
    metadata.input_schemas.clear();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected task stream")
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let results = Arc::new(Mutex::new(None));
    let recorded = results.clone();
    let deltas = Arc::new(Mutex::new(Vec::new()));
    let emitted = deltas.clone();
    let masked_events = Arc::new(Mutex::new(Vec::new()));
    let masked = masked_events.clone();
    let masked_model = Arc::new(Mutex::new(None));
    let model_snapshot = masked_model.clone();
    let streaming = options["stream"] == true;
    let initial_messages = options
        .get("messages")
        .cloned()
        .unwrap_or_else(|| json!([{ "role":"user","content":"question" }]));
    let manual_tools = options.get("tools").cloned();
    let manual_names: BTreeSet<_> = manual_tools
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    let messages = Arc::new(Mutex::new(initial_messages));
    let transcript = messages.clone();
    let observed = seen.clone();
    let steps = Arc::new(Mutex::new(Vec::<Value>::new()));
    let completed = steps.clone();
    let parent: TaskFlowNode = FlowNode {
        definition_id: "parent".into(),
        metadata,
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(move |inputs, context| {
            assert!(context.call_targets().is_empty());
            assert!(!context.can_invoke_calls());
            let before = observed.lock().unwrap().len();
            let mut tools = inputs["tools"].as_array().unwrap().to_vec();
            if let Some(extra) = &manual_tools {
                tools.extend(
                    extra
                        .as_array()
                        .unwrap()
                        .iter()
                        .cloned()
                        .map(ValueRef::from),
                );
            }
            let inputs = Inputs::from([
                ("messages".into(), transcript.lock().unwrap().clone().into()),
                ("tools".into(), ValueRef::array(tools)),
            ]);
            let result = if streaming {
                executor.execute_with_sink(
                    inputs,
                    context,
                    &mut Sink(emitted.clone(), masked.clone()),
                )
            } else {
                executor.execute(inputs, context)
            }?;
            assert_eq!(
                observed.lock().unwrap().len(),
                before,
                "model execution must not execute a tool"
            );
            assert!(!result.outputs.contains_key("tool_results"));
            *model_snapshot.lock().unwrap() = Some(
                serde_json::to_value(
                    ValueRef::object(
                        result
                            .outputs
                            .iter()
                            .map(|(name, value)| (name.clone().into(), value.clone())),
                    )
                    .redacted(),
                )
                .unwrap(),
            );
            let output = serde_json::to_value(&result.outputs).unwrap();
            completed.lock().unwrap().push(output.clone());
            *recorded.lock().unwrap() = Some(output);
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
    let invoked = seen.clone();
    let tool: TaskFlowNode = FlowNode {
        definition_id: "tool".into(),
        metadata: tool_metadata,
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(move |inputs, context| {
            assert!(inputs["token"].is_secret());
            assert_eq!(inputs["token"], ValueRef::from("private-token"));
            assert_eq!(context.invocation_path().len(), 1);
            assert_eq!(context.invocation_path()[0].node_id(), "dispatch");
            invoked
                .lock()
                .unwrap()
                .push(serde_json::to_value(&inputs).unwrap());
            let text = inputs["text"].as_str().unwrap();
            if text == "deadline" {
                assert!(
                    context
                        .effective_deadline(None)
                        .unwrap()
                        .saturating_duration_since(std::time::Instant::now())
                        > std::time::Duration::from_secs(30)
                );
            }
            if text == "slow" {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
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
            secret: options["secret_argument"] == true,
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
            caller: NodeId::new(3),
            target: NodeId::new(4),
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
    let catalog = FlowNode::new(
        "catalog",
        mfn_llm::prepare_tool_catalog(json!({"source":"dispatch"})).unwrap(),
    )
    .into_task()
    .unwrap();
    let mut dispatch_config = json!({"call_outputs":{"query":"string"}});
    if let Some(patch) = options["dispatch_config"].as_object() {
        dispatch_config
            .as_object_mut()
            .unwrap()
            .extend(patch.clone());
    }
    let prepared = mfn_llm::prepare_tool_dispatch(dispatch_config).unwrap();
    let NodeExecution::Task(dispatcher) = prepared.execution else {
        panic!("expected task dispatch")
    };
    let dispatched = Arc::new(Mutex::new(None));
    let captured = dispatched.clone();
    let dispatch = TaskFlowNode {
        definition_id: "dispatch".into(),
        metadata: prepared.metadata,
        output_keys: Default::default(),
        node: Box::new(Task(Box::new(move |mut inputs, context| {
            assert!(context.can_invoke_calls());
            // Explicit Flow routing leaves manually declared tools to their own handlers.
            let calls = inputs["tool_calls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|call| !manual_names.contains(call["name"].as_str().unwrap()))
                .cloned();
            let calls = ValueRef::array(calls);
            inputs.insert("tool_calls".into(), calls);
            let result = dispatcher.execute(inputs, context)?;
            *captured.lock().unwrap() =
                Some(serde_json::to_value(&result.outputs["tool_results"]).unwrap());
            Ok(result)
        }))),
    };
    let dependency =
        |node: &'static str, output: &'static str, input: &'static str| FlowDependency {
            exclusive: false,
            input: Some(input.into()),
            source_node: node.into(),
            source_output: output.into(),
            source_key: output_id(node, output).into(),
        };
    let keys = [
        "catalog.tools",
        "config.token",
        "parent.outcome",
        "parent.tool_calls",
    ];
    let domains = (0..4)
        .map(|id| ExecutionDomain {
            id,
            nodes: vec![NodeId::new(id)].into(),
            positions: vec![id].into(),
            predecessors: if id > 0 { vec![id - 1] } else { vec![] }.into(),
            successors: if id < 3 { vec![id + 1] } else { vec![] }.into(),
            first_position: id,
            snapshot_visibility: OutputVisibility::from_sorted(
                keys.iter().map(|key| (*key).into()).collect(),
            ),
        })
        .collect::<Vec<_>>();
    let flow = Flow::from_plan(
        vec![catalog, parent, supplied, dispatch, tool],
        FlowPlan {
            connections: vec![].into(),
            dependencies: vec![
                vec![].into(),
                vec![dependency("catalog", "tools", "tools")].into(),
                vec![].into(),
                vec![
                    dependency("parent", "outcome", "outcome"),
                    dependency("parent", "tool_calls", "tool_calls"),
                ]
                .into(),
                vec![].into(),
            ]
            .into(),
            execution_order: (0..5).map(NodeId::new).collect::<Vec<_>>().into(),
            outputs: vec![].into(),
            execution_domains: ExecutionDomains::from_parts(
                domains.into(),
                (0..4)
                    .map(|id| (0..id).collect::<Vec<_>>().into())
                    .collect::<Vec<_>>()
                    .into(),
            ),
        },
        WorkflowInputSchema::default(),
    )
    .with_prepared_calls(vec![call])
    .unwrap();
    let mut context = ExecutionContext::default();
    let mut outcome = Ok(Outputs::new());
    for _ in 0..options["rounds"].as_u64().unwrap_or(1) {
        outcome = flow.execute_in_context(&mut context);
        if outcome.is_err() {
            break;
        }
        let mut output = results.lock().unwrap().clone().unwrap();
        output["tool_results"] = dispatched.lock().unwrap().clone().unwrap();
        *results.lock().unwrap() = Some(output.clone());
        *steps.lock().unwrap().last_mut().unwrap() = output.clone();
        let mut current = messages.lock().unwrap();
        if let Some(message) = output.get("message") {
            current.as_array_mut().unwrap().push(message.clone());
        }
        for result in output["tool_results"].as_array().unwrap() {
            current.as_array_mut().unwrap().push(json!({"role":"tool","tool_call_id":result["tool_call_id"],"content":result["content"]}));
        }
        if output["outcome"] != "tool_calls" {
            break;
        }
    }
    println!(
        "TOOL_RESULT:{}",
        json!({"ok":outcome.is_ok(),"error":outcome.err().map(|error| error.to_string()),"outputs":*results.lock().unwrap(),"calls":*seen.lock().unwrap(),"deltas":*deltas.lock().unwrap(),"steps":*steps.lock().unwrap(),"messages":*messages.lock().unwrap(),"observed_model":*masked_model.lock().unwrap(),"observed_events":*masked_events.lock().unwrap()})
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
    let replayed = probe(&history_server, json!({"messages":result["messages"]}));
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
fn dispatch_uses_its_own_deadline_after_model_completion() {
    let server = Server::start(vec![reply(vec![call(
        "a",
        "search",
        json!({"query":"deadline"}),
    )])]);
    let result = probe(
        &server,
        json!({"config":{"timeout_ms":10000},"dispatch_config":{"timeout_ms":60000}}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["outputs"]["status"], "succeeded");
    assert_eq!(
        result["outputs"]["tool_results"][0]["content"],
        "{\"reply\":\"deadline\"}"
    );
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);

    let server = Server::start(vec![reply(vec![call(
        "a",
        "search",
        json!({"query":"slow"}),
    )])]);
    let result = probe(&server, json!({"dispatch_config":{"timeout_ms":5}}));
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["steps"][0]["status"], "succeeded");
    assert_eq!(result["steps"][0]["usages"][0]["usage"]["total_tokens"], 3);
    assert!(
        !result["steps"][0]
            .as_object()
            .unwrap()
            .contains_key("tool_results")
    );
    assert!(result["error"].as_str().unwrap().contains("deadline"));
}

#[test]
fn catalog_sensitivity_marks_model_outputs_and_events_before_dispatch() {
    for stream in [false, true] {
        let response = if stream {
            stream_call("{\"query\":\"private-query\"}", true)
        } else {
            reply(vec![call("a", "search", json!({"query":"private-query"}))])
        };
        let server = Server::start(vec![response]);
        let result = probe(&server, json!({"stream":stream,"secret_argument":true}));
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["calls"][0]["text"], "private-query");
        assert!(
            !result["observed_model"]
                .to_string()
                .contains("private-query")
        );
        assert!(
            !result["observed_events"]
                .to_string()
                .contains("private-query")
        );
        assert!(result["observed_model"].to_string().contains("[REDACTED]"));
        assert!(
            !result["outputs"]["tool_results"]
                .to_string()
                .contains("private-query")
        );
        assert_eq!(
            server.requests()[0].body["tools"][0]["function"]["parameters"]["properties"]["query"]
                ["writeOnly"],
            true
        );
    }
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
        let result = probe(&server, json!({"dispatch_config":config}));
        assert_eq!(result["ok"], false, "{result}");
        assert!(result["calls"].as_array().unwrap().is_empty());
        assert_eq!(server.requests().len(), 1);
    }
    let server = Server::start(vec![
        reply(vec![call("a", "search", json!({"query":"first"}))]),
        reply(vec![call("b", "search", json!({"query":"second"}))]),
    ]);
    let result = probe(&server, json!({}));
    assert_eq!(result["outputs"]["status"], "succeeded", "{result}");
    assert_eq!(result["outputs"]["outcome"], "tool_calls");
    assert_eq!(result["outputs"]["tool_results"][0]["tool_call_id"], "a");
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
    let events = result["deltas"].as_array().unwrap();
    let deltas: Vec<_> = events
        .iter()
        .filter(|event| event.get("delta").is_some())
        .collect();
    let calls: Vec<_> = events
        .iter()
        .filter_map(|event| event.get("tool_call"))
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["model_call_id"],
        result["steps"][0]["usages"][0]["call_id"]
    );
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
    assert_eq!(result["outputs"]["status"], "succeeded");
    assert_eq!(result["outputs"]["usages"][0]["usage"]["total_tokens"], 3);
    assert!(
        !result["outputs"]
            .as_object()
            .unwrap()
            .contains_key("tool_results")
    );
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

#[test]
fn manual_tools_keep_external_dispatch_while_bound_calls_use_runtime_execution() {
    let start = json!({"id":"mixed","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"tool_calls":[
        {"index":0,"id":"bound","type":"function","function":{"name":"search","arguments":"{\"query\":\"value\"}"}},
        {"index":1,"id":"manual","type":"function","function":{"name":"external","arguments":"{}"}}
    ]},"finish_reason":null}]});
    let finish = json!({"id":"mixed","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
    let mut response = Reply::stream_text("");
    response.body = format!("data: {start}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    let server = Server::start(vec![response]);
    let result = probe(
        &server,
        json!({"stream":true,"tools":[{"name":"external","description":"Caller-owned handler","input_schema":{"type":"object"}}]}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["calls"].as_array().unwrap().len(), 1);
    assert_eq!(result["outputs"]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(
        result["outputs"]["tool_results"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        result["outputs"]["tool_results"][0]["tool_call_id"],
        "bound"
    );
    let events = result["deltas"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["tool_call"]["call"]["id"], "bound");
    assert_eq!(events[1]["tool_call"]["call"]["id"], "manual");
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        server.requests()[0].body["tools"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn shared_protocol_validation_rejects_name_collisions_and_truncation_never_dispatches() {
    let server = Server::start(vec![]);
    let result = probe(
        &server,
        json!({"tools":[{"name":"search","description":"duplicate","input_schema":{"type":"object"}}]}),
    );
    assert_eq!(result["ok"], false);
    assert!(server.requests().is_empty());
    let mut response: Value =
        serde_json::from_str(&reply(vec![call("a", "search", json!({"query":"value"}))]).body)
            .unwrap();
    response["choices"][0]["finish_reason"] = "length".into();
    let server = Server::start(vec![Reply::json(response)]);
    let result = probe(&server, json!({}));
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["outputs"]["outcome"], "truncated");
    assert!(
        result["outputs"]["tool_results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(result["calls"].as_array().unwrap().is_empty());
    assert_eq!(server.requests().len(), 1);
}
