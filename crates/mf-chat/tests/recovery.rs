use diesel::{Connection, connection::SimpleConnection};
use mf_chat::executable::{
    ChatContract, CompletionState, ConversationExit, MAX_RECOVERY_BYTES, TurnExit,
};
use mf_chat::{ChatDatabase, ModelProfile, Reply};
use mf_runtime::NodeValue;
use serde_json::json;

fn contract() -> ChatContract {
    let definition = mf_runtime::WorkflowDefinition::from_json(
        &json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"model", "kind":"builtin.llm", "config":{"model":"fixture-model"}}],
            "outputs":[{"name":"text", "node":"model", "port":"text"}, {"name":"status", "node":"model", "port":"status"}, {"name":"usages", "node":"model", "port":"usages"}]
        })
        .to_string(),
    )
    .unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    ChatContract::from_manifest(&plan.manifest(&registry).unwrap()).unwrap()
}
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
fn reply() -> Reply {
    Reply {
        usages: Some(usages()),
        metadata: Some(metadata()),
        text: "paid reply".into(),
        usage: None,
        finish_reason: None,
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
    let contract = contract();
    let invocation = begin(&database);
    let bytes = stdout("paid reply");
    failure(&database, true);
    assert!(database.complete_turn(invocation.clone(), reply()).is_err());
    let resolution = database
        .reconcile_completion(ConversationExit {
            contract: &contract,
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
    assert_eq!(saved.reply, reply());
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
    let contract = contract();
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
                contract: &contract,
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
fn retained_replies_keep_database_and_purpose_ownership_until_abandoned() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let contract = contract();
    let invocation = begin(&database);
    let bytes = stdout("paid reply");
    let resolution = database
        .reconcile_completion(ConversationExit {
            contract: &contract,
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
    wrong = pending.clone();
    wrong.purpose = mf_chat::CallPurpose::Title;
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
fn malformed_or_missing_model_output_finalizes_only_the_admitted_invocation() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let contract = contract();
    let untouched = begin(&database);
    for bytes in [
        None,
        Some(b"{}".to_vec()),
        Some(b"{\"text\":\"a\",\"history\":\"bad\"}".to_vec()),
        Some(b"{\"text\":true,\"history\":[]}".to_vec()),
        Some(b"{\"text\":\"a\",\"text\":\"b\",\"history\":[]}".to_vec()),
        Some(stdout(&"a".repeat(MAX_RECOVERY_BYTES))),
    ] {
        let invocation = begin(&database);
        let resolution = database
            .reconcile_completion(ConversationExit {
                contract: &contract,
                invocation: &invocation,
                stdout: bytes.as_deref(),
                outcome: TurnExit::Interrupted,
                summary: "no valid reply",
            })
            .unwrap();
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
    let contract = contract();
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
                contract: &contract,
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
    let contract = contract();
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
        contract: &contract,
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
fn response_validation_rejects_ambiguous_or_invalid_usage_facts() {
    let contract = contract();
    let invocation = mf_chat::Invocation::new("session");
    let first = usages().remove(0);
    let mut invalid = first.clone();
    invalid.usage = Some(std::collections::BTreeMap::from([(
        "input_tokens".into(),
        -1,
    )]));
    for reported in [vec![first.clone(), first], vec![invalid]] {
        let bytes =
            serde_json::to_vec(&json!({"status":"succeeded", "text":"reply", "usages":reported}))
                .unwrap();
        assert!(contract.decode_completion(&bytes, &invocation).is_err());
    }
    for result in [
        json!({"status":"succeeded", "text":"reply"}),
        json!({"status":"unknown", "text":"reply", "usages":[]}),
        json!({"status":"succeeded", "text":"reply", "metadata":[], "usages":[]}),
    ] {
        assert!(
            contract
                .decode_completion(&serde_json::to_vec(&result).unwrap(), &invocation)
                .is_err()
        );
    }
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
    let completion = contract()
        .decode_completion(&serde_json::to_vec(&output).unwrap(), &second.invocation)
        .unwrap();
    database
        .complete_turn(completion.invocation, completion.reply)
        .unwrap();
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
        config_id: "workflow".into(),
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
            Reply {
                text: "no model was needed".into(),
                usages: Some(Vec::new()),
                usage: None,
                metadata: None,
                finish_reason: None,
            },
        )
        .unwrap();
    let statistics = database.usage(Default::default()).unwrap();
    assert_eq!(statistics.overall.attempted_calls, 0);
    assert_eq!(statistics.unreported_invocations, 1);
    assert_eq!(statistics.overall.input_tokens.known_subtotal, None);
}
