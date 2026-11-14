use super::*;
use crate::{
    ConversationResult, ModelIdentity, ToolOrigin, ToolUse, ToolUseHistory, ToolUsePhase,
    ToolUseReducer,
};
use diesel::connection::SimpleConnection;
use serde_json::json;

fn begin(database: &ChatDatabase) -> Invocation {
    let session = database.create_session("default", None, None).unwrap();
    database
        .begin_turn(
            &session.id,
            "calculate",
            ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
        )
        .unwrap()
        .invocation
}
fn request(id: &str, content: &str) -> ChatMessage {
    ChatMessage::Assistant {
        content: content.into(),
        tool_calls: Some(vec![mfn_llm::ToolCall {
            id: id.into(),
            name: "calculator".into(),
            arguments: json!({"input": "界".repeat(3000)}).into(),
        }]),
        metadata: Some(BTreeMap::from([(
            "source".into(),
            json!({"nested": [1, "kept"]}).into(),
        )])),
    }
}
fn result(id: &str) -> ChatMessage {
    ChatMessage::ToolResult {
        tool_call_id: id.into(),
        content: "result".repeat(1500),
        metadata: Some(BTreeMap::from([("format".into(), "full".into())])),
    }
}
fn final_message() -> ChatMessage {
    ChatMessage::Assistant {
        content: "20".into(),
        tool_calls: Some(Vec::new()),
        metadata: Some(BTreeMap::new()),
    }
}
fn execution(id: &str, model: &str, tool: &str, phase: ToolUsePhase) -> ToolUse {
    ToolUse {
        origin: Some(ToolOrigin {
            model_call_id: model.into(),
            tool_call_id: tool.into(),
        }),
        id: id.into(),
        name: "observed-calculator".into(),
        phase,
        arguments: "resolved arguments".into(),
        output: "observed result differs from returned content".into(),
        duration_ns: Some(100),
    }
}
fn observations(invocation: &Invocation, runs: Vec<ToolUse>) -> ToolUseHistory {
    ToolUseHistory {
        invocation: invocation.clone(),
        calls: Vec::new(),
        runs,
        incomplete: false,
        finished: false,
    }
}

#[test]
fn canonical_arguments_and_metadata_retain_sensitivity_without_exposing_it_in_tool_previews() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let invocation = begin(&database);
    let mut assistant = request("secret", "");
    if let ChatMessage::Assistant {
        tool_calls: Some(calls),
        metadata,
        ..
    } = &mut assistant
    {
        calls[0].arguments = ValueRef::object([
            (
                "private".into(),
                ValueRef::from("protected argument").into_secret(),
            ),
            ("public".into(), 12.into()),
        ]);
        *metadata = Some(BTreeMap::from([(
            "private".into(),
            ValueRef::from("protected metadata").into_secret(),
        )]));
    }
    let messages = vec![assistant, result("secret"), final_message()];
    database
        .complete_turn(
            invocation.clone(),
            ConversationResult {
                text: "20".into(),
                messages: Some(messages),
                ..Default::default()
            },
        )
        .unwrap();
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    let restored = database
        .conversation_messages(&invocation.call_id)
        .unwrap()
        .unwrap();
    let ChatMessage::Assistant {
        tool_calls: Some(calls),
        metadata: Some(metadata),
        ..
    } = &restored[0]
    else {
        panic!("expected full restored assistant")
    };
    assert_eq!(
        calls[0].arguments["private"].as_str(),
        Some("protected argument")
    );
    assert!(calls[0].arguments["private"].is_secret());
    assert!(metadata["private"].is_secret());
    let tools = database.tool_uses(&invocation.call_id).unwrap().unwrap();
    assert!(tools.calls[0].arguments.contains("[REDACTED]"));
    assert!(!tools.calls[0].arguments.contains("protected argument"));
    let history = database.history(&invocation.session_id).unwrap();
    assert!(
        matches!(&history.history[1], ChatMessage::Assistant { tool_calls: Some(calls), .. } if calls[0].arguments.contains_secret())
    );
}

