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
            max_active: MAX_ACTIVE_CHAT_RUNS,
            run: Default::default(),
        },
    }
}
fn next(controller: &ChatController) -> ChatEvent {
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
fn joined_controller_keeps_database_ownership_until_controller_drop() {
    let directory = tempfile::tempdir().unwrap();
    let launch = launch(directory.path());
    let path = launch.options.database_path.clone();
    let mut controller = ChatController::open(launch).unwrap();
    controller.join_workers().unwrap();
    assert!(matches!(
        ChatDatabase::open(&path),
        Err(mf_chat::ChatError::Busy { .. })
    ));
    drop(controller);
    assert!(ChatDatabase::open(&path).is_ok());
}

#[test]
fn controller_admissions_keep_sessions_independent_and_shutdown_reaps_children() {
    let directory = tempfile::tempdir().unwrap();
    let controller = ChatController::open(launch(directory.path())).unwrap();
    assert!(matches!(next(&controller), ChatEvent::Sessions { .. }));
    let mut ids = vec![];
    for _ in 0..5 {
        controller
            .send(ChatCommand::Create {
                system: None,
                title: None,
            })
            .unwrap();
        let ChatEvent::Created(session) = next(&controller) else {
            panic!("expected session")
        };
        ids.push(session.id);
    }
    let mut count = 0;
    for id in &ids[..4] {
        controller
            .send(ChatCommand::Submit {
                session_id: id.clone(),
                prompt: "one".into(),
            })
            .unwrap();
        loop {
            match next(&controller) {
                ChatEvent::Submitted { .. } => {
                    count += 1;
                    break;
                }
                ChatEvent::Observing { .. } => {}
                _ => panic!("expected admission"),
            }
        }
    }
    controller
        .send(ChatCommand::Submit {
            session_id: ids[0].clone(),
            prompt: "duplicate".into(),
        })
        .unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: ids[4].clone(),
            prompt: "overflow".into(),
        })
        .unwrap();
    controller
        .send(ChatCommand::Load {
            session_id: ids[4].clone(),
            after: None,
        })
        .unwrap();
    let mut busy = false;
    let mut capacity = false;
    loop {
        match next(&controller) {
            ChatEvent::Error {
                source: ChatRunError::SessionBusy { .. },
                ..
            } => busy = true,
            ChatEvent::Error {
                source: ChatRunError::Capacity { .. },
                ..
            } => capacity = true,
            ChatEvent::Loaded { session, .. } => {
                assert_eq!(session.id, ids[4]);
                break;
            }
            ChatEvent::Observing { .. } => {}
            _ => panic!("unexpected admission result"),
        }
    }
    assert!(busy && capacity && count == 4);
    controller
        .send(ChatCommand::Retry {
            session_id: ids[4].clone(),
            turn_id: "missing-turn".into(),
        })
        .unwrap();
    loop {
        match next(&controller) {
            ChatEvent::Error {
                admission,
                session_id,
                ..
            } => {
                assert!(admission);
                assert_eq!(session_id.as_deref(), Some(ids[4].as_str()));
                break;
            }
            ChatEvent::Observing { .. } => {}
            _ => panic!("expected a tagged retry error"),
        }
    }
    controller.close().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    assert_eq!(
        database
            .list_sessions(false, 50, None)
            .unwrap()
            .sessions
            .len(),
        5
    );
}

