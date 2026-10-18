use super::*;
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, time::Instant};

pub fn launch(directory: &Path) -> ChatLaunch {
    let mut manifest = crate::test_support::sample_manifest();
    manifest.definition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[{"id":"start","kind":"builtin.llm","config":{"model":"fixture-model"}}],
        "outputs":[
            {"name":"status","node":"start","port":"status"},
            {"name":"text","node":"start","port":"text"},
            {"name":"usages","node":"start","port":"usages"}
        ]
    }))
    .unwrap();
    manifest.execution_order = vec!["start".into()];
    manifest.input_schema = serde_json::from_value(json!({"inputs":{"start":{
        "prompt":{"type":"string","required":true},
        "system":{"type":"string","required":false},
        "history":{"type":mf_runtime::ValueType::List(Box::new(mf_runtime::ValueType::Object)),"required":false}
    }},"stdin":{}})).unwrap();
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    let executable = directory.join("worker");
    fs::write(&executable, "#!/bin/sh\ntrap '' INT\nsleep 20\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    ChatLaunch {
        executable,
        manifest,
        contract,
        parameters: Default::default(),
        options: ChatOptions {
            database_path: directory.join("chat.sqlite"),
            profile: "default".into(),
            run: Default::default(),
        },
    }
}
pub fn next(controller: &ChatController) -> ChatEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(event) = controller.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "controller event deadline");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn argument_transports_and_invalid_chat_options_fail_before_database_ownership() {
    let root = tempfile::tempdir().unwrap();
    let prepared = launch(root.path());
    let binary = root.path().join("manifest");
    fs::write(
        &binary,
        crate::test_support::elf(Some(&prepared.manifest.to_bytes().unwrap()), 1),
    )
    .unwrap();
    let input = root.path().join("inputs.json");
    assert!(ChatLaunch::prepare(&binary, prepared.options.clone()).is_ok());
    let mut options = prepared.options.clone();
    options.profile = " ".into();
    assert!(ChatLaunch::prepare(&binary, options).is_err());
    fs::write(&input, "{}").unwrap();
    for run in [
        RunOptions {
            inputs: Some("{}".into()),
            ..Default::default()
        },
        RunOptions {
            inputs_file: Some(input.clone()),
            ..Default::default()
        },
    ] {
        let mut options = prepared.options.clone();
        options.run = run;
        assert!(ChatLaunch::prepare(&binary, options).is_ok());
    }
    for run in [
        RunOptions {
            inputs: Some("{}".into()),
            inputs_file: Some(input.clone()),
            ..Default::default()
        },
        RunOptions {
            inputs: Some("{}".into()),
            arguments: vec!["--start.prompt=owned".into()],
            ..Default::default()
        },
        RunOptions {
            arguments: vec!["--start.prompt=owned".into()],
            ..Default::default()
        },
        RunOptions {
            inputs: Some(r#"{"start.prompt":"owned"}"#.into()),
            ..Default::default()
        },
        RunOptions {
            inputs_file: Some(root.path().to_owned()),
            ..Default::default()
        },
        RunOptions {
            inputs_file: Some(root.path().join("missing.json")),
            ..Default::default()
        },
    ] {
        let mut options = prepared.options.clone();
        options.run = run;
        assert!(ChatLaunch::prepare(&binary, options).is_err());
    }
    assert!(!prepared.options.database_path.exists());
}

#[test]
fn one_execution_lane_rejects_queued_submissions_and_cancels_without_switching() {
    let root = tempfile::tempdir().unwrap();
    let prepared = launch(root.path());
    let path = prepared.options.database_path.clone();
    let controller = ChatController::open(prepared).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "first prompt".into(),
        })
        .unwrap();
    assert!(matches!(
        controller.send(ChatCommand::Submit {
            session_id: None,
            prompt: "must not queue".into()
        }),
        Err(ChatRunError::Busy)
    ));
    let ChatEvent::Submitted {
        invocation,
        session,
        ..
    } = next(&controller)
    else {
        panic!("expected admission")
    };
    assert_eq!(session.title, "first prompt");
    assert!(matches!(next(&controller), ChatEvent::Observing(_)));
    assert!(matches!(
        ChatDatabase::open(&path),
        Err(mf_chat::ChatError::Busy { .. })
    ));
    controller.cancel();
    let ChatEvent::Finished { resolution, .. } = next(&controller) else {
        panic!("expected cancelled turn")
    };
    assert!(
        matches!(resolution.unwrap().state, CompletionState::Finished(turn) if turn.state == "cancelled")
    );
    assert!(!controller.busy());
    controller.close().unwrap();
    let database = ChatDatabase::open(path).unwrap();
    assert_eq!(
        database
            .list_sessions(false, 50, None)
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert_eq!(
        database.turn(&invocation.turn_id).unwrap().state,
        "cancelled"
    );
}

