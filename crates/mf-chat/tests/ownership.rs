use mf_chat::contract::{InvocationState, TurnExit};
use mf_chat::{ChatDatabase, ConversationResult, ModelProfile};
use serde_json::json;

#[test]
fn exact_cleanup_keeps_other_sessions_and_committed_success() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let profile = ModelProfile::new("default", json!({"model":"fixture-model"})).unwrap();
    let a = database.create_session("default", None, None).unwrap();
    let b = database.create_session("default", None, None).unwrap();
    let first = database
        .begin_turn(&a.id, "first", profile.identity().clone(), None, 1024)
        .unwrap();
    let second = database
        .begin_turn(&b.id, "second", profile.identity().clone(), None, 1024)
        .unwrap();
    let mut wrong_session = first.invocation.clone();
    wrong_session.session_id = b.id.clone();
    let mut wrong_call = first.invocation.clone();
    wrong_call.call_id = second.invocation.call_id.clone();
    for mismatched in [wrong_session, wrong_call] {
        assert!(matches!(
            database.finalize_invocation(mismatched, TurnExit::Cancelled, "wrong owner"),
            Err(mf_chat::ChatError::Conflict { .. })
        ));
    }
    let state = database
        .finalize_invocation(
            first.invocation.clone(),
            TurnExit::Failed,
            "provider failure",
        )
        .unwrap();
    assert!(matches!(state, InvocationState::Finished(_)));
    assert!(matches!(
        database.inspect_invocation(&second.invocation).unwrap(),
        InvocationState::Running
    ));
    let (original, prompt) = database.retry_prompt(&first.invocation.turn_id).unwrap();
    assert_eq!(original.id, first.invocation.turn_id);
    assert_eq!(prompt, "first");
    database
        .complete_turn(
            second.invocation.clone(),
            ConversationResult {
                usages: Vec::new(),
                metadata: None,
                text: "saved".into(),
                finish_reason: None,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(
        database
            .finalize_invocation(
                second.invocation.clone(),
                TurnExit::Cancelled,
                "late cancellation"
            )
            .unwrap(),
        InvocationState::Committed(_)
    ));
    let missing = mf_chat::Invocation {
        session_id: a.id,
        turn_id: "missing".into(),
        call_id: "missing".into(),
    };
    assert!(matches!(
        database
            .finalize_invocation(missing, TurnExit::Interrupted, "no begin")
            .unwrap(),
        InvocationState::NotBegun
    ));
}
