use mf_chat::{
    ChatHost, ConversationInputs, HostConfig, Invocation, PreviewDelta, WorkflowProfile,
};
use mf_runtime::{ExecutionContext, NodeValue, WorkflowArguments};
use mfn_llm::test_support::{Reply, Server};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::TryRecvError,
    },
    time::{Duration, Instant},
};

fn calls() -> Reply {
    let mut reply = Reply::stream_text("");
    let chunks = [
        json!({"content":"Checking tools"}),
        json!({"tool_calls":[
            {"index":0,"id":"first","type":"function","function":{"name":"calculator","arguments":"{ \"operation\":\"add\","}},
            {"index":1,"id":"second","type":"function","function":{"name":"unknown","arguments":"{"}}
        ]}),
        json!({"tool_calls":[{"index":1,"function":{"arguments":"}"}}]}),
        json!({"tool_calls":[{"index":0,"function":{"arguments":"\"left\":2,\"right\":3 }"}}]}),
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
fn parallel_tool_argument_previews_precede_the_answer_without_changing_the_transcript() {
    let release = Arc::new(AtomicBool::new(false));
    let mut reply = calls();
    let prefix = reply
        .body
        .split_inclusive("\n\n")
        .take(2)
        .map(str::len)
        .sum();
    let (prefix, tail) = reply.body.as_bytes().split_at(prefix);
    reply.fragments = Some(vec![prefix.to_vec(), tail.to_vec()]);
    reply.release_after_prefix = Some(release.clone());
    let finish = Arc::new(AtomicBool::new(false));
    let mut answer = Reply::stream_text("Final answer");
    answer.fragments = Some(vec![vec![], answer.body.as_bytes().to_vec()]);
    answer.release_after_prefix = Some(finish.clone());
    let server = Server::start(vec![reply, answer]);
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
    let (mut job, receiver) = host.submit_stream(&session.id, "Use tools").unwrap();
    let invocation = job.invocation().clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    let first_fragment = loop {
        match receiver.try_recv() {
            Ok(
                event @ mf_chat::PreviewEvent {
                    delta: PreviewDelta::ToolCall { .. },
                    ..
                },
            ) => break event,
            Ok(_) => continue,
            Err(TryRecvError::Disconnected) => panic!("model ended without an argument fragment"),
            Err(TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "argument publication waited for the withheld SSE tail"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    };
    assert!(!job.poll());
    let tool_metrics = job.tool_metrics().clone();
    assert!(tool_metrics.snapshot().is_empty());
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        host.database().model_context(&session.id).unwrap().revision,
        0
    );
    release.store(true, Ordering::Release);
    let mut fragments = BTreeMap::<(String, u64), Vec<String>>::new();
    let mut named = BTreeSet::new();
    let mut final_text = false;
    for event in std::iter::once(first_fragment).chain(std::iter::from_fn(|| {
        // Continue draining previews while the model's final response is withheld.
        loop {
            match receiver.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {
                    if server.requests().len() == 2 && !finish.load(Ordering::Acquire) {
                        let metrics = tool_metrics.snapshot();
                        assert_eq!(
                            metrics.len(),
                            1,
                            "rejected tool must not execute: {metrics:?}"
                        );
                        assert_eq!(
                            metrics["first"].phase,
                            Some(mf_chat::ToolExecutionPhase::Succeeded)
                        );
                        assert!(metrics["first"].duration_ns.is_some());
                        assert_eq!(
                            host.database().model_context(&session.id).unwrap().revision,
                            0
                        );
                        finish.store(true, Ordering::Release);
                    }
                    assert!(
                        Instant::now() < deadline,
                        "expected observed tool and final preview"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    })) {
        assert_eq!(event.invocation, invocation);
        assert!(
            !serde_json::to_string(&event)
                .unwrap()
                .contains("synthetic-private-key")
        );
        match event.delta {
            PreviewDelta::ToolOutput { .. } => panic!("this tool has no output stream"),
            PreviewDelta::Text { delta } => {
                if delta.text == "Final answer" {
                    assert_eq!(fragments.len(), 2);
                    assert!(fragments.values().all(|parts| parts.len() == 2));
                    final_text = true;
                }
            }
            PreviewDelta::ToolCall { delta } => {
                if let Some(id) = delta.id {
                    assert!(named.insert((id, delta.name.unwrap())));
                }
                fragments
                    .entry((delta.model_call_id, delta.index))
                    .or_default()
                    .push(delta.arguments);
            }
        }
    }
    assert!(final_text);
    assert_eq!(fragments.len(), 2);
    assert_eq!(
        named,
        BTreeSet::from([
            ("first".into(), "calculator".into()),
            ("second".into(), "unknown".into()),
        ])
    );
    let completion = job.join().unwrap();
    assert_eq!(completion.result.final_text(), Some("Final answer"));
    assert_eq!(completion.result.display_messages().len(), 4);
    assert!(completion.result.usages.iter().all(|call| {
        call.timing
            .as_ref()
            .is_some_and(|timing| timing.first_token_us.is_some())
    }));
    let model = &completion.result.usages[0].call_id;
    assert_eq!(
        fragments[&(model.clone(), 0)],
        [r#"{ "operation":"add","#, r#""left":2,"right":3 }"#]
    );
    assert_eq!(fragments[&(model.clone(), 1)], ["{", "}"]);
    let page = host.database().messages(&session.id, 50, None).unwrap();
    assert_eq!(page.messages.len(), 5);
    let results: BTreeMap<_, _> = page
        .messages
        .iter()
        .filter_map(|record| match &record.message.payload {
            mf_chat::ChatMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => Some((tool_call_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(results["first"], "5.0");
    assert!(results["second"].contains("unknown tool"));
    assert_eq!(
        host.database().model_context(&session.id).unwrap().revision,
        1
    );
    assert_eq!(server.requests().len(), 2);
    host.close().unwrap();
}

#[test]
fn native_tool_only_previews_accept_call_deltas_and_reject_other_payloads() {
    for mismatched in [false, true] {
        let server = Server::start(vec![calls()]);
        let port = if mismatched {
            "delta"
        } else {
            "tool_call_delta"
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
            "outputs":[{"name":"tool_call_delta","node":"model","port":port,"optional":true}, {"name":"append","node":"result","port":"value"}]
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
            assert!(matches!(event.delta, PreviewDelta::ToolCall { .. }));
            events += 1;
        }
        let result = job.join();
        if mismatched {
            assert!(result.is_err());
            assert_eq!(events, 0);
            assert_eq!(
                host.database().model_context(&session.id).unwrap().revision,
                0
            );
        } else {
            assert_eq!(result.unwrap().result.final_text(), Some("Done"));
            assert_eq!(events, 4);
        }
        host.close().unwrap();
    }
}

#[test]
fn cancellation_during_tool_argument_streaming_does_not_complete_the_turn() {
    let server = Server::start(vec![calls()]);
    let backend = profile(&server).backend();
    let mut context = ExecutionContext::default();
    let cancellation = context.cancellation();
    let mut saw_fragment = false;
    let mut consume = |delta| -> Result<(), mf_chat::ChatError> {
        if let PreviewDelta::ToolCall { delta } = delta {
            assert_eq!(delta.arguments, r#"{ "operation":"add","#);
            saw_fragment = true;
            cancellation.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "cancel from argument preview",
                }
                .build(),
            );
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
    assert!(saw_fragment);
    assert_eq!(server.requests().len(), 1);
}