#[test]
fn legacy_json_migrates_as_observations_without_invented_messages_or_origins() {
    use diesel_migrations::MigrationHarness;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let invocation = begin(&database);
    let mut history = observations(
        &invocation,
        vec![execution(
            "legacy",
            "unused",
            "unused",
            ToolUsePhase::Succeeded,
        )],
    );
    history.runs[0].origin = None;
    history.finished = true;
    database.record_tool_uses(&invocation, &history).unwrap();
    database
        .complete_turn(
            invocation.clone(),
            ConversationResult {
                text: "legacy answer".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut connection = storage::connect(&path).unwrap();
    connection
        .revert_last_migration(storage::MIGRATIONS)
        .unwrap();
    drop(connection);
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(
        database.tool_uses(&invocation.call_id).unwrap(),
        Some(history)
    );
    assert!(
        database
            .conversation_messages(&invocation.call_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database
            .history(&invocation.session_id)
            .unwrap()
            .history
            .len(),
        2
    );
    let mut connection = storage::connect(&path).unwrap();
    assert!(
        connection
            .batch_execute("SELECT tool_uses FROM model_calls;")
            .is_err()
    );
}

#[test]
fn failed_results_keep_canonical_messages_and_scalar_rejected_arguments_outside_model_context() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let mut message = request("bad", "");
    if let ChatMessage::Assistant {
        tool_calls: Some(calls),
        ..
    } = &mut message
    {
        calls[0].arguments = json!("rejected non-object arguments").into();
    }
    let messages = vec![message, result("bad")];
    let response = ConversationResult {
        status: "failed".into(),
        error: Some("provider stopped".into()),
        messages: Some(messages.clone()),
        ..Default::default()
    };
    let resolution = database
        .reconcile_result(
            &invocation,
            Some(Ok(response.clone())),
            crate::contract::TurnExit::Failed,
            "failed",
        )
        .unwrap();
    assert!(
        matches!(resolution.state, crate::contract::CompletionState::Finished(ref turn) if turn.state == "failed")
    );
    assert_eq!(
        database.conversation_messages(&invocation.call_id).unwrap(),
        Some(messages)
    );
    assert!(
        database
            .history(&invocation.session_id)
            .unwrap()
            .history
            .is_empty()
    );
    assert!(
        database
            .reconcile_result(
                &invocation,
                Some(Ok(response)),
                crate::contract::TurnExit::Failed,
                "failed"
            )
            .unwrap()
            .diagnostics
            .is_empty()
    );
}

#[test]
fn full_messages_round_trip_and_next_turn_receives_all_successful_context() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let invocation = begin(&database);
    let messages = vec![
        request("reused", ""),
        result("reused"),
        request("reused", "another model round"),
        result("reused"),
        final_message(),
    ];
    for (offset, model, range) in [
        (0, "model-one", 0..2),
        (2, "model-two", 2..4),
        (4, "model-three", 4..5),
    ] {
        database
            .record_messages(
                &invocation,
                &MessageBatch {
                    offset,
                    model_call_id: Some(model.into()),
                    messages: messages[range].to_vec(),
                },
            )
            .unwrap();
    }
    let response = ConversationResult {
        text: "20".into(),
        messages: Some(messages.clone()),
        ..Default::default()
    };
    assert_eq!(
        ConversationResult::from_json(&serde_json::to_vec(&response).unwrap()).unwrap(),
        response
    );
    let completion = database
        .complete_turn(invocation.clone(), response.clone())
        .unwrap();
    assert_eq!(completion.result, response);
    assert_eq!(
        database
            .complete_turn(invocation.clone(), response.clone())
            .unwrap(),
        completion
    );
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(
        database.conversation_messages(&invocation.call_id).unwrap(),
        Some(messages.clone())
    );
    let page = database
        .messages(&invocation.session_id, 100, None)
        .unwrap();
    assert_eq!(page.messages.len(), messages.len() + 1);
    assert_eq!(
        page.messages[1..]
            .iter()
            .map(|record| record.chat_message.clone())
            .collect::<Vec<_>>(),
        messages
    );
    let next = database
        .begin_turn(
            &invocation.session_id,
            "continue",
            ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
        )
        .unwrap();
    let expected = std::iter::once(ChatMessage::User {
        content: "calculate".into(),
        metadata: None,
    })
    .chain(messages.clone())
    .collect::<Vec<_>>();
    assert_eq!(next.request.history.as_deref(), Some(expected.as_slice()));
    let crate::contract::InvocationState::Committed(saved) =
        database.inspect_invocation(&invocation).unwrap()
    else {
        panic!("expected committed message recovery")
    };
    assert_eq!(saved.result, response);
}

#[test]
fn exact_replays_do_not_write_messages_payloads_relations_or_cursors() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let batch = MessageBatch {
        offset: 0,
        model_call_id: Some("model".into()),
        messages: vec![request("one", ""), result("one")],
    };
    database.record_messages(&invocation, &batch).unwrap();
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("CREATE TRIGGER no_message_insert BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT, 'duplicate message'); END;
        CREATE TRIGGER no_message_update BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'message rewrite'); END;
        CREATE TRIGGER no_call_write BEFORE UPDATE ON chat_tool_calls BEGIN SELECT RAISE(ABORT, 'call rewrite'); END;
        CREATE TRIGGER no_cursor_write BEFORE UPDATE ON message_sources BEGIN SELECT RAISE(ABORT, 'cursor rewrite'); END;
        CREATE TRIGGER no_order_write BEFORE UPDATE ON sessions BEGIN SELECT RAISE(ABORT, 'order rewrite'); END;").unwrap();
    database.record_messages(&invocation, &batch).unwrap();
    assert_eq!(
        database.conversation_messages(&invocation.call_id).unwrap(),
        Some(batch.messages)
    );
}

