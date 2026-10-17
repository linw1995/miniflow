use super::*;
fn identity() -> ModelIdentity {
    ModelIdentity {
        profile: "default".into(),
        model: Some("model".into()),
    }
}
fn reply(text: &str) -> Reply {
    Reply {
        usages: Some(vec![mfn_llm::CallUsage {
            started_at: None,
            call_id: "reported-call".into(),
            node: None,
            model: "model".into(),
            usage: Some(std::collections::BTreeMap::from([(
                "output_tokens".into(),
                1,
            )])),
            error: None,
        }]),
        metadata: None,
        text: text.into(),
        finish_reason: None,
    }
}

#[test]
fn explicit_scheduling_is_idempotent_and_manual_titles_win() {
    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let owner = database.create_session("default", None, None).unwrap();
    let turn = database
        .begin_turn(&owner.id, "question", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(turn.invocation.clone(), reply("answer"))
        .unwrap();
    database
        .complete_turn(turn.invocation, reply("answer"))
        .unwrap();
    assert!(database.pending_titles(10).unwrap().is_empty());
    assert!(database.schedule_titles(10).unwrap());
    assert!(!database.schedule_titles(10).unwrap());
    let calls = database.pending_titles(10).unwrap();
    assert_eq!(calls.len(), 1);
    assert!(matches!(
        prepare(PrepareTitleInputs {
            database_path: database.path().to_str().unwrap().into(),
            call_id: calls[0].id.clone(),
            model: identity(),
            max_bytes: 0,
        }),
        Err(ChatError::Invalid {
            field: "max_bytes",
            ..
        })
    ));
    assert!(matches!(
        retry(crate::nodes::SessionInputs {
            database_path: database.path().to_str().unwrap().into(),
            session_id: owner.id.clone(),
        }),
        Err(ChatError::TitleBusy { .. })
    ));
    let started = prepare(PrepareTitleInputs {
        database_path: database.path().to_str().unwrap().into(),
        call_id: calls[0].id.clone(),
        model: identity(),
        max_bytes: 1024,
    })
    .unwrap();
    assert_eq!(started.request.as_ref().unwrap().history, None);
    database.rename_session(&owner.id, "Manual").unwrap();
    let result = database
        .complete_title(
            started.invocation.clone(),
            started.title_revision,
            reply("Generated"),
        )
        .unwrap();
    assert_eq!(result.outcome, TitleOutcome::Stale);
    let completion = CompleteTitleInputs {
        database_path: database.path().to_str().unwrap().into(),
        invocation: started.invocation.clone(),
        title_revision: started.title_revision,
        response: reply("Different response"),
    };
    assert!(matches!(
        complete(completion.clone()),
        Err(ChatError::Conflict { .. })
    ));
    let mut foreign = completion;
    foreign.invocation.session_id = "different".into();
    assert!(matches!(complete(foreign), Err(ChatError::Conflict { .. })));
    assert!(matches!(
        retry(crate::nodes::SessionInputs {
            database_path: database.path().to_str().unwrap().into(),
            session_id: owner.id.clone(),
        }),
        Err(ChatError::Invalid { field: "title", .. })
    ));
    assert_eq!(database.session(&owner.id).unwrap().title, "Manual");
    assert_eq!(
        database.call(&started.invocation.call_id).unwrap().usages,
        reply("Generated").usages
    );
    assert_eq!(
        database
            .messages(&owner.id, 10, None)
            .unwrap()
            .messages
            .len(),
        2
    );
    let other = database.create_session("default", None, None).unwrap();
    let turn = database
        .begin_turn(&other.id, "other", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(turn.invocation, reply("answer"))
        .unwrap();
    assert!(database.schedule_titles(10).unwrap());
    let call = database.pending_titles(10).unwrap().pop().unwrap();
    database
        .rename_session(&other.id, "Already manual")
        .unwrap();
    let started = prepare(PrepareTitleInputs {
        database_path: database.path().to_str().unwrap().into(),
        call_id: call.id.clone(),
        model: identity(),
        max_bytes: 1024,
    })
    .unwrap();
    assert!(started.request.is_none());
    assert_eq!(database.call(&call.id).unwrap().started_at, None);
    assert_eq!(database.call(&call.id).unwrap().state, "skipped");
}

#[test]
fn pending_titles_survive_restart_and_rejected_titles_keep_usage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let owner = database.create_session("default", None, None).unwrap();
    let turn = database
        .begin_turn(&owner.id, "question", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(turn.invocation, reply("answer"))
        .unwrap();
    assert!(database.schedule_titles(10).unwrap());
    let call = database.pending_titles(10).unwrap().pop().unwrap();
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(database.pending_titles(10).unwrap()[0].id, call.id);
    let started = prepare(PrepareTitleInputs {
        database_path: path.to_str().unwrap().into(),
        call_id: call.id.clone(),
        model: identity(),
        max_bytes: 1024,
    })
    .unwrap();
    let result = database
        .complete_title(
            started.invocation,
            started.title_revision,
            reply("two\nlines"),
        )
        .unwrap();
    assert_eq!(result.outcome, TitleOutcome::Rejected);
    assert_eq!(
        database.call(&call.id).unwrap().usages,
        reply("two\nlines").usages
    );
    let next = database.retry_title(&owner.id).unwrap();
    assert_ne!(next.id, call.id);
    let started = prepare(PrepareTitleInputs {
        database_path: path.to_str().unwrap().into(),
        call_id: next.id.clone(),
        model: identity(),
        max_bytes: 1024,
    })
    .unwrap();
    database
        .fail_call(&next.id, "failed", "title failed")
        .unwrap();
    assert!(started.request.is_some());
    assert!(!database.schedule_titles(10).unwrap());
    assert_eq!(database.history(&owner.id, 1024).unwrap().history.len(), 2);
}

#[test]
fn scheduling_failure_does_not_rollback_replies_and_uses_the_first_successful_turn() {
    use diesel::connection::SimpleConnection;

    let directory = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
    let owner = database.create_session("default", None, None).unwrap();
    let failed = database
        .begin_turn(&owner.id, "failed", identity(), None, 1024)
        .unwrap();
    database
        .fail_turn(failed.invocation, "failed", "provider failed")
        .unwrap();
    assert!(!database.schedule_titles(10).unwrap());
    let manual = database
        .create_session("default", None, Some("Manual".into()))
        .unwrap();
    let manual_turn = database
        .begin_turn(&manual.id, "question", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(manual_turn.invocation, reply("answer"))
        .unwrap();
    assert!(!database.schedule_titles(10).unwrap());
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("CREATE TRIGGER reject_title_schedule BEFORE INSERT ON model_calls WHEN NEW.purpose = 'title' BEGIN SELECT RAISE(ABORT, 'injected scheduling failure'); END;").unwrap();
    let first = database
        .begin_turn(&owner.id, "first success", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(first.invocation.clone(), reply("answer"))
        .unwrap();
    assert!(database.pending_titles(10).unwrap().is_empty());
    assert!(database.schedule_titles(10).is_err());
    assert_eq!(
        database.turn(&first.invocation.turn_id).unwrap().state,
        "succeeded"
    );
    let next = database
        .begin_turn(&owner.id, "later success", identity(), None, 1024)
        .unwrap();
    database
        .complete_turn(next.invocation, reply("another answer"))
        .unwrap();
    connection
        .batch_execute("DROP TRIGGER reject_title_schedule;")
        .unwrap();
    assert!(database.schedule_titles(10).unwrap());
    assert!(!database.schedule_titles(10).unwrap());
    assert_eq!(
        database.pending_titles(10).unwrap()[0].turn_id.as_deref(),
        Some(first.invocation.turn_id.as_str())
    );
    assert_eq!(database.history(&owner.id, 1024).unwrap().history.len(), 4);
}
