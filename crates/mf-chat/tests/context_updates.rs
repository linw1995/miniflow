use diesel::{Connection, connection::SimpleConnection};
use mf_chat::{
    ChatDatabase, ChatError, ChatMessage, ConversationResult, MessageUpdate, ModelIdentity,
    ResultAlternatives,
    contract::{CompletionState, TurnExit},
};
use mf_runtime::InputValue;
use serde_json::json;

fn identity() -> ModelIdentity {
    ModelIdentity {
        profile: "default".into(),
        model: None,
    }
}
fn messages(value: serde_json::Value) -> Vec<ChatMessage> {
    Vec::<ChatMessage>::decode(value.into()).unwrap()
}
fn exchange(prefix: &str) -> Vec<ChatMessage> {
    messages(json!([
        {"role":"assistant","content":"Checking","tool_calls":[
            {"id":format!("{prefix}-one"),"name":"lookup","arguments":{"key":"a"}},
            {"id":format!("{prefix}-two"),"name":"lookup","arguments":["b"]}
        ]},
        {"role":"tool","tool_call_id":format!("{prefix}-two"),"content":"B","metadata":{"original":true}},
        {"role":"tool","tool_call_id":format!("{prefix}-one"),"content":""},
        {"role":"assistant","content":format!("Answer {prefix}"),"metadata":{"source":"original"}}
    ]))
}
fn result(prefix: &str) -> ConversationResult {
    let mut result = ConversationResult::append(exchange(prefix));
    result.usages = serde_json::from_value(json!([
        {"call_id":format!("model-{prefix}-one"),"model":"model","usage":{"output_tokens":2}},
        {"call_id":format!("model-{prefix}-two"),"model":"model"}
    ]))
    .unwrap();
    result
}
fn database() -> (tempfile::TempDir, ChatDatabase, String) {
    let root = tempfile::tempdir().unwrap();
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database
        .create_session("default", Some("Original instructions".into()), None)
        .unwrap();
    (root, database, session.id)
}
fn connection(database: &ChatDatabase) -> diesel::SqliteConnection {
    diesel::SqliteConnection::establish(database.path().to_str().unwrap()).unwrap()
}
fn advance_context(database: &ChatDatabase, id: &str) {
    connection(database).batch_execute(&format!(
        "BEGIN IMMEDIATE; INSERT INTO context_revisions (session_id,revision,parent_revision,operation,snapshot) VALUES ('{id}',1,0,'checkpoint','[]'); UPDATE sessions SET context_revision=1 WHERE id='{id}'; COMMIT;"
    )).unwrap();
}

#[test]
fn append_retains_all_tool_records_and_reopens_the_exact_next_request() {
    let (root, database, id) = database();
    let first = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    assert_eq!(
        first
            .request
            .context
            .as_ref()
            .unwrap()
            .model_context
            .revision,
        0
    );
    assert_eq!(first.request.messages.len(), 2);
    let result = result("first");
    database
        .complete_turn(first.invocation.clone(), result.clone())
        .unwrap();
    let page = database.messages(&id, 50, None).unwrap();
    assert_eq!(page.messages.len(), 5);
    assert_eq!(
        page.messages[1..]
            .iter()
            .map(|record| record.message.payload.clone())
            .collect::<Vec<_>>(),
        exchange("first")
    );
    assert_eq!(
        page.messages
            .iter()
            .filter(|record| record.usages.is_some())
            .count(),
        1
    );
    assert_eq!(database.session(&id).unwrap().next_sequence, 5);
    let context = database.model_context(&id).unwrap();
    assert_eq!(context.revision, 1);
    assert_eq!(
        context.messages,
        result.update.candidate_context(&first.request.messages)
    );
    drop(database);
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    assert_eq!(
        database
            .messages(&id, 50, None)
            .unwrap()
            .messages
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>(),
        page.messages
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>()
    );
    let next = database.begin_turn(&id, "Next", identity(), None).unwrap();
    assert_eq!(
        &next.request.messages[..context.messages.len()],
        context.messages
    );
    assert_eq!(next.request.prompt().unwrap(), "Next");
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        2
    );
}

