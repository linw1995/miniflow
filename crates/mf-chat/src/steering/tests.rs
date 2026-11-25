use super::*;
use crate::{
    ChatDatabase, ConversationResult, ModelIdentity,
    contract::{CompletionState, TurnExit},
};
use serde_json::json;

fn admitted() -> (tempfile::TempDir, ChatDatabase, Invocation) {
    let root = tempfile::tempdir().unwrap();
    let db = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = db.create_session("agent", None, None).unwrap();
    let invocation = db
        .begin_turn(
            &session.id,
            "Initial",
            ModelIdentity {
                profile: "agent".into(),
                model: None,
            },
            None,
        )
        .unwrap()
        .invocation;
    (root, db, invocation)
}

fn boundary(outcome: CallOutcome, position: i64) -> Boundary {
    Boundary { position, outcome }
}

#[test]
fn failure_closes_admission_without_consuming_or_replaying_pending_input() {
    let inbox = SteeringInbox::new(Invocation::new("session"));
    assert!(inbox.submit(" \n".into()).is_err());
    assert!(inbox.submit("x".repeat(MAX_PENDING_BYTES + 1)).is_err());
    assert!(inbox.take_at(CallOutcome::ToolCalls).unwrap().is_empty());
    for i in 0..MAX_PENDING_MESSAGES {
        inbox.submit(format!("note {i}")).unwrap();
    }
    assert!(inbox.submit("one too many".into()).is_err());
    assert!(inbox.take_unsent().is_empty());
    assert!(inbox.take_at(CallOutcome::Failed).unwrap().is_empty());
    assert!(inbox.submit("after failure".into()).is_err());
    assert!(inbox.take_at(CallOutcome::ToolCalls).is_err());
    assert_eq!(inbox.take_unsent().len(), MAX_PENDING_MESSAGES);
    assert!(inbox.take_unsent().is_empty());
}

#[test]
fn receipts_bind_exact_positions_text_and_invocation_before_recovery() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("Correction".into()).unwrap();
    let request = || boundary(CallOutcome::Completed, 1);
    assert!(Listener::start(db.path(), &Invocation::new(&id.session_id), &inbox).is_err());
    with_endpoint(db.path(), &id, Some(&inbox), |endpoint| {
        let endpoint = endpoint.unwrap();
        assert!(!format!("{endpoint:?}").contains(&endpoint.token));
        let other = Endpoint {
            token: Uuid::new_v4().to_string(),
            ..endpoint.clone()
        };
        assert!(other.pull(request()).is_err());
        assert_eq!(endpoint.pull(request()).unwrap(), ["Correction"]);
        assert!(endpoint.pull(request()).unwrap().is_empty());
        Ok(())
    })
    .unwrap();
    let answer = json!({"role":"assistant","content":"Reply"});
    let user = json!({"role":"user","content":"Correction"});
    let make = |messages| ConversationResult::append(serde_json::from_value(messages).unwrap());
    let good = make(json!([answer, user, answer]));
    for invalid in [
        json!([answer, answer]),
        json!([answer,{"role":"user","content":"Changed"},answer]),
        json!([answer, answer, user, answer]),
        json!([answer, user, user, answer]),
        json!([answer,{"role":"user","content":"Correction","metadata":{"invented":true}},answer]),
    ] {
        assert!(db.complete_turn(id.clone(), make(invalid)).is_err());
    }
    let other_session = db.create_session("agent", None, None).unwrap();
    let other = db
        .begin_turn(
            &other_session.id,
            "Other",
            ModelIdentity {
                profile: "agent".into(),
                model: None,
            },
            None,
        )
        .unwrap();
    let result = db
        .reconcile_result(
            &other.invocation,
            Some(Ok(good.clone())),
            TurnExit::Failed,
            "unadmitted user",
        )
        .unwrap();
    assert!(matches!(result.state, CompletionState::Finished(_)));
    assert!(!result.diagnostics.is_empty());
    assert_eq!(
        db.complete_turn(id, good)
            .unwrap()
            .result
            .display_messages()
            .len(),
        3
    );
}

#[test]
fn failed_checkpoint_rolls_back_the_batch_and_returns_the_original_storage_error() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("one".into()).unwrap();
    inbox.submit("two".into()).unwrap();
    let error = with_endpoint(db.path(), &id, Some(&inbox), |endpoint| {
        assert!(
            endpoint
                .unwrap()
                .pull(boundary(CallOutcome::ToolCalls, i64::MAX))
                .is_err()
        );
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(error, ChatError::Arithmetic { .. }));
    assert_eq!(inbox.take_unsent(), ["one", "two"]);
    validate(&mut storage::connect(db.path()).unwrap(), &id.turn_id, &[]).unwrap();
    let texts = vec!["one".to_owned()];
    assert!(record(db.path(), &id, 0, &texts).is_err());
    let mut wrong = id.clone();
    wrong.session_id = "other".into();
    assert!(matches!(
        record(db.path(), &wrong, 1, &texts),
        Err(ChatError::Conflict { .. })
    ));
}
