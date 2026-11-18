use super::*;
use mf_runtime::OutputValue;
use serde_json::json;
use std::{fs, time::Instant};

fn chat_manifest() -> mf_runtime::WorkflowManifest {
    let mut manifest = crate::test_support::sample_manifest();
    manifest.definition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[{"id":"start","kind":"builtin.llm","config":{"model":"fixture-model"}}],
        "outputs":[
            {"name":"append","node":"start","port":"append"}
        ]
    }))
    .unwrap();
    manifest.descriptor.description.execution_order = vec!["start".into()];
    manifest.descriptor.input_schema = serde_json::from_value(json!({"inputs":{"start":{
        "messages":{"type":mf_runtime::ValueType::List(Box::new(mf_runtime::ValueType::Object)),"required":true}
    }},"stdin":{}})).unwrap();
    manifest.descriptor.description = mf_runtime::describe_graph(&mf_runtime::CompiledGraph {
        definition: manifest.definition.clone(),
        execution_order: vec!["start".into()],
        relations: Default::default(),
        children: Default::default(),
    })
    .unwrap();
    manifest
}

fn live_manifest() -> mf_runtime::WorkflowManifest {
    let mut manifest = chat_manifest();
    manifest.descriptor.description.execution =
        Some(mf_telemetry::description::ExecutionDescription {
            mode: mf_telemetry::description::ExecutionMode::Stream,
        });
    manifest.definition.outputs.push(
        serde_json::from_value(json!({"name":"answer","node":"start","port":"delta"})).unwrap(),
    );
    manifest
}

fn chat_options(directory: &Path) -> ChatOptions {
    ChatOptions {
        database_path: directory.join("chat.sqlite"),
        profile: "default".into(),
        run: Default::default(),
    }
}

fn launch_runner(
    directory: &Path,
    manifest: &mf_runtime::WorkflowManifest,
    script: &str,
) -> ChatLaunch {
    let contract = ChatContract::from_manifest(manifest).unwrap();
    let executable = directory.join("worker");
    crate::test_support::write_runner(&executable, manifest, script);
    ChatLaunch {
        runner: PreparedRunner::load(&executable, mf_chat::contract::MAX_RECOVERY_BYTES as u64)
            .unwrap(),
        contract,
        parameters: Default::default(),
        options: chat_options(directory),
    }
}

pub fn launch(directory: &Path) -> ChatLaunch {
    launch_runner(
        directory,
        &chat_manifest(),
        "#!/bin/sh\ntrap '' INT\nsleep 20\n",
    )
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
    let defaults = chat_options(root.path());
    let binary = root.path().join("manifest");
    fs::write(
        &binary,
        crate::test_support::elf(Some(&chat_manifest().to_bytes().unwrap()), 1),
    )
    .unwrap();
    let input = root.path().join("inputs.json");
    assert!(ChatLaunch::prepare(&binary, defaults.clone()).is_ok());
    let mut options = defaults.clone();
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
        let mut options = defaults.clone();
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
        let mut options = defaults.clone();
        options.run = run;
        assert!(ChatLaunch::prepare(&binary, options).is_err());
    }
    assert!(!defaults.database_path.exists());
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
        let result = json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"retained answer","metadata":{"source":"fixture"}}],"usages":[{"call_id":"actual-call","model":"fixture"}]}});
        fs::write(
            prepared.runner.executable().with_extension("script"),
            format!(
                "#!/bin/sh\nprintf x >> '{}'\nsleep 0.3\nprintf '%s' '{}'\n",
                count.display(),
                result
            ),
        )
        .unwrap();
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
            let history = database.model_context(&invocation.session_id).unwrap();
            let message = history.messages[1].clone().encode().unwrap();
            assert_eq!(message["metadata"], json!({"source":"fixture"}));
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
    assert_eq!(page.messages.last().unwrap().message.content(), "prompt-59");
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
            prepared.runner.executable().with_extension("script"),
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
    let mut manifest = live_manifest();
    let gate = directory.join("release");
    let prefix = if mode == "gated_tool_call" {
        let output = manifest.definition.outputs.last_mut().unwrap();
        output.name = "tool_call_delta".into();
        output.port = "tool_call_delta".into();
        json!({"tool_call_delta":{"arguments":"{\"query\":","model_call_id":"model-call","index":0}})
    } else {
        json!({"answer":{"text":"provisional","model_call_id":"model-call"}})
    };
    let records = json!([
        prefix,
        {"append":{"status":"succeeded","messages":[{"role":"assistant","content":"canonical"}],"usages":[]}}
    ]);
    // Exceed preview and queue limits without turning delivery checks into a throughput benchmark.
    let flood_chunk_bytes = 2048;
    let flood_records = (super::preview::MAX_PREVIEW_BYTES * 2).div_ceil(flood_chunk_bytes);
    let script = format!(
        r#"#!/usr/bin/env python3
import json, os, sys, time
records = json.loads({records:?})
mode = {mode:?}
if mode == 'flood':
    records[0]['answer']['text'] = 'x' * {flood_chunk_bytes}
    records = [records[0].copy() for _ in range({flood_records})] + records[-1:]
if mode == 'empty':
    records = records[-1:]
replacement = {{'replace_context': {{'status': 'succeeded', 'context_messages': [], 'display_messages': [{{'role': 'assistant', 'content': 'Other answer'}}], 'usages': []}}}}
if mode == 'duplicate': records += records[-1:]
if mode == 'malformed_delta': records.append({{'answer': None, **records[-1]}})
if mode == 'undeclared': records.append(replacement)
if mode == 'both': records[-1].update(replacement)
if mode == 'null': records.append({{'append': None}})
if mode == 'malformed': records.append({{'append': {{'status': 'succeeded', 'messages': None, 'usages': []}}}})
split = 0 if mode == 'empty' else 1
for record in records[:split]:
    print(json.dumps(record), flush=True)
if mode in ('gated', 'gated_tool_call'):
    while not os.path.exists({gate:?}): time.sleep(0.01)
if mode == 'prefix': sys.exit(2)
for record in records[split:]: print(json.dumps(record), flush=True)
if mode == 'late':
    print('late diagnostic', flush=True)
    sys.exit(13)
"#,
        records = serde_json::to_string(&records).unwrap(),
        gate = gate.to_str().unwrap()
    );
    (launch_runner(directory, &manifest, &script), gate)
}

