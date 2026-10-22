use diesel::{Connection, connection::SimpleConnection};
use mf_chat::contract::{CompletionState, ConversationExit, MAX_RECOVERY_BYTES, TurnExit};
use mf_chat::{ChatDatabase, ConversationResult, ModelProfile};
use mf_runtime::NodeValue;
use serde_json::json;

fn begin(database: &ChatDatabase) -> mf_chat::Invocation {
    let session = database.create_session("default", None, None).unwrap();
    let profile = ModelProfile::new("default", json!({"model":"fixture-model"})).unwrap();
    let invocation = mf_chat::Invocation::new(&session.id);
    let begun = database
        .begin_turn_with_invocation(
            invocation.clone(),
            "prompt",
            profile.identity().clone(),
            None,
            1024,
        )
        .unwrap();
    assert_eq!(begun.invocation, invocation);
    invocation
}
fn usages() -> Vec<mfn_llm::CallUsage> {
    vec![mfn_llm::CallUsage {
        attribution: None,
        started_at: Some(86_399_999),
        call_id: "provider-call".into(),
        node: Some(mfn_llm::UsageNode {
            id: "model".into(),
            kind: "builtin.llm".into(),
            path: Vec::new(),
        }),
        model: "fixture-model".into(),
        usage: None,
        error: None,
    }]
}
fn metadata() -> mfn_llm::MessageMetadata {
    std::collections::BTreeMap::from([(
        "citations".into(),
        json!([{"uri":"https://example.com/reference"}]).into(),
    )])
}
fn reply() -> ConversationResult {
    ConversationResult {
        usages: usages(),
        metadata: Some(metadata()),
        text: "paid reply".into(),
        finish_reason: None,
        ..Default::default()
    }
}
fn stdout(text: &str) -> Vec<u8> {
    serde_json::to_vec(
        &mfn_llm::LlmOutputs {
            status: "succeeded".into(),
            usages: usages(),
            error: None,
            metadata: Some(metadata()),
            text: text.into(),
            history: vec![],
            usage: None,
            finish_reason: None,
        }
        .into_values()
        .unwrap(),
    )
    .unwrap()
}
fn failure(database: &ChatDatabase, enabled: bool) {
    let mut connection =
        diesel::SqliteConnection::establish(database.path().to_str().unwrap()).unwrap();
    connection.batch_execute(if enabled {"CREATE TRIGGER fail_reply BEFORE INSERT ON messages WHEN NEW.role='assistant' BEGIN SELECT RAISE(ABORT, 'synthetic save failure'); END;"} else {"DROP TRIGGER fail_reply;"}).unwrap();
}

#[test]
fn parent_save_retry_retains_the_original_reply_without_starting_another_call() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let bytes = stdout("paid reply");
    failure(&database, true);
    assert!(database.complete_turn(invocation.clone(), reply()).is_err());
    let resolution = database
        .reconcile_completion(ConversationExit {
            invocation: &invocation,
            stdout: Some(&bytes),
            outcome: TurnExit::Failed,
            summary: "save failed",
        })
        .unwrap();
    let CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected retained model reply")
    };
    assert_eq!(pending.completion.invocation, invocation);
    assert!(matches!(
        database.save_recovered(pending.clone()),
        Err(mf_chat::ChatError::Uncommitted { .. })
    ));
    failure(&database, false);
    let saved = database.save_recovered(pending.clone()).unwrap();
    assert_eq!(
        database
            .history(&invocation.session_id, 1024)
            .unwrap()
            .history[1]
            .metadata,
        Some(metadata())
    );
    assert_eq!(saved.result, reply());
    assert_eq!(database.save_recovered(pending).unwrap(), saved);
    let mut conflicting = reply();
    conflicting.metadata = None;
    assert!(
        database
            .complete_turn(invocation.clone(), conflicting)
            .is_err()
    );
    let history = database
        .history(&invocation.session_id, 1024)
        .unwrap()
        .history;
    let encoded = serde_json::to_vec(
        &history
            .into_iter()
            .map(|message| message.into_values().unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    database
        .history(&invocation.session_id, encoded.len())
        .unwrap();
    assert!(
        database
            .history(&invocation.session_id, encoded.len() - 1)
            .is_err()
    );

    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        1
    );
}

