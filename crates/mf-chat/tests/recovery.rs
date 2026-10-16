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
            "outputs":[{"name":"text", "node":"model", "port":"text"}]
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
fn reply() -> Reply {
    Reply {
        text: "paid reply".into(),
        usage: None,
        finish_reason: None,
    }
}
fn stdout(text: &str) -> Vec<u8> {
    serde_json::to_vec(
        &mfn_llm::LlmOutputs {
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
    assert_eq!(saved.reply, reply());
    assert_eq!(database.save_recovered(pending).unwrap(), saved);
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
    for bytes in [b"broken JSON".to_vec(), stdout("disagreeing reply")] {
        let resolution = database
            .reconcile_completion(ConversationExit {
                contract: &contract,
                invocation: &invocation,
                stdout: Some(&bytes),
                outcome: TurnExit::Cancelled,
                summary: "late cancellation",
            })
            .unwrap();
        assert!(matches!(resolution.state,CompletionState::Committed(value) if value==saved));
        assert!(!resolution.diagnostics.is_empty());
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
