//! Canonical conversation messages. Display/telemetry budgets never truncate this log.

use crate::{
    ChatDatabase, ChatError, ChatMessage, Invocation, Message, error::*, schema::*, session,
    storage, turn,
};
use diesel::{connection::SimpleConnection, prelude::*};
use mf_runtime::ValueRef;
use mfn_llm::MessageBatch;
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = message_sources)]
struct Source {
    message_id: String,
    call_id: String,
    ordinal: i64,
    model_call_id: Option<String>,
    tool_calls_present: bool,
    metadata_secrets: Option<String>,
}

#[derive(Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = chat_tool_calls)]
struct SemanticCall {
    id: String,
    call_id: String,
    assistant_message_id: String,
    ordinal: i64,
    provider_tool_call_id: String,
    name: String,
    arguments: String,
    argument_secrets: Option<String>,
    public_arguments: String,
    result_message_id: Option<String>,
}

// JSON business encoding intentionally omits native sensitivity. Preserve its small path sidecar
// separately so reopening history cannot turn protected arguments/metadata into public values.
fn secret_paths(value: &ValueRef) -> Result<Option<String>, ChatError> {
    fn visit(value: &ValueRef, path: &mut Vec<String>, paths: &mut Vec<Vec<String>>) {
        if value.is_secret() {
            paths.push(path.clone());
            return;
        }
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                path.push(key.to_string());
                visit(value, path, paths);
                path.pop();
            }
        } else if let Some(array) = value.as_array() {
            for (index, value) in array.iter().enumerate() {
                path.push(index.to_string());
                visit(value, path, paths);
                path.pop();
            }
        }
    }
    let mut paths = Vec::new();
    visit(value, &mut Vec::new(), &mut paths);
    if paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(&paths).context(JsonSnafu)?))
    }
}
fn restore_secrets(value: ValueRef, encoded: Option<&str>) -> Result<ValueRef, ChatError> {
    fn apply(
        value: &ValueRef,
        path: &mut Vec<String>,
        paths: &mut BTreeSet<Vec<String>>,
    ) -> ValueRef {
        if paths.remove(path) {
            return value.clone().into_secret();
        }
        if let Some(object) = value.as_object() {
            ValueRef::object(object.iter().map(|(key, value)| {
                path.push(key.to_string());
                let value = apply(value, path, paths);
                path.pop();
                (key.clone(), value)
            }))
        } else if let Some(array) = value.as_array() {
            ValueRef::array(array.iter().enumerate().map(|(index, value)| {
                path.push(index.to_string());
                let value = apply(value, path, paths);
                path.pop();
                value
            }))
        } else {
            value.clone()
        }
    }
    let Some(encoded) = encoded else {
        return Ok(value);
    };
    let mut paths: BTreeSet<Vec<String>> = serde_json::from_str(encoded).context(JsonSnafu)?;
    let value = apply(&value, &mut Vec::new(), &mut paths);
    ensure!(
        paths.is_empty(),
        InvalidSnafu {
            field: "message sensitivity",
            reason: "stored sensitivity paths must identify existing values"
        }
    );
    Ok(value)
}
fn metadata_value(metadata: &mfn_llm::MessageMetadata) -> ValueRef {
    ValueRef::object(
        metadata
            .iter()
            .map(|(key, value)| (key.as_str().into(), value.clone())),
    )
}

pub fn validate(messages: &[ChatMessage], complete: bool) -> Result<(), ChatError> {
    let mut pending = BTreeSet::new();
    for message in messages {
        match message {
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                ensure!(
                    pending.is_empty(),
                    InvalidSnafu {
                        field: "messages",
                        reason: "assistant messages must follow outstanding tool results"
                    }
                );
                let calls = tool_calls.as_deref().unwrap_or_default();
                ensure!(
                    !content.is_empty() || !calls.is_empty(),
                    InvalidSnafu {
                        field: "messages",
                        reason: "assistant messages require content or tool calls"
                    }
                );
                for call in calls {
                    ensure!(
                        !call.id.trim().is_empty()
                            && !call.name.trim().is_empty()
                            && pending.insert(&call.id),
                        InvalidSnafu {
                            field: "messages",
                            reason: "tool calls require unique pending IDs and nonblank names"
                        }
                    );
                }
            }
            ChatMessage::ToolResult { tool_call_id, .. } => {
                ensure!(
                    pending.remove(tool_call_id),
                    InvalidSnafu {
                        field: "messages",
                        reason: "tool results must identify an outstanding assistant tool call"
                    }
                );
            }
            ChatMessage::User { content, .. } | ChatMessage::System { content, .. } => {
                ensure!(
                    !content.is_empty() && pending.is_empty(),
                    InvalidSnafu {
                        field: "messages",
                        reason: "user/system messages require content and cannot interrupt tool results"
                    }
                );
            }
        }
    }
    ensure!(
        !complete || pending.is_empty(),
        InvalidSnafu {
            field: "messages",
            reason: "successful messages must resolve outstanding tool calls"
        }
    );
    Ok(())
}

