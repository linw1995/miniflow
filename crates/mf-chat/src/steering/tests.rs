use super::*;
use crate::{
    ChatDatabase, ConversationResult, ModelIdentity,
    contract::{CompletionState, TurnExit},
};
use diesel::connection::SimpleConnection;
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
use std::thread;

#[test]
fn closing_admission_preserves_pending_input_without_consuming_or_replaying_it() {
    let inbox = SteeringInbox::new(Invocation::new("session"));
    assert!(inbox.submit(" \n".into()).is_err());
    assert!(inbox.submit("x".repeat(MAX_PENDING_BYTES + 1)).is_err());
    assert!(inbox.take().unwrap().is_empty());
    for i in 0..MAX_PENDING_MESSAGES {
        inbox.submit(format!("note {i}")).unwrap();
    }
    assert!(inbox.submit("one too many".into()).is_err());
    assert!(inbox.take_unsent().is_empty());
    inbox.close();
    assert!(inbox.submit("after close".into()).is_err());
    assert!(inbox.take().is_err());
    assert_eq!(inbox.take_unsent().len(), MAX_PENDING_MESSAGES);
    assert!(inbox.take_unsent().is_empty());
}

#[test]
fn withdrawal_and_claiming_a_batch_have_one_winner() {
    let inbox = SteeringInbox::new(Invocation::new("target"));
    let other = SteeringInbox::new(Invocation::new("target"));
    let first = inbox.submit("same".into()).unwrap();
    let second = inbox.submit("same".into()).unwrap();
    assert!(other.withdraw(first).is_none());
    assert_eq!(inbox.withdraw(second).as_deref(), Some("same"));
    assert!(inbox.withdraw(second).is_none());
    let claimed = inbox.take().unwrap();
    assert_eq!(
        claimed.into_iter().collect::<Vec<_>>(),
        [(first, "same".to_string())]
    );
    assert!(inbox.withdraw(first).is_none());
    assert!(inbox.withdraw_last().is_none());
    let late = inbox.submit("later boundary".into()).unwrap();
    assert_eq!(inbox.withdraw_last().as_deref(), Some("later boundary"));
    assert!(inbox.withdraw(late).is_none());
    assert!(inbox.take().unwrap().is_empty());
    inbox.close();
    assert!(inbox.submit("after close".into()).is_err());
}

#[test]
fn concurrent_withdrawal_never_removes_an_input_that_the_reader_obtained() {
    let inbox = SteeringInbox::new(Invocation::new("session"));
    let id = inbox.submit("racing".into()).unwrap();
    let start = std::sync::Barrier::new(2);
    thread::scope(|scope| {
        let claim = scope.spawn(|| {
            start.wait();
            inbox.take().unwrap()
        });
        start.wait();
        let withdrawn = inbox.withdraw(id);
        let claimed = claim.join().unwrap();
        match withdrawn {
            Some(text) => {
                assert_eq!(text, "racing");
                assert!(claimed.is_empty());
            }
            None => assert_eq!(
                claimed.into_iter().collect::<Vec<_>>(),
                [(id, "racing".to_owned())]
            ),
        }
    });
}