#[test]
fn reconciliation_failure_retains_model_outputs_until_explicit_save_or_shutdown() {
    use std::os::unix::process::ExitStatusExt;
    let directory = tempfile::tempdir().unwrap();
    let launch = Arc::new(launch(directory.path()));
    let database = Arc::new(ChatDatabase::open(&launch.options.database_path).unwrap());
    let (_, input) = mpsc::sync_channel(1);
    let (output, _) = mpsc::sync_channel(1);
    let mut controller = Controller::new(
        Arc::clone(&launch),
        Arc::clone(&database),
        input,
        output,
        Arc::new(AtomicBool::new(false)),
    );
    let mut invocations = vec![];
    for _ in 0..2 {
        let session = database.create_session("default", None, None).unwrap();
        let invocation = database
            .begin_turn(
                &session.id,
                "paid prompt",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                1024,
            )
            .unwrap()
            .invocation;
        let source = database.session("missing-query-result").unwrap_err();
        controller.worker_event(WorkerEvent::Finished {
            invocation: invocation.clone(),
            result: Box::new(Ok(WorkerCompletion {
                resolution: Err(source),
                outcome: LocalOutcome {
                    status: ExitStatus::from_raw(256),
                    interrupted: false,
                    stdout: serde_json::to_vec(
                        &json!({"status":"succeeded", "text":"paid reply", "usages":[{
                            "call_id":invocation.call_id, "model":"fixture",
                            "started_at":1
                        }]}),
                    )
                    .unwrap(),
                    stderr: "synthetic database query failure".into(),
                    capture_error: None,
                },
            })),
        });
        invocations.push(invocation);
    }
    assert_eq!(controller.retained.len(), 2);
    controller.command(ChatCommand::Submit {
        session_id: invocations[0].session_id.clone(),
        prompt: "must not resend".into(),
    });
    assert!(matches!(
        controller.outbox.back(),
        Some(ChatEvent::Error {
            source: ChatRunError::SessionBusy { .. },
            ..
        })
    ));
    controller.command(ChatCommand::Save {
        call_id: invocations[0].call_id.clone(),
    });
    assert!(matches!(
        controller.outbox.back(),
        Some(ChatEvent::Finished {
            resolution: ReconciledCompletion {
                state: CompletionState::Committed(_),
                ..
            },
            ..
        })
    ));
    assert_eq!(
        database.turn(&invocations[0].turn_id).unwrap().state,
        "succeeded"
    );
    assert!(controller.shutdown().is_empty());
    assert_eq!(
        database.turn(&invocations[1].turn_id).unwrap().state,
        "cancelled"
    );
    let statistics = database.usage(Default::default()).unwrap();
    assert_eq!(statistics.overall.attempted_calls, 1);
    assert_eq!(statistics.unreported_invocations, 1);
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
    for invalid in [0, MAX_ACTIVE_CHAT_RUNS + 1] {
        let mut options = prepared.options.clone();
        options.max_active = invalid;
        assert!(ChatLaunch::prepare(&binary, options).is_err());
    }
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
fn shutdown_reports_storage_failure_and_releases_database_ownership() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    let storage = root.path().join("state");
    let moved = root.path().join("offline");
    launch.options.database_path = storage.join("chat.sqlite");
    let database_path = launch.options.database_path.clone();
    let controller = ChatController::open(launch).unwrap();
    assert!(matches!(next(&controller), ChatEvent::Sessions { .. }));
    controller
        .send(ChatCommand::Create {
            system: None,
            title: None,
        })
        .unwrap();
    let ChatEvent::Created(session) = next(&controller) else {
        panic!("expected session")
    };
    controller
        .send(ChatCommand::Submit {
            session_id: session.id.clone(),
            prompt: "pending".into(),
        })
        .unwrap();
    loop {
        match next(&controller) {
            ChatEvent::Observing { .. } => break,
            ChatEvent::Submitted { .. } => {}
            _ => panic!("expected active child"),
        }
    }
    fs::rename(&storage, &moved).unwrap();
    fs::write(&storage, "database temporarily inaccessible").unwrap();
    let result = controller.close();
    fs::remove_file(&storage).unwrap();
    fs::rename(&moved, &storage).unwrap();
    assert!(matches!(result, Err(ChatRunError::Shutdown { errors }) if !errors.is_empty()));
    let database = ChatDatabase::open(database_path).unwrap();
    let messages = database.messages(&session.id, 10, None).unwrap();
    assert_eq!(messages.messages[0].turn_state, "interrupted");
}