/// Batch reconstruction also serves history pagination; no per-message database reads.
pub fn reconstruct(
    connection: &mut SqliteConnection,
    rows: &[Message],
) -> Result<BTreeMap<String, ChatMessage>, ChatError> {
    let ids: Vec<_> = rows.iter().map(|row| &row.id).collect();
    let calls = chat_tool_calls::table
        .filter(chat_tool_calls::assistant_message_id.eq_any(&ids))
        .order((
            chat_tool_calls::assistant_message_id.asc(),
            chat_tool_calls::ordinal.asc(),
        ))
        .select(SemanticCall::as_select())
        .load::<SemanticCall>(connection)
        .context(DatabaseSnafu {
            operation: "load canonical assistant tool calls",
        })?;
    let results = chat_tool_calls::table
        .filter(chat_tool_calls::result_message_id.eq_any(&ids))
        .select((
            chat_tool_calls::result_message_id,
            chat_tool_calls::provider_tool_call_id,
        ))
        .load::<(Option<String>, String)>(connection)
        .context(DatabaseSnafu {
            operation: "load canonical tool result identities",
        })?;
    let call_arrays = message_sources::table
        .filter(message_sources::message_id.eq_any(&ids))
        .filter(message_sources::tool_calls_present.eq(true))
        .select(message_sources::message_id)
        .load::<String>(connection)
        .context(DatabaseSnafu {
            operation: "load assistant call-array presence",
        })?;
    let sensitivities: BTreeMap<_, _> = message_sources::table
        .filter(message_sources::message_id.eq_any(&ids))
        .filter(message_sources::metadata_secrets.is_not_null())
        .select((
            message_sources::message_id,
            message_sources::metadata_secrets,
        ))
        .load::<(String, Option<String>)>(connection)
        .context(DatabaseSnafu {
            operation: "load canonical metadata sensitivity",
        })?
        .into_iter()
        .collect();
    let mut requests: BTreeMap<String, Vec<mfn_llm::ToolCall>> =
        call_arrays.into_iter().map(|id| (id, Vec::new())).collect();
    for call in calls {
        requests
            .entry(call.assistant_message_id)
            .or_default()
            .push(mfn_llm::ToolCall {
                id: call.provider_tool_call_id,
                name: call.name,
                arguments: restore_secrets(
                    serde_json::from_str::<ValueRef>(&call.arguments).context(JsonSnafu)?,
                    call.argument_secrets.as_deref(),
                )?,
            });
    }
    let mut results: BTreeMap<_, _> = results
        .into_iter()
        .filter_map(|(message, id)| message.map(|message| (message, id)))
        .collect();
    rows.iter()
        .map(|row| {
            let content = row.content.clone();
            let metadata = row
                .metadata
                .as_ref()
                .map(|metadata| -> Result<_, ChatError> {
                    let value = restore_secrets(
                        metadata_value(metadata),
                        sensitivities
                            .get(&row.id)
                            .and_then(|encoded| encoded.as_deref()),
                    )?;
                    Ok(value
                        .as_object()
                        .expect("metadata stays an object")
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.clone()))
                        .collect())
                })
                .transpose()?;
            let message = match row.role.as_str() {
                "system" => ChatMessage::System { content, metadata },
                "user" => ChatMessage::User { content, metadata },
                "assistant" => ChatMessage::Assistant {
                    content,
                    metadata,
                    tool_calls: requests.remove(&row.id),
                },
                "tool" => ChatMessage::ToolResult {
                    content,
                    metadata,
                    tool_call_id: results
                        .remove(&row.id)
                        .context(ConflictSnafu { id: &row.id })?,
                },
                _ => return ConflictSnafu { id: &row.id }.fail(),
            };
            Ok((row.id.clone(), message))
        })
        .collect()
}

