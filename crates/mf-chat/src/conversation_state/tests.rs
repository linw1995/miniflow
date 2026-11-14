use super::*;
use crate::{ConversationResult, ModelIdentity, ToolUseHistory, ToolUsePhase};
use diesel::connection::SimpleConnection;
use serde_json::json;

fn begin(database: &ChatDatabase, session: &str, prompt: &str) -> crate::turn::TurnStarted {
    database
        .begin_turn(
            session,
            prompt,
            ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
        )
        .unwrap()
}
fn assistant(text: &str) -> ChatMessage {
    ChatMessage::Assistant {
        content: text.into(),
        tool_calls: None,
        metadata: None,
    }
}
fn user(text: &str) -> ChatMessage {
    ChatMessage::User {
        content: text.into(),
        metadata: None,
    }
}
fn system(text: &str) -> ChatMessage {
    ChatMessage::System {
        content: text.into(),
        metadata: None,
    }
}
fn complete(
    database: &ChatDatabase,
    invocation: &Invocation,
    messages: Vec<ChatMessage>,
    text: &str,
) {
    database
        .complete_turn(
            invocation.clone(),
            ConversationResult {
                messages,
                text: text.into(),
                ..Default::default()
            },
        )
        .unwrap();
}

#[test]
fn rewritten_compacted_and_reset_context_is_the_only_next_request_state_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let session = database
        .create_session("default", Some("Original system".into()), None)
        .unwrap();
    let first = begin(&database, &session.id, "Original prompt");
    let mut original = first.request.messages.clone();
    original.push(assistant("first"));
    complete(&database, &first.invocation, original.clone(), "first");
    let second = begin(&database, &session.id, "Second prompt");
    assert_eq!(
        &second.request.messages,
        &vec![
            system("Original system"),
            user("Original prompt"),
            assistant("first"),
            user("Second prompt")
        ]
    );
    // Reorder roles, replace both prompts, drop the old reply, and replace the system instruction.
    let rewritten = vec![
        system("Replacement system"),
        assistant("Summary of earlier work"),
        user("Edited prompt"),
        assistant("second"),
    ];
    complete(&database, &second.invocation, rewritten.clone(), "second");
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    let history = database.history(&session.id).unwrap();
    assert_eq!(history.history, rewritten);
    assert!(history.system.is_none());
    assert_eq!(
        database
            .conversation_messages(&first.invocation.call_id)
            .unwrap(),
        Some(original)
    );
    let third = begin(&database, &session.id, "Third prompt");
    let mut expected = rewritten;
    expected.push(user("Third prompt"));
    assert_eq!(third.request.messages, expected);
    complete(
        &database,
        &third.invocation,
        vec![assistant("reset")],
        "reset",
    );
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("reset")]
    );
    assert!(
        database.history(&session.id).unwrap().system.is_none(),
        "a dropped system prompt must never be injected again"
    );
    // Admission/display records remain inspectable, but no longer reconstruct model context.
    let audit = database.messages(&session.id, 100, None).unwrap();
    assert!(
        audit
            .messages
            .iter()
            .any(|row| row.message.content == "Original prompt")
    );
    assert!(
        audit
            .messages
            .iter()
            .any(|row| row.message.content == "first")
    );
}

#[test]
fn equal_payloads_are_shared_without_merging_distinct_ordered_members_and_replay_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let first = begin(&database, &session.id, "prompt");
    let messages = vec![
        user("same"),
        user("same"),
        assistant("same"),
        assistant("same"),
    ];
    complete(&database, &first.invocation, messages.clone(), "same");
    let mut connection = storage::connect(database.path()).unwrap();
    assert_eq!(
        conversation_payloads::table
            .count()
            .get_result::<i64>(&mut connection)
            .unwrap(),
        2
    );
    assert_eq!(
        conversation_state_entries::table
            .count()
            .get_result::<i64>(&mut connection)
            .unwrap(),
        4
    );
    let second = begin(&database, &session.id, "ignored by custom Flow");
    connection.batch_execute("CREATE TRIGGER no_payload_insert BEFORE INSERT ON conversation_payloads BEGIN SELECT RAISE(FAIL, 'payload already stored'); END; CREATE TRIGGER no_payload_update BEFORE UPDATE ON conversation_payloads BEGIN SELECT RAISE(FAIL, 'immutable payload'); END;").unwrap();
    complete(&database, &second.invocation, messages.clone(), "same");
    for table in [
        "conversation_states",
        "conversation_state_entries",
        "conversation_payloads",
        "conversation_payload_tools",
        "messages",
        "model_calls",
        "turns",
        "sessions",
    ] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            connection.batch_execute(&format!("CREATE TRIGGER replay_{table}_{operation} BEFORE {operation} ON {table} BEGIN SELECT RAISE(FAIL, 'replay writes'); END;")).unwrap();
        }
    }
    complete(&database, &second.invocation, messages.clone(), "same");
    database
        .record_conversation_state(&second.invocation, &messages)
        .unwrap();
    assert_eq!(database.history(&session.id).unwrap().history, messages);
}