#[test]
fn captured_results_keep_the_lane_reserved_until_save_or_discard_without_resending() {
    for abandon in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut prepared = launch(root.path());
        let state = root.path().join("state");
        let offline = root.path().join("offline");
        let count = root.path().join("calls");
        prepared.options.database_path = state.join("chat.sqlite");
        let path = prepared.options.database_path.clone();
        fs::write(&prepared.executable, format!("#!/bin/sh\nprintf x >> '{}'\nsleep 0.3\nprintf '%s' '{{\"status\":\"succeeded\",\"text\":\"retained answer\",\"metadata\":{{\"source\":\"fixture\"}},\"usages\":[{{\"call_id\":\"actual-call\",\"model\":\"fixture\"}}]}}'\n", count.display())).unwrap();
        let controller = ChatController::open(prepared).unwrap();
        controller
            .send(ChatCommand::Submit {
                session_id: None,
                prompt: "paid prompt".into(),
            })
            .unwrap();
        let ChatEvent::Submitted { invocation, .. } = next(&controller) else {
            panic!("expected admission")
        };
        assert!(matches!(next(&controller), ChatEvent::Observing(_)));
        fs::rename(&state, &offline).unwrap();
        fs::write(&state, "unavailable").unwrap();
        let ChatEvent::Finished { resolution, .. } = next(&controller) else {
            panic!("expected retained result")
        };
        assert!(resolution.is_err());
        assert!(controller.busy());
        assert!(matches!(
            controller.send(ChatCommand::Submit {
                session_id: None,
                prompt: "must not resend".into()
            }),
            Err(ChatRunError::Busy)
        ));
        fs::remove_file(&state).unwrap();
        fs::rename(&offline, &state).unwrap();
        controller
            .send(if abandon {
                ChatCommand::Abandon
            } else {
                ChatCommand::Save
            })
            .unwrap();
        let ChatEvent::Finished { resolution, .. } = next(&controller) else {
            panic!("expected recovery")
        };
        let resolution = resolution.unwrap();
        if abandon {
            assert!(
                matches!(resolution.state, CompletionState::Finished(turn) if turn.state == "cancelled")
            );
        } else {
            assert!(matches!(resolution.state, CompletionState::Committed(_)));
        }
        assert!(!controller.busy());
        controller.close().unwrap();
        assert_eq!(fs::read(count).unwrap(), b"x");
        let database = ChatDatabase::open(path).unwrap();
        assert_eq!(
            database
                .messages(&invocation.session_id, 50, None)
                .unwrap()
                .messages
                .len(),
            if abandon { 1 } else { 2 }
        );
        if !abandon {
            assert_eq!(
                serde_json::to_value(
                    database
                        .history(&invocation.session_id, 4096)
                        .unwrap()
                        .history[1]
                        .metadata
                        .clone()
                )
                .unwrap(),
                json!({"source":"fixture"})
            );
        }
    }
}