#[test]
fn status_only_changes_never_rewrite_execution_payloads_or_siblings_and_replay_is_zero_write() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let mut history = observations(
        &invocation,
        vec![
            execution("first", "model", "one", ToolUsePhase::Running),
            execution("sibling", "model", "two", ToolUsePhase::Succeeded),
        ],
    );
    database.record_tool_uses(&invocation, &history).unwrap();
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("CREATE TRIGGER no_payload_write BEFORE UPDATE OF name, arguments, output, duration_ns ON tool_executions BEGIN SELECT RAISE(ABORT, 'payload rewrite'); END;
        CREATE TRIGGER no_sibling_write BEFORE UPDATE ON tool_executions WHEN OLD.id = 'sibling' BEGIN SELECT RAISE(ABORT, 'sibling rewrite'); END;
        CREATE TRIGGER no_model_write BEFORE UPDATE ON model_calls BEGIN SELECT RAISE(ABORT, 'model-call rewrite'); END;").unwrap();
    history.runs[0].phase = ToolUsePhase::Succeeded;
    database.record_tool_uses(&invocation, &history).unwrap();
    connection.batch_execute("CREATE TRIGGER no_execution_write BEFORE UPDATE ON tool_executions BEGIN SELECT RAISE(ABORT, 'replay write'); END;
        CREATE TRIGGER no_source_write BEFORE UPDATE ON tool_use_sources BEGIN SELECT RAISE(ABORT, 'source replay write'); END;
        CREATE TRIGGER no_execution_insert BEFORE INSERT ON tool_executions BEGIN SELECT RAISE(ABORT, 'duplicate execution'); END;").unwrap();
    database.record_tool_uses(&invocation, &history).unwrap();
}

#[test]
fn explicit_identities_link_both_arrival_orders_multiple_attempts_and_keep_conflicts() {
    for messages_first in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let invocation = begin(&database);
        let batch = MessageBatch {
            offset: 0,
            model_call_id: Some("model-one".into()),
            messages: vec![request("reused", ""), result("reused")],
        };
        let history = observations(
            &invocation,
            vec![
                execution("attempt-one", "model-one", "reused", ToolUsePhase::Failed),
                execution(
                    "attempt-two",
                    "model-one",
                    "reused",
                    ToolUsePhase::Succeeded,
                ),
                execution("unlinked", "other-model", "reused", ToolUsePhase::Succeeded),
            ],
        );
        if messages_first {
            database.record_messages(&invocation, &batch).unwrap();
        }
        database.record_tool_uses(&invocation, &history).unwrap();
        if !messages_first {
            database.record_messages(&invocation, &batch).unwrap();
        }
        let merged = database.tool_uses(&invocation.call_id).unwrap().unwrap();
        assert_eq!(merged.calls.len(), 1);
        assert_eq!(
            merged.calls[0].execution_ids,
            ["attempt-one", "attempt-two"]
        );
        assert_eq!(merged.calls[0].name, "calculator");
        assert!(
            merged.calls[0]
                .output
                .as_ref()
                .unwrap()
                .starts_with("result")
        );
        assert_eq!(merged.runs[0].phase, ToolUsePhase::Failed);
        assert_eq!(merged.runs[1].phase, ToolUsePhase::Succeeded);
        assert_eq!(merged.runs[0].name, "observed-calculator");
        assert_ne!(
            merged.calls[0].output.as_ref().unwrap(),
            &merged.runs[1].output
        );
        let other = begin(&database);
        database
            .record_tool_uses(&other, &observations(&other, history.runs.clone()))
            .unwrap();
        assert!(
            database
                .tool_uses(&other.call_id)
                .unwrap()
                .unwrap()
                .calls
                .is_empty()
        );
        let changed = diesel::update(tool_executions::table.find((&other.call_id, "attempt-one")))
            .set(tool_executions::semantic_call_id.eq(&merged.calls[0].id))
            .execute(&mut storage::connect(database.path()).unwrap());
        assert!(
            changed.is_err(),
            "a foreign key must reject cross-conversation links"
        );
    }
}

