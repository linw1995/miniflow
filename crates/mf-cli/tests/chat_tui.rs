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
fn supplied_title_workflow_runs_inside_the_application_lifecycle() {
    use mf_chat::contract::CompletionState;
    use mf_tui::{
        chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions},
        run::RunOptions,
    };

    let root = tempfile::tempdir().unwrap();
    let server = Server::start(vec![
        Reply::stream_text("first answer"),
        Reply::text("Generated title"),
        Reply::stream_text("second answer"),
    ]);
    let binary = compiled(root.path(), &server, "chat-title-flow.json");
    let path = root.path().join("chat.sqlite");
    let launch = ChatLaunch::prepare(
        &binary,
        ChatOptions {
            database_path: path.clone(),
            profile: "assistant-flow".into(),
            run: RunOptions {
                arguments: vec![
                    "--params.model=fixture-model".into(),
                    "--params.api_key=synthetic-key".into(),
                ],
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller = ChatController::open(launch).unwrap();
    let mut session_id = None;
    for (prompt, expected, calls) in [("first", "first answer", 2), ("second", "second answer", 1)]
    {
        controller
            .send(ChatCommand::Submit {
                session_id: session_id.clone(),
                prompt: prompt.into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "conversation did not finish");
            match controller.try_event() {
                Some(ChatEvent::Submitted { session, .. }) => session_id = Some(session.id),
                Some(ChatEvent::Preview { preview, .. }) => {
                    assert!(preview.segments.len() <= 32);
                }
                Some(ChatEvent::Observing(_)) | None => thread::sleep(Duration::from_millis(10)),
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
                    assert_eq!(completion.result.final_text(), Some(expected));
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
                        (calls == 2).then_some("Generated title")
                    );
                    if calls == 2 {
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
                Some(ChatEvent::Error { source, .. }) => panic!("conversation failed: {source}"),
                _ => panic!("unexpected controller event"),
            }
        }
    }
    controller.close().unwrap();
    let database = ChatDatabase::open(path).unwrap();
    let id = session_id.unwrap();
    assert_eq!(database.session(&id).unwrap().title, "Generated title");
    assert_eq!(database.model_context(&id).unwrap().messages.len(), 4);
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
    if example == "chat-context-flow.json" {
        let examples = crates.parent().unwrap().join("examples");
        let mut conversation: serde_json::Value =
            serde_json::from_slice(&fs::read(examples.join("chat-calculator-flow.json")).unwrap())
                .unwrap();
        conversation["dependencies"] = value["dependencies"].clone();
        fs::write(
            root.join("chat-calculator-flow.json"),
            serde_json::to_vec(&conversation).unwrap(),
        )
        .unwrap();
        fs::create_dir(root.join("agent-calculator")).unwrap();
        for entry in fs::read_dir(examples.join("agent-calculator")).unwrap() {
            let source = entry.unwrap().path();
            let mut tool: serde_json::Value =
                serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
            for (alias, dependency) in tool["dependencies"].as_object_mut().unwrap() {
                dependency["path"] = value["dependencies"][alias]["path"].clone();
            }
            fs::write(
                root.join("agent-calculator")
                    .join(source.file_name().unwrap()),
                serde_json::to_vec(&tool).unwrap(),
            )
            .unwrap();
        }
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
    if example == "chat-context-flow.json" {
        fs::remove_file(root.join("chat-calculator-flow.json")).unwrap();
        fs::remove_dir_all(root.join("agent-calculator")).unwrap();
    }
    binary
}

fn calculator_call(id: &str, operation: &str, left: f64, right: f64) -> Reply {
    let mut reply = Reply::stream_text("");
    let arguments =
        serde_json::json!({"operation":operation,"left":left,"right":right}).to_string();
    let identity = serde_json::json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"calculator","arguments":""}}]},"finish_reason":null}]});
    let chunk = serde_json::json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":arguments}}]},"finish_reason":null}]});
    let finish = serde_json::json!({"id":"response","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
    reply.body = format!("data: {identity}\n\ndata: {chunk}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    reply
}

fn next_completion(
    controller: &mf_tui::chat::ChatController,
) -> mf_chat::contract::ReconciledCompletion {
    use mf_tui::chat::ChatEvent;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Instant::now() < deadline,
            "context conversation did not complete"
        );
        match controller.try_event() {
            Some(ChatEvent::Finished {
                resolution,
                status,
                capture_error,
                stderr,
                ..
            }) => {
                assert!(
                    status.is_none_or(|status| status.success()),
                    "{status:?}: {stderr}; {:?}",
                    resolution.as_ref().err()
                );
                assert!(capture_error.is_none(), "{capture_error:?}");
                return resolution.unwrap();
            }
            Some(ChatEvent::Error { source, .. }) => {
                panic!("context conversation failed: {source}")
            }
            _ => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[test]
fn compiled_context_replacement_keeps_tool_audit_and_save_only_retries_after_reopen() {
    use mf_chat::{MessageUpdate, ToolExecutionPhase, contract::CompletionState};
    use mf_tui::{
        chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions},
        run::RunOptions,
    };
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let release = Arc::new(AtomicBool::new(false));
    let mut answer = Reply::stream_text("First answer: 5");
    answer.fragments = Some(vec![vec![], answer.body.as_bytes().to_vec()]);
    answer.release_after_prefix = Some(release.clone());
    let server = Server::start(vec![
        calculator_call("first", "add", 2.0, 3.0),
        answer,
        calculator_call("second", "multiply", 3.0, 4.0),
        Reply::stream_text("Second answer: 12"),
        Reply::text("Summary only: 5 and 12"),
        Reply::stream_text("Third answer"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let binary = compiled(root.path(), &server, "chat-context-flow.json");
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let id = database
        .create_session(
            "context",
            Some("Original instructions".into()),
            Some("Manual name".into()),
        )
        .unwrap()
        .id;
    drop(database);
    let mut original = vec![];
    for (turn, compact) in [false, true, false].into_iter().enumerate() {
        let launch = ChatLaunch::prepare(&binary, ChatOptions {
            database_path: path.clone(), profile: "context".into(),
            run: RunOptions { inputs: Some(json!({"params.model":"fixture-model","params.api_key":"synthetic-key","params.compact":compact}).to_string()), ..Default::default() },
        }).unwrap();
        let controller = ChatController::open(launch).unwrap();
        if compact {
            fail_save(&path, true);
        }
        controller
            .send(ChatCommand::Submit {
                session_id: Some(id.clone()),
                prompt: format!("Question {turn}"),
            })
            .unwrap();
        if turn == 0 {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut observation = None;
            let mut arguments_seen = false;
            loop {
                assert!(
                    Instant::now() < deadline,
                    "no correlated tool preview and runner observation"
                );
                match controller.try_event() {
                    Some(ChatEvent::Observing(run)) => observation = Some(run),
                    Some(ChatEvent::Preview { preview, .. }) => {
                        arguments_seen |= preview.segments.iter().any(|segment| {
                            segment.tool.as_ref().is_some_and(|tool| {
                                tool.id.as_deref() == Some("first")
                                    && tool.name.as_deref() == Some("calculator")
                            }) && segment.text.contains("\"operation\"")
                        });
                    }
                    Some(ChatEvent::Finished { .. }) => panic!("settled before observation"),
                    _ => thread::sleep(Duration::from_millis(2)),
                }
                let Some(run) = &observation else { continue };
                let statuses = run.reader.tool_metrics().snapshot();
                if arguments_seen
                    && statuses
                        .get("first")
                        .is_some_and(|tool| tool.phase == Some(ToolExecutionPhase::Succeeded))
                {
                    assert_eq!(statuses.len(), 1);
                    assert_eq!(statuses["first"].name, "calculator");
                    assert!(statuses["first"].duration_ns.is_some());
                    break;
                }
            }
            assert!(controller.busy());
            release.store(true, Ordering::Release);
        }
        let mut resolution = next_completion(&controller);
        if compact {
            let CompletionState::Uncommitted(pending) = resolution.state else {
                panic!("expected complete replacement candidate")
            };
            assert!(matches!(
                pending.completion.result.update,
                MessageUpdate::ReplaceContext { .. }
            ));
            assert_eq!(pending.completion.result.display_messages().len(), 3);
            assert_eq!(pending.completion.result.usages.len(), 3);
            assert_eq!(server.requests().len(), 5);
            fail_save(&path, false);
            controller.send(ChatCommand::Save).unwrap();
            resolution = next_completion(&controller);
            assert_eq!(
                server.requests().len(),
                5,
                "save-only retry must not call a model or tool"
            );
        }
        let CompletionState::Committed(completion) = resolution.state else {
            panic!("expected committed context result")
        };
        assert_eq!(
            matches!(
                completion.result.update,
                MessageUpdate::ReplaceContext { .. }
            ),
            compact
        );
        assert_eq!(
            completion.result.usages.len(),
            if compact {
                3
            } else if turn == 0 {
                2
            } else {
                1
            }
        );
        if turn == 0 {
            assert!(completion.result.usages.iter().all(|call| {
                call.timing
                    .as_ref()
                    .is_some_and(|timing| timing.first_token_us.is_some())
            }));
        }
        controller.close().unwrap();
        let database = ChatDatabase::open(&path).unwrap();
        let page = database.messages(&id, 50, None).unwrap();
        if turn == 1 {
            assert_eq!(
                database.model_context(&id).unwrap().messages,
                vec![mf_chat::ChatMessage::System {
                    content: "Summary only: 5 and 12".into(),
                    metadata: None
                }]
            );
            original = page
                .messages
                .iter()
                .map(|record| record.message.clone())
                .collect();
            assert_eq!(original.len(), 8);
        } else if turn == 2 {
            assert_eq!(
                page.messages[..8]
                    .iter()
                    .map(|record| &record.message)
                    .collect::<Vec<_>>(),
                original.iter().collect::<Vec<_>>()
            );
            assert_eq!(
                page.messages
                    .iter()
                    .filter(|record| record.message.role() == "tool")
                    .count(),
                2
            );
            assert_eq!(
                database
                    .usage(Default::default())
                    .unwrap()
                    .overall
                    .attempted_calls,
                6
            );
        }
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert_eq!(
        messages(&requests[5]),
        vec![
            json!({"role":"system","content":"Summary only: 5 and 12"}),
            json!({"role":"user","content":"Question 2"})
        ]
    );
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
}

fn fail_save(path: &Path, enabled: bool) {
    let mut connection = diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
    connection
        .batch_execute("PRAGMA busy_timeout=5000;")
        .unwrap();
    connection.batch_execute(if enabled { "CREATE TRIGGER fail_reply BEFORE INSERT ON messages WHEN json_extract(NEW.payload, '$.role')='assistant' BEGIN SELECT RAISE(ABORT, 'synthetic save failure'); END;" } else { "DROP TRIGGER fail_reply;" }).unwrap();
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
                        6 if text.contains("[not saved]")
                            && text.contains("Model context update pending") =>
                        {
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
                        9 if text.contains("Workflow:") => Some(b"\x1b[<35;40;10M\x1b"),
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
        Reply::text("hello"),
        second,
        Reply::text("session B prompt"),
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
    assert_eq!(server.requests().len(), 7);
    interact(&binary, &path, true);
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(
        requests
            .iter()
            .all(|request| request.body["model"] == "fixture-model")
    );
    assert_eq!(
        messages(&requests[0]).last().unwrap()["content"],
        "hello 🦀\nsecond line"
    );
    assert_eq!(messages(&requests[5]), messages(&requests[4]));
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
            .filter(|m| m.message.role() == "assistant")
            .count(),
        4
    );
    let failed = transcript
        .iter()
        .find(|m| m.turn_state == "failed")
        .unwrap();
    let retry = transcript
        .iter()
        .find(|m| m.message.role() == "assistant" && m.message.content() == "retried reply")
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
        8
    );
    let usage = database.usage(Default::default()).unwrap();
    assert_eq!(usage.overall.attempted_calls, 8);
    assert_eq!(usage.overall.total_tokens.missing_calls, 1);
    assert_eq!(usage.overall.performance.elapsed.reported_calls, 8);
    let retained = transcript
        .iter()
        .find(|message| message.message.content() == "retained reply")
        .unwrap();
    let mut retained_facts = transcript
        .iter()
        .filter(|record| record.message.turn_id == retained.message.turn_id)
        .filter_map(|record| record.usages.as_ref());
    assert!(retained_facts.next().unwrap().iter().all(|usage| {
        usage
            .timing
            .as_ref()
            .is_some_and(|timing| timing.is_valid() && timing.elapsed_us > 0)
    }));
    assert!(retained_facts.next().is_none());
    assert!(
        sessions
            .iter()
            .all(|session| session.title_source == "generated")
    );
    let title_usage = transcript
        .iter()
        .flat_map(|message| message.usages.iter().flatten())
        .find(|usage| {
            usage
                .attribution
                .as_ref()
                .and_then(|labels| labels.purpose.as_deref())
                == Some("title")
        })
        .expect("the parent flow records the built-in title call");
    assert_eq!(title_usage.node.as_ref().unwrap().path[0].node, "title");
    application_exit(
        &binary,
        &root.path().join("owner.sqlite"),
        &server,
        9,
        false,
    );
    application_exit(
        &binary,
        &root.path().join("display.sqlite"),
        &server,
        10,
        true,
    );
    assert_eq!(server.requests().len(), 10);
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
        Reply::stream_text("intermediate answer"),
        Reply::stream_text("final answer"),
        Reply::text("Loop title"),
        Reply::stream_text("partial answer"),
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
        let mut saw_preview = false;
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
                ChatEvent::Preview { preview, .. } => {
                    saw_preview = true;
                    assert!(!preview.segments.is_empty());
                }
                ChatEvent::Finished {
                    resolution,
                    status,
                    capture_error,
                    ..
                } => {
                    assert!(saw_preview);
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
                            panic!(
                                "expected committed loop result: {:?}; requests: {}",
                                resolution.state,
                                server.requests().len()
                            )
                        };
                        assert_eq!(completion.result.final_text(), Some("final answer"));
                        assert_eq!(completion.result.usages.len(), 3);
                        assert_eq!(completion.result.title.as_deref(), Some("Loop title"));
                        assert_eq!(
                            serde_json::to_value(
                                completion.result.display_messages().last().unwrap()
                            )
                            .unwrap()["metadata"],
                            serde_json::json!({"strategy":"iterative"})
                        );
                    } else {
                        let CompletionState::Committed(completion) = resolution.state else {
                            panic!("expected a durable failed loop result")
                        };
                        assert_eq!(completion.result.status, "failed");
                        assert!(completion.result.error.unwrap().contains("HTTP 400"));
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
        .model_context(session_id.as_ref().unwrap())
        .unwrap()
        .messages;
    assert_eq!(history.len(), 3);
    assert!(matches!(&history[1],
        mfn_llm::ChatMessage::Assistant { metadata: Some(metadata), .. }
            if serde_json::to_value(metadata).unwrap() == serde_json::json!({"strategy":"iterative"})
    ));
    for (index, invocation) in invocations.iter().enumerate() {
        let call = database.call(&invocation.call_id).unwrap();
        let usages = call.usages.unwrap();
        assert_eq!(usages.len(), if index == 0 { 3 } else { 2 });
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
    assert_eq!(statistics.overall.attempted_calls, 5);
    assert_eq!(statistics.overall.statuses["succeeded"], 4);
    assert_eq!(statistics.overall.statuses["failed"], 1);
    assert_eq!(statistics.unreported_invocations, 0);
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    assert!(
        messages(&requests[3])
            .iter()
            .any(|message| message["content"] == "final answer")
    );
}

#[test]
fn model_selection_defaults_to_first_model_persists_preferences_and_reuses_on_restart() {
    use mf_chat::contract::CompletionState;
    use mf_tui::{
        chat::{ChatCommand, ChatController, ChatEvent, ChatLaunch, ChatOptions},
        run::RunOptions,
    };

    let root = tempfile::tempdir().unwrap();
    let server1 = Server::start_with_models(
        vec![
            Reply::text("reply-alpha"),
            Reply::text("Generated title"),
            Reply::text("reply-beta-1"),
            Reply::text("reply-beta-2"),
        ],
        vec![
            Reply::models(serde_json::json!([
                {"id": "model-alpha", "context_window": 131072},
                {"id": "model-beta", "context_window": 64000}
            ])),
            Reply::models(serde_json::json!([
                {"id": "model-alpha", "context_window": 131072},
                {"id": "model-beta", "context_window": 64000}
            ])),
            Reply::models(serde_json::json!([
                {"id": "model-alpha", "context_window": 131072},
                {"id": "model-beta", "context_window": 64000}
            ])),
        ],
    );
    let server2 = Server::start_with_models(
        vec![Reply::text("reply-server2")],
        vec![Reply::models(serde_json::json!([
            {"id": "server2-first", "context_window": 32768},
            {"id": "server2-second", "context_window": 16384}
        ]))],
    );
    // Compile chat-flow.json without hardcoding server1's URL as the default, so restart
    // genuinely proves that saved startup parameters supply both base_url and model.
    let placeholder_server = Server::start(vec![]);
    let binary = compiled(root.path(), &placeholder_server, "chat-flow.json");
    let db_path = root.path().join("chat.sqlite");
    let prefs_path = root.path().join("chat.sqlite.preferences.json");

    let wait_event = |controller: &ChatController| -> ChatEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "controller event timed out");
            if let Some(event) = controller.try_event() {
                return event;
            }
            thread::sleep(Duration::from_millis(10));
        }
    };
    let run_turn = |controller: &ChatController,
                    session_id: &mut Option<String>,
                    prompt: &str,
                    expected: &str| {
        controller
            .send(ChatCommand::Submit {
                session_id: session_id.clone(),
                prompt: prompt.into(),
            })
            .unwrap();
        loop {
            match wait_event(controller) {
                ChatEvent::Submitted { session, .. } => *session_id = Some(session.id),
                ChatEvent::Observing(_) | ChatEvent::Preview { .. } => {}
                ChatEvent::Finished { resolution, .. } => {
                    let CompletionState::Committed(completion) = resolution.unwrap().state else {
                        panic!("expected committed turn");
                    };
                    assert_eq!(completion.result.final_text(), Some(expected));
                    break;
                }
                ChatEvent::Error { source, .. } => panic!("turn failed: {source}"),
                _ => {}
            }
        }
    };

    // Launch 1: first run supplies base_url (no --params.model); defaults to first model.
    let launch1 = ChatLaunch::prepare(
        &binary,
        ChatOptions {
            database_path: db_path.clone(),
            profile: "default".into(),
            run: RunOptions {
                arguments: vec![
                    format!("--params.base_url={}", server1.base_url).into(),
                    "--params.api_key=synthetic-secret-key".into(),
                ],
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller1 = ChatController::open(launch1).unwrap();
    assert!(controller1.supports_model_selection());
    assert!(controller1.loading_default_model());
    match wait_event(&controller1) {
        ChatEvent::ModelSelected {
            model,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("model-alpha"));
            assert!(automatic);
            assert_eq!(warning, None);
        }
        other => panic!(
            "expected automatic ModelSelected, got {}",
            std::any::type_name_of_val(&other)
        ),
    }

    let mut session_id = None;
    run_turn(&controller1, &mut session_id, "first prompt", "reply-alpha");
    assert_eq!(server1.requests()[0].body["model"], "model-alpha");

    // Preferences file persists base_url and auto-selected model, never api_key.
    let saved_text = fs::read_to_string(&prefs_path).unwrap();
    assert!(saved_text.contains(&server1.base_url), "{saved_text}");
    assert!(saved_text.contains("model-alpha"), "{saved_text}");
    assert!(!saved_text.contains("synthetic-secret-key"), "{saved_text}");
    assert!(!saved_text.contains("api_key"), "{saved_text}");

    // List and switch model interactively inside the session.
    controller1.send(ChatCommand::ListModels).unwrap();
    match wait_event(&controller1) {
        ChatEvent::Models(Ok(models)) => {
            let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, vec!["model-alpha", "model-beta"]);
        }
        _ => panic!("expected Models list"),
    }
    controller1
        .send(ChatCommand::SelectModel {
            model: "model-beta".into(),
        })
        .unwrap();
    match wait_event(&controller1) {
        ChatEvent::ModelSelected {
            model,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("model-beta"));
            assert!(!automatic);
            assert_eq!(warning, None);
        }
        _ => panic!("expected manual ModelSelected"),
    }
    run_turn(
        &controller1,
        &mut session_id,
        "second prompt",
        "reply-beta-1",
    );
    assert_eq!(server1.requests()[1].body["model"], "model-alpha");
    assert_eq!(server1.requests()[2].body["model"], "model-beta");
    controller1.close().unwrap();

    // Launch 2: restart without passing base_url or model; reuses saved startup config.
    let launch2 = ChatLaunch::prepare(
        &binary,
        ChatOptions {
            database_path: db_path.clone(),
            profile: "default".into(),
            run: RunOptions {
                arguments: vec!["--params.api_key=synthetic-secret-key".into()],
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller2 = ChatController::open(launch2).unwrap();
    assert!(!controller2.loading_default_model());
    assert_eq!(controller2.initial_model(), Some("model-beta"));
    // Launch 1 seeded its context_window cache from /models (no extra compaction probe),
    // and Launch 2 startup needs no /models request because model-beta was restored.
    assert_eq!(server1.model_requests().len(), 2);
    run_turn(
        &controller2,
        &mut session_id,
        "third prompt",
        "reply-beta-2",
    );
    assert_eq!(server1.requests()[3].body["model"], "model-beta");
    // Turn 3 probes /models once for compaction's fresh session cache on controller2.
    assert_eq!(server1.model_requests().len(), 3);
    controller2.close().unwrap();

    // Launch 3: switching base_url without passing model drops stale saved model and
    // defaults to the first model of the new endpoint.
    let launch3 = ChatLaunch::prepare(
        &binary,
        ChatOptions {
            database_path: db_path,
            profile: "default".into(),
            run: RunOptions {
                arguments: vec![
                    format!("--params.base_url={}", server2.base_url).into(),
                    "--params.api_key=synthetic-secret-key".into(),
                ],
                ..Default::default()
            },
        },
    )
    .unwrap();
    let controller3 = ChatController::open(launch3).unwrap();
    assert!(controller3.loading_default_model());
    match wait_event(&controller3) {
        ChatEvent::ModelSelected {
            model, automatic, ..
        } => {
            assert_eq!(model.as_deref(), Some("server2-first"));
            assert!(automatic);
        }
        _ => panic!("expected server2 automatic ModelSelected"),
    }
    run_turn(
        &controller3,
        &mut session_id,
        "fourth prompt",
        "reply-server2",
    );
    assert_eq!(server2.requests()[0].body["model"], "server2-first");
    controller3.close().unwrap();
    assert!(placeholder_server.requests().is_empty());
}