#[test]
fn committed_reply_wins_over_broken_output_and_late_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let saved = database.complete_turn(invocation.clone(), reply()).unwrap();
    for (bytes, has_diagnostics) in [
        (Some(b"broken JSON".to_vec()), true),
        (Some(stdout("disagreeing reply")), true),
        (Some(stdout("paid reply")), false),
        (None, false),
    ] {
        let resolution = database
            .reconcile_completion(ConversationExit {
                invocation: &invocation,
                stdout: bytes.as_deref(),
                outcome: TurnExit::Cancelled,
                summary: "late cancellation",
            })
            .unwrap();
        assert!(matches!(resolution.state,CompletionState::Committed(value) if value==saved));
        assert_eq!(!resolution.diagnostics.is_empty(), has_diagnostics);
    }
}

#[test]
fn retained_replies_keep_database_ownership_until_abandoned() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let bytes = stdout("paid reply");
    let resolution = database
        .reconcile_completion(ConversationExit {
            invocation: &invocation,
            stdout: Some(&bytes),
            outcome: TurnExit::Interrupted,
            summary: "database unavailable",
        })
        .unwrap();
    let CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected retained model reply")
    };
    let mut wrong = pending.clone();
    wrong.database_path = root.path().join("other.sqlite");
    assert!(database.save_recovered(wrong.clone()).is_err());
    assert!(database.abandon_recovered(&wrong).is_err());
    database.abandon_recovered(&pending).unwrap();
    assert_eq!(
        database.turn(&invocation.turn_id).unwrap().state,
        "cancelled"
    );
    assert!(database.save_recovered(pending).is_err());
}

#[test]
fn invalid_or_missing_results_finalize_only_the_admitted_invocation() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let untouched = begin(&database);
    let first = usages().remove(0);
    let mut invalid = first.clone();
    invalid.usage = Some(std::collections::BTreeMap::from([(
        "input_tokens".into(),
        -1,
    )]));
    let invalid_results = [
        json!({"status":"succeeded", "text":"reply", "usages":[first.clone(), first]}),
        json!({"status":"succeeded", "text":"reply", "usages":[invalid]}),
        json!({"status":"succeeded", "text":"reply"}),
        json!({"status":"unknown", "text":"reply", "usages":[]}),
        json!({"status":"succeeded", "text":"reply", "metadata":[], "usages":[]}),
        json!({"status":"succeeded", "text":true, "usages":[]}),
        json!({"status":"succeeded", "text":"", "usages":[]}),
        json!({"status":"succeeded", "text":"reply", "error":"failed", "usages":[]}),
        json!({"status":"failed", "usages":[]}),
        json!({"status":"failed", "error":" ", "usages":[]}),
        json!({"status":"failed", "error":"failed", "title":"invalid update", "usages":[]}),
        json!({"status":"succeeded", "text":"reply", "usages":[{
            "call_id":"call", "model":"model", "attribution":{"purpose":" "}
        }]}),
        json!({"status":"succeeded", "text":"reply", "usages":[{
            "call_id":"call", "model":"model", "attribution":{"profile":""}
        }]}),
    ];
    for bytes in [
        None,
        Some(b"{}".to_vec()),
        Some(br#"{"status":"succeeded","text":"a","text":"b","usages":[]}"#.to_vec()),
        Some(stdout(&"a".repeat(MAX_RECOVERY_BYTES))),
    ]
    .into_iter()
    .chain(invalid_results.map(|result| Some(serde_json::to_vec(&result).unwrap())))
    {
        let invocation = begin(&database);
        let resolution = database
            .reconcile_completion(ConversationExit {
                invocation: &invocation,
                stdout: bytes.as_deref(),
                outcome: TurnExit::Interrupted,
                summary: "no valid reply",
            })
            .unwrap();
        assert_eq!(resolution.diagnostics.len(), usize::from(bytes.is_some()));
        assert!(matches!(resolution.state, CompletionState::Finished(_)));
        assert_eq!(
            database.turn(&invocation.turn_id).unwrap().state,
            "interrupted"
        );
    }
    assert_eq!(database.turn(&untouched.turn_id).unwrap().state, "running");
}

