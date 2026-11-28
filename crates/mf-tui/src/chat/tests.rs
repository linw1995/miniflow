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

pub fn capture(db: &ChatDatabase, invocation: &Invocation) -> Capture {
    let mut capture = Capture::default();
    capture.begin(db, invocation.clone());
    capture
}

pub fn admitted(db: &ChatDatabase, session: &str, prompt: &str) -> Invocation {
    db.begin_turn(
        session,
        prompt,
        mf_chat::ModelIdentity {
            profile: "default".into(),
            model: None,
        },
        None,
    )
    .unwrap()
    .invocation
}

pub fn preview_turn() -> (tempfile::TempDir, ChatDatabase, Invocation) {
    let root = tempfile::tempdir().unwrap();
    let db = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = db.create_session("default", None, None).unwrap();
    let invocation = admitted(&db, &session.id, "question");
    (root, db, invocation)
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
fn callbacks_do_not_become_configurable_chat_arguments() {
    let manifest = chat_manifest();
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    assert!(!contract.supports_steering);
    assert_eq!(
        contract.input_targets().collect::<Vec<_>>(),
        ["start.messages"]
    );
    assert!(
        RunOptions {
            arguments: vec!["--callback:steering".into()],
            ..Default::default()
        }
        .parsed_workflow_arguments(&contract.parameters)
        .is_err()
    );
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
    assert!(!controller.supports_steering());
    assert!(matches!(
        controller.steer(&invocation, "not a new turn".into()),
        Err(ChatRunError::SteeringClosed)
    ));
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

pub(super) const HOSTED_RUNNER: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys, time
from pathlib import Path
assert sys.stdin.read() == '' and not sys.argv[1:]
grant = json.loads(os.environ['MF_RUNNER_RPC_V1'])
assert grant['io'] == {'inputs': True, 'output': True, 'diagnostics': True} and not grant['bindings']
sock = socket.socket(socket.AF_UNIX)
sock.connect(grant['socket'])
wire = sock.makefile('rb')
def send(frame): sock.sendall(json.dumps(frame).encode() + b'\n')
send({'get_inputs': {}})
messages = json.loads(wire.readline())['inputs']['values']['start.messages']
def emit(record): send({'output': record})
def finish():
    send({'flush': {}})
    assert json.loads(wire.readline()) == {'flushed': True}
"#;

#[test]
fn queued_turns_run_without_a_view_and_withdrawal_is_scoped_to_the_session() {
    let root = tempfile::tempdir().unwrap();
    let gate = root.path().join("release");
    let script = format!(
        "{HOSTED_RUNNER}\nwhile not Path({gate:?}).exists(): time.sleep(0.02)\nemit({{'append':{{'status':'succeeded','messages':[{{'role':'assistant','content':'ok'}}],'usages':[]}}}})\nfinish()"
    );
    let controller =
        ChatController::open(launch_runner(root.path(), &chat_manifest(), &script)).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "first".into(),
        })
        .unwrap();
    let ChatEvent::Submitted { session, .. } = next(&controller) else {
        panic!("missing admission")
    };
    assert!(matches!(
        controller.queue_next("other", "wrong session".into()),
        Err(ChatRunError::NextTurnClosed)
    ));
    let removed = controller
        .queue_next(&session.id, "removed".into())
        .unwrap();
    let kept = controller.queue_next(&session.id, "second".into()).unwrap();
    assert!(controller.withdraw_next("other", removed).is_none());
    assert_eq!(
        controller.withdraw_next(&session.id, removed).as_deref(),
        Some("removed")
    );
    fs::write(gate, "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while controller.busy() {
        assert!(
            Instant::now() < deadline,
            "the worker waited for the view to schedule its next turn"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(controller.withdraw_next(&session.id, kept).is_none());
    assert!(matches!(
        controller.queue_next(&session.id, "too late".into()),
        Err(ChatRunError::NextTurnClosed)
    ));
    let page = controller
        ._database
        .messages(&session.id, 20, None)
        .unwrap();
    assert_eq!(page.messages.len(), 4);
    assert_eq!(page.messages[2].message.content(), "second");
    assert_ne!(
        page.messages[0].message.turn_id,
        page.messages[2].message.turn_id
    );
    let mut order = Vec::new();
    let mut claimed = None;
    while order.len() < 5 {
        match next(&controller) {
            ChatEvent::NextTurnClaimed { id, invocation } => {
                assert_eq!(id, kept);
                claimed = Some(invocation);
                order.push("claimed");
            }
            ChatEvent::Finished { .. } => order.push("finished"),
            ChatEvent::Loaded { page, .. } => {
                assert_eq!(page.messages.len(), 2);
                order.push("history");
            }
            ChatEvent::Submitted { invocation, .. } => {
                assert_eq!(claimed.as_ref(), Some(&invocation));
                order.push("submitted");
            }
            _ => {}
        }
    }
    assert_eq!(
        order,
        ["claimed", "finished", "history", "submitted", "finished"]
    );
    controller.close().unwrap();
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
            format!("{HOSTED_RUNNER}\nwith open({count:?}, 'ab') as f: f.write(b'x')\ntime.sleep(0.3)\nemit(json.loads({result:?}))\nfinish()", result=result.to_string()),
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
    let display::Message::User {
        content: text,
        placement: display::UserPlacement::Prompt,
    } = &page.messages.last().unwrap().message
    else {
        panic!("expected a prompt");
    };
    assert_eq!(text.load().unwrap(), "prompt-59");
    let first = page.messages[0].sequence;
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
    assert!(page.messages.iter().all(|record| record.sequence < first));
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

const FLOOD_RECORDS: usize = 128;
const FLOOD_CHUNK_BYTES: usize = 2048;

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
    let script = format!(
        r#"{HOSTED_RUNNER}
assert messages[-1]['content'] == 'hello'
send({{'diagnostic': {{'level':'warning', 'message':'fixture warning'}}}})
print('process diagnostic', file=sys.stderr, flush=True)
records = json.loads({records:?})
mode = {mode:?}
if mode == 'flood':
    records[0]['answer']['text'] = 'x' * {FLOOD_CHUNK_BYTES}
    records = [records[0].copy() for _ in range({FLOOD_RECORDS})] + records[-1:]
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
for record in records[:split]: emit(record)
if mode in ('gated', 'gated_tool_call'):
    while not os.path.exists({gate:?}): time.sleep(0.01)
if mode == 'prefix':
    finish()
    sys.exit(2)
for record in records[split:]: emit(record)
if mode == 'late':
    send({{'diagnostic': {{'level':'error', 'message':'late diagnostic'}}}})
    sock.sendall(b'invalid\n')
    assert not wire.readline()
    sys.exit(13)
finish()
"#,
        records = serde_json::to_string(&records).unwrap(),
        gate = gate.to_str().unwrap()
    );
    (launch_runner(directory, &manifest, &script), gate)
}

#[test]
fn a_stdout_only_chat_runner_cannot_settle_or_offer_a_reply_to_save() {
    let root = tempfile::tempdir().unwrap();
    let script = "#!/bin/sh\nprintf '%s\\n' '{\"append\":{\"status\":\"succeeded\",\"messages\":[{\"role\":\"assistant\",\"content\":\"wrong transport\"}],\"usages\":[]}}'\n";
    let controller =
        ChatController::open(launch_runner(root.path(), &chat_manifest(), script)).unwrap();
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
            invocation,
            ..
        } = next(&controller)
        {
            assert!(
                capture_error
                    .unwrap()
                    .contains("without completing hosted I/O")
            );
            assert!(
                matches!(resolution.unwrap().state, CompletionState::Finished(turn) if turn.state == "failed")
            );
            assert!(
                controller
                    ._database
                    .call(&invocation.call_id)
                    .unwrap()
                    .terminal_result
                    .is_none()
            );
            assert!(!controller.busy());
            break;
        }
    }
    controller.close().unwrap();
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
                    assert_eq!(preview.segments[0].text.load().unwrap(), expected_fragment);
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
            if let ChatEvent::Finished {
                resolution, stderr, ..
            } = next(&controller)
            {
                assert!(
                    stderr.contains("warning: fixture warning")
                        && stderr.contains("process diagnostic"),
                    "{stderr}"
                );
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
fn hosted_recovery_keeps_the_typed_reply_when_storage_is_temporarily_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let (mut launch, gate) = live_launch(root.path(), "gated");
    let state = root.path().join("state");
    let offline = root.path().join("offline");
    launch.options.database_path = state.join("chat.sqlite");
    let runner = launch.runner.executable().to_path_buf();
    let controller = ChatController::open(launch).unwrap();
    controller
        .send(ChatCommand::Submit {
            session_id: None,
            prompt: "hello".into(),
        })
        .unwrap();
    while !matches!(next(&controller), ChatEvent::Preview { .. }) {}
    fs::rename(&state, &offline).unwrap();
    fs::write(&state, "unavailable").unwrap();
    fs::write(gate, "finish").unwrap();
    loop {
        if let ChatEvent::Finished { resolution, .. } = next(&controller) {
            assert!(resolution.is_err());
            break;
        }
    }
    fs::remove_file(runner).unwrap();
    fs::remove_file(&state).unwrap();
    fs::rename(&offline, &state).unwrap();
    controller.send(ChatCommand::Save).unwrap();
    let ChatEvent::Finished { resolution, .. } = next(&controller) else {
        panic!("expected recovery")
    };
    let CompletionState::Committed(completion) = resolution.unwrap().state else {
        panic!("expected saved reply")
    };
    assert_eq!(completion.result.final_text(), Some("canonical"));
    controller.close().unwrap();
}

#[test]
fn failed_host_sessions_only_offer_complete_replies_for_save_only() {
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
fn invalid_terminal_records_invalidate_hosted_recovery_instead_of_saving_first() {
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
                prompt: "hello".into(),
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
fn paused_rendering_delivers_the_full_preview_before_authoritative_settlement() {
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
    let mut saw_preview = false;
    loop {
        match next(&controller) {
            ChatEvent::Preview { preview, .. } => {
                assert_eq!(preview.segments.len(), 1);
                let text = preview.segments[0].text.load().unwrap();
                assert_eq!(text.len(), FLOOD_RECORDS * FLOOD_CHUNK_BYTES);
                assert!(text.bytes().all(|byte| byte == b'x'));
                saw_preview = true;
            }
            ChatEvent::Finished { resolution, .. } => {
                assert!(saw_preview);
                let CompletionState::Committed(completion) = resolution.unwrap().state else {
                    panic!("expected saved final");
                };
                assert_eq!(completion.result.final_text(), Some("canonical"));
                assert!(controller.preview.lock().unwrap().segments.is_empty());
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

fn model_selectable_manifest() -> mf_runtime::WorkflowManifest {
    let mut manifest = chat_manifest();
    manifest.definition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[
            {"id":"params","kind":"builtin.identity","config":{}},
            {"id":"start","kind":"builtin.llm","config":{"model":"fixture-model"}}
        ],
        "outputs":[
            {"name":"append","node":"start","port":"append"}
        ]
    }))
    .unwrap();
    manifest.descriptor.input_schema = serde_json::from_value(json!({
        "inputs": {
            "start": {
                "messages": {
                    "type": mf_runtime::ValueType::List(Box::new(mf_runtime::ValueType::Object)),
                    "required": true
                }
            },
            "params": {
                "model": {
                    "type": "string",
                    "required": true,
                    "binding": {"default": "fallback-model", "secret": false}
                },
                "base_url": {
                    "type": "string",
                    "required": true,
                    "binding": {"default": "", "secret": false}
                },
                "api_key": {
                    "type": "string",
                    "required": false,
                    "binding": {"default": "", "secret": true}
                }
            }
        },
        "stdin": {}
    }))
    .unwrap();
    manifest.descriptor.description = mf_runtime::describe_graph(&mf_runtime::CompiledGraph {
        definition: manifest.definition.clone(),
        execution_order: vec!["params".into(), "start".into()],
        relations: Default::default(),
        children: Default::default(),
    })
    .unwrap();
    manifest
}

struct MockModelsServer {
    addr: std::net::SocketAddr,
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockModelsServer {
    fn start(replies: Vec<(u16, String)>) -> Self {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            addr,
            base_url: format!("http://{addr}/v1"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockModelsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn controller_discovers_first_model_lists_switches_and_reuses_preferences() {
    let body = json!({
        "data": [
            {"id": "first-model", "context_window": 131_072},
            {"id": "second-model", "context_window": 65_536}
        ]
    })
    .to_string();
    let unlisted_body = json!({
        "data": [
            {"id": "unlisted-model", "context_window": 262_144}
        ]
    })
    .to_string();
    let server = MockModelsServer::start(vec![
        (200, body.clone()),
        (200, body.clone()),
        (200, body),
        (200, unlisted_body),
    ]);
    let root = tempfile::tempdir().unwrap();
    let manifest = model_selectable_manifest();
    let mut first_launch = launch_runner(root.path(), &manifest, "#!/bin/sh\nexit 0\n");
    first_launch.parameters.0.insert(
        "params.base_url".into(),
        mf_runtime::ValueRef::from(server.base_url.as_str()),
    );
    first_launch.parameters.0.insert(
        "params.api_key".into(),
        mf_runtime::ValueRef::from("sk-test"),
    );

    let controller = ChatController::open(first_launch).unwrap();
    assert!(controller.supports_model_selection());
    assert!(controller.loading_default_model());
    assert_eq!(controller.initial_model(), None);

    match next(&controller) {
        ChatEvent::ModelSelected {
            model,
            context_window,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("first-model"));
            assert_eq!(context_window, Some(131_072));
            assert!(automatic);
            assert!(warning.is_none());
        }
        other => panic!("unexpected event: {}", std::any::type_name_of_val(&other)),
    }

    // List models and switch to second-model.
    controller.send(ChatCommand::ListModels).unwrap();
    match next(&controller) {
        ChatEvent::Models(Ok(models)) => {
            assert_eq!(models.len(), 2);
            assert_eq!(models[0].id, "first-model");
            assert_eq!(models[1].id, "second-model");
        }
        _ => panic!("expected models listing"),
    }

    controller
        .send(ChatCommand::SelectModel {
            model: "second-model".into(),
        })
        .unwrap();
    match next(&controller) {
        ChatEvent::ModelSelected {
            model,
            context_window,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("second-model"));
            assert_eq!(context_window, Some(65_536));
            assert!(!automatic);
            assert!(warning.is_none());
        }
        _ => panic!("expected model selection event"),
    }

    // Reject blank model names cleanly.
    controller
        .send(ChatCommand::SelectModel {
            model: "   ".into(),
        })
        .unwrap();
    assert!(matches!(next(&controller), ChatEvent::Error { .. }));
    controller.close().unwrap();

    // Reopen without explicit parameters: saved base_url and second-model are reused,
    // and startup discovers second-model's context window via the llm.model_info node.
    let second_launch = launch_runner(root.path(), &manifest, "#!/bin/sh\nexit 0\n");
    let reopened = ChatController::open(second_launch).unwrap();
    assert!(reopened.supports_model_selection());
    assert!(!reopened.loading_default_model());
    assert_eq!(reopened.initial_model(), Some("second-model"));
    match next(&reopened) {
        ChatEvent::ContextWindow {
            model,
            context_window,
        } => {
            assert_eq!(model, "second-model");
            assert_eq!(context_window, Some(65_536));
        }
        other => panic!("unexpected event: {}", std::any::type_name_of_val(&other)),
    }

    // Selecting an uncached model emits ModelSelected immediately and probes its window.
    reopened
        .send(ChatCommand::SelectModel {
            model: "unlisted-model".into(),
        })
        .unwrap();
    match next(&reopened) {
        ChatEvent::ModelSelected {
            model,
            context_window,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("unlisted-model"));
            assert_eq!(context_window, None);
            assert!(!automatic);
            assert!(warning.is_none());
        }
        other => panic!("unexpected event: {}", std::any::type_name_of_val(&other)),
    }
    match next(&reopened) {
        ChatEvent::ContextWindow {
            model,
            context_window,
        } => {
            assert_eq!(model, "unlisted-model");
            assert_eq!(context_window, Some(262_144));
        }
        other => panic!("unexpected event: {}", std::any::type_name_of_val(&other)),
    }
    reopened.close().unwrap();
}

#[test]
fn controller_model_selection_handles_missing_ports_missing_base_url_and_provider_error() {
    // 1. Workflow without provider parameter ports.
    let root = tempfile::tempdir().unwrap();
    let plain = ChatController::open(launch(root.path())).unwrap();
    assert!(!plain.supports_model_selection());
    plain.send(ChatCommand::ListModels).unwrap();
    assert!(matches!(next(&plain), ChatEvent::Models(Err(_))));
    plain
        .send(ChatCommand::SelectModel {
            model: "any".into(),
        })
        .unwrap();
    assert!(matches!(next(&plain), ChatEvent::Error { .. }));
    plain.close().unwrap();

    // 2. Model-selectable workflow with empty base_url falls back to schema default.
    let empty_root = tempfile::tempdir().unwrap();
    let manifest = model_selectable_manifest();
    let empty_launch = launch_runner(empty_root.path(), &manifest, "#!/bin/sh\nexit 0\n");
    let empty_controller = ChatController::open(empty_launch).unwrap();
    match next(&empty_controller) {
        ChatEvent::ModelSelected {
            model,
            context_window,
            automatic,
            warning,
        } => {
            assert_eq!(model.as_deref(), Some("fallback-model"));
            assert_eq!(context_window, None);
            assert!(automatic);
            assert!(warning.is_none());
        }
        _ => panic!("expected automatic fallback model event"),
    }
    empty_controller.send(ChatCommand::ListModels).unwrap();
    match next(&empty_controller) {
        ChatEvent::Models(Err(message)) => {
            assert!(message.contains("base_url"), "{message}");
        }
        _ => panic!("expected missing base_url error"),
    }
    empty_controller.close().unwrap();

    // 3. Provider HTTP failure falls back cleanly and surfaces errors on ListModels.
    let server = MockModelsServer::start(vec![
        (500, json!({"error": {"message": "boom"}}).to_string()),
        (500, json!({"error": {"message": "boom"}}).to_string()),
    ]);
    let error_root = tempfile::tempdir().unwrap();
    let mut err_launch = launch_runner(error_root.path(), &manifest, "#!/bin/sh\nexit 0\n");
    err_launch.parameters.0.insert(
        "params.base_url".into(),
        mf_runtime::ValueRef::from(server.base_url.as_str()),
    );
    let err_controller = ChatController::open(err_launch).unwrap();
    match next(&err_controller) {
        ChatEvent::ModelSelected {
            model, automatic, ..
        } => {
            assert_eq!(model.as_deref(), Some("fallback-model"));
            assert!(automatic);
        }
        _ => panic!("expected fallback model selection"),
    }
    err_controller.send(ChatCommand::ListModels).unwrap();
    assert!(matches!(next(&err_controller), ChatEvent::Models(Err(_))));
    err_controller.close().unwrap();
}