#[test]
fn shutdown_reports_storage_failure_and_releases_ownership() {
    let root = tempfile::tempdir().unwrap();
    let mut prepared = launch(root.path());
    let state = root.path().join("state");
    let offline = root.path().join("offline");
    prepared.options.database_path = state.join("chat.sqlite");
    let path = prepared.options.database_path.clone();
    let controller = ChatController::open(prepared).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "pending".into(),
        })
        .unwrap();
    let ChatEvent::Submitted { invocation, .. } = next(&controller) else {
        panic!("expected admission")
    };
    assert!(matches!(next(&controller), ChatEvent::Observing(_)));
    fs::rename(&state, &offline).unwrap();
    fs::write(&state, "unavailable").unwrap();
    let result = controller.close();
    fs::remove_file(&state).unwrap();
    fs::rename(&offline, &state).unwrap();
    assert!(result.is_err());
    assert_eq!(
        ChatDatabase::open(path)
            .unwrap()
            .turn(&invocation.turn_id)
            .unwrap()
            .state,
        "interrupted"
    );
}

#[test]
fn history_pages_start_at_the_tail_and_do_not_overlap_across_failed_turns() {
    let root = tempfile::tempdir().unwrap();
    let prepared = launch(root.path());
    let database = ChatDatabase::open(&prepared.options.database_path).unwrap();
    let session = database
        .create_session("default", None, Some("history".into()))
        .unwrap();
    for index in 0..60 {
        let started = database
            .begin_turn(
                &session.id,
                &format!("prompt-{index}"),
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                4096,
            )
            .unwrap();
        database
            .fail_turn(started.invocation, "failed", "fixture failure")
            .unwrap();
    }
    drop(database);
    let controller = ChatController::open(prepared).unwrap();
    controller
        .send(ChatCommand::Load {
            session_id: session.id.clone(),
            before: None,
        })
        .unwrap();
    let ChatEvent::Loaded { page, .. } = next(&controller) else {
        panic!("expected tail")
    };
    assert_eq!(page.messages.last().unwrap().message.content, "prompt-59");
    let first = page.messages[0].message.sequence;
    assert!(first > 0);
    controller
        .send(ChatCommand::Load {
            session_id: session.id,
            before: Some(first),
        })
        .unwrap();
    let ChatEvent::Loaded { page, .. } = next(&controller) else {
        panic!("expected earlier history")
    };
    assert!(!page.messages.is_empty());
    assert!(
        page.messages
            .iter()
            .all(|record| record.message.sequence < first)
    );
    controller.close().unwrap();
}

#[test]
fn event_backpressure_does_not_prevent_cancellation_or_shutdown_before_launch() {
    for shutdown in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let prepared = launch(root.path());
        let path = prepared.options.database_path.clone();
        let marker = root.path().join("started");
        fs::write(
            &prepared.executable,
            format!("#!/bin/sh\ntouch '{}'\nsleep 20\n", marker.display()),
        )
        .unwrap();
        let controller = ChatController::open(prepared).unwrap();
        for _ in 0..QUEUE_CAPACITY {
            controller
                .send(ChatCommand::ListSessions { after: None })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match controller.send(ChatCommand::Submit {
                session_id: None,
                prompt: "cancel before launch".into(),
            }) {
                Ok(()) => break,
                Err(ChatRunError::QueueFull) => {
                    assert!(!controller.busy());
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("unexpected admission: {error}"),
            }
        }
        let turn_id = loop {
            if let Some(session) = controller
                ._database
                .list_sessions(false, 50, None)
                .unwrap()
                .sessions
                .first()
                && let Some(message) = controller
                    ._database
                    .messages(&session.id, 10, None)
                    .unwrap()
                    .messages
                    .first()
            {
                break message.message.turn_id.clone();
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        assert!(!marker.exists());
        if !shutdown {
            controller.cancel();
            loop {
                match next(&controller) {
                    ChatEvent::Sessions(_) | ChatEvent::Submitted { .. } => {}
                    ChatEvent::Error {
                        admission,
                        source: ChatRunError::Cancelled,
                    } => {
                        assert!(admission);
                        break;
                    }
                    _ => panic!("cancelled submission must not launch a child"),
                }
            }
            assert!(!controller.busy());
        }
        controller.close().unwrap();
        assert!(!marker.exists());
        assert_eq!(
            ChatDatabase::open(path)
                .unwrap()
                .turn(&turn_id)
                .unwrap()
                .state,
            "cancelled"
        );
    }
}
