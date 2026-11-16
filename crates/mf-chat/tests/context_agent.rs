use mf_chat::{ChatDatabase, ChatHost, HostConfig, MessageUpdate, WorkflowProfile};
use mf_runtime::WorkflowArguments;
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::json;
use std::path::Path;

fn tool_call(id: &str, operation: &str, left: f64, right: f64) -> Reply {
    let mut reply = Reply::stream_text("");
    let arguments = json!({"operation":operation,"left":left,"right":right}).to_string();
    let chunk = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"calculator","arguments":arguments}}]},"finish_reason":null}]});
    let finish = json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
    reply.body = format!("data: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    reply
}

fn host(path: &Path, server: &Server, compact: bool) -> ChatHost {
    let profile = WorkflowProfile::from_path("context", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/chat-context-flow.json"), WorkflowArguments::try_from(json!({
        "params.model":"fixture-model","params.base_url":server.base_url,"params.api_key":"synthetic-key","params.compact":compact
    })).unwrap()).unwrap();
    ChatHost::open(path, HostConfig::new(profile)).unwrap()
}

#[test]
fn agent_compaction_keeps_old_and_current_tool_records_but_reopens_with_summary_only() {
    let server = Server::start(vec![
        tool_call("first", "add", 2.0, 3.0),
        Reply::stream_text("First answer: 5"),
        tool_call("second", "multiply", 3.0, 4.0),
        Reply::stream_text("Second answer: 12"),
        Reply::text("Summary only: the calculated values were 5 and 12"),
        Reply::stream_text("Third answer"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let first = host(&path, &server, false);
    let id = first
        .database()
        .create_session(
            "context",
            Some("Original instructions".into()),
            Some("Manual name".into()),
        )
        .unwrap()
        .id;
    let completion = first.submit(&id, "First question").unwrap().join().unwrap();
    assert!(matches!(
        completion.result.update,
        MessageUpdate::Append { .. }
    ));
    assert_eq!(completion.result.display_messages().len(), 3);
    assert_eq!(completion.result.usages.len(), 2);
    first.close().unwrap();
    let second = host(&path, &server, true);
    let completion = second
        .submit(&id, "Second question")
        .unwrap()
        .join()
        .unwrap();
    assert!(matches!(
        completion.result.update,
        MessageUpdate::ReplaceContext { .. }
    ));
    assert_eq!(completion.result.display_messages().len(), 3);
    assert_eq!(completion.result.usages.len(), 3);
    assert_eq!(
        completion.result.usages[2]
            .attribution
            .as_ref()
            .unwrap()
            .purpose
            .as_deref(),
        Some("compaction")
    );
    let original = second
        .database()
        .messages(&id, 50, None)
        .unwrap()
        .messages
        .into_iter()
        .map(|record| record.message)
        .collect::<Vec<_>>();
    assert_eq!(original.len(), 8);
    assert_eq!(
        original
            .iter()
            .filter(|message| message.role() == "tool")
            .map(|message| message.content())
            .collect::<Vec<_>>(),
        ["5.0", "12.0"]
    );
    assert_eq!(second.database().model_context(&id).unwrap().revision, 2);
    second.close().unwrap();
    let third = host(&path, &server, false);
    third.submit(&id, "Third question").unwrap().join().unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    let summary = requests[4].body["messages"].as_array().unwrap();
    assert!(
        summary
            .iter()
            .any(|message| message["tool_call_id"] == "first" && message["content"] == "5.0")
    );
    assert!(
        summary
            .iter()
            .any(|message| message["tool_call_id"] == "second" && message["content"] == "12.0")
    );
    assert_eq!(
        messages(&requests[5]),
        vec![
            json!({"role":"system","content":"Summary only: the calculated values were 5 and 12"}),
            json!({"role":"user","content":"Third question"})
        ]
    );
    let page = third.database().messages(&id, 50, None).unwrap();
    assert_eq!(
        page.messages[..8]
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>(),
        original.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        third
            .database()
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        6
    );
    third.close().unwrap();
}

#[test]
fn failed_or_incomplete_summary_is_audited_without_publishing_or_retrying() {
    for incomplete in [false, true] {
        let summary = if incomplete {
            let mut reply = Reply::text("Incomplete summary");
            reply.body = reply.body.replace("\"stop\"", "\"length\"");
            reply
        } else {
            let mut reply = Reply::json(json!({"error":{"message":"Summary unavailable"}}));
            reply.status = 503;
            reply
        };
        let server = Server::start(vec![
            tool_call("current", "add", 2.0, 3.0),
            Reply::stream_text("Original display answer"),
            summary,
        ]);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("chat.sqlite");
        let host = host(&path, &server, true);
        let id = host
            .database()
            .create_session(
                "context",
                Some("Original instructions".into()),
                Some("Manual name".into()),
            )
            .unwrap()
            .id;
        let job = host.submit(&id, "Question").unwrap();
        let invocation = job.invocation().clone();
        assert!(job.join().is_err());
        let call = host.database().call(&invocation.call_id).unwrap();
        let result = call.terminal_result.unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(result.display_messages().len(), 3);
        assert_eq!(result.final_text(), Some("Original display answer"));
        assert_eq!(result.usages.len(), 3);
        assert!(
            matches!(result.update, MessageUpdate::ReplaceContext { ref context_messages, .. } if context_messages.is_empty())
        );
        assert_eq!(host.database().model_context(&id).unwrap().revision, 0);
        assert_eq!(
            host.database()
                .messages(&id, 50, None)
                .unwrap()
                .messages
                .len(),
            4
        );
        assert_eq!(server.requests().len(), 3);
        host.close().unwrap();
        let database = ChatDatabase::open(&path).unwrap();
        assert_eq!(database.model_context(&id).unwrap().messages.len(), 1);
        assert_eq!(
            database
                .usage(Default::default())
                .unwrap()
                .overall
                .attempted_calls,
            3
        );
    }
}
