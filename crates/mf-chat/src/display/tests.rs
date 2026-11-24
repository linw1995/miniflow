use super::*;
use crate::{ConversationResult, ModelIdentity, PreviewDelta, TextDelta, ToolCallDelta};
use serde_json::json;

#[test]
fn index_reads_only_the_requested_saved_field_and_does_not_own_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let db = ChatDatabase::open(&path).unwrap();
    let session = db.create_session("default", None, None).unwrap();
    let begun = db
        .begin_turn(
            &session.id,
            "ask",
            ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
        )
        .unwrap();
    let raw = "A\0é\n".repeat(48 * 1024);
    let args = json!({"text": raw, "nested": [null, 42, false]});
    let result = ConversationResult::append(serde_json::from_value(json!([
        {"role":"assistant", "content":"before", "tool_calls":[{"id":"first", "name":"search", "arguments":args}, {"id":"second", "name":"show", "arguments":{}}]},
        {"role":"tool", "tool_call_id":"second", "content":raw},
        {"role":"tool", "tool_call_id":"first", "content":""},
        {"role":"assistant", "content":"complete"}
    ])).unwrap());
    db.complete_turn(begun.invocation.clone(), result).unwrap();
    let page = db.display_page(&session.id, 2, Some(0)).unwrap();
    assert_eq!(page.messages.len(), 2);
    let Message::Assistant { content, tools } = &page.messages[0].message else {
        panic!("expected assistant");
    };
    let Message::ToolResult {
        id,
        content: output,
        bytes,
    } = &page.messages[1].message
    else {
        panic!("expected result");
    };
    assert_eq!(id, "second");
    assert_eq!(*bytes, raw.len());
    assert_eq!(tools[0].id, "first");
    assert_eq!(tools[1].name, "show");
    let reply = db.display_reply(&begun.invocation, 4).unwrap();
    assert!(db.display_reply(&begun.invocation, 3).is_err());
    assert_eq!(reply.last().unwrap().content().load().unwrap(), "complete");
    drop(db);
    let _reopened = ChatDatabase::open(&path).unwrap();
    assert_eq!(output.load().unwrap(), raw);
    // Mutate the fixture after indexing: an eager page or a body copied from the
    // settlement would still return the old text. Other fields must be independent.
    diesel::sql_query(
        "UPDATE messages SET payload = json_set(payload, '$.content', 'after') WHERE id = ?",
    )
    .bind::<sql_types::Text, _>(&*page.messages[0].id)
    .execute(&mut storage::connect(&path).unwrap())
    .unwrap();
    assert_eq!(content.load().unwrap(), "after");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&tools[0].arguments.load().unwrap()).unwrap(),
        args
    );
    assert_eq!(tools[1].arguments.load().unwrap(), "{}");
}

fn model() -> ModelIdentity {
    ModelIdentity {
        profile: "default".into(),
        model: None,
    }
}

fn text(call: &str, value: &str) -> PreviewDelta {
    TextDelta {
        model_call_id: call.into(),
        text: value.into(),
    }
    .into()
}

fn tool(call: &str, index: u64, id: Option<&str>, value: &str) -> PreviewDelta {
    ToolCallDelta {
        model_call_id: call.into(),
        index,
        id: id.map(str::to_owned),
        name: id.map(|_| "lookup".into()),
        arguments: value.into(),
    }
    .into()
}

fn chunk_count(path: &Path) -> i64 {
    diesel::select(sql::<sql_types::BigInt>(
        "(SELECT count(*) FROM preview_chunks)",
    ))
    .get_result(&mut storage::connect(path).unwrap())
    .unwrap()
}

