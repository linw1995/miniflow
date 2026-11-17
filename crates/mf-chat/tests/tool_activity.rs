use mf_chat::{
    ChatHost, ConversationInputs, HostConfig, Invocation, PreviewDelta, ToolActivity,
    WorkflowProfile,
};
use mf_runtime::{ExecutionContext, NodeValue, WorkflowArguments};
use mfn_llm::test_support::{Reply, Server};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

fn calls() -> Reply {
    let mut reply = Reply::stream_text("");
    let chunks = [
        json!({"content":"Checking tools"}),
        json!({"tool_calls":[
            {"index":0,"id":"first","type":"function","function":{"name":"calculator","arguments":"{\"operation\":\"add\","}},
            {"index":1,"id":"second","type":"function","function":{"name":"unknown","arguments":"{"}}
        ]}),
        json!({"tool_calls":[{"index":1,"function":{"arguments":"}"}}]}),
        json!({"tool_calls":[{"index":0,"function":{"arguments":"\"left\":2,\"right\":3}"}}]}),
    ];
    reply.body = chunks.into_iter().map(|delta| {
        let chunk = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
        format!("data: {chunk}\n\n")
    }).collect();
    let finish = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
    reply
        .body
        .push_str(&format!("data: {finish}\n\ndata: [DONE]\n\n"));
    reply.fragments = Some(
        reply
            .body
            .as_bytes()
            .chunks(7)
            .map(<[u8]>::to_vec)
            .collect(),
    );
    reply
}

fn profile(server: &Server) -> WorkflowProfile {
    WorkflowProfile::from_path("tools", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/chat-calculator-flow.json"),
        WorkflowArguments::try_from(json!({"params.model":"fixture", "params.base_url":server.base_url, "params.api_key":"synthetic-private-key"})).unwrap()).unwrap()
}

#[test]
fn parallel_tool_previews_precede_terminal_text_and_never_become_saved_messages() {
    let server = Server::start(vec![calls(), Reply::stream_text("Final answer")]);
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(
        root.path().join("chat.sqlite"),
        HostConfig::new(profile(&server)),
    )
    .unwrap();
    let session = host
        .database()
        .create_session("tools", None, Some("Manual title".into()))
        .unwrap();
    let (job, receiver) = host.submit_stream(&session.id, "Use tools").unwrap();
    let invocation = job.invocation().clone();
    let mut fragments = BTreeMap::<(String, u64), String>::new();
    let mut ready = BTreeMap::new();
    let mut running = BTreeMap::new();
    let mut results = BTreeMap::new();
    let mut final_text = false;
    while let Ok(event) = receiver.recv() {
        assert_eq!(event.invocation, invocation);
        assert!(
            !serde_json::to_string(&event)
                .unwrap()
                .contains("synthetic-private-key")
        );
        match event.delta {
            PreviewDelta::Text { delta } => {
                if delta.text == "Final answer" {
                    assert_eq!(results.len(), 2);
                    final_text = true;
                }
            }
            PreviewDelta::Tool {
                activity:
                    ToolActivity::Call {
                        model_call_id,
                        part,
                        arguments,
                        call,
                    },
            } => {
                assert!(!model_call_id.is_empty());
                let text = fragments.entry((model_call_id, part)).or_default();
                if let Some(call) = call {
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(text).unwrap(),
                        serde_json::to_value(&call.arguments).unwrap()
                    );
                    ready.insert(call.id, call.name);
                } else {
                    text.push_str(&arguments);
                }
            }
            PreviewDelta::Tool {
                activity: ToolActivity::Running { tool_call_id, name },
            } => {
                assert_eq!(ready.get(&tool_call_id), Some(&name));
                running.insert(tool_call_id, name);
            }
            PreviewDelta::Tool {
                activity:
                    ToolActivity::Result {
                        tool_call_id,
                        failed,
                        content,
                    },
            } => {
                assert!(running.contains_key(&tool_call_id));
                results.insert(tool_call_id, (failed, content));
            }
        }
    }
    assert!(final_text);
    assert_eq!(fragments.len(), 2);
    assert_eq!(results["first"], (false, "5.0".into()));
    assert!(results["second"].0 && results["second"].1.contains("unknown tool"));
    let completion = job.join().unwrap();
    assert_eq!(completion.result.final_text(), Some("Final answer"));
    assert_eq!(completion.result.display_messages().len(), 4);
    assert_eq!(
        host.database().model_context(&session.id).unwrap().revision,
        1
    );
    let page = host.database().messages(&session.id, 50, None).unwrap();
    assert_eq!(page.messages.len(), 5);
    assert!(
        page.messages
            .iter()
            .all(|record| !record.message.content().contains("model_call_id"))
    );
    assert_eq!(server.requests().len(), 2);
    host.close().unwrap();
}