#[test]
fn replacement_is_exact_and_does_not_rewrite_either_old_or_current_display() {
    let (root, database, id) = database();
    let first = database.begin_turn(&id, "First", identity(), None).unwrap();
    database
        .complete_turn(first.invocation, result("first"))
        .unwrap();
    let old = database.messages(&id, 50, None).unwrap();
    let second = database
        .begin_turn(&id, "Second", identity(), None)
        .unwrap();
    let snapshot = messages(json!([
        {"role":"system","content":"Rewritten instructions"},
        {"role":"user","content":"Summary without either original user or final answer"}
    ]));
    let mut replacement = result("second");
    replacement.update = MessageUpdate::ReplaceContext {
        context_messages: snapshot.clone(),
        display_messages: exchange("second"),
    };
    database
        .complete_turn(second.invocation.clone(), replacement.clone())
        .unwrap();
    assert_eq!(database.model_context(&id).unwrap().messages, snapshot);
    let display = database.messages(&id, 50, None).unwrap();
    assert_eq!(display.messages.len(), 10);
    assert_eq!(
        display.messages[..5]
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>(),
        old.messages
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>()
    );
    assert_eq!(database.session(&id).unwrap().next_sequence, 10);
    assert_eq!(
        database
            .call(&second.invocation.call_id)
            .unwrap()
            .terminal_result,
        Some(replacement)
    );
    drop(database);
    let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let third = database.begin_turn(&id, "Third", identity(), None).unwrap();
    assert_eq!(&third.request.messages[..2], snapshot);
    assert_eq!(third.request.messages.len(), 3);
    assert_eq!(third.request.context.unwrap().model_context.revision, 2);
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        4
    );
}

#[test]
fn empty_replacement_clears_system_context_without_erasing_display() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    let result = ConversationResult {
        update: MessageUpdate::ReplaceContext {
            context_messages: vec![],
            display_messages: exchange("clear"),
        },
        ..result("clear")
    };
    database.complete_turn(begun.invocation, result).unwrap();
    assert!(database.model_context(&id).unwrap().messages.is_empty());
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 5);
    let next = database
        .begin_turn(&id, "Only user", identity(), None)
        .unwrap();
    assert_eq!(
        next.request.messages,
        messages(json!([{"role":"user","content":"Only user"}]))
    );
}

#[test]
fn overlapping_replacement_views_are_validated_independently_and_counted_once() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    let mut result = result("same");
    result.update = MessageUpdate::ReplaceContext {
        context_messages: exchange("same"),
        display_messages: exchange("same"),
    };
    database.complete_turn(begun.invocation, result).unwrap();
    assert_eq!(
        database.model_context(&id).unwrap().messages,
        exchange("same")
    );
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 5);
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        2
    );
}

#[test]
fn failed_replacement_is_audited_and_save_only_retry_keeps_its_full_error() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    let error = "error-".repeat(1000);
    let result = ConversationResult {
        status: "failed".into(),
        update: MessageUpdate::ReplaceContext {
            context_messages: vec![],
            display_messages: exchange("failed")[..2].to_vec(),
        },
        error: Some(error.clone()),
        ..result("failed")
    };
    connection(&database).batch_execute("CREATE TRIGGER fail_result BEFORE UPDATE OF terminal_result ON model_calls BEGIN SELECT RAISE(ABORT, 'failed result write'); END;").unwrap();
    assert!(
        database
            .complete_turn(begun.invocation.clone(), result.clone())
            .is_err()
    );
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 1);
    assert_eq!(database.session(&id).unwrap().next_sequence, 1);
    let resolution = database
        .reconcile_result(
            &begun.invocation,
            Some(Ok(result.clone())),
            TurnExit::Failed,
            "save failure",
        )
        .unwrap();
    let CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected full failed candidate")
    };
    assert_eq!(pending.completion.result, result);
    connection(&database)
        .batch_execute("DROP TRIGGER fail_result;")
        .unwrap();
    assert_eq!(
        database.save_recovered(pending.clone()).unwrap().result,
        result
    );
    database.save_recovered(pending).unwrap();
    let call = database.call(&begun.invocation.call_id).unwrap();
    assert_eq!(call.error.as_ref().unwrap().len(), 4096);
    assert_eq!(call.terminal_result.unwrap().error, Some(error));
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 3);
    assert_eq!(database.model_context(&id).unwrap().revision, 0);
    assert_eq!(database.model_context(&id).unwrap().messages.len(), 1);
    assert_eq!(database.session(&id).unwrap().title_source, "provisional");
}

