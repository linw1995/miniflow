//! Durable, display-only prefixes for an admitted conversation turn.

use super::{Reader, Text};
use crate::{
    ChatDatabase, ChatError, ChatMessage, ConversationResult, Invocation, PreviewDelta,
    error::*,
    schema::{model_calls, turns},
    storage,
};
use diesel::{
    connection::SimpleConnection,
    prelude::*,
    sql_types::{BigInt, Nullable, Text as SqlText},
};
use snafu::{ResultExt, ensure};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: Text,
    pub tool: Option<ToolCallPreview>,
}

#[derive(Clone, Debug)]
pub struct ToolCallPreview {
    pub index: u64,
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(QueryableByName)]
struct Header {
    #[diesel(sql_type = BigInt)]
    id: i64,
    #[diesel(sql_type = SqlText)]
    model_call_id: String,
    #[diesel(sql_type = SqlText)]
    part: String,
    #[diesel(sql_type = Nullable<SqlText>)]
    tool_id: Option<String>,
    #[diesel(sql_type = Nullable<SqlText>)]
    name: Option<String>,
    #[diesel(sql_type = BigInt)]
    bytes: i64,
}

impl Header {
    fn segment(self, reader: &Reader) -> Result<PreviewSegment, ChatError> {
        let tool = if self.part.is_empty() {
            None
        } else {
            Some(ToolCallPreview {
                index: serde_json::from_str(&self.part).context(JsonSnafu)?,
                id: self.tool_id,
                name: self.name,
            })
        };
        Ok(PreviewSegment {
            model_call_id: self.model_call_id,
            text: reader.draft_text(self.id, self.bytes),
            tool,
        })
    }
}

/// Commits each available batch before publication. Handles refer to immutable byte prefixes.
/// Seal the reader after the last batch and before the turn is settled.
pub struct DraftWriter {
    invocation: Invocation,
    writer: SqliteConnection,
    reader: Reader,
}

impl std::fmt::Debug for DraftWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftWriter")
            .field("turn", &self.invocation.turn_id)
            .finish_non_exhaustive()
    }
}

impl DraftWriter {
    pub fn open(db: &ChatDatabase, invocation: Invocation) -> Result<Self, ChatError> {
        Ok(Self {
            invocation,
            writer: storage::connect(db.path())?,
            reader: Reader::open(db.path())?,
        })
    }

    pub fn append(&mut self, deltas: &[PreviewDelta]) -> Result<Vec<PreviewSegment>, ChatError> {
        let inv = &self.invocation;
        self.writer.immediate_transaction(|conn| {
            let active = model_calls::table
                .inner_join(turns::table)
                .filter(model_calls::id.eq(&inv.call_id))
                .filter(model_calls::session_id.eq(&inv.session_id))
                .filter(model_calls::purpose.eq("conversation"))
                .filter(model_calls::state.eq("running"))
                .filter(turns::id.eq(&inv.turn_id))
                .filter(turns::session_id.eq(&inv.session_id))
                .filter(turns::state.eq("running"));
            ensure!(
                diesel::select(diesel::dsl::exists(active))
                    .get_result::<bool>(conn)
                    .context(DatabaseSnafu {
                        operation: "check preview ownership"
                    })?,
                ConflictSnafu { id: &inv.call_id }
            );
            deltas
                .iter()
                .map(|delta| {
                    delta.validate()?;
                    append_chunk(conn, &inv.turn_id, delta)?.segment(&self.reader)
                })
                .collect()
        })
    }

    pub fn seal(self) -> Result<(), ChatError> {
        // No read transaction spans the incoming stream. Pin its final WAL view only
        // now so a delayed frame still loads its prefixes after settlement deletes them.
        self.reader
            .0
            .lock()
            .unwrap()
            .batch_execute("BEGIN; SELECT id FROM preview_segments LIMIT 1;")
            .context(DatabaseSnafu {
                operation: "seal turn preview",
            })
    }
}