#[test]
fn late_output_cannot_admit_or_revive_a_conversation() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let cancelled = begin(&database);
    database
        .fail_turn(cancelled.clone(), "cancelled", "cancelled before reply")
        .unwrap();
    let bytes = stdout("late reply");
    for invocation in [
        mf_chat::Invocation::new("never-admitted"),
        cancelled.clone(),
    ] {
        let resolution = database
            .reconcile_completion(ConversationExit {
                invocation: &invocation,
                stdout: Some(&bytes),
                outcome: TurnExit::Failed,
                summary: "late output",
            })
            .unwrap();
        if invocation == cancelled {
            assert!(
                matches!(resolution.state, CompletionState::Finished(turn) if turn.state == "cancelled")
            );
        } else {
            assert!(matches!(resolution.state, CompletionState::NotBegun));
        }
        assert!(matches!(
            resolution.diagnostics.as_slice(),
            [mf_chat::ChatError::Invalid {
                field: "model reply",
                ..
            }]
        ));
    }
    assert!(
        database
            .history(&cancelled.session_id, 1024)
            .unwrap()
            .history
            .is_empty()
    );
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        1
    );
}

#[test]
fn failed_workflow_persists_all_reported_calls_atomically_without_an_assistant_message() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let invocation = begin(&database);
    let mut reported = usages();
    reported[0].usage = Some(std::collections::BTreeMap::from([(
        "input_tokens".into(),
        7,
    )]));
    let mut failed = reported[0].clone();
    failed.call_id = "second-call".into();
    failed.model = "other-model".into();
    failed.started_at = Some(86_400_000);
    failed.usage = None;
    failed.error = Some("provider unavailable".into());
    reported.push(failed);
    let bytes = serde_json::to_vec(
        &json!({"status":"failed", "usages":reported, "error":"workflow failed"}),
    )
    .unwrap();
    let exit = || ConversationExit {
        invocation: &invocation,
        stdout: Some(&bytes),
        outcome: TurnExit::Failed,
        summary: "execution failed",
    };
    let mut connection = diesel::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
    connection.batch_execute("CREATE TRIGGER reject_usages BEFORE UPDATE OF usages ON model_calls BEGIN SELECT RAISE(ABORT, 'save failed'); END;").unwrap();
    assert!(database.reconcile_completion(exit()).is_err());
    assert_eq!(database.turn(&invocation.turn_id).unwrap().state, "running");
    assert!(database.call(&invocation.call_id).unwrap().usages.is_none());
    connection
        .batch_execute("DROP TRIGGER reject_usages;")
        .unwrap();
    assert!(
        matches!(database.reconcile_completion(exit()).unwrap().state, CompletionState::Finished(turn) if turn.state == "failed")
    );
    assert!(
        database
            .reconcile_completion(exit())
            .unwrap()
            .diagnostics
            .is_empty()
    );
    assert_eq!(
        database
            .messages(&invocation.session_id, 10, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert!(
        database
            .history(&invocation.session_id, 1024)
            .unwrap()
            .history
            .is_empty()
    );
    let statistics = database.usage(Default::default()).unwrap().overall;
    assert_eq!(statistics.attempted_calls, 2);
    assert_eq!(statistics.statuses["failed"], 1);
    assert_eq!(statistics.statuses["succeeded"], 1);
    assert_eq!(statistics.input_tokens.known_subtotal, Some(7));
    assert_eq!(statistics.input_tokens.missing_calls, 1);
    let days = database
        .usage(mf_chat::UsageQuery {
            group_by: Some(mf_chat::UsageGrouping::Day),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        days.groups
            .iter()
            .map(|group| group.key.as_str())
            .collect::<Vec<_>>(),
        ["1970-01-01", "1970-01-02"]
    );

    drop(connection);
    drop(database);
    let reopened = ChatDatabase::open(path).unwrap();
    assert_eq!(
        reopened.call(&invocation.call_id).unwrap().usages,
        Some(reported)
    );
    assert_eq!(
        reopened.usage(Default::default()).unwrap().overall,
        statistics
    );
}

#[test]
fn returned_history_cannot_rewrite_saved_message_metadata() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let first = begin(&database);
    database.complete_turn(first.clone(), reply()).unwrap();
    let profile = ModelProfile::new("default", json!({"model":"fixture-model"})).unwrap();
    let second = database
        .begin_turn(
            &first.session_id,
            "next",
            profile.identity().clone(),
            None,
            1024,
        )
        .unwrap();
    assert_eq!(
        second.request.history.unwrap()[1].metadata,
        Some(metadata())
    );
    let mut output: serde_json::Value = serde_json::from_slice(&stdout("next reply")).unwrap();
    output["history"] =
        json!([{"role":"assistant","content":"rewritten","metadata":{"changed":true}}]);
    output["metadata"] = json!({"current":true});
    let bytes = serde_json::to_vec(&output).unwrap();
    let resolution = database
        .reconcile_completion(ConversationExit {
            invocation: &second.invocation,
            stdout: Some(&bytes),
            outcome: TurnExit::Failed,
            summary: "workflow finished",
        })
        .unwrap();
    let CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected retained reply");
    };
    database.save_recovered(pending).unwrap();
    let history = database.history(&first.session_id, 4096).unwrap().history;
    assert_eq!(history[1].content, "paid reply");
    assert_eq!(history[1].metadata, Some(metadata()));
    assert_eq!(
        history[3].metadata.as_ref().unwrap()["current"],
        json!(true)
    );
}

