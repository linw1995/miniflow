//! Reloadable display indexes; drafts are separate from audit and model-context messages.

mod drafts;
pub(crate) use drafts::settle as settle_drafts;
pub use drafts::{DraftWriter, PreviewCoverage, PreviewSegment, ToolCallPreview};

#[cfg(test)]
mod tests;
mod text;
use text::Fragments;
pub use text::Text;

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

/// Where a user message entered the authoritative display exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserPlacement {
    Prompt,
    Steering,
    Queued,
}

impl UserPlacement {
    pub fn admitted(queued: bool) -> Self {
        if queued { Self::Queued } else { Self::Prompt }
    }

    fn observe(&mut self, message: &Message) {
        match message {
            Message::ToolResult { .. } => *self = Self::Steering,
            Message::Assistant { .. } => *self = Self::Queued,
            Message::System(_) => *self = Self::Prompt,
            // All consecutive users belong to the same reply boundary.
            Message::User { .. } => {}
        }
    }
}

#[derive(Debug)]
pub enum Message {
    User {
        content: Text,
        placement: UserPlacement,
    },
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
            Self::User { content: text, .. }
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
    pub unfinished: Vec<Arc<PreviewSegment>>,
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
    queued_admission: Option<bool>,
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
        reader: &Reader,
    ) -> Result<Vec<Record>, ChatError> {
        let query = messages::table.inner_join(turns::table)
            .filter(messages::session_id.eq(session))
            .select((messages::id, messages::turn_id, messages::sequence, turns::state,
                sql::<sql_types::Text>("payload ->> '$.role'"),
                sql::<sql_types::BigInt>("length(CAST(payload ->> '$.content' AS BLOB))"),
                sql::<sql_types::Nullable<sql_types::Text>>("payload ->> '$.tool_call_id'"),
                sql::<sql_types::Text>("(SELECT json_group_array(json_object('id', value ->> '$.id', 'name', value ->> '$.name') ORDER BY key) FROM json_each(messages.payload, '$.tool_calls') WHERE type = 'object')"),
                sql::<sql_types::Nullable<sql_types::Bool>>("CASE WHEN messages.sequence = turns.sequence THEN EXISTS (
                    SELECT 1 FROM model_calls WHERE turn_id = turns.id AND purpose = 'conversation' AND metadata ->> '$.queued_prompt' = 1
                ) END")))
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
        let headers = query.load::<Self>(conn).context(DatabaseSnafu {
            operation: "read chat display index",
        })?;
        let mut placement = UserPlacement::Prompt;
        if let Some(first) = headers.first()
            && first.role == "user"
            && first.queued_admission.is_none()
        {
            // A keyset page can start in the middle of a batch of identical user messages.
            let role = sql::<sql_types::Text>("payload ->> '$.role'");
            let after_tool = messages::table
                .filter(messages::turn_id.eq(&first.turn_id))
                .filter(messages::sequence.lt(first.sequence))
                .filter(role.clone().ne("user"))
                .order(messages::sequence.desc())
                .select(role.eq("tool"))
                .first::<bool>(conn)
                .context(DatabaseSnafu {
                    operation: "read preceding reply boundary",
                })?;
            placement = if after_tool {
                UserPlacement::Steering
            } else {
                UserPlacement::Queued
            };
        }
        headers
            .into_iter()
            .map(|header| {
                if let Some(queued) = header.queued_admission {
                    placement = UserPlacement::admitted(queued);
                }
                let record = header.into_record(reader, placement)?;
                placement.observe(&record.message);
                Ok(record)
            })
            .collect()
    }

    fn into_record(self, reader: &Reader, placement: UserPlacement) -> Result<Record, ChatError> {
        let id: Arc<str> = self.id.into();
        let bytes = usize::try_from(self.bytes).context(IntegerSnafu {
            operation: "size saved message",
        })?;
        let content = if bytes == 0 {
            Text::default()
        } else {
            reader.message_text(&id, None)
        };
        let message = match self.role.as_str() {
            "user" => Message::User { content, placement },
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
                        arguments: reader.message_text(&id, Some(index)),
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
            unfinished: Vec::new(),
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
            let mut messages = Header::load(
                conn,
                id,
                Window::Page {
                    limit,
                    after: after_sequence.unwrap_or(-1),
                },
                &reader,
            )?;
            let mut usages = session::page_usages(
                conn,
                id,
                messages.iter().map(|r| r.turn_id.as_str()).collect(),
            )?;
            for record in &mut messages {
                record.usages = usages.remove(&record.turn_id);
            }
            for records in messages.chunk_by_mut(|a, b| a.turn_id == b.turn_id) {
                let tail = records.last_mut().expect("nonempty turn group");
                if matches!(
                    tail.turn_state.as_str(),
                    "interrupted" | "failed" | "cancelled"
                ) {
                    tail.unfinished =
                        drafts::at_page_tail(conn, &reader, &tail.turn_id, tail.sequence)?;
                }
            }
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
        let records = Header::load(
            &mut reader.0.lock().unwrap(),
            &invocation.session_id,
            Window::Reply {
                turn: &invocation.turn_id,
            },
            &reader,
        )?;
        ensure!(
            records.len() == expected_messages,
            InvalidSnafu {
                field: "committed reply",
                reason: "display index disagrees with settlement"
            }
        );
        Ok(records.into_iter().map(|r| r.message).collect())
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
    let mut placement = UserPlacement::Prompt;
    let messages = messages
        .into_iter()
        .map(|message| {
            let message = match message {
                ChatMessage::User { content, .. } => Message::User {
                    content: text(content),
                    placement,
                },
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
            };
            placement.observe(&message);
            message
        })
        .collect();
    (messages, store.err())
}