#[test]
fn ambiguous_late_identity_keeps_both_messages_and_execution_without_a_guessed_link() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let mut reducer = ToolUseReducer::new(invocation.clone());
    let run = execution(
        "execution",
        "ambiguous-model",
        "reused",
        ToolUsePhase::Succeeded,
    );
    reducer.observe(vec![run.clone()], false);
    database
        .record_tool_uses(&invocation, &observations(&invocation, vec![run]))
        .unwrap();
    for offset in [0, 2] {
        let batch = MessageBatch {
            offset,
            model_call_id: Some("ambiguous-model".into()),
            messages: vec![request("reused", ""), result("reused")],
        };
        reducer.apply_messages(batch.clone()).unwrap();
        database.record_messages(&invocation, &batch).unwrap();
        assert_eq!(
            database.tool_uses(&invocation.call_id).unwrap().as_ref(),
            Some(reducer.history())
        );
    }
    let merged = database.tool_uses(&invocation.call_id).unwrap().unwrap();
    assert_eq!(merged.calls.len(), 2);
    assert_eq!(merged.runs.len(), 1);
    assert!(
        merged
            .calls
            .iter()
            .all(|call| call.execution_ids.is_empty())
    );
    assert_eq!(
        database
            .conversation_messages(&invocation.call_id)
            .unwrap()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn messages_without_origins_do_not_guess_an_execution_from_name_arguments_or_tool_id() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let batch = MessageBatch {
        offset: 0,
        model_call_id: None,
        messages: vec![request("same", ""), result("same")],
    };
    database.record_messages(&invocation, &batch).unwrap();
    let history = observations(
        &invocation,
        vec![execution("actual", "model", "same", ToolUsePhase::Running)],
    );
    database.record_tool_uses(&invocation, &history).unwrap();
    let merged = database.tool_uses(&invocation.call_id).unwrap().unwrap();
    assert!(merged.calls[0].execution_ids.is_empty());
    database
        .record_messages(
            &invocation,
            &MessageBatch {
                model_call_id: Some("model".into()),
                ..batch
            },
        )
        .unwrap();
    assert_eq!(
        database
            .tool_uses(&invocation.call_id)
            .unwrap()
            .unwrap()
            .calls[0]
            .execution_ids,
        ["actual"]
    );
}