#[test]
fn receipts_validate_ordered_steering_text_and_invocation_on_final_output() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("First".into()).unwrap();
    inbox.submit("Second".into()).unwrap();
    let log = DeliveryLog::new(inbox.clone());
    assert!(
        db.deliveries
            .with_active(&Invocation::new(&id.session_id), Some(&log), || Ok(()))
            .is_err()
    );
    let mut writer = crate::storage::connect(db.path()).unwrap();
    writer.batch_execute("BEGIN IMMEDIATE").unwrap();
    db.deliveries
        .with_active(&id, Some(&log), || {
            assert!(
                db.deliveries
                    .with_active(&id, Some(&log), || Ok(()))
                    .is_err()
            );
            let result = log.claim()?.delivered();
            assert!(result["messages"][0].is_secret());
            assert_eq!(result["messages"][0]["content"], json!("First"));
            assert_eq!(result["messages"][1]["content"], json!("Second"));
            assert!(
                log.claim()?.delivered()["messages"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            Ok(())
        })
        .unwrap();
    writer.batch_execute("ROLLBACK").unwrap();
    let answer = json!({"role":"assistant","content":"Reply"});
    let first = json!({"role":"user","content":"First"});
    let second = json!({"role":"user","content":"Second"});
    let make = |messages| ConversationResult::append(serde_json::from_value(messages).unwrap());
    // The flow chooses where delivered user messages appear in its final display output.
    let good = make(json!([answer, answer, first, answer, second, answer]));
    for invalid in [
        json!([answer, answer]),
        json!([answer, first, answer]),
        json!([answer, second, first, answer]),
        json!([answer, {"role":"user","content":"Changed"}, second, answer]),
        json!([answer, first, second, first, answer]),
        json!([answer, {"role":"user","content":"First","metadata":{"invented":true}}, second, answer]),
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
    let saved = db.complete_turn(id.clone(), good.clone()).unwrap();
    assert_eq!(saved.result.display_messages().len(), 6);
    assert!(db.deliveries.calls.lock().unwrap().is_empty());
    let replay = db
        .reconcile_result(&id, Some(Ok(good.clone())), TurnExit::Failed, "late exit")
        .unwrap();
    assert!(replay.diagnostics.is_empty());
    assert!(matches!(replay.state, CompletionState::Committed(actual) if actual == saved));
    assert_eq!(db.complete_turn(id, good).unwrap(), saved);
}

#[test]
fn partial_writes_and_overlapping_pulls_cannot_authorize_undelivered_users() {
    let inbox = SteeringInbox::new(Invocation::new("session"));
    let log = DeliveryLog::new(inbox.clone());
    inbox.submit("sent".into()).unwrap();
    log.claim().unwrap().delivered();
    assert_eq!(*log.messages.lock().unwrap(), ["sent".to_owned()]);
    let next = SteeringInbox::new(Invocation::new("another session"));
    next.submit("not written".into()).unwrap();
    let second = DeliveryLog::new(next.clone());
    let pending = second.claim().unwrap();
    let late = next.submit("later".into()).unwrap();
    assert!(second.claim().is_err());
    drop(pending);
    assert_eq!(next.withdraw(late).as_deref(), Some("later"));
    assert_eq!(next.take_unsent(), ["not written"]);
    assert!(second.messages.lock().unwrap().is_empty());
}

#[test]
fn unexpected_callback_arguments_never_claim_user_input() {
    let inbox = SteeringInbox::new(Invocation::new("target"));
    let id = inbox.submit("keep".into()).unwrap();
    let handler = DeliveryLog::new(inbox.clone()).callback();
    for args in [
        json!({"reply.position":1,"reply.outcome":"completed"}),
        json!({"unexpected":true}),
    ] {
        assert!(
            handler
                .call(
                    &mf_runtime::WorkflowArguments::try_from(args).unwrap(),
                    &Default::default()
                )
                .is_err()
        );
    }
    assert_eq!(inbox.withdraw(id).as_deref(), Some("keep"));
    assert!(
        handler
            .call(
                &mf_runtime::WorkflowArguments::default(),
                &Default::default()
            )
            .unwrap()
            .delivered()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn finalizing_a_call_closes_its_live_inbox_before_another_delivery() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("not sent".into()).unwrap();
    let log = DeliveryLog::new(inbox.clone());
    let error = db
        .deliveries
        .with_active(&id, Some(&log), || {
            db.fail_call(&id.call_id, "cancelled", "stop now")?;
            assert!(inbox.submit("too late".into()).is_err());
            log.claim().map(drop)
        })
        .unwrap_err();
    assert!(matches!(error, ChatError::SteeringClosed));
    assert_eq!(inbox.take_unsent(), ["not sent"]);
    assert!(db.deliveries.calls.lock().unwrap().is_empty());
}
