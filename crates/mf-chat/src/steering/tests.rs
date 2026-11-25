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
    assert!(Listener::start(&db.deliveries, &Invocation::new(&id.session_id), &inbox).is_err());
    let mut writer = crate::storage::connect(db.path()).unwrap();
    writer.batch_execute("BEGIN IMMEDIATE").unwrap();
    with_endpoint(&db.deliveries, &id, Some(&inbox), |endpoint| {
        assert!(db.deliveries.register(&inbox).is_err());
        let endpoint = endpoint.unwrap();
        assert!(!format!("{endpoint:?}").contains(&endpoint.token));
        let other = Endpoint {
            token: Uuid::new_v4().to_string(),
            ..endpoint.clone()
        };
        assert!(other.pull(request()).is_err());
        for frame in [b"{}".as_slice(), &[b' '; 257]] {
            let mut stream =
                TcpStream::connect((Ipv4Addr::LOCALHOST, endpoint.port as u16)).unwrap();
            stream
                .write_all(Uuid::parse_str(&endpoint.token).unwrap().as_bytes())
                .unwrap();
            stream.write_all(frame).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).unwrap();
            assert!(reply.is_empty());
        }
        assert_eq!(endpoint.pull(request()).unwrap(), ["Correction"]);
        assert!(endpoint.pull(request()).unwrap().is_empty());
        Ok(())
    })
    .unwrap();
    writer.batch_execute("ROLLBACK").unwrap();
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
    let saved = db.complete_turn(id.clone(), good.clone()).unwrap();
    assert_eq!(saved.result.display_messages().len(), 3);
    assert!(db.deliveries.calls.lock().unwrap().is_empty());
    let replay = db
        .reconcile_result(&id, Some(Ok(good.clone())), TurnExit::Failed, "late exit")
        .unwrap();
    assert!(replay.diagnostics.is_empty());
    assert!(matches!(replay.state, CompletionState::Committed(actual) if actual == saved));
    assert_eq!(db.complete_turn(id, good).unwrap(), saved);
}

#[test]
fn invalid_checkpoint_restores_the_batch_and_abandonment_drops_its_receipts() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("one".into()).unwrap();
    inbox.submit("two".into()).unwrap();
    let error = with_endpoint(&db.deliveries, &id, Some(&inbox), |endpoint| {
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
    db.deliveries.validate(&id.call_id, &[]).unwrap();
    db.fail_turn(id, "cancelled", "discard").unwrap();
    assert!(db.deliveries.calls.lock().unwrap().is_empty());
}

#[test]
fn partial_writes_and_out_of_order_pulls_cannot_authorize_undelivered_users() {
    let deliveries = Deliveries::default();
    let inbox = SteeringInbox::new(Invocation::new("session"));
    let log = deliveries.register(&inbox).unwrap();
    let at = |pos| boundary(CallOutcome::ToolCalls, pos);
    inbox.submit("sent".into()).unwrap();
    log.deliver(at(2), std::io::sink()).unwrap();
    inbox.submit("unsent".into()).unwrap();
    assert!(log.deliver(at(2), std::io::sink()).is_err());
    assert_eq!(inbox.take_unsent(), ["unsent"]);
    let next = SteeringInbox::new(Invocation::new("another session"));
    next.submit("not written".into()).unwrap();
    let second = deliveries.register(&next).unwrap();
    assert!(matches!(
        second.deliver(at(4), &mut [0u8; 2][..]),
        Err(ChatError::SteeringIo { .. })
    ));
    assert_eq!(next.take_unsent(), ["not written"]);
    assert_eq!(*log.messages.lock().unwrap(), [(2, "sent".to_owned())]);
    assert!(second.messages.lock().unwrap().is_empty());
}

#[test]
fn finalizing_a_call_closes_its_live_inbox_before_another_delivery() {
    let (_root, db, id) = admitted();
    let inbox = SteeringInbox::new(id.clone());
    inbox.submit("not sent".into()).unwrap();
    let error = with_endpoint(&db.deliveries, &id, Some(&inbox), |endpoint| {
        db.fail_call(&id.call_id, "cancelled", "stop now")?;
        assert!(inbox.submit("too late".into()).is_err());
        assert!(
            endpoint
                .unwrap()
                .pull(boundary(CallOutcome::ToolCalls, 1))
                .is_err()
        );
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(error, ChatError::SteeringClosed));
    assert_eq!(inbox.take_unsent(), ["not sent"]);
    assert!(db.deliveries.calls.lock().unwrap().is_empty());
}
