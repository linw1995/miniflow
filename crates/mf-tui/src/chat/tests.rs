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
    assert_eq!(session.title_source, "provisional");
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

fn live_launch(directory: &Path, mode: &str) -> (ChatLaunch, std::path::PathBuf) {
    let mut launch = launch(directory);
    launch.manifest.version = mf_runtime::WorkflowManifestVersion::V2026_10_23;
    let mut definition = serde_json::to_value(&launch.manifest.definition).unwrap();
    definition["version"] = json!("2026-10-23");
    definition["stream_outputs"] = json!([{"name":"answer","path":["start"],"port":"delta"}]);
    launch.manifest.definition = serde_json::from_value(definition).unwrap();
    launch.manifest.stream_outputs = vec![mf_runtime::StreamOutputContract {
        name: "answer".into(),
        path: vec!["start".into()],
        port: "delta".into(),
        value_type: mf_runtime::ValueType::Object,
        required: true,
    }];
    launch.contract = ChatContract::from_manifest(&launch.manifest).unwrap();
    let gate = directory.join("release");
    let event = |kind| json!({"export":"answer","source":["start"],"invocation":{"id":1,"node":"start","path":[]},"event":kind});
    let kinds = [
        json!({"kind":"event","event":event(json!({"kind":"open"}))}),
        json!({"kind":"event","event":event(json!({"kind":"value","value":{"text":"provisional","model_call_id":"model-call"}}))}),
        json!({"kind":"event","event":event(json!({"kind":"close","completed":true}))}),
        json!({"kind":"final","outputs":{"status":"succeeded","text":"canonical","usages":[]}}),
    ];
    let records: Vec<_> = kinds.into_iter().enumerate().map(|(sequence, record)| json!({"version":1,"run_id":"RUN_ID","sequence":sequence,"record":record})).collect();
    let script = format!(
        r#"#!/usr/bin/env python3
import json, os, sys, time
records = json.loads({records:?})
mode = {mode:?}
if mode == 'flood':
    records = records[:2] + [records[1].copy() for _ in range(20000)] + records[2:]
if mode == 'empty':
    records = records[-1:]
    records[0]['sequence'] = 0
split = 3 if mode == 'gated' else (0 if mode == 'empty' else 2)
for record in records[:split]:
    record['run_id'] = os.environ['MF_RUN_ID']
    print(json.dumps(record), flush=True)
if mode == 'gated':
    while not os.path.exists({gate:?}):
        time.sleep(0.01)
if mode == 'prefix':
    sys.exit(2)
for sequence, record in enumerate(records[split:], start=split):
    record['sequence'] = sequence
    record['run_id'] = os.environ['MF_RUN_ID']
    print(json.dumps(record), flush=True)
if mode == 'late':
    print('late diagnostic', flush=True)
    sys.exit(13)
"#,
        records = serde_json::to_string(&records).unwrap(),
        gate = gate.to_str().unwrap()
    );
    fs::write(&launch.executable, script).unwrap();
    (launch, gate)
}

#[test]
fn executable_live_previews_arrive_before_authoritative_settlement() {
    let root = tempfile::tempdir().unwrap();
    let (launch, gate) = live_launch(root.path(), "gated");
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "hello".into(),
        })
        .unwrap();
    let invocation = loop {
        if let ChatEvent::Preview {
            invocation,
            preview,
        } = next(&controller)
            && preview
                .segments
                .iter()
                .any(|segment| !segment.text.is_empty())
        {
            break invocation;
        }
    };
    assert!(controller.busy());
    assert_eq!(
        controller
            ._database
            .messages(&invocation.session_id, 50, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    fs::write(gate, "release").unwrap();
    loop {
        if let ChatEvent::Finished { resolution, .. } = next(&controller) {
            let CompletionState::Committed(completion) = resolution.unwrap().state else {
                panic!("expected saved final")
            };
            assert_eq!(completion.result.text, "canonical");
            break;
        }
    }
    controller.close().unwrap();
}

#[test]
fn valid_live_final_survives_late_diagnostics_and_prefix_death_never_saves_text() {
    for mode in ["late", "prefix", "empty"].into_iter().cycle().take(24) {
        let root = tempfile::tempdir().unwrap();
        let (launch, _) = live_launch(root.path(), mode);
        let controller = ChatController::open(launch).unwrap();
        controller
            .send(ChatCommand::Submit {
                session_id: None,
                prompt: "hello".into(),
            })
            .unwrap();
        loop {
            let event = next(&controller);
            if let ChatEvent::Error { source, .. } = &event {
                panic!("{mode} failed before settlement: {source}");
            }
            if let ChatEvent::Finished {
                resolution,
                capture_error,
                ..
            } = event
            {
                let resolution = resolution.unwrap();
                if mode == "prefix" {
                    let mut typed_capture = false;
                    for diagnostic in &resolution.diagnostics {
                        let mut source: Option<&(dyn std::error::Error + 'static)> =
                            Some(diagnostic);
                        while let Some(error) = source {
                            typed_capture |= error.is::<Arc<crate::run::CaptureFailure>>();
                            source = error.source();
                        }
                    }
                    assert!(
                        typed_capture,
                        "capture failures must retain their typed source"
                    );
                }
                match resolution.state {
                    CompletionState::Committed(completion) => {
                        assert!(matches!(mode, "late" | "empty"));
                        assert_eq!(completion.result.text, "canonical");
                        assert_eq!(capture_error.is_some(), mode == "late");
                    }
                    CompletionState::Finished(turn) => {
                        assert_eq!(mode, "prefix");
                        assert_eq!(turn.state, "failed");
                    }
                    _ => panic!("unexpected settlement"),
                }
                break;
            }
        }
        controller.close().unwrap();
    }
}

#[test]
fn incompatible_public_answer_contract_is_rejected_without_launching() {
    let root = tempfile::tempdir().unwrap();
    let (mut launch, _) = live_launch(root.path(), "prefix");
    launch.manifest.stream_outputs[0].value_type = mf_runtime::ValueType::String;
    assert!(ChatContract::from_manifest(&launch.manifest).is_err());
    launch.manifest.version = mf_runtime::WorkflowManifestVersion::V2026_10_16;
    assert!(ChatContract::from_manifest(&launch.manifest).is_err());
}

#[test]
fn paused_rendering_keeps_capture_draining_with_bounded_previews() {
    let root = tempfile::tempdir().unwrap();
    let (launch, _) = live_launch(root.path(), "flood");
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "hello".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while controller.busy() {
        assert!(
            Instant::now() < deadline,
            "capture stalled behind paused rendering"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let preview = controller.preview.lock().unwrap().clone();
    assert_eq!(preview.segments.len(), 1);
    assert_eq!(
        preview.segments[0].text.len(),
        super::live::MAX_PREVIEW_BYTES
    );
    assert!(preview.omitted);
    let mut saw_preview = false;
    loop {
        match next(&controller) {
            ChatEvent::Preview { .. } => saw_preview = true,
            ChatEvent::Finished { resolution, .. } => {
                assert!(saw_preview);
                assert!(matches!(
                    resolution.unwrap().state,
                    CompletionState::Committed(_)
                ));
                break;
            }
            _ => {}
        }
    }
    controller.close().unwrap();
}

#[test]
fn closing_a_live_controller_reaps_a_child_without_rendering() {
    let root = tempfile::tempdir().unwrap();
    let (launch, _) = live_launch(root.path(), "gated");
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "hello".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller
        .preview
        .lock()
        .unwrap()
        .segments
        .iter()
        .all(|segment| segment.text.is_empty())
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    controller.close().unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
}