pub fn load(
    connection: &mut SqliteConnection,
    call_id: &str,
) -> Result<Option<Vec<ChatMessage>>, ChatError> {
    let exists = message_streams::table
        .find(call_id)
        .select(message_streams::finished)
        .first::<bool>(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "load canonical message stream",
        })?;
    if exists.is_none() {
        return Ok(None);
    }
    let rows = messages::table
        .inner_join(message_sources::table)
        .filter(message_sources::call_id.eq(call_id))
        .order(message_sources::ordinal.asc())
        .select(Message::as_select())
        .load::<Message>(connection)
        .context(DatabaseSnafu {
            operation: "load canonical message sequence",
        })?;
    let mut messages = reconstruct(connection, &rows)?;
    Ok(Some(
        rows.iter()
            .map(|row| {
                messages
                    .remove(&row.id)
                    .expect("reconstructed each message")
            })
            .collect(),
    ))
}

pub fn append(
    connection: &mut SqliteConnection,
    invocation: &Invocation,
    batch: &MessageBatch,
) -> Result<(), ChatError> {
    let call = turn::load_call(connection, &invocation.call_id)?;
    ensure!(
        call.session_id == invocation.session_id
            && call.turn_id.as_deref() == Some(&invocation.turn_id)
            && call.purpose == "conversation",
        ConflictSnafu { id: &call.id }
    );
    ensure!(
        batch.offset >= 0
            && batch
                .model_call_id
                .as_ref()
                .is_none_or(|id| !id.trim().is_empty()),
        InvalidSnafu {
            field: "message event",
            reason: "requires a nonnegative offset and nonblank model identity"
        }
    );
    let stored = load(connection, &call.id)?;
    let mut prefix = stored.clone().unwrap_or_default();
    let offset = usize::try_from(batch.offset).context(IntegerSnafu {
        operation: "index message event",
    })?;
    ensure!(offset <= prefix.len(), ConflictSnafu { id: &call.id });
    for (index, message) in batch.messages.iter().enumerate() {
        if let Some(previous) = prefix.get(offset + index) {
            ensure!(previous == message, ConflictSnafu { id: &call.id });
        } else {
            prefix.push(message.clone());
        }
    }
    validate(&prefix, false)?;
    let old_len = stored.as_ref().map_or(0, Vec::len);
    ensure!(
        call.state != "succeeded" || old_len == prefix.len(),
        ConflictSnafu { id: &call.id }
    );
    let finished = message_streams::table
        .find(&call.id)
        .select(message_streams::finished)
        .first::<bool>(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "check settled message stream",
        })?
        .unwrap_or(false);
    ensure!(
        !finished || old_len == prefix.len(),
        ConflictSnafu { id: &call.id }
    );
    if stored.is_none() {
        diesel::insert_into(message_streams::table)
            .values((
                message_streams::call_id.eq(&call.id),
                message_streams::finished.eq(false),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "begin canonical message stream",
            })?;
    }
    let timestamp = session::now()?;
    for (index, message) in batch.messages.iter().enumerate() {
        let source_index = offset + index;
        let ordinal = i64::try_from(source_index).context(IntegerSnafu {
            operation: "order source message",
        })?;
        if offset + index < old_len {
            if let Some(origin) = &batch.model_call_id {
                let source = message_sources::table
                    .filter(message_sources::call_id.eq(&call.id))
                    .filter(message_sources::ordinal.eq(ordinal))
                    .select(Source::as_select())
                    .first::<Source>(connection)
                    .context(DatabaseSnafu {
                        operation: "load replayed message origin",
                    })?;
                ensure!(
                    source
                        .model_call_id
                        .as_ref()
                        .is_none_or(|old| old == origin),
                    ConflictSnafu {
                        id: &source.message_id
                    }
                );
                if source.model_call_id.is_none() {
                    diesel::update(message_sources::table.find(&source.message_id))
                        .set(message_sources::model_call_id.eq(origin))
                        .execute(connection)
                        .context(DatabaseSnafu {
                            operation: "attach explicit message origin",
                        })?;
                }
            }
            continue;
        }
        let owner = session::load(connection, &call.session_id)?;
        let next = owner
            .next_sequence
            .checked_add(1)
            .context(ArithmeticSnafu {
                operation: "allocate canonical message order",
            })?;
        let (role, content, metadata) = match message {
            ChatMessage::System { content, metadata } => ("system", content, metadata),
            ChatMessage::User { content, metadata } => ("user", content, metadata),
            ChatMessage::Assistant {
                content, metadata, ..
            } => ("assistant", content, metadata),
            ChatMessage::ToolResult {
                content, metadata, ..
            } => ("tool", content, metadata),
        };
        let row = Message {
            id: crate::tool_uses::message_id(invocation, offset + index),
            session_id: call.session_id.clone(),
            turn_id: invocation.turn_id.clone(),
            sequence: owner.next_sequence,
            role: role.into(),
            content: content.clone(),
            metadata: metadata.clone(),
            created_at: timestamp,
        };
        diesel::insert_into(messages::table)
            .values(row.clone())
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "append canonical message",
            })?;
        diesel::insert_into(message_sources::table)
            .values(Source {
                message_id: row.id.clone(),
                call_id: call.id.clone(),
                ordinal,
                model_call_id: batch.model_call_id.clone(),
                tool_calls_present: matches!(
                    message,
                    ChatMessage::Assistant {
                        tool_calls: Some(_),
                        ..
                    }
                ),
                metadata_secrets: metadata
                    .as_ref()
                    .map(|metadata| secret_paths(&metadata_value(metadata)))
                    .transpose()?
                    .flatten(),
            })
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "checkpoint source message",
            })?;
        match message {
            ChatMessage::Assistant {
                tool_calls: Some(calls),
                ..
            } => {
                for (index, tool) in calls.iter().enumerate() {
                    let tool = SemanticCall {
                        id: crate::tool_uses::call_id(invocation, source_index, index),
                        call_id: call.id.clone(),
                        assistant_message_id: row.id.clone(),
                        ordinal: i64::try_from(index).context(IntegerSnafu {
                            operation: "order assistant tool calls",
                        })?,
                        provider_tool_call_id: tool.id.clone(),
                        name: tool.name.clone(),
                        arguments: serde_json::to_string(&tool.arguments).context(JsonSnafu)?,
                        argument_secrets: secret_paths(&tool.arguments)?,
                        public_arguments: {
                            let text = serde_json::to_string(&tool.arguments.redacted())
                                .context(JsonSnafu)?;
                            text[..text
                                .floor_char_boundary(text.len().min(crate::MAX_TOOL_TEXT_BYTES))]
                                .into()
                        },
                        result_message_id: None,
                    };
                    diesel::insert_into(chat_tool_calls::table)
                        .values(tool)
                        .execute(connection)
                        .context(DatabaseSnafu {
                            operation: "persist canonical tool request",
                        })?;
                }
            }
            ChatMessage::ToolResult { tool_call_id, .. } => {
                let changed = diesel::update(
                    chat_tool_calls::table
                        .filter(chat_tool_calls::call_id.eq(&call.id))
                        .filter(chat_tool_calls::provider_tool_call_id.eq(tool_call_id))
                        .filter(chat_tool_calls::result_message_id.is_null()),
                )
                .set(chat_tool_calls::result_message_id.eq(&row.id))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "associate canonical tool result",
                })?;
                ensure!(changed == 1, ConflictSnafu { id: &row.id });
            }
            _ => {}
        }
        diesel::update(sessions::table.find(&owner.id))
            .set(sessions::next_sequence.eq(next))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "advance canonical message order",
            })?;
    }
    crate::tool_uses::link_executions(connection, &call.id)?;
    Ok(())
}