#[test]
fn recovered_drafts_survive_later_turns_without_entering_requests_or_audit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let db = ChatDatabase::open(&path).unwrap();
    let session = db.create_session("default", None, None).unwrap().id;
    let first = db
        .begin_turn(&session, "first question", model(), None)
        .unwrap()
        .invocation;
    let mut writer = DraftWriter::open(&db, first.clone()).unwrap();
    let first_frame = writer
        .append(&[
            text("a", "A\0é"),
            tool("a", u64::MAX, None, "{ "),
            tool("b", 0, None, "["),
            text("c", "later"),
        ])
        .unwrap();
    writer
        .append(&[
            tool("a", u64::MAX, Some("late-id"), ""),
            text("a", " tail"),
            tool("a", u64::MAX, None, "\"key\":"),
        ])
        .unwrap();
    assert_eq!(first_frame[0].text.load().unwrap(), "A\0é");
    assert_eq!(first_frame[1].text.load().unwrap(), "{ ");
    assert_eq!(
        chunk_count(&path),
        6,
        "hints are metadata, not extra bodies"
    );
    assert!(
        db.display_page(&session, 10, None).unwrap().messages[0]
            .unfinished
            .is_empty()
    );
    drop((first_frame, writer, db));

    let db = ChatDatabase::open(&path).unwrap();
    assert_eq!(db.turn(&first.turn_id).unwrap().state, "interrupted");
    let retry = db
        .begin_turn(&session, "retry question", model(), Some(first.turn_id))
        .unwrap();
    assert_eq!(retry.request.messages.len(), 1);
    assert_eq!(retry.request.prompt().unwrap(), "retry question");
    let mut writer = DraftWriter::open(&db, retry.invocation.clone()).unwrap();
    writer.append(&[text("a", "retry prefix")]).unwrap();
    db.fail_turn(retry.invocation, "cancelled", "stopped")
        .unwrap();
    let next = db
        .begin_turn(&session, "next question", model(), None)
        .unwrap();
    db.complete_turn(next.invocation, ConversationResult::assistant("final"))
        .unwrap();
    drop((writer, db));

    let db = ChatDatabase::open(path).unwrap();
    let page = db.display_page(&session, 10, None).unwrap();
    let [one, two, three, four] = page.messages.as_slice() else {
        panic!("expected four audit rows");
    };
    assert_eq!(one.turn_state, "interrupted");
    assert_eq!(
        one.unfinished
            .iter()
            .map(|s| s.text.load().unwrap())
            .collect::<Vec<_>>(),
        ["A\0é tail", "{ \"key\":", "[", "later"]
    );
    let call = one.unfinished[1].tool.as_ref().unwrap();
    assert_eq!(
        (call.index, call.id.as_deref(), call.name.as_deref()),
        (u64::MAX, Some("late-id"), Some("lookup"))
    );
    assert_eq!(two.turn_state, "cancelled");
    assert_eq!(two.unfinished[0].text.load().unwrap(), "retry prefix");
    assert!(three.unfinished.is_empty() && four.unfinished.is_empty());
    assert_eq!(db.messages(&session, 10, None).unwrap().messages.len(), 4);
    assert_eq!(db.model_context(&session).unwrap().messages.len(), 2);
    assert!(db.call(&first.call_id).unwrap().terminal_result.is_none());
}

#[test]
fn failed_completion_removes_only_replaced_segments_and_attaches_the_rest_to_the_actual_tail() {
    let dir = tempfile::tempdir().unwrap();
    let db = ChatDatabase::open(dir.path().join("chat.sqlite")).unwrap();
    let session = db.create_session("default", None, None).unwrap().id;
    let inv = db
        .begin_turn(&session, "question", model(), None)
        .unwrap()
        .invocation;
    let mut writer = DraftWriter::open(&db, inv.clone()).unwrap();
    writer
        .append(&[
            text("completed", "postprocessed"),
            text("partial", "saved"),
            text("unknown", "unrepresented"),
            tool("partial", 0, Some("covered"), "{ \"preview\":"),
            tool("partial", 1, None, "["),
        ])
        .unwrap();
    let mut result = ConversationResult::append(serde_json::from_value(json!([
        {"role":"assistant","content":"saved text","tool_calls":[{"id":"covered","name":"lookup","arguments":{"canonical":true}}]},
        {"role":"tool","tool_call_id":"covered","content":"value"}
    ])).unwrap());
    result.status = "failed".into();
    result.error = Some("provider failed".into());
    result.usages = serde_json::from_value(json!([{"call_id":"completed","model":"fixture"},{"call_id":"partial","model":"fixture","error":"provider failed"}])).unwrap();
    db.complete_turn(inv.clone(), result.clone()).unwrap();
    db.complete_turn(inv, result).unwrap();
    assert_eq!(chunk_count(db.path()), 2);
    let page = db.display_page(&session, 2, None).unwrap();
    assert!(
        page.messages
            .iter()
            .all(|record| record.unfinished.is_empty())
    );
    let Message::Assistant { tools, .. } = &page.messages[1].message else {
        panic!("expected a canonical call");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&tools[0].arguments.load().unwrap()).unwrap(),
        json!({"canonical":true})
    );
    let tail = db
        .display_page(&session, 1, Some(page.messages[1].sequence))
        .unwrap();
    let parts = &tail.messages[0].unfinished;
    assert_eq!(
        parts
            .iter()
            .map(|s| s.text.load().unwrap())
            .collect::<Vec<_>>(),
        ["unrepresented", "["]
    );
    assert!(parts[1].tool.as_ref().unwrap().id.is_none());
    assert_eq!(db.model_context(&session).unwrap().revision, 0);
}