fn append_chunk(
    conn: &mut SqliteConnection,
    turn: &str,
    delta: &PreviewDelta,
) -> Result<Header, ChatError> {
    let (model, part, id, name, fragment) = match delta {
        PreviewDelta::Text { delta } => {
            (&delta.model_call_id, String::new(), None, None, &delta.text)
        }
        PreviewDelta::ToolCall { delta } => (
            &delta.model_call_id,
            delta.index.to_string(),
            delta.id.as_deref(),
            delta.name.as_deref(),
            &delta.arguments,
        ),
    };
    // Empty denotes assistant text; decimal tool indices preserve all u64 values.
    let header = diesel::sql_query("INSERT INTO preview_segments (turn_id, model_call_id, part, tool_id, name, bytes)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT(turn_id, model_call_id, part) DO UPDATE SET bytes = bytes + excluded.bytes,
                    tool_id = coalesce(excluded.tool_id, tool_id), name = coalesce(excluded.name, name)
                RETURNING id, model_call_id, part, tool_id, name, bytes")
                .bind::<SqlText,_>(turn).bind::<SqlText,_>(model).bind::<SqlText,_>(part)
                .bind::<Nullable<SqlText>,_>(id.filter(|s| !s.trim().is_empty()))
                .bind::<Nullable<SqlText>,_>(name.filter(|s| !s.trim().is_empty()))
                .bind::<BigInt,_>(fragment.len() as i64)
                .get_result::<Header>(conn).context(DatabaseSnafu { operation: "update turn preview" })?;
    if !fragment.is_empty() {
        diesel::sql_query(
            "INSERT INTO preview_chunks (segment_id, end_byte, value) VALUES (?, ?, ?)",
        )
        .bind::<BigInt, _>(header.id)
        .bind::<BigInt, _>(header.bytes)
        .bind::<SqlText, _>(fragment)
        .execute(conn)
        .context(DatabaseSnafu {
            operation: "append turn preview",
        })?;
    }
    Ok(header)
}

pub fn at_page_tail(
    conn: &mut SqliteConnection,
    reader: &Reader,
    turn: &str,
    sequence: i64,
) -> Result<Vec<Arc<PreviewSegment>>, ChatError> {
    diesel::sql_query("SELECT id, model_call_id, part, tool_id, name, bytes FROM preview_segments
        WHERE turn_id = ? AND NOT EXISTS (SELECT 1 FROM messages WHERE turn_id = ? AND sequence > ?) ORDER BY id")
        .bind::<SqlText,_>(turn).bind::<SqlText,_>(turn).bind::<BigInt,_>(sequence)
        .load::<Header>(conn).context(DatabaseSnafu { operation: "read unfinished preview index" })?
        .into_iter().map(|h| Ok(Arc::new(h.segment(reader)?))).collect()
}

/// The same replacement rule applies to a live frame, save-only recovery, and a reopened page.
/// A missing usage is not evidence that a model call completed.
#[derive(Default)]
pub struct PreviewCoverage<'a> {
    complete: bool,
    calls: BTreeSet<&'a str>,
    texts: BTreeSet<&'a str>,
    tools: BTreeSet<&'a str>,
}

impl<'a> PreviewCoverage<'a> {
    pub fn new(result: &'a ConversationResult) -> Self {
        let mut coverage = Self {
            complete: result.status == "succeeded",
            ..Self::default()
        };
        if !result.display_messages().is_empty() {
            coverage.calls.extend(
                result
                    .usages
                    .iter()
                    .filter(|u| u.error.is_none())
                    .map(|u| u.call_id.as_str()),
            );
            for message in result.display_messages() {
                if let ChatMessage::Assistant {
                    content,
                    tool_calls,
                    ..
                } = message
                {
                    coverage.texts.insert(content);
                    coverage
                        .tools
                        .extend(tool_calls.iter().flatten().map(|c| c.id.as_str()));
                }
            }
        }
        coverage
    }

    pub fn covers(&self, segment: &PreviewSegment) -> bool {
        self.replaces(
            &segment.model_call_id,
            segment.tool.as_ref().map(|tool| tool.id.as_deref()),
            || segment.text.load(),
        )
    }

    fn replaces(
        &self,
        model: &str,
        tool: Option<Option<&str>>,
        load: impl FnOnce() -> Result<String, ChatError>,
    ) -> bool {
        if self.complete || self.calls.contains(model) {
            return true;
        }
        match tool {
            Some(id) => id.is_some_and(|id| self.tools.contains(id)),
            // If a draft cannot be read, leave it unfinished rather than vetoing a valid reply.
            None => {
                !self.texts.is_empty()
                    && load().is_ok_and(|prefix| {
                        self.texts
                            .range(prefix.as_str()..)
                            .next()
                            .is_some_and(|text| text.starts_with(&prefix))
                    })
            }
        }
    }
}

pub fn settle(
    conn: &mut SqliteConnection,
    turn: &str,
    result: &ConversationResult,
) -> Result<(), ChatError> {
    if result.status == "succeeded" {
        diesel::sql_query("DELETE FROM preview_segments WHERE turn_id = ?")
            .bind::<SqlText, _>(turn)
            .execute(conn)
            .context(DatabaseSnafu {
                operation: "release completed turn preview",
            })?;
    } else if !result.display_messages().is_empty() {
        let coverage = PreviewCoverage::new(result);
        let headers = diesel::sql_query("SELECT id, model_call_id, part, tool_id, name, bytes FROM preview_segments WHERE turn_id = ?")
            .bind::<SqlText,_>(turn).load::<Header>(conn).context(DatabaseSnafu { operation: "compare failed turn preview" })?;
        for h in headers {
            if coverage.replaces(
                &h.model_call_id,
                (!h.part.is_empty()).then_some(h.tool_id.as_deref()),
                || super::text::read_draft(conn, h.id, h.bytes),
            ) {
                diesel::sql_query("DELETE FROM preview_segments WHERE id = ?")
                    .bind::<BigInt, _>(h.id)
                    .execute(conn)
                    .context(DatabaseSnafu {
                        operation: "release covered turn preview",
                    })?;
            }
        }
    }
    Ok(())
}