pub fn settle(
    connection: &mut SqliteConnection,
    invocation: &Invocation,
    messages: &[ChatMessage],
) -> Result<(), ChatError> {
    append(
        connection,
        invocation,
        &MessageBatch {
            offset: 0,
            model_call_id: None,
            messages: messages.to_vec(),
        },
    )?;
    ensure!(
        load(connection, &invocation.call_id)?.as_deref() == Some(messages),
        ConflictSnafu {
            id: &invocation.call_id
        }
    );
    diesel::update(
        message_streams::table
            .find(&invocation.call_id)
            .filter(message_streams::finished.eq(false)),
    )
    .set(message_streams::finished.eq(true))
    .execute(connection)
    .context(DatabaseSnafu {
        operation: "settle canonical message stream",
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;

impl ChatDatabase {
    pub fn record_messages(
        &self,
        invocation: &Invocation,
        batch: &MessageBatch,
    ) -> Result<(), ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0;")
            .context(DatabaseSnafu {
                operation: "configure canonical message checkpoint",
            })?;
        connection.immediate_transaction(|connection| append(connection, invocation, batch))
    }
    pub fn conversation_messages(
        &self,
        call_id: &str,
    ) -> Result<Option<Vec<ChatMessage>>, ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection.transaction(|connection| load(connection, call_id))
    }
}