#[test]
fn draft_batches_and_save_only_settlement_roll_back_without_publishing_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let db = ChatDatabase::open(dir.path().join("chat.sqlite")).unwrap();
    let session = db.create_session("default", None, None).unwrap().id;
    let inv = db
        .begin_turn(&session, "question", model(), None)
        .unwrap()
        .invocation;
    let mut conn = storage::connect(db.path()).unwrap();
    let mut writer = DraftWriter::open(&db, inv.clone()).unwrap();
    let original = writer.append(&[text("m", "first")]).unwrap();
    let mut foreign = inv.clone();
    foreign.call_id = "unrelated".into();
    assert!(matches!(
        DraftWriter::open(&db, foreign)
            .unwrap()
            .append(&[text("m", "no")]),
        Err(ChatError::Conflict { .. })
    ));
    conn.batch_execute("CREATE TRIGGER reject_chunk BEFORE INSERT ON preview_chunks WHEN NEW.value = 'blocked' BEGIN SELECT RAISE(ABORT, 'disk full'); END;").unwrap();
    assert!(matches!(
        writer.append(&[text("new", "unsaved"), text("m", "blocked")]),
        Err(ChatError::Database {
            operation: "append turn preview",
            ..
        })
    ));
    assert!(
        writer
            .append(&[text("m", "another"), text("m", "")])
            .is_err()
    );
    assert_eq!(chunk_count(db.path()), 1);
    let full = writer.append(&[text("m", " last")]).unwrap();
    writer.seal().unwrap();
    let result = ConversationResult::assistant("canonical");
    conn.batch_execute("CREATE TRIGGER reject_settlement BEFORE UPDATE OF terminal_result ON model_calls BEGIN SELECT RAISE(ABORT, 'save failed'); END;").unwrap();
    assert!(db.complete_turn(inv.clone(), result.clone()).is_err());
    assert_eq!(chunk_count(db.path()), 2);
    assert_eq!(db.messages(&session, 10, None).unwrap().messages.len(), 1);
    assert_eq!(db.turn(&inv.turn_id).unwrap().state, "running");
    conn.batch_execute("DROP TRIGGER reject_settlement")
        .unwrap();
    db.save_recovered(crate::UncommittedCompletion {
        database_path: db.path().to_owned(),
        completion: crate::Completion {
            invocation: inv,
            result,
        },
    })
    .unwrap();
    assert_eq!(chunk_count(db.path()), 0);
    assert_eq!(db.messages(&session, 10, None).unwrap().messages.len(), 2);
    assert_eq!(original[0].text.load().unwrap(), "first");
    assert_eq!(full[0].text.load().unwrap(), "first last");
}

#[test]
fn unreadable_unfinished_text_does_not_block_an_authoritative_failure_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = ChatDatabase::open(dir.path().join("chat.sqlite")).unwrap();
    let session = db.create_session("default", None, None).unwrap().id;
    let inv = db
        .begin_turn(&session, "question", model(), None)
        .unwrap()
        .invocation;
    DraftWriter::open(&db, inv.clone())
        .unwrap()
        .append(&[text("m", "lost"), text("m", " tail")])
        .unwrap();
    storage::connect(db.path())
        .unwrap()
        .batch_execute("DELETE FROM preview_chunks WHERE end_byte = 4")
        .unwrap();
    let mut result = ConversationResult::assistant("recorded");
    result.status = "failed".into();
    result.error = Some("stopped".into());
    db.complete_turn(inv, result).unwrap();
    let page = db.display_page(&session, 10, None).unwrap();
    assert_eq!(
        page.messages[1].message.content().load().unwrap(),
        "recorded"
    );
    assert!(matches!(
        page.messages[1].unfinished[0].text.load(),
        Err(ChatError::Invalid {
            field: "turn preview",
            ..
        })
    ));
}