#[test]
fn executable_live_previews_arrive_before_authoritative_settlement() {
    for (mode, expected_fragment) in [("gated", "provisional"), ("gated_tool_call", "{\"query\":")]
    {
        let root = tempfile::tempdir().unwrap();
        let (launch, gate) = live_launch(root.path(), mode);
        let controller = ChatController::open(launch).unwrap();
        controller
            .send(ChatCommand::Submit {
                session_id: None,
                prompt: "hello".into(),
            })
            .unwrap();
        let invocation = loop {
            match next(&controller) {
                ChatEvent::Preview {
                    invocation,
                    preview,
                } => {
                    assert_eq!(preview.segments.len(), 1);
                    assert_eq!(preview.segments[0].text, expected_fragment);
                    assert_eq!(
                        preview.segments[0].tool.is_some(),
                        mode == "gated_tool_call"
                    );
                    break invocation;
                }
                ChatEvent::Finished { .. } => panic!("settled before preview delivery"),
                _ => {}
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
                assert_eq!(completion.result.final_text(), Some("canonical"));
                break;
            }
        }
        controller.close().unwrap();
    }
}

#[test]
fn native_failures_keep_complete_replies_for_save_only_and_never_save_prefixes() {
    for mode in ["late", "prefix", "empty"] {
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
            if let ChatEvent::Finished {
                resolution,
                capture_error,
                ..
            } = next(&controller)
            {
                let resolution = resolution.unwrap();
                match resolution.state {
                    CompletionState::Uncommitted(pending) => {
                        assert_eq!(mode, "late");
                        assert_eq!(pending.completion.result.final_text(), Some("canonical"));
                        assert!(capture_error.is_some());
                        controller.send(ChatCommand::Save).unwrap();
                        loop {
                            if let ChatEvent::Finished { resolution, .. } = next(&controller) {
                                assert!(matches!(
                                    resolution.unwrap().state,
                                    CompletionState::Committed(_)
                                ));
                                break;
                            }
                        }
                    }
                    CompletionState::Committed(completion) => {
                        assert_eq!(mode, "empty");
                        assert_eq!(completion.result.final_text(), Some("canonical"));
                    }
                    CompletionState::Finished(turn) => {
                        assert_eq!(mode, "prefix");
                        assert_eq!(turn.state, "failed");
                        let typed = resolution.diagnostics.iter().any(|diagnostic| {
                            let mut cause: Option<&(dyn std::error::Error + 'static)> =
                                Some(diagnostic);
                            while let Some(error) = cause {
                                if matches!(
                                    error.downcast_ref::<mf_chat::ChatError>(),
                                    Some(mf_chat::ChatError::Capture { .. })
                                ) {
                                    return true;
                                }
                                cause = error.source();
                            }
                            false
                        });
                        assert!(
                            typed,
                            "missing terminal replies must keep their typed application error"
                        );
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
fn invalid_terminal_records_invalidate_native_recovery_instead_of_saving_first() {
    for mode in [
        "duplicate",
        "undeclared",
        "both",
        "null",
        "malformed",
        "malformed_delta",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (launch, _) = live_launch(root.path(), mode);
        let controller = ChatController::open(launch).unwrap();
        controller
            .send(ChatCommand::Submit {
                session_id: None,
                prompt: "Question".into(),
            })
            .unwrap();
        let mut invocation = None;
        loop {
            match next(&controller) {
                ChatEvent::Submitted {
                    invocation: started,
                    ..
                } => invocation = Some(started),
                ChatEvent::Finished { resolution, .. } => {
                    let resolution = resolution.unwrap();
                    assert!(
                        matches!(resolution.state, CompletionState::Finished(ref turn) if turn.state == "failed"),
                        "{mode}: {:?}",
                        resolution.state
                    );
                    assert!(!resolution.diagnostics.is_empty());
                    break;
                }
                _ => {}
            }
        }
        let invocation = invocation.unwrap();
        assert!(
            !controller.busy(),
            "{mode}: ambiguous results cannot be retained"
        );
        assert_eq!(
            controller
                ._database
                .messages(&invocation.session_id, 50, None)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert_eq!(
            controller
                ._database
                .model_context(&invocation.session_id)
                .unwrap()
                .revision,
            0
        );
        assert!(
            controller
                ._database
                .call(&invocation.call_id)
                .unwrap()
                .terminal_result
                .is_none()
        );
        controller.close().unwrap();
    }
}

#[test]
fn preview_selections_require_native_streaming_execution() {
    for name in ["answer", "tool_call_delta"] {
        let mut manifest = live_manifest();
        manifest.descriptor.description.execution = None;
        manifest.definition.outputs.last_mut().unwrap().name = name.into();
        assert!(ChatContract::from_manifest(&manifest).is_err());
    }
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
        preview.segments[0].text.len() + preview.segments[0].model_call_id.len(),
        super::preview::MAX_PREVIEW_BYTES
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