#[test]
fn stale_success_rolls_back_and_cannot_be_rebased_by_save_retry() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    advance_context(&database, &id);
    let result = result("stale");
    assert!(matches!(
        database.complete_turn(begun.invocation.clone(), result.clone()),
        Err(ChatError::ContextConflict {
            expected: 0,
            actual: 1,
            ..
        })
    ));
    let resolution = database
        .reconcile_result(
            &begun.invocation,
            Some(Ok(result.clone())),
            TurnExit::Failed,
            "stale",
        )
        .unwrap();
    let CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected original retained result")
    };
    assert!(
        matches!(database.save_recovered(pending), Err(ChatError::Uncommitted { source, .. }) if matches!(*source, ChatError::ContextConflict { .. }))
    );
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 1);
    assert_eq!(database.session(&id).unwrap().next_sequence, 1);
    assert_eq!(
        database.turn(&begun.invocation.turn_id).unwrap().state,
        "running"
    );
    assert!(
        database
            .call(&begun.invocation.call_id)
            .unwrap()
            .terminal_result
            .is_none()
    );
    assert!(database.model_context(&id).unwrap().messages.is_empty());
}

#[test]
fn failed_audit_can_settle_after_context_changes_without_publication() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    advance_context(&database, &id);
    let result = ConversationResult {
        status: "failed".into(),
        error: Some("Failure".into()),
        ..result("failed")
    };
    database
        .complete_turn(begun.invocation.clone(), result)
        .unwrap();
    assert_eq!(
        database.turn(&begun.invocation.turn_id).unwrap().state,
        "failed"
    );
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 5);
    assert_eq!(database.model_context(&id).unwrap().revision, 1);
    assert!(database.model_context(&id).unwrap().messages.is_empty());
}

#[test]
fn replay_after_newer_turns_never_republishes_context_and_changed_variants_conflict() {
    let (_root, database, id) = database();
    let first = database.begin_turn(&id, "First", identity(), None).unwrap();
    let original = result("first");
    database
        .complete_turn(first.invocation.clone(), original.clone())
        .unwrap();
    let next = database.begin_turn(&id, "Next", identity(), None).unwrap();
    let later = ConversationResult {
        update: MessageUpdate::ReplaceContext {
            context_messages: vec![],
            display_messages: exchange("later"),
        },
        ..result("later")
    };
    database.complete_turn(next.invocation, later).unwrap();
    database
        .complete_turn(first.invocation.clone(), original.clone())
        .unwrap();
    assert_eq!(database.model_context(&id).unwrap().revision, 2);
    assert!(database.model_context(&id).unwrap().messages.is_empty());
    let changed = ConversationResult {
        update: MessageUpdate::ReplaceContext {
            context_messages: original.update.candidate_context(&first.request.messages),
            display_messages: exchange("first"),
        },
        ..original
    };
    assert!(matches!(
        database.complete_turn(first.invocation, changed),
        Err(ChatError::Conflict { .. })
    ));
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 10);
}

#[test]
fn admission_pins_result_alternatives_without_reopening_a_manifest() {
    let (_root, database, id) = database();
    let invocation = mf_chat::Invocation::new(&id);
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let node = registry
        .get("mf_chat.turn.begin")
        .unwrap()
        .instantiate(json!({}))
        .unwrap();
    let input = mf_chat::nodes::BeginTurnInputs {
        database_path: database.path().to_str().unwrap().into(),
        session_id: id.clone(),
        turn_id: invocation.turn_id.clone(),
        call_id: invocation.call_id.clone(),
        prompt: "Question".into(),
        model: identity(),
        retry_of: None,
        result_alternatives: Some(ResultAlternatives::Append),
    };
    use mf_runtime::NodeValue;
    node.execution
        .as_task_node()
        .unwrap()
        .execute(
            input.into_values().unwrap(),
            &mut mf_runtime::ExecutionContext::default(),
        )
        .unwrap();
    let replacement = ConversationResult {
        update: MessageUpdate::ReplaceContext {
            context_messages: vec![],
            display_messages: exchange("undeclared"),
        },
        ..result("undeclared")
    };
    assert!(matches!(
        database.complete_turn(invocation, replacement),
        Err(ChatError::Invalid {
            field: "workflow result",
            ..
        })
    ));
    assert_eq!(database.model_context(&id).unwrap().revision, 0);
    assert_eq!(database.messages(&id, 50, None).unwrap().messages.len(), 1);
}