#[test]
fn context_replacement_does_not_erase_streamed_tool_facts_or_execution_links() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let invocation = begin(&database, &session.id, "calculate").invocation;
    let request = ChatMessage::Assistant {
        content: String::new(),
        tool_calls: Some(vec![mfn_llm::ToolCall {
            id: "tool".into(),
            name: "calculator".into(),
            arguments: json!({"input": 2}).into(),
        }]),
        metadata: None,
    };
    let result = ChatMessage::ToolResult {
        content: "4".into(),
        tool_call_id: "tool".into(),
        metadata: None,
    };
    for (id, messages) in [
        ("request", vec![request.clone()]),
        ("result", vec![result.clone()]),
    ] {
        database
            .record_message_event(
                &invocation,
                &crate::MessageEvent {
                    id: id.into(),
                    model_call_id: Some("model".into()),
                    messages,
                },
            )
            .unwrap();
    }
    database
        .record_tool_uses(
            &invocation,
            &ToolUseHistory {
                invocation: invocation.clone(),
                calls: vec![],
                runs: vec![crate::ToolUse {
                    id: "execution".into(),
                    name: "calculator".into(),
                    phase: ToolUsePhase::Succeeded,
                    arguments: "resolved".into(),
                    output: "4".into(),
                    duration_ns: Some(1),
                    origin: Some(crate::ToolOrigin {
                        model_call_id: "model".into(),
                        tool_call_id: "tool".into(),
                    }),
                }],
                incomplete: false,
                finished: false,
            },
        )
        .unwrap();
    complete(
        &database,
        &invocation,
        vec![assistant("Summary: answer is 4")],
        "Summary: answer is 4",
    );
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("Summary: answer is 4")]
    );
    let mut connection = storage::connect(database.path()).unwrap();
    assert_eq!(
        crate::message_log::load(&mut connection, &invocation.call_id).unwrap(),
        Some(vec![request, result])
    );
    let tools = database.tool_uses(&invocation.call_id).unwrap().unwrap();
    assert_eq!(tools.calls.len(), 1);
    assert_eq!(tools.calls[0].execution_ids, ["execution"]);
    assert_eq!(tools.calls[0].output.as_deref(), Some("4"));
}

#[test]
fn failed_states_and_failed_settlements_cannot_advance_the_context_head() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let first = begin(&database, &session.id, "first");
    complete(
        &database,
        &first.invocation,
        vec![assistant("stable")],
        "stable",
    );
    let failed = begin(&database, &session.id, "failed");
    database
        .record_conversation_state(&failed.invocation, &[assistant("failed rewritten state")])
        .unwrap();
    database
        .fail_turn(failed.invocation, "failed", "failure")
        .unwrap();
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("stable")]
    );
    let pending = begin(&database, &session.id, "retry");
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("CREATE TRIGGER reject_settlement BEFORE UPDATE OF state ON turns WHEN NEW.state = 'succeeded' BEGIN SELECT RAISE(FAIL, 'settlement failed'); END;").unwrap();
    let reply = ConversationResult {
        messages: vec![assistant("replacement")],
        text: "replacement".into(),
        ..Default::default()
    };
    assert!(
        database
            .complete_turn(pending.invocation.clone(), reply.clone())
            .is_err()
    );
    assert!(
        load(&mut connection, &pending.invocation.call_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("stable")]
    );
    connection
        .batch_execute("DROP TRIGGER reject_settlement;")
        .unwrap();
    database.complete_turn(pending.invocation, reply).unwrap();
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("replacement")]
    );
}

#[test]
fn payload_interning_distinguishes_sensitivity_and_preserves_full_arguments_and_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let first = begin(&database, &session.id, "first");
    let private = ValueRef::object([(
        "key".into(),
        ValueRef::from("保密".repeat(3000)).into_secret(),
    )]);
    let request = ChatMessage::Assistant {
        content: String::new(),
        tool_calls: Some(vec![mfn_llm::ToolCall {
            id: "tool".into(),
            name: "calculator".into(),
            arguments: private.clone(),
        }]),
        metadata: Some(BTreeMap::from([("private".into(), private)])),
    };
    let messages = vec![
        request,
        ChatMessage::ToolResult {
            tool_call_id: "tool".into(),
            content: "結果".repeat(3000),
            metadata: None,
        },
        assistant("done"),
    ];
    complete(&database, &first.invocation, messages.clone(), "done");
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(database.history(&session.id).unwrap().history, messages);
    let tools = database
        .tool_uses(&first.invocation.call_id)
        .unwrap()
        .unwrap();
    assert!(tools.calls[0].arguments.contains("[REDACTED]"));
    assert!(!tools.calls[0].arguments.contains("保密"));
    assert!(tools.calls[0].output.as_ref().unwrap().len() <= crate::MAX_TOOL_TEXT_BYTES);
    let second = begin(&database, &session.id, "second");
    let public: Vec<ChatMessage> =
        serde_json::from_value(serde_json::to_value(&messages).unwrap()).unwrap();
    complete(&database, &second.invocation, public, "done");
    let mut connection = storage::connect(database.path()).unwrap();
    let ids = conversation_state_entries::table
        .filter(conversation_state_entries::ordinal.eq(0))
        .select(conversation_state_entries::payload_id)
        .load::<String>(&mut connection)
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "protection is part of payload identity, not just public JSON"
    );
}

