use super::*;
use crate::{ConversationResult, ModelIdentity};
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