#[test]
fn save_only_retry_never_reinvokes_a_backend_for_either_result_status() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Backend {
        calls: AtomicUsize,
        result: ConversationResult,
    }
    impl mf_chat::ConversationBackend for Backend {
        fn invoke(
            &self,
            _: &mf_chat::Invocation,
            _: mf_chat::ConversationInputs,
            _: &mut mf_runtime::ExecutionContext,
            _: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), ChatError>>,
            _: Option<&mf_telemetry::observation::Observer>,
        ) -> Result<ConversationResult, ChatError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.result.clone())
        }
    }
    for failed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut response = result("retry");
        response.update = MessageUpdate::ReplaceContext {
            context_messages: vec![],
            display_messages: exchange("retry"),
        };
        if failed {
            response.status = "failed".into();
            response.error = Some("Failure".into());
        }
        let backend = Arc::new(Backend {
            calls: AtomicUsize::new(0),
            result: response.clone(),
        });
        let profile = mf_chat::WorkflowProfile::with_backend("default", backend.clone()).unwrap();
        let host = mf_chat::ChatHost::open(
            root.path().join("chat.sqlite"),
            mf_chat::HostConfig::new(profile),
        )
        .unwrap();
        let id = host
            .database()
            .create_session("default", None, None)
            .unwrap()
            .id;
        connection(host.database()).batch_execute("CREATE TRIGGER fail_terminal BEFORE UPDATE OF terminal_result ON model_calls BEGIN SELECT RAISE(ABORT, 'save failed'); END;").unwrap();
        let error = host.submit(&id, "Question").unwrap().join().unwrap_err();
        let ChatError::Uncommitted { pending, .. } = error else {
            panic!("expected save-only candidate")
        };
        assert_eq!(pending.completion.result, response);
        connection(host.database())
            .batch_execute("DROP TRIGGER fail_terminal;")
            .unwrap();
        let saved = host.retry_completion(*pending).unwrap();
        assert_eq!(saved.result, response);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            host.database().model_context(&id).unwrap().revision,
            i64::from(!failed)
        );
        assert_eq!(
            host.database()
                .messages(&id, 50, None)
                .unwrap()
                .messages
                .len(),
            5
        );
        host.close().unwrap();
    }
}

#[test]
fn admission_does_not_turn_a_configured_model_hint_into_an_attempt() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(
            &id,
            "Question",
            ModelIdentity {
                profile: "default".into(),
                model: Some("configured-model".into()),
            },
            None,
        )
        .unwrap();
    let statistics = database.usage(Default::default()).unwrap();
    assert_eq!(statistics.overall.attempted_calls, 0);
    assert_eq!(statistics.unreported_invocations, 1);
    database
        .fail_turn(begun.invocation, "interrupted", "No terminal result")
        .unwrap();
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        0
    );
}

#[test]
fn pagination_and_manual_title_changes_do_not_change_model_context_or_totals() {
    let (_root, database, id) = database();
    let begun = database
        .begin_turn(&id, "Question", identity(), None)
        .unwrap();
    database.rename_session(&id, "Manual name").unwrap();
    let mut result = result("pages");
    result.title = Some("Stale generated name".into());
    database.complete_turn(begun.invocation, result).unwrap();
    assert_eq!(database.session(&id).unwrap().title, "Manual name");
    let context = database.model_context(&id).unwrap();
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = database.messages(&id, 1, cursor).unwrap();
        assert!(page.messages[0].usages.is_some());
        ids.push(page.messages[0].message.id.clone());
        cursor = page.next_after;
        if cursor.is_none() {
            break;
        }
    }
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 5);
    assert_eq!(context, database.model_context(&id).unwrap());
    assert_eq!(
        database
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        2
    );
}
