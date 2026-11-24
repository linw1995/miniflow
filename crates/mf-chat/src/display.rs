//! Read-only presentation indexes. Audit and model-context APIs still return full messages.

#[cfg(test)]
mod tests;
mod text;
pub use text::{Fragments, Text};

use crate::{
    CallUsage, ChatDatabase, ChatError, ChatMessage, Invocation,
    error::*,
    schema::{messages, turns},
    session, storage,
};
use diesel::{connection::SimpleConnection, dsl::sql, prelude::*, sql_types};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub arguments: Text,
}

#[derive(Debug)]
pub enum Message {
    User(Text),
    System(Text),
    Assistant {
        content: Text,
        tools: Vec<Tool>,
    },
    ToolResult {
        id: String,
        content: Text,
        bytes: usize,
    },
}

impl Message {
    pub fn content(&self) -> &Text {
        match self {
            Self::User(text)
            | Self::System(text)
            | Self::Assistant { content: text, .. }
            | Self::ToolResult { content: text, .. } => text,
        }
    }
}

#[derive(Debug)]
pub struct Record {
    pub id: Arc<str>,
    pub turn_id: String,
    pub sequence: i64,
    pub message: Message,
    pub turn_state: String,
    pub usages: Option<Vec<CallUsage>>,
}

#[derive(Debug)]
pub struct Page {
    pub messages: Vec<Record>,
}

// Each index shares a query-only connection, not a database ownership lock. Keeping
// it open avoids a connection per body and ties all handles to the indexed database.
#[derive(Clone)]
struct Reader(Arc<Mutex<SqliteConnection>>);

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reader")
    }
}

impl Reader {
    fn open(path: &Path) -> Result<Self, ChatError> {
        let mut conn = storage::connect(path)?;
        conn.batch_execute("PRAGMA query_only = ON; PRAGMA cache_size = -512;")
            .context(DatabaseSnafu {
                operation: "configure display reader",
            })?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }
}

#[derive(Queryable)]
struct Header {
    id: String,
    turn_id: String,
    sequence: i64,
    state: String,
    role: String,
    bytes: i64,
    tool_id: Option<String>,
    calls: String,
}

enum Window<'a> {
    Page { limit: usize, after: i64 },
    Reply { turn: &'a str },
}

impl Header {
    fn load(
        conn: &mut SqliteConnection,
        session: &str,
        window: Window<'_>,
    ) -> Result<Vec<Self>, ChatError> {
        let query = messages::table.inner_join(turns::table)
            .filter(messages::session_id.eq(session))
            .select((messages::id, messages::turn_id, messages::sequence, turns::state,
                sql::<sql_types::Text>("payload ->> '$.role'"),
                sql::<sql_types::BigInt>("length(CAST(payload ->> '$.content' AS BLOB))"),
                sql::<sql_types::Nullable<sql_types::Text>>("payload ->> '$.tool_call_id'"),
                sql::<sql_types::Text>("(SELECT json_group_array(json_object('id', value ->> '$.id', 'name', value ->> '$.name') ORDER BY key) FROM json_each(messages.payload, '$.tool_calls') WHERE type = 'object')")))
            .order(messages::sequence.asc()).into_boxed();
        let query = match window {
            Window::Page { limit, after } => query
                .filter(messages::sequence.gt(after))
                .limit(limit as i64),
            // Admission owns turns.sequence; only subsequent audit rows are the reply.
            Window::Reply { turn } => query
                .filter(messages::turn_id.eq(turn))
                .filter(messages::sequence.gt(turns::sequence)),
        };
        query.load(conn).context(DatabaseSnafu {
            operation: "read chat display index",
        })
    }