#[test]
fn text_only_results_are_rejected_without_advancing_or_appending_to_the_context() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database
        .create_session("default", Some("Legacy system".into()), None)
        .unwrap();
    let legacy = begin(&database, &session.id, "old user");
    assert!(
        database
            .complete_turn(
                legacy.invocation.clone(),
                ConversationResult {
                    text: "old reply".into(),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(
        database
            .conversation_messages(&legacy.invocation.call_id)
            .unwrap()
            .is_none()
    );
    complete(
        &database,
        &legacy.invocation,
        vec![assistant("summary")],
        "summary",
    );
    let later = begin(&database, &session.id, "later user");
    assert!(
        database
            .complete_turn(
                later.invocation.clone(),
                ConversationResult {
                    text: "later reply".into(),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(
        database.history(&session.id).unwrap().history,
        [assistant("summary")]
    );
    let mut messages = later.request.messages.clone();
    messages.push(assistant("later reply"));
    complete(&database, &later.invocation, messages, "later reply");
    let history = database.history(&session.id).unwrap();
    assert_eq!(
        history.history,
        [
            assistant("summary"),
            user("later user"),
            assistant("later reply")
        ]
    );
    assert!(history.system.is_none());
}

#[test]
fn checkpoint_contention_fails_promptly_and_cross_session_updates_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let invocation = begin(&database, &session.id, "prompt").invocation;
    let event = crate::MessageEvent {
        id: "event".into(),
        model_call_id: None,
        messages: vec![assistant("fact")],
    };
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("BEGIN IMMEDIATE;").unwrap();
    let started = std::time::Instant::now();
    assert!(database.record_message_event(&invocation, &event).is_err());
    assert!(
        database
            .record_conversation_state(&invocation, &[assistant("state")])
            .is_err()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    connection.batch_execute("ROLLBACK;").unwrap();
    let mut wrong = invocation.clone();
    wrong.session_id = "another session".into();
    assert!(
        database
            .record_conversation_state(&wrong, &[assistant("state")])
            .is_err()
    );
    assert!(database.record_message_event(&wrong, &event).is_err());
    assert!(
        database
            .conversation_messages(&invocation.call_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conversation_states::table
            .count()
            .get_result::<i64>(&mut connection)
            .unwrap(),
        0
    );
}

#[test]
fn failed_event_checkpoint_keeps_native_identity_for_save_only_recovery_and_later_replay() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let invocation = begin(&database, &session.id, "prompt").invocation;
    let event = crate::MessageEvent {
        id: "original-native-id".into(),
        model_call_id: Some("model".into()),
        messages: vec![assistant("first fact"), assistant("second fact")],
    };
    let mut reducer = crate::ToolUseReducer::new(invocation.clone());
    let batch = reducer.event_batch(event.clone()).unwrap();
    reducer.apply_messages(batch.clone()).unwrap();
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("BEGIN IMMEDIATE;").unwrap();
    assert!(database.record_messages(&invocation, &batch).is_err());
    let pending = reducer.batches();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event_id.as_deref(), Some(event.id.as_str()));
    assert_eq!(pending[0].messages, event.messages);
    connection.batch_execute("ROLLBACK;").unwrap();
    for batch in pending {
        database.record_messages(&invocation, &batch).unwrap();
    }
    for table in [
        "message_event_sources",
        "messages",
        "message_sources",
        "sessions",
    ] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            connection.batch_execute(&format!("CREATE TRIGGER saved_{table}_{operation} BEFORE {operation} ON {table} BEGIN SELECT RAISE(FAIL, 'saved event replay writes'); END;")).unwrap();
        }
    }
    assert_eq!(
        database
            .record_message_event(&invocation, &event)
            .unwrap()
            .event_id
            .as_deref(),
        Some(event.id.as_str())
    );
    assert_eq!(reducer.event_batch(event).unwrap().offset, 0);
}

#[test]
fn producer_event_replay_has_no_flow_supplied_cursor_and_no_writes() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let invocation = begin(&database, &session.id, "prompt").invocation;
    let event = crate::MessageEvent {
        id: "publisher/iteration-0".into(),
        model_call_id: Some("model".into()),
        messages: vec![assistant("event")],
    };
    assert_eq!(
        database
            .record_message_event(&invocation, &event)
            .unwrap()
            .offset,
        0
    );
    let mut connection = storage::connect(database.path()).unwrap();
    for table in [
        "message_event_sources",
        "messages",
        "message_sources",
        "message_streams",
        "sessions",
    ] {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            connection.batch_execute(&format!("CREATE TRIGGER event_{table}_{operation} BEFORE {operation} ON {table} BEGIN SELECT RAISE(FAIL, 'event replay writes'); END;")).unwrap();
        }
    }
    database.record_message_event(&invocation, &event).unwrap();
    let mut conflicting = event.clone();
    conflicting.messages[0] = assistant("different");
    assert!(
        database
            .record_message_event(&invocation, &conflicting)
            .is_err()
    );
}