#[test]
fn successful_settlement_is_atomic_and_retry_retains_the_complete_original_messages() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let messages = vec![request("one", ""), result("one"), final_message()];
    let response = ConversationResult {
        text: "20".into(),
        messages: Some(messages.clone()),
        ..Default::default()
    };
    let mut connection = storage::connect(database.path()).unwrap();
    connection.batch_execute("CREATE TRIGGER fail_settlement BEFORE UPDATE ON model_calls WHEN NEW.state = 'succeeded' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(
        database
            .complete_turn(invocation.clone(), response.clone())
            .is_err()
    );
    assert!(
        database
            .conversation_messages(&invocation.call_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database
            .messages(&invocation.session_id, 100, None)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(database.turn(&invocation.turn_id).unwrap().state, "running");
    connection
        .batch_execute("DROP TRIGGER fail_settlement;")
        .unwrap();
    let pending = crate::UncommittedCompletion {
        database_path: database.path().to_owned(),
        completion: crate::Completion {
            invocation: invocation.clone(),
            result: response,
        },
    };
    assert_eq!(
        database.save_recovered(pending).unwrap().result.messages,
        Some(messages)
    );
}

#[test]
fn canonical_messages_are_not_limited_by_observation_windows_and_history_pages_keep_tool_links() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let tools = (0..130)
        .map(|index| mfn_llm::ToolCall {
            id: format!("id-{index}"),
            name: "calculator".into(),
            arguments: json!({"index": index}).into(),
        })
        .collect();
    let mut messages = vec![ChatMessage::Assistant {
        content: String::new(),
        tool_calls: Some(tools),
        metadata: None,
    }];
    messages.extend((0..130).map(|index| result(&format!("id-{index}"))));
    messages.push(final_message());
    database
        .complete_turn(
            invocation.clone(),
            ConversationResult {
                text: "20".into(),
                messages: Some(messages.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        database.conversation_messages(&invocation.call_id).unwrap(),
        Some(messages.clone())
    );
    assert_eq!(
        &database.history(&invocation.session_id).unwrap().history[1..],
        &messages
    );
    let tools = database.tool_uses(&invocation.call_id).unwrap().unwrap();
    assert_eq!(tools.calls.len(), crate::MAX_TOOL_USES);
    assert!(tools.incomplete);
}

#[test]
fn interrupted_message_prefixes_survive_restart_but_never_enter_successful_context() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let invocation = begin(&database);
    let batch = MessageBatch {
        offset: 0,
        model_call_id: Some("model".into()),
        messages: vec![request("one", "")],
    };
    database.record_messages(&invocation, &batch).unwrap();
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    assert_eq!(
        database.turn(&invocation.turn_id).unwrap().state,
        "interrupted"
    );
    assert_eq!(
        database.conversation_messages(&invocation.call_id).unwrap(),
        Some(batch.messages)
    );
    assert_eq!(
        database
            .messages(&invocation.session_id, 100, None)
            .unwrap()
            .messages
            .len(),
        2
    );
    assert!(
        database
            .history(&invocation.session_id)
            .unwrap()
            .history
            .is_empty()
    );
}

#[test]
fn reducer_and_storage_agree_for_out_of_order_delivery_and_neither_source_finishes_the_other() {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let invocation = begin(&database);
    let mut reducer = ToolUseReducer::new(invocation.clone());
    let returned = MessageBatch {
        offset: 1,
        model_call_id: Some("model".into()),
        messages: vec![result("one")],
    };
    assert!(!reducer.apply_messages(returned).unwrap());
    reducer
        .apply_messages(MessageBatch {
            offset: 0,
            model_call_id: Some("model".into()),
            messages: vec![request("one", "")],
        })
        .unwrap();
    assert!(reducer.history().calls[0].output.is_some());
    assert!(reducer.history().calls[0].execution_ids.is_empty());
    reducer.observe(
        vec![execution("first", "model", "one", ToolUsePhase::Running)],
        false,
    );
    assert_eq!(
        reducer.history().runs[0].phase,
        ToolUsePhase::Running,
        "a returned tool message is not observed success"
    );
    reducer.finish(false);
    reducer
        .apply_messages(MessageBatch {
            offset: 2,
            model_call_id: Some("final-model".into()),
            messages: vec![final_message()],
        })
        .unwrap();
    reducer.observe(
        vec![execution("first", "model", "one", ToolUsePhase::Succeeded)],
        false,
    );
    for batch in reducer.batches() {
        database.record_messages(&invocation, &batch).unwrap();
    }
    database
        .record_tool_uses(&invocation, reducer.history())
        .unwrap();
    assert_eq!(
        database.tool_uses(&invocation.call_id).unwrap().as_ref(),
        Some(reducer.history())
    );
}