    fn into_record(self, reader: &Reader) -> Result<Record, ChatError> {
        let id: Arc<str> = self.id.into();
        let bytes = usize::try_from(self.bytes).context(IntegerSnafu {
            operation: "size saved message",
        })?;
        let content = if bytes == 0 {
            Text::default()
        } else {
            Text::saved(reader, &id, None)
        };
        let message = match self.role.as_str() {
            "user" => Message::User(content),
            "system" => Message::System(content),
            "assistant" => {
                #[derive(serde::Deserialize)]
                struct Hint {
                    id: String,
                    name: String,
                }
                let calls: Vec<Hint> = serde_json::from_str(&self.calls).context(JsonSnafu)?;
                let tools = calls
                    .into_iter()
                    .enumerate()
                    .map(|(index, hint)| Tool {
                        id: hint.id,
                        name: hint.name,
                        arguments: Text::saved(reader, &id, Some(index)),
                    })
                    .collect();
                Message::Assistant { content, tools }
            }
            "tool" => Message::ToolResult {
                id: self.tool_id.context(InvalidSnafu {
                    field: "tool result",
                    reason: "missing tool call id",
                })?,
                content,
                bytes,
            },
            _ => {
                return InvalidSnafu {
                    field: "display index",
                    reason: "unknown message role",
                }
                .fail();
            }
        };
        Ok(Record {
            id,
            sequence: self.sequence,
            usages: None,
            turn_id: self.turn_id,
            turn_state: self.state,
            message,
        })
    }
}

impl ChatDatabase {
    /// Like `messages`, but does not deserialize message bodies, metadata or tool arguments.
    pub fn display_page(
        &self,
        id: &str,
        limit: usize,
        after_sequence: Option<i64>,
    ) -> Result<Page, ChatError> {
        session::validate_page(limit)?;
        let reader = Reader::open(self.path())?;
        reader.0.lock().unwrap().transaction(|conn| {
            session::load(conn, id)?;
            let headers = Header::load(
                conn,
                id,
                Window::Page {
                    limit,
                    after: after_sequence.unwrap_or(-1),
                },
            )?;
            let mut usages = session::page_usages(
                conn,
                id,
                headers.iter().map(|h| h.turn_id.as_str()).collect(),
            )?;
            let messages = headers
                .into_iter()
                .map(|h| {
                    let mut record = h.into_record(&reader)?;
                    record.usages = usages.remove(&record.turn_id);
                    Ok(record)
                })
                .collect::<Result<_, ChatError>>()?;
            Ok(Page { messages })
        })
    }

    /// Present a committed reply immediately without restaging its bodies or waiting for page navigation.
    pub fn display_reply(
        &self,
        invocation: &Invocation,
        expected_messages: usize,
    ) -> Result<Vec<Message>, ChatError> {
        let reader = Reader::open(self.path())?;
        let headers = Header::load(
            &mut reader.0.lock().unwrap(),
            &invocation.session_id,
            Window::Reply {
                turn: &invocation.turn_id,
            },
        )?;
        ensure!(
            headers.len() == expected_messages,
            InvalidSnafu {
                field: "committed reply",
                reason: "display index disagrees with settlement"
            }
        );
        headers
            .into_iter()
            .map(|h| Ok(h.into_record(&reader)?.message))
            .collect()
    }
}

/// Stage an authoritative reply while its history page is loading, or while save-only
/// recovery is pending. A scratch failure must not hide or invalidate that reply.
pub fn stage(messages: Vec<ChatMessage>) -> (Vec<Message>, Option<ChatError>) {
    if messages.is_empty() {
        return (Vec::new(), None);
    }
    stage_with(messages, Fragments::open())
}

fn stage_with(
    messages: Vec<ChatMessage>,
    mut store: Result<Fragments, ChatError>,
) -> (Vec<Message>, Option<ChatError>) {
    let mut text = |value: String| {
        if let Ok(db) = &store {
            match db.append(&Text::default(), &value) {
                Ok(handle) => return handle,
                Err(error) => store = Err(error),
            }
        }
        Text::inline(value)
    };
    let messages = messages
        .into_iter()
        .map(|message| match message {
            ChatMessage::User { content, .. } => Message::User(text(content)),
            ChatMessage::System { content, .. } => Message::System(text(content)),
            ChatMessage::ToolResult {
                tool_call_id: id,
                content,
                ..
            } => Message::ToolResult {
                id,
                bytes: content.len(),
                content: text(content),
            },
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => Message::Assistant {
                content: text(content),
                tools: tool_calls
                    .into_iter()
                    .flatten()
                    .map(|call| Tool {
                        id: call.id,
                        name: call.name,
                        arguments: text(
                            serde_json::to_string_pretty(&call.arguments)
                                .unwrap_or_else(|e| format!("[Arguments unavailable: {e}]")),
                        ),
                    })
                    .collect(),
            },
        })
        .collect();
    (messages, store.err())
}
