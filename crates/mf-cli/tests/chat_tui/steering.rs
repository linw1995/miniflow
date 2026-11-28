use super::*;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn gated(mut reply: Reply, prefix: String) -> (Reply, Arc<AtomicBool>) {
    let tail = reply
        .body
        .strip_prefix(&prefix)
        .unwrap()
        .as_bytes()
        .to_owned();
    let release = Arc::new(AtomicBool::new(false));
    reply.fragments = Some(vec![prefix.into_bytes(), tail]);
    reply.release_after_prefix = Some(release.clone());
    (reply, release)
}

#[test]
fn steering_during_a_running_tool_is_fifo_and_save_only_never_replays_it() {
    use mf_chat::{ChatError, ChatHost, HostConfig, WorkflowProfile};
    use mfn_deepseek as _;

    let call = tool_call(
        "only-fetch",
        "fetch",
        json!({"url":"https://example.org/article"}),
    );
    let model = Server::start(vec![call, Reply::stream_text("Revised answer")]);
    let (fetch, release) = gated(
        Reply::json(json!({"title":"Article","content":"Tool evidence"})),
        String::new(),
    );
    let tools = Server::start(vec![fetch]);
    let profile = WorkflowProfile::from_path("web", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/chat-deepseek-flow.json"), mf_runtime::WorkflowArguments::try_from(json!({
        "params.llm_model":"fixture-model", "params.llm_api_key":"synthetic-key", "params.llm_base_url":model.base_url,
        "params.deepseek_api_key":"tool-key", "params.deepseek_base_url":tools.base_url
    })).unwrap()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let host = ChatHost::open(&path, HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("web", None, Some("Manual".into()))
        .unwrap();
    let job = host.submit(&session.id, "Read article").unwrap();
    tools.wait_for_requests(1);
    job.steer("Consider limitations").unwrap();
    let withdrawn = job.steer("Do not send this correction").unwrap();
    let kept = job.steer("Use a short answer").unwrap();
    assert_eq!(
        job.steering().unwrap().withdraw(withdrawn).as_deref(),
        Some("Do not send this correction")
    );
    let retained_next = host.queue_next(&session.id, "A different task").unwrap();
    assert!(job.steering().unwrap().withdraw(retained_next.id).is_none());
    assert!(host.withdraw_next(&session.id, kept).is_none());
    assert_eq!(model.requests().len(), 1);
    let inbox = job.steering().unwrap().clone();
    fail_save(&path, true);
    release.store(true, Ordering::Release);
    let ChatError::Uncommitted { pending, .. } = job.join().unwrap_err() else {
        panic!("expected save-only recovery")
    };
    assert!(
        matches!(retained_next.wait(), Err(ChatError::NextTurnNotStarted { prompt }) if prompt == "A different task")
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let sent = requests[1].body["messages"].as_array().unwrap();
    assert_eq!(sent[2]["tool_call_id"], "only-fetch");
    assert!(
        sent[2]["content"]
            .as_str()
            .unwrap()
            .contains("Tool evidence")
    );
    assert_eq!(
        sent[3..],
        [
            json!({"role":"user","content":"Consider limitations"}),
            json!({"role":"user","content":"Use a short answer"})
        ]
    );
    assert!(inbox.withdraw(kept).is_none());
    assert!(inbox.submit("Too late".into()).is_err());
    assert!(inbox.take_unsent().is_empty());
    assert_eq!(
        host.database().model_context(&session.id).unwrap().revision,
        0
    );
    fail_save(&path, false);
    let mut tampered = pending.as_ref().clone();
    let mf_chat::MessageUpdate::Append { messages } = &mut tampered.completion.result.update else {
        panic!("expected append")
    };
    messages.remove(2);
    assert!(matches!(
        host.retry_completion(tampered),
        Err(ChatError::Uncommitted { .. })
    ));
    assert_eq!(
        host.database()
            .messages(&session.id, 10, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    let saved = host.retry_completion(*pending).unwrap();
    assert_eq!(saved.result.display_messages().len(), 5);
    assert_eq!(saved.result.usages.len(), 2);
    assert_eq!(
        host.database()
            .model_context(&session.id)
            .unwrap()
            .messages
            .len(),
        6
    );
    assert_eq!(model.requests().len(), 2);
    assert_eq!(tools.requests().len(), 1);
}

#[test]
fn cancelling_before_delivery_restores_steering_as_a_draft_not_as_a_follow_up() {
    let busy = Reply::stream_text("Paused reply");
    let prefix = format!("{}\n\n", busy.body.split_once("\n\n").unwrap().0);
    let (busy, release) = gated(busy, prefix);
    let server = Server::start(vec![busy]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-context-flow.json");
    let path = root.path().join("chat.sqlite");
    let mut terminal = ChatTerminal::open(&binary, &path);
    terminal.wait_for(&["What would you like"]);
    terminal.send(b"Original question\x1b\r");
    terminal.wait_for(&["Paused reply", "Alt+Enter queue", "Alt+S steer"]);
    terminal.send(b"\x05After success\x1b\rWithdraw next\x1b\rWithdraw steer\x1bs");
    terminal.wait_for(&[
        "Queued (next turn)\nAfter success",
        "Queued (next turn)\nWithdraw next",
        "Steering requested\nWithdraw steer",
    ]);
    terminal.send(b"\x1b[1;3A");
    let screen = terminal.wait_for(&[
        "Withdrew next-turn input",
        "Withdraw next",
        "Steering requested\nWithdraw steer",
    ]);
    assert!(!screen.contains("Queued (next turn)\nWithdraw next"));
    terminal.send(b"\x15\x1b[1;3B");
    let screen = terminal.wait_for(&[
        "Withdrew steering input",
        "Withdraw steer",
        "Queued (next turn)\nAfter success",
    ]);
    assert!(!screen.contains("Steering requested"));
    terminal.send(b"\x15Unsent correction\x1bs");
    terminal.wait_for(&["Steering requested\nUnsent correction"]);
    terminal.send(b"Newer draft");
    terminal.wait_for(&["Newer draft"]);
    terminal.send(b"\x03");
    terminal.wait_for(&[
        "After success",
        "Unsent correction",
        "Newer draft",
        "Alt+Enter send",
    ]);
    release.store(true, Ordering::Release);
    terminal.quit();
    let db = ChatDatabase::open(path).unwrap();
    let session = &db.list_sessions(false, 8, None).unwrap().sessions[0];
    assert_eq!(db.messages(&session.id, 8, None).unwrap().messages.len(), 1);
    assert_eq!(db.model_context(&session.id).unwrap().revision, 0);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn failed_agent_reply_skips_steering_callback_and_preserves_unsent_input() {
    use mf_chat::{ChatHost, HostConfig, WorkflowProfile};

    let mut reply = Reply::json(json!({"error":{"message":"provider unavailable"}}));
    reply.status = 500;
    let (failed, release) = gated(reply, String::new());
    let server = Server::start(vec![failed]);
    let profile = WorkflowProfile::from_path(
        "calc",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/chat-calculator-flow.json"),
        mf_runtime::WorkflowArguments::try_from(json!({
            "params.model": "fixture-model",
            "params.api_key": "synthetic-key",
            "params.base_url": server.base_url
        }))
        .unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let host = ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
    let session = host
        .database()
        .create_session("calc", None, Some("Manual".into()))
        .unwrap();
    let job = host.submit(&session.id, "Compute 2 + 2").unwrap();
    server.wait_for_requests(1);
    job.steer("Do not lose this steering").unwrap();
    let invocation = job.invocation().clone();
    let inbox = job.steering().unwrap().clone();
    release.store(true, Ordering::Release);
    let error = job.join().unwrap_err();
    assert!(matches!(
        error,
        mf_chat::ChatError::Execution {
            source,
            ..
        } if matches!(source.as_ref(), mf_chat::ChatError::WorkflowFailure { .. })
    ));
    assert_eq!(
        host.database().turn(&invocation.turn_id).unwrap().state,
        "failed"
    );
    assert_eq!(
        host.database()
            .messages(&session.id, 10, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(inbox.take_unsent(), ["Do not lose this steering"]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_chatflow_without_a_steering_callback_queues_separate_turns_without_prefetching() {
    let (first, release) = gated(Reply::text("First reply"), String::new());
    let server = Server::start(vec![
        first,
        Reply::text("Title"),
        Reply::text("Second reply"),
        Reply::text("Third reply"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-flow.json");
    let path = root.path().join("chat.sqlite");
    let mut terminal = ChatTerminal::open(&binary, &path);
    terminal.wait_for(&["What would you like"]);
    terminal.send(b"Original\x1b\r");
    terminal.wait_for(&["Alt+Enter queue"]);
    server.wait_for_requests(1);
    terminal.send(b"\x05Withdrawn\x1b\r");
    terminal.wait_for(&["Queued (next turn)\nWithdrawn"]);
    terminal.send(b"\x1b[1;3A");
    terminal.wait_for(&["Withdrew next-turn input", "Withdrawn"]);
    terminal.send(b"\x15Second question\x1b\rThird question\x1b\rStill editing");
    terminal.wait_for(&[
        "Queued (next turn)\nSecond question\nQueued (next turn)\nThird question",
        "Still editing",
    ]);
    assert_eq!(server.requests().len(), 1);
    release.store(true, Ordering::Release);
    let saved = terminal.wait_for(&[
        "You\nOriginal",
        "You [queued]\nSecond question",
        "You [queued]\nThird question",
        "Third reply",
        "Still editing",
        "Alt+Enter send",
    ]);
    assert!(!saved.contains("Queued (next turn)"));
    terminal.quit();
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for (index, text, len) in [
        (0, "Original", 1),
        (2, "Second question", 3),
        (3, "Third question", 5),
    ] {
        let sent = messages(&requests[index]);
        assert_eq!(sent.len(), len);
        assert_eq!(sent.last().unwrap()["content"], text);
    }
    let db = ChatDatabase::open(path).unwrap();
    let session = &db.list_sessions(false, 2, None).unwrap().sessions[0];
    let stored = db.messages(&session.id, 10, None).unwrap().messages;
    assert_eq!(stored.len(), 6);
    assert_ne!(stored[0].message.turn_id, stored[2].message.turn_id);
    assert_ne!(stored[2].message.turn_id, stored[4].message.turn_id);
    assert_eq!(db.model_context(&session.id).unwrap().revision, 3);
}

#[test]
fn killing_the_host_after_delivery_leaves_no_steering_on_disk_or_for_replay() {
    use std::os::unix::process::ExitStatusExt;

    let pause = |text| {
        let reply = Reply::stream_text(text);
        let prefix = format!("{}\n\n", reply.body.split_once("\n\n").unwrap().0);
        gated(reply, prefix)
    };
    let (first, advance) = pause("First reply");
    let (second, finish) = pause("Unfinished revision");
    let server = Server::start(vec![first, second]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-context-flow.json");
    let path = root.path().join("chat.sqlite");
    let marker = "OnlyInTheSteeredRequest";
    let mut live = ChatTerminal::open(&binary, &path);
    live.wait_for(&["What would you like"]);
    live.send(b"Original question\x1b\r");
    live.wait_for(&["First reply", "Alt+S steer"]);
    let queued_marker = "OnlyInTheNextTurnQueue";
    live.send(format!("\x05{queued_marker}\x1b\r{marker}\x1bs").as_bytes());
    live.wait_for(&[
        &format!("Queued (next turn)\n{queued_marker}"),
        &format!("Steering requested\n{marker}"),
    ]);
    advance.store(true, Ordering::Release);
    let screen = live.wait_for(&["Unfinished revision", queued_marker]);
    assert!(!screen.contains("Steering requested"));
    assert_eq!(
        server.requests()[1].body["messages"][2],
        json!({"role":"user","content":marker})
    );
    live.child.kill().unwrap();
    assert_eq!(
        live.child.wait().unwrap().signal(),
        Some(nix::libc::SIGKILL)
    );
    drop(live);
    finish.store(true, Ordering::Release);

    for name in ["chat.sqlite", "chat.sqlite-wal"] {
        let file = root.path().join(name);
        if file.exists() {
            let bytes = fs::read(file).unwrap();
            for text in [marker, queued_marker] {
                assert!(
                    !bytes.windows(text.len()).any(|b| b == text.as_bytes()),
                    "pending input persisted in {name}"
                );
            }
        }
    }
    let db = ChatDatabase::open(path).unwrap();
    let session = &db.list_sessions(false, 2, None).unwrap().sessions[0];
    let page = db.messages(&session.id, 10, None).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(
        db.turn(&page.messages[0].message.turn_id).unwrap().state,
        "interrupted"
    );
    assert_eq!(db.model_context(&session.id).unwrap().revision, 0);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn live_steering_and_next_turn_input_reach_different_requests_and_turns() {
    let mut tools = Reply::stream_text("");
    let calls = [("add", 2, 3), ("multiply", 3, 4)].into_iter().enumerate().map(|(index, (operation, left, right))| json!({"index":index,"id":operation,"type":"function","function":{"name":"calculator","arguments":json!({"operation":operation,"left":left,"right":right}).to_string()}})).collect::<Vec<_>>();
    let chunk = |delta, finish| {
        json!({"id":"batch","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":delta,"finish_reason":finish}]}).to_string()
    };
    let prefix = format!(
        "data: {}\n\n",
        chunk(json!({"tool_calls":calls}), json!(null))
    );
    tools.body = format!(
        "{prefix}data: {}\n\ndata: [DONE]\n\n",
        chunk(json!({}), json!("tool_calls"))
    );
    let (tools, first) = gated(tools, prefix);
    let text = Reply::stream_text("Intermediate answer");
    let prefix = format!("{}\n\n", text.body.split_once("\n\n").unwrap().0);
    let (text, second) = gated(text, prefix);
    let (title, finish_title) = gated(Reply::text("Generated title"), String::new());
    let server = Server::start(vec![
        tools,
        text,
        Reply::stream_text("Steered final answer"),
        title,
        Reply::stream_text("Separate reply"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-context-flow.json");
    let path = root.path().join("chat.sqlite");
    let mut terminal = ChatTerminal::open(&binary, &path);
    terminal.wait_for(&["What would you like"]);
    terminal.send(b"Original question\x1b\r");
    terminal.wait_for(&["calculator [add]", "Alt+Enter queue", "Alt+S steer"]);
    terminal.send(b"\x05Separate question\x1b\rUse both results\x1bs");
    terminal.wait_for(&[
        "Queued (next turn)\nSeparate question",
        "Steering requested\nUse both results",
    ]);
    first.store(true, Ordering::Release);
    let screen = terminal.wait_for(&[
        "Intermediate answer",
        "Queued (next turn)\nSeparate question",
    ]);
    assert!(!screen.contains("Steering requested"));
    terminal.send(b"Revise the conclusion\x1bs");
    terminal.wait_for(&["Steering requested\nRevise the conclusion"]);
    assert_eq!(
        server.requests().len(),
        2,
        "nothing queued for the next turn was prefetched"
    );
    second.store(true, Ordering::Release);
    server.wait_for_requests(4);
    terminal.send(b"Late correction\x1bs");
    terminal.wait_for(&["not accepting steering input", "Late correction"]);
    assert_eq!(server.requests().len(), 4);
    finish_title.store(true, Ordering::Release);
    terminal.wait_for(&[
        "You [steering]\nUse both results",
        "You [queued]\nRevise the conclusion",
        "You [queued]\nSeparate question",
        "Separate reply",
        "Alt+Enter send",
        "Late correction",
    ]);
    terminal.quit();

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        5,
        "three calls in the steered turn, one title and a separate turn"
    );
    let exchanged = requests[1].body["messages"].as_array().unwrap();
    assert_eq!(
        exchanged
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["user", "assistant", "tool", "tool", "user"]
    );
    assert_eq!(exchanged[2]["tool_call_id"], "add");
    assert_eq!(exchanged[2]["content"], "5.0");
    assert_eq!(exchanged[3]["tool_call_id"], "multiply");
    assert_eq!(exchanged[3]["content"], "12.0");
    assert_eq!(
        exchanged[4],
        json!({"role":"user","content":"Use both results"})
    );
    let next = requests[2].body["messages"].as_array().unwrap();
    assert_eq!(&next[..5], exchanged);
    assert_eq!(next[5]["content"][0]["text"], "Intermediate answer");
    assert_eq!(
        next[6],
        json!({"role":"user","content":"Revise the conclusion"})
    );
    assert!(
        requests
            .iter()
            .all(|r| !r.body.to_string().contains("\"token\":"))
    );

    let db = ChatDatabase::open(path).unwrap();
    let session = &db.list_sessions(false, 8, None).unwrap().sessions[0];
    let page = db.messages(&session.id, 50, None).unwrap();
    let separate = requests[4].body["messages"].as_array().unwrap();
    assert_eq!(separate.len(), 9);
    assert_eq!(&separate[..7], next);
    assert_eq!(separate[7]["content"][0]["text"], "Steered final answer");
    assert_eq!(
        separate[8],
        json!({"role":"user","content":"Separate question"})
    );
    assert!(
        requests[..4]
            .iter()
            .all(|r| !r.body.to_string().contains("Separate question"))
    );
    assert_eq!(page.messages.len(), 10);
    let (original, following) = page.messages.split_at(8);
    assert!(
        original
            .iter()
            .all(|r| r.message.turn_id == original[0].message.turn_id)
    );
    assert!(
        following
            .iter()
            .all(|r| r.message.turn_id == following[0].message.turn_id)
    );
    assert_ne!(original[0].message.turn_id, following[0].message.turn_id);
    let context = db.model_context(&session.id).unwrap();
    assert_eq!(context.revision, 2);
    assert_eq!(
        context.messages,
        page.messages
            .into_iter()
            .map(|r| r.message.payload)
            .collect::<Vec<_>>()
    );
}
