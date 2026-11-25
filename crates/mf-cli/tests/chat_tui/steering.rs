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
    job.steer("Use a short answer").unwrap();
    assert_eq!(model.requests().len(), 1);
    let inbox = job.steering().unwrap().clone();
    fail_save(&path, true);
    release.store(true, Ordering::Release);
    let ChatError::Uncommitted { pending, .. } = job.join().unwrap_err() else {
        panic!("expected save-only recovery")
    };
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
    terminal.wait_for(&["Paused reply", "Alt+Enter steer"]);
    terminal.send(b"Unsent correction\x1b\r");
    terminal.wait_for(&["Steering queued"]);
    terminal.send(b"Newer draft");
    terminal.wait_for(&["Newer draft"]);
    terminal.send(b"\x03");
    terminal.wait_for(&["Unsent correction", "Newer draft", "Alt+Enter send"]);
    release.store(true, Ordering::Release);
    terminal.quit();
    let db = ChatDatabase::open(path).unwrap();
    let session = &db.list_sessions(false, 8, None).unwrap().sessions[0];
    assert_eq!(db.messages(&session.id, 8, None).unwrap().messages.len(), 1);
    assert_eq!(db.model_context(&session.id).unwrap().revision, 0);
    assert_eq!(server.requests().len(), 1);
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
    live.wait_for(&["First reply", "Alt+Enter steer"]);
    live.send(format!("{marker}\x1b\r").as_bytes());
    live.wait_for(&["Steering queued"]);
    advance.store(true, Ordering::Release);
    live.wait_for(&["Unfinished revision"]);
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
            assert!(
                !fs::read(file)
                    .unwrap()
                    .windows(marker.len())
                    .any(|bytes| bytes == marker.as_bytes()),
                "steering persisted in {name}"
            );
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
fn live_steering_continues_the_same_runner_after_a_complete_tool_batch_and_a_text_reply() {
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
    let server = Server::start(vec![
        tools,
        text,
        Reply::stream_text("Steered final answer"),
        Reply::text("Generated title"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-context-flow.json");
    let path = root.path().join("chat.sqlite");
    let mut terminal = ChatTerminal::open(&binary, &path);
    terminal.wait_for(&["What would you like"]);
    terminal.send(b"Original question\x1b\r");
    terminal.wait_for(&["calculator [add]", "Alt+Enter steer"]);
    terminal.send(b"Use both results\x1b\r");
    terminal.wait_for(&["Steering queued"]);
    first.store(true, Ordering::Release);
    terminal.wait_for(&["Intermediate answer"]);
    terminal.send(b"Revise the conclusion");
    terminal.wait_for(&["Revise the conclusion"]);
    terminal.send(b"\x1b\r");
    // The second checkpoint must not race ahead of the actual UI submission.
    let deadline = Instant::now() + Duration::from_secs(5);
    while terminal
        .wait_for(&["Steering queued"])
        .contains("Revise the conclusion")
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    second.store(true, Ordering::Release);
    terminal.wait_for(&["Steered final answer", "Alt+Enter send"]);
    terminal.quit();

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        4,
        "three Agent calls and one auxiliary title"
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
    assert_eq!(page.messages.len(), 8);
    assert!(
        page.messages
            .iter()
            .all(|r| r.message.turn_id == page.messages[0].message.turn_id)
    );
    let context = db.model_context(&session.id).unwrap();
    assert_eq!(context.revision, 1);
    assert_eq!(
        context.messages,
        page.messages
            .into_iter()
            .map(|r| r.message.payload)
            .collect::<Vec<_>>()
    );
}
