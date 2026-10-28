#![cfg(unix)]

use diesel::{Connection, connection::SimpleConnection};
use mf_chat::ChatDatabase;
use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use mfn_llm::test_support::{Reply, Server, messages};
use nix::{
    fcntl::{FcntlArg, FdFlag, OFlag, fcntl},
    pty::{Winsize, openpty},
    sys::termios::{LocalFlags, tcgetattr},
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "support/terminal.rs"]
mod terminal;
use terminal::Screen;

#[test]
fn supplied_conversation_workflows_run_inside_the_application_lifecycle() {
    use mf_chat::contract::CompletionState;
    use mf_tui::{
        chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions},
        run::RunOptions,
    };

    for example in ["chat-title-flow.json", "chat-tools-flow.json"] {
        let tools = example == "chat-tools-flow.json";
        let root = tempfile::tempdir().unwrap();
        let replies = if tools {
            let call = Reply::json(
                serde_json::json!({"id":"tool-round","object":"chat.completion","created":0,"model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"multiply-1","type":"function","function":{"name":"multiply","arguments":"{\"amount\":21}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
            );
            vec![
                call,
                Reply::text("first answer"),
                Reply::text("second answer"),
            ]
        } else {
            vec![
                Reply::stream_text("first answer"),
                Reply::text("Generated title"),
                Reply::stream_text("second answer"),
            ]
        };
        let server = Server::start(replies);
        let binary = compiled(root.path(), &server, example);
        let prefix = if tools { "params" } else { "input" };
        let path = root.path().join("chat.sqlite");
        let launch = ChatLaunch::prepare(
            &binary,
            ChatOptions {
                database_path: path.clone(),
                profile: "assistant-flow".into(),
                run: RunOptions {
                    arguments: vec![
                        format!("--{prefix}.model=fixture-model").into(),
                        format!("--{prefix}.api_key=synthetic-key").into(),
                    ],
                    ..Default::default()
                },
            },
        )
        .unwrap();
        let controller = ChatController::open(launch).unwrap();
        let mut session_id = None;
        for (prompt, expected, calls) in
            [("first", "first answer", 2), ("second", "second answer", 1)]
        {
            controller
                .send(ChatCommand::Submit {
                    session_id: session_id.clone(),
                    prompt: prompt.into(),
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut saw_preview = false;
            loop {
                assert!(Instant::now() < deadline, "conversation did not finish");
                match controller.try_event() {
                    Some(ChatEvent::Submitted { session, .. }) => session_id = Some(session.id),
                    Some(ChatEvent::Preview { preview, .. }) => {
                        saw_preview = true;
                        assert!(preview.segments.len() <= 32);
                    }
                    Some(ChatEvent::Observing(_)) | None => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Some(ChatEvent::Finished { resolution, .. }) => {
                        let resolution = resolution.unwrap();
                        assert!(
                            resolution.diagnostics.is_empty(),
                            "{:?}",
                            resolution.diagnostics
                        );
                        let CompletionState::Committed(completion) = resolution.state else {
                            panic!("expected committed conversation")
                        };
                        assert_eq!(saw_preview, !tools);
                        assert_eq!(completion.result.text, expected);
                        assert_eq!(completion.result.usages.len(), calls);
                        for usage in &completion.result.usages {
                            let timing = usage
                                .timing
                                .as_ref()
                                .expect("runner request events populate timing");
                            assert!(timing.is_valid() && timing.elapsed_us > 0);
                        }
                        assert_eq!(
                            completion.result.title.as_deref(),
                            (!tools && calls == 2).then_some("Generated title")
                        );
                        if !tools && calls == 2 {
                            assert_eq!(
                                completion.result.usages[1]
                                    .attribution
                                    .as_ref()
                                    .unwrap()
                                    .purpose
                                    .as_deref(),
                                Some("title")
                            );
                        }
                        break;
                    }
                    Some(ChatEvent::Error { source, .. }) => {
                        panic!("conversation failed: {source}")
                    }
                    _ => panic!("unexpected controller event"),
                }
            }
        }
        controller.close().unwrap();
        let database = ChatDatabase::open(path).unwrap();
        let id = session_id.unwrap();
        assert_eq!(
            database.session(&id).unwrap().title,
            if tools { "first" } else { "Generated title" }
        );
        assert_eq!(database.history(&id, 4096).unwrap().history.len(), 4);
        assert_eq!(
            database
                .usage(mf_chat::UsageQuery {
                    profile: Some("assistant-flow".into()),
                    ..Default::default()
                })
                .unwrap()
                .overall
                .attempted_calls,
            3,
        );
        assert_eq!(server.requests().len(), 3);
        assert!(
            messages(&server.requests()[2])
                .iter()
                .any(|message| message["content"] == "first answer")
        );
        if tools {
            let requests = server.requests();
            let schema = &requests[0].body["tools"][0]["function"]["parameters"]["properties"];
            assert!(schema.get("amount").is_some());
            assert!(schema.get("factor").is_none());
            let result = requests[1].body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|message| message["role"] == "tool")
                .unwrap();
            assert_eq!(result["tool_call_id"], "multiply-1");
            let output: serde_json::Value =
                serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
            assert_eq!(output, serde_json::json!({"result":42}));
            let transcript = database.history(&id, 4096).unwrap().history;
            assert_eq!(transcript[1].content, "first answer");
            assert_eq!(transcript[3].content, "second answer");
            assert!(
                transcript
                    .iter()
                    .all(|message| message.tool_calls.is_none() && message.tool_call_id.is_none())
            );
        }
    }
}

fn compiled(root: &Path, server: &Server, example: &str) -> std::path::PathBuf {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let definition = root.join("flow.json");
    let mut value: serde_json::Value = serde_json::from_slice(
        &fs::read(crates.parent().unwrap().join("examples").join(example)).unwrap(),
    )
    .unwrap();
    value["dependencies"]["llm"]["path"] =
        crates.join("builtin-nodes/llm").to_str().unwrap().into();
    value["dependencies"]["core"]["path"] =
        crates.join("builtin-nodes/core").to_str().unwrap().into();
    if let Some(code) = value["dependencies"].get_mut("code") {
        code["path"] = crates.join("builtin-nodes/code").to_str().unwrap().into();
    }
    let parameters = &mut value["nodes"][0]["config"]["parameters"];
    parameters["model"]["default"] = "wrong-default-model".into();
    parameters["base_url"]["default"] = server.base_url.clone().into();
    parameters["api_key"]["env"] = "MF_CHAT_TUI_KEY".into();
    fs::write(&definition, serde_json::to_vec(&value).unwrap()).unwrap();
    let binary = root.join("chat-flow");
    let build = root.join("build");
    compile_project_with_options(
        &CompileRequest {
            definition: &definition,
            output: &binary,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: crates.into(),
            },
        },
        &RunnerOptions { telemetry: true },
    )
    .unwrap();
    fs::remove_file(&definition).unwrap();
    fs::remove_dir_all(build).unwrap();
    binary
}

fn fail_save(path: &Path, enabled: bool) {
    let mut connection = diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
    connection
        .batch_execute("PRAGMA busy_timeout=5000;")
        .unwrap();
    connection.batch_execute(if enabled { "CREATE TRIGGER fail_reply BEFORE INSERT ON messages WHEN NEW.role='assistant' BEGIN SELECT RAISE(ABORT, 'synthetic save failure'); END;" } else { "DROP TRIGGER fail_reply;" }).unwrap();
}

// Drive the terminal with parent-owned storage and child model calls.
fn interact(binary: &Path, database: &Path, restart: bool) -> String {
    let pty = openpty(
        Some(&Winsize {
            ws_row: 50,
            ws_col: 180,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let flags = FdFlag::from_bits_truncate(fcntl(&pty.master, FcntlArg::F_GETFD).unwrap());
    fcntl(&pty.master, FcntlArg::F_SETFD(flags | FdFlag::FD_CLOEXEC)).unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
    command.args(["run", "--chat"]).env_clear();
    if restart {
        command.arg("--chat-db").arg(database);
    } else {
        command.env("HOME", binary.parent().unwrap().join("home"));
    }
    let mut child = command
        .arg(binary)
        .args(["--params.model", "fixture-model"])
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("PATH", "")
        .env("MF_CHAT_TUI_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .spawn()
        .unwrap();
    let mut master = fs::File::from(pty.master);
    fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
    let mut screen = Screen::new(180, 50);
    let mut history = String::new();
    let mut stage = 0usize;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let mut bytes = [0; 16384];
        match master.read(&mut bytes) {
            Ok(n) if n > 0 => {
                screen.feed(&bytes[..n]);
                let text = screen.text();
                history.push_str(&text);
                history.push('\n');
                let keys: Option<&[u8]> = if restart {
                    match stage {
                        0 if text.contains("New chat") => Some(b"\x13"),
                        1 if text.contains("Switch chat") && text.contains("session B prompt") => {
                            Some(b"\x1b[B\r")
                        }
                        2 if text.contains("retained reply") => {
                            Some(b"continued after restart\x1b\r")
                        }
                        3 if text.contains("restart reply") => Some(b"\x11"),
                        _ => None,
                    }
                } else {
                    match stage {
                        0 if text.contains("What would you like") => {
                            Some("\x1b[200~hello 🦀\r\nsecond line\x1b[201~\x1b\r".as_bytes())
                        }
                        1 if text.contains("first reply") => {
                            assert!(text.contains("Flow execution"), "{text}");
                            assert!(text.contains("Flow: Succeeded"), "{text}");
                            Some(b"\x0e")
                        }
                        2 if text.contains("New chat") => Some(b"session B prompt\x1b\r"),
                        3 if text.contains("second reply") => Some(b"failed prompt\x1b\r"),
                        4 if text.contains("[failed]") => Some(b"\x10\x1b[B\x1b[B\r"),
                        5 if text.contains("retried reply") => {
                            fail_save(database, true);
                            Some(b"retain this reply\x1b\r")
                        }
                        6 if text.contains("Assistant [not saved]") => {
                            fail_save(database, false);
                            Some(b"\x10\x1b[B\x1b[B\r")
                        }
                        7 if text.contains("retained reply")
                            && text.contains("Saved")
                            && !text.contains("[not saved]") =>
                        {
                            Some(b"\x10")
                        }
                        8 if text.contains("Inspect current run") => Some(b"\x1b[B\x1b[B\r"),
                        9 if text.contains("Workflow:") => Some(b"\x1b"),
                        10 if text.contains("Alt+Enter send") => Some(b"\x13"),
                        11 if text.contains("Switch chat") && text.contains("hello") => Some(b"\r"),
                        12 if text.contains("first reply") && !text.contains("second reply") => {
                            Some(b"\x11")
                        }
                        _ => None,
                    }
                };
                if let Some(keys) = keys {
                    master.write_all(keys).unwrap();
                    stage += 1;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Ok(_) | Err(_) => {}
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}: {history}");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("chat stalled at stage {stage}: {history}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(stage, if restart { 4 } else { 13 }, "{history}");
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    let mut stdout = vec![];
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    assert!(stdout.is_empty());
    history
}

#[test]
fn executable_chat_preserves_sessions_retry_save_usage_and_source_free_restart() {
    let mut failure =
        Reply::json(serde_json::json!({"error":{"message":"synthetic provider failure"}}));
    failure.status = 400;
    let mut second = Reply::text("second reply");
    second.header_delay = Duration::from_millis(500);
    let server = Server::start(vec![
        Reply::text("first reply"),
        second,
        failure,
        Reply::text("retried reply"),
        Reply::text("retained reply"),
        Reply::text("restart reply"),
        {
            let mut reply = Reply::text("late model reply");
            reply.header_delay = Duration::from_secs(2);
            reply
        },
        {
            let mut reply = Reply::text("display reply");
            reply.header_delay = Duration::from_secs(2);
            reply
        },
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-flow.json");
    #[cfg(target_os = "macos")]
    let path = root
        .path()
        .join("home/Library/Application Support/miniflow/chat.sqlite");
    #[cfg(not(target_os = "macos"))]
    let path = root.path().join("home/.local/share/miniflow/chat.sqlite");
    let nonterminal = Command::new(env!("CARGO_BIN_EXE_mf"))
        .args(["run", "--tui", "--chat"])
        .arg(&binary)
        .args(["--params.api_key", "synthetic-key"])
        .env("HOME", root.path().join("home"))
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!nonterminal.status.success());
    assert!(String::from_utf8_lossy(&nonterminal.stderr).contains("requires terminal"));
    assert!(!root.path().join("home").exists());
    let observed = interact(&binary, &path, false);
    assert!(!observed.contains("synthetic-key"));
    assert_eq!(server.requests().len(), 5);
    interact(&binary, &path, true);
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(
        requests
            .iter()
            .all(|request| request.body["model"] == "fixture-model")
    );
    assert_eq!(
        messages(&requests[0]).last().unwrap()["content"],
        "hello 🦀\nsecond line"
    );
    assert_eq!(messages(&requests[3]), messages(&requests[2]));
    let database = ChatDatabase::open(&path).unwrap();
    let sessions = database.list_sessions(false, 50, None).unwrap().sessions;
    assert_eq!(sessions.len(), 2);
    let session = sessions
        .iter()
        .find(|s| s.title == "session B prompt")
        .unwrap();
    let transcript = database.messages(&session.id, 50, None).unwrap().messages;
    assert_eq!(
        transcript
            .iter()
            .filter(|m| m.message.role == "assistant")
            .count(),
        4
    );
    let failed = transcript
        .iter()
        .find(|m| m.turn_state == "failed")
        .unwrap();
    let retry = transcript
        .iter()
        .find(|m| m.message.role == "assistant" && m.message.content == "retried reply")
        .unwrap();
    assert_eq!(
        database
            .turn(&retry.message.turn_id)
            .unwrap()
            .retry_of
            .as_deref(),
        Some(failed.message.turn_id.as_str())
    );
    assert_eq!(
        database
            .usage(mf_chat::UsageQuery {
                model: Some("fixture-model".into()),
                ..Default::default()
            })
            .unwrap()
            .overall
            .attempted_calls,
        6
    );
    let usage = database.usage(Default::default()).unwrap();
    assert_eq!(usage.overall.attempted_calls, 6);
    assert_eq!(usage.overall.total_tokens.missing_calls, 1);
    assert_eq!(usage.overall.performance.elapsed.reported_calls, 6);
    let retained = transcript
        .iter()
        .find(|message| message.message.content == "retained reply")
        .unwrap();
    assert!(retained.usages.as_ref().unwrap().iter().all(|usage| {
        usage
            .timing
            .as_ref()
            .is_some_and(|timing| timing.is_valid() && timing.elapsed_us > 0)
    }));
    assert!(
        sessions
            .iter()
            .all(|session| session.title_source == "provisional")
    );
    application_exit(
        &binary,
        &root.path().join("owner.sqlite"),
        &server,
        7,
        false,
    );
    application_exit(
        &binary,
        &root.path().join("display.sqlite"),
        &server,
        8,
        true,
    );
    assert_eq!(server.requests().len(), 8);
    loop_conversations();
}
fn application_exit(
    binary: &Path,
    database: &Path,
    server: &Server,
    expected_requests: usize,
    display_failure: bool,
) {
    let size = Winsize {
        ws_row: 40,
        ws_col: 160,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let input = openpty(Some(&size), None).unwrap();
    let display = openpty(Some(&size), None).unwrap();
    for master in [&input.master, &display.master] {
        let flags = FdFlag::from_bits_truncate(fcntl(master, FcntlArg::F_GETFD).unwrap());
        fcntl(master, FcntlArg::F_SETFD(flags | FdFlag::FD_CLOEXEC)).unwrap();
    }
    let original = tcgetattr(&input.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .args(["run", "--tui", "--chat", "--chat-db"])
        .arg(database)
        .arg(binary)
        .args(["--params.model", "fixture-model"])
        .stdin(Stdio::from(input.slave.try_clone().unwrap()))
        .stderr(Stdio::from(display.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env("PATH", "")
        .env("MF_CHAT_TUI_KEY", "synthetic-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .spawn()
        .unwrap();
    let mut keyboard = fs::File::from(input.master);
    let mut output = fs::File::from(display.master);
    fcntl(&output, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
    let mut screen = Screen::new(160, 40);
    let mut stage = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut bytes = [0; 8192];
        if let Ok(n) = output.read(&mut bytes)
            && n > 0
        {
            screen.feed(&bytes[..n]);
            let text = screen.text();
            let keys: Option<&[u8]> = match stage {
                0 if text.contains("What would you like") => Some(b"exit recovery probe\x1b\r"),
                _ => None,
            };
            if let Some(keys) = keys {
                keyboard.write_all(keys).unwrap();
                stage += 1;
            }
        }
        if server.requests().len() >= expected_requests {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "application exit stalled at {stage}: {}",
            screen.text()
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(stage, 1);
    if display_failure {
        drop(output);
    } else {
        child.kill().unwrap();
    }
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("display cleanup did not finish");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    if display_failure {
        assert_eq!(status.code(), Some(1));
        let restored = tcgetattr(&input.slave).unwrap();
        assert_eq!(
            original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
            restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
        );
    }
    let database = ChatDatabase::open(database).unwrap();
    let session = database
        .list_sessions(false, 50, None)
        .unwrap()
        .sessions
        .remove(0);
    let messages = database.messages(&session.id, 50, None).unwrap().messages;
    assert_eq!(messages.len(), 1);
    let turn = database.turn(&messages[0].message.turn_id).unwrap();
    assert_eq!(
        turn.state,
        if display_failure {
            "cancelled"
        } else {
            "interrupted"
        }
    );
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .total_tokens
            .known_subtotal,
        None
    );
    if !display_failure {
        // The restarted application owns storage while the old model process can still return a reply.
        thread::sleep(Duration::from_secs(2));
        assert_eq!(database.turn(&turn.id).unwrap().state, "interrupted");
        assert_eq!(
            database
                .messages(&session.id, 50, None)
                .unwrap()
                .messages
                .len(),
            1
        );
    }
}

fn loop_conversations() {
    use mf_chat::contract::CompletionState;
    use mf_tui::chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions};

    let mut failure =
        Reply::json(serde_json::json!({"error":{"message":"second iteration failed"}}));
    failure.status = 400;
    let server = Server::start(vec![
        Reply::text("intermediate answer"),
        Reply::text("final answer"),
        Reply::text("partial answer"),
        failure,
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-loop-flow.json");
    let path = root.path().join("chat.sqlite");
    let launch = ChatLaunch::prepare(
        &binary,
        ChatOptions {
            database_path: path.clone(),
            profile: "loop".into(),
            run: mf_tui::run::RunOptions {
                inputs: Some(
                    serde_json::json!({
                        "params.model":"loop-model", "params.api_key":"synthetic-key"
                    })
                    .to_string(),
                ),
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller = ChatController::open(launch).unwrap();
    let next = || {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(event) = controller.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "loop controller event deadline");
            thread::sleep(Duration::from_millis(10));
        }
    };
    let mut session_id = None;
    let mut invocations = vec![];
    for prompt in ["first turn", "failed turn"] {
        controller
            .send(ChatCommand::Submit {
                session_id: session_id.clone(),
                prompt: prompt.into(),
            })
            .unwrap();
        loop {
            match next() {
                ChatEvent::Submitted {
                    session,
                    invocation,
                    ..
                } => {
                    session_id = Some(session.id);
                    invocations.push(invocation);
                }
                ChatEvent::Observing(_) => {}
                ChatEvent::Finished {
                    resolution,
                    status,
                    capture_error,
                    ..
                } => {
                    assert!(status.unwrap().success());
                    assert!(capture_error.is_none());
                    let resolution = resolution.unwrap();
                    assert!(
                        resolution.diagnostics.is_empty(),
                        "{:?}",
                        resolution.diagnostics
                    );
                    if prompt == "first turn" {
                        let CompletionState::Committed(completion) = resolution.state else {
                            panic!("expected committed loop result")
                        };
                        assert_eq!(completion.result.text, "final answer");
                        assert_eq!(completion.result.usages.len(), 2);
                        assert_eq!(
                            serde_json::to_value(completion.result.metadata).unwrap(),
                            serde_json::json!({"strategy":"iterative"})
                        );
                    } else {
                        let CompletionState::Finished(turn) = resolution.state else {
                            panic!("expected failed loop result")
                        };
                        assert_eq!(turn.state, "failed");
                        assert!(turn.error.unwrap().contains("HTTP 400"));
                    }
                    break;
                }
                ChatEvent::Error { source, .. } => {
                    panic!("loop controller failed: {source}")
                }
                _ => panic!("unexpected loop event"),
            }
        }
    }
    controller.close().unwrap();
    let database = ChatDatabase::open(path).unwrap();
    let history = database
        .history(session_id.as_ref().unwrap(), 4096)
        .unwrap()
        .history;
    assert_eq!(history.len(), 2);
    assert_eq!(
        serde_json::to_value(&history[1].metadata).unwrap(),
        serde_json::json!({"strategy":"iterative"})
    );
    for invocation in &invocations {
        let call = database.call(&invocation.call_id).unwrap();
        let usages = call.usages.unwrap();
        assert_eq!(usages.len(), 2);
        assert_ne!(usages[0].call_id, usages[1].call_id);
        assert!(usages.iter().all(|usage| usage.model == "loop-model"));
        assert!(usages.iter().all(|usage| {
            usage
                .timing
                .as_ref()
                .is_some_and(|timing| timing.is_valid() && timing.elapsed_us > 0)
        }));
        assert_ne!(
            usages[0].node.as_ref().unwrap().path,
            usages[1].node.as_ref().unwrap().path
        );
    }
    let statistics = database.usage(Default::default()).unwrap();
    assert_eq!(statistics.overall.attempted_calls, 4);
    assert_eq!(statistics.overall.statuses["succeeded"], 3);
    assert_eq!(statistics.overall.statuses["failed"], 1);
    assert_eq!(statistics.unreported_invocations, 0);
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(
        messages(&requests[2])
            .iter()
            .any(|message| message["content"] == "final answer")
    );
}