#[test]
fn unreported_workflow_invocations_are_not_counted_as_zero_model_calls() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let owner = database
        .create_session("default", None, Some("Fixed".into()))
        .unwrap();
    let identity = mf_chat::ModelIdentity {
        profile: "default".into(),
        model: None,
    };
    let begun = database
        .begin_turn(&owner.id, "first", identity.clone(), None, 1024)
        .unwrap();
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .unreported_invocations,
        1
    );
    database
        .fail_turn(begun.invocation, "interrupted", "process stopped")
        .unwrap();
    let known_empty = database
        .begin_turn(&owner.id, "second", identity, None, 1024)
        .unwrap();
    database
        .complete_turn(
            known_empty.invocation,
            ConversationResult {
                text: "no model was needed".into(),
                usages: Vec::new(),
                metadata: None,
                finish_reason: None,
                ..Default::default()
            },
        )
        .unwrap();
    let statistics = database.usage(Default::default()).unwrap();
    assert_eq!(statistics.overall.attempted_calls, 0);
    assert_eq!(statistics.unreported_invocations, 1);
    assert_eq!(statistics.overall.input_tokens.known_subtotal, None);
}

#[test]
fn failed_results_cannot_overwrite_committed_or_different_terminal_results() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let committed = begin(&database);
    let saved = database.complete_turn(committed.clone(), reply()).unwrap();
    let failed = mf_chat::ConversationResult {
        status: "failed".into(),
        error: Some("workflow failed".into()),
        usages: usages(),
        ..Default::default()
    };
    let resolution = database
        .reconcile_result(
            &committed,
            Some(Ok(failed.clone())),
            TurnExit::Failed,
            "late failure",
        )
        .unwrap();
    assert!(matches!(resolution.state, CompletionState::Committed(actual) if actual == saved));
    assert_eq!(resolution.diagnostics.len(), 1);
    let invocation = begin(&database);
    let mut invalid_completion = failed.clone();
    invalid_completion.text = "intermediate answer".into();
    assert!(
        database
            .complete_turn(invocation.clone(), invalid_completion)
            .is_err()
    );
    assert_eq!(database.turn(&invocation.turn_id).unwrap().state, "running");
    database
        .reconcile_result(
            &invocation,
            Some(Ok(failed.clone())),
            TurnExit::Failed,
            "failure",
        )
        .unwrap();
    let original = database.call(&invocation.call_id).unwrap();
    let mut conflicting = failed.clone();
    conflicting.error = Some("different failure".into());
    let resolution = database
        .reconcile_result(
            &invocation,
            Some(Ok(conflicting)),
            TurnExit::Failed,
            "late failure",
        )
        .unwrap();
    assert_eq!(resolution.diagnostics.len(), 1);
    assert_eq!(database.call(&invocation.call_id).unwrap(), original);
    let cancelled = begin(&database);
    database
        .fail_turn(cancelled.clone(), "cancelled", "cancelled")
        .unwrap();
    let resolution = database
        .reconcile_result(
            &cancelled,
            Some(Ok(failed)),
            TurnExit::Failed,
            "late failure",
        )
        .unwrap();
    assert_eq!(resolution.diagnostics.len(), 1);
    assert!(
        matches!(resolution.state, CompletionState::Finished(turn) if turn.state == "cancelled")
    );
}