#[test]
fn native_tool_only_streams_are_observed_and_mismatched_kinds_fail() {
    for mismatched in [false, true] {
        let server = Server::start(vec![calls()]);
        let name = if mismatched {
            "tool_finished"
        } else {
            "tool_call"
        };
        let definition = serde_json::from_value(json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[
                {"id":"params","kind":"builtin.parameters","config":{"parameters":{"messages":{"type":{"list":"object"},"secret":false},"api_key":{"type":"string","default":"synthetic-private-key"}}}},
                {"id":"tools","kind":"builtin.constant","config":{"value":[{"name":"calculator","description":"Calculate","input_schema":{"type":"object"}}]}},
                {"id":"model","kind":"builtin.llm","config":{"model":"fixture","base_url":server.base_url,"api_key":{"$ref":"params.api_key"}},
                    "inputs":{"messages":{"from":{"node":"params","port":"messages"}},"tools":{"from":{"node":"tools","port":"value"}}}},
                {"id":"result","kind":"builtin.constant","config":{"value":{"status":"succeeded","messages":[{"role":"assistant","content":"Done"}],"usages":[]}}}
            ],
            "edges":[{"from_node":"model","to_node":"result"}],
            "outputs":[{"name":name,"node":"model","port":"tool_call_delta","optional":true}, {"name":"append","node":"result","port":"value"}]
        })).unwrap();
        let profile = WorkflowProfile::new("tools", definition, Default::default()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let host =
            ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
        let session = host
            .database()
            .create_session("tools", None, Some("Manual".into()))
            .unwrap();
        let (job, receiver) = host.submit_stream(&session.id, "Use tools").unwrap();
        let mut events = 0;
        while let Ok(event) = receiver.recv() {
            assert!(matches!(
                event.delta,
                PreviewDelta::Tool {
                    activity: ToolActivity::Call { .. }
                }
            ));
            events += 1;
        }
        let result = job.join();
        if mismatched {
            assert!(result.is_err());
            assert_eq!(events, 0);
        } else {
            assert!(result.is_ok());
            assert!(events >= 2);
        }
        host.close().unwrap();
    }
}

#[test]
fn cancellation_from_a_running_tool_preview_cannot_publish_success_or_an_error_result() {
    let server = Server::start(vec![calls()]);
    let backend = profile(&server).backend();
    let mut context = ExecutionContext::default();
    let cancellation = context.cancellation();
    let mut started = false;
    let mut consume = |delta| -> Result<(), mf_chat::ChatError> {
        match delta {
            PreviewDelta::Tool {
                activity: ToolActivity::Running { .. },
            } => {
                started = true;
                cancellation.cancel(
                    mf_runtime::StreamExecutionSnafu {
                        message: "cancel from tool preview",
                    }
                    .build(),
                );
            }
            PreviewDelta::Tool {
                activity: ToolActivity::Result { .. },
            } => panic!("cancelled dispatch published a result"),
            _ => {}
        }
        Ok(())
    };
    let input = ConversationInputs::from_values(
        serde_json::from_value(json!({"messages":[{"role":"user","content":"Use tools"}]}))
            .unwrap(),
    )
    .unwrap();
    assert!(
        backend
            .invoke(
                &Invocation::new("session"),
                input,
                &mut context,
                Some(&mut consume),
                None
            )
            .is_err()
    );
    assert!(started);
    assert_eq!(server.requests().len(), 1);
}
