//! Complete, authoritative Flow context versions. Payload interning never establishes message identity.

use crate::message_log::{metadata_value, restore_secrets, secret_paths};
use crate::{ChatDatabase, ChatError, ChatMessage, Invocation, error::*, schema::*, storage, turn};
use diesel::{connection::SimpleConnection, prelude::*};
use mf_runtime::ValueRef;
use serde::Serialize;
use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, Queryable, Selectable, Insertable, Serialize)]
#[diesel(table_name = conversation_payloads)]
struct Payload {
    session_id: String,
    id: String,
    role: String,
    content: String,
    metadata: Option<String>,
    metadata_secrets: Option<String>,
    tool_calls_present: bool,
    tool_call_id: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Queryable, Selectable, Insertable, Serialize)]
#[diesel(table_name = conversation_payload_tools)]
struct Tool {
    session_id: String,
    payload_id: String,
    ordinal: i64,
    provider_tool_call_id: String,
    name: String,
    arguments: String,
    argument_secrets: Option<String>,
    public_arguments: String,
}

fn encode(session: &str, message: &ChatMessage) -> Result<(Payload, Vec<Tool>), ChatError> {
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
    let mut payload = Payload {
        session_id: session.into(),
        id: String::new(),
        role: role.into(),
        content: content.clone(),
        metadata: metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context(JsonSnafu)?,
        metadata_secrets: metadata
            .as_ref()
            .map(|value| secret_paths(&metadata_value(value)))
            .transpose()?
            .flatten(),
        tool_calls_present: matches!(
            message,
            ChatMessage::Assistant {
                tool_calls: Some(_),
                ..
            }
        ),
        tool_call_id: match message {
            ChatMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        },
    };
    let mut tools = Vec::new();
    if let ChatMessage::Assistant {
        tool_calls: Some(calls),
        ..
    } = message
    {
        for (index, call) in calls.iter().enumerate() {
            let public = serde_json::to_string(&call.arguments.redacted()).context(JsonSnafu)?;
            tools.push(Tool {
                session_id: session.into(),
                payload_id: String::new(),
                ordinal: i64::try_from(index).context(IntegerSnafu {
                    operation: "order context tool calls",
                })?,
                provider_tool_call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments).context(JsonSnafu)?,
                argument_secrets: secret_paths(&call.arguments)?,
                public_arguments: public
                    [..public.floor_char_boundary(public.len().min(crate::MAX_TOOL_TEXT_BYTES))]
                    .into(),
            });
        }
    }
    // Include native sensitivity in the storage key. Equal public JSON is not equal protected data.
    let encoded = serde_json::to_vec(&(&payload, &tools)).context(JsonSnafu)?;
    payload.id = Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    for tool in &mut tools {
        tool.payload_id = payload.id.clone();
    }
    Ok((payload, tools))
}

pub fn load_many(
    connection: &mut SqliteConnection,
    ids: &[String],
) -> Result<BTreeMap<String, Vec<ChatMessage>>, ChatError> {
    let states = conversation_states::table
        .filter(conversation_states::call_id.eq_any(ids))
        .select(conversation_states::call_id)
        .load::<String>(connection)
        .context(DatabaseSnafu {
            operation: "load conversation context versions",
        })?;
    let entries = conversation_state_entries::table
        .inner_join(
            conversation_payloads::table.on(conversation_state_entries::session_id
                .eq(conversation_payloads::session_id)
                .and(conversation_state_entries::payload_id.eq(conversation_payloads::id))),
        )
        .filter(conversation_state_entries::call_id.eq_any(ids))
        .order((
            conversation_state_entries::call_id.asc(),
            conversation_state_entries::ordinal.asc(),
        ))
        .select((conversation_state_entries::call_id, Payload::as_select()))
        .load::<(String, Payload)>(connection)
        .context(DatabaseSnafu {
            operation: "load ordered conversation context",
        })?;
    let tools = conversation_payload_tools::table
        .inner_join(
            conversation_state_entries::table.on(conversation_payload_tools::session_id
                .eq(conversation_state_entries::session_id)
                .and(
                    conversation_payload_tools::payload_id
                        .eq(conversation_state_entries::payload_id),
                )),
        )
        .filter(conversation_state_entries::call_id.eq_any(ids))
        .order(conversation_payload_tools::ordinal.asc())
        .select(Tool::as_select())
        .distinct()
        .load::<Tool>(connection)
        .context(DatabaseSnafu {
            operation: "load context tool-call payloads",
        })?;
    let mut by_payload: BTreeMap<_, Vec<mfn_llm::ToolCall>> = BTreeMap::new();
    for tool in tools {
        by_payload
            .entry((tool.session_id, tool.payload_id))
            .or_default()
            .push(mfn_llm::ToolCall {
                id: tool.provider_tool_call_id,
                name: tool.name,
                arguments: restore_secrets(
                    serde_json::from_str(&tool.arguments).context(JsonSnafu)?,
                    tool.argument_secrets.as_deref(),
                )?,
            });
    }
    let mut result: BTreeMap<_, Vec<_>> = states.into_iter().map(|id| (id, Vec::new())).collect();
    for (call, payload) in entries {
        let metadata = payload
            .metadata
            .as_deref()
            .map(|encoded| -> Result<mfn_llm::MessageMetadata, ChatError> {
                let value = restore_secrets(
                    serde_json::from_str::<ValueRef>(encoded).context(JsonSnafu)?,
                    payload.metadata_secrets.as_deref(),
                )?;
                Ok(value
                    .as_object()
                    .context(InvalidSnafu {
                        field: "context metadata",
                        reason: "must be an object",
                    })?
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.clone()))
                    .collect())
            })
            .transpose()?;
        let message = match payload.role.as_str() {
            "system" => ChatMessage::System {
                content: payload.content,
                metadata,
            },
            "user" => ChatMessage::User {
                content: payload.content,
                metadata,
            },
            "assistant" => ChatMessage::Assistant {
                content: payload.content,
                metadata,
                tool_calls: payload.tool_calls_present.then(|| {
                    by_payload
                        .get(&(payload.session_id, payload.id))
                        .cloned()
                        .unwrap_or_default()
                }),
            },
            "tool" => ChatMessage::ToolResult {
                content: payload.content,
                metadata,
                tool_call_id: payload.tool_call_id.context(InvalidSnafu {
                    field: "context tool result",
                    reason: "requires a tool-call identity",
                })?,
            },
            _ => {
                return InvalidSnafu {
                    field: "context role",
                    reason: "unknown stored role",
                }
                .fail();
            }
        };
        result
            .get_mut(&call)
            .expect("loaded state owns its entries")
            .push(message);
    }
    Ok(result)
}

pub fn load(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<Option<Vec<ChatMessage>>, ChatError> {
    Ok(load_many(connection, &[id.into()])?.remove(id))
}

pub fn save(
    connection: &mut SqliteConnection,
    invocation: &Invocation,
    messages: &[ChatMessage],
) -> Result<(), ChatError> {
    let call = turn::load_call(connection, &invocation.call_id)?;
    ensure!(
        call.session_id == invocation.session_id
            && call.turn_id.as_deref() == Some(&invocation.turn_id)
            && call.purpose == "conversation",
        ConflictSnafu { id: &call.id }
    );
    crate::message_log::validate(messages, false)?;
    if let Some(previous) = load(connection, &call.id)? {
        ensure!(previous == messages, ConflictSnafu { id: &call.id });
        return Ok(());
    }
    ensure!(
        matches!(
            call.state.as_str(),
            "running" | "failed" | "cancelled" | "interrupted"
        ),
        ConflictSnafu { id: &call.id }
    );
    diesel::insert_into(conversation_states::table)
        .values((
            conversation_states::call_id.eq(&call.id),
            conversation_states::session_id.eq(&call.session_id),
            conversation_states::finished.eq(false),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "begin authoritative context version",
        })?;
    for (ordinal, message) in messages.iter().enumerate() {
        let (payload, tools) = encode(&call.session_id, message)?;
        let previous = conversation_payloads::table
            .find((&payload.session_id, &payload.id))
            .select(Payload::as_select())
            .first::<Payload>(connection)
            .optional()
            .context(DatabaseSnafu {
                operation: "find interned context payload",
            })?;
        if let Some(previous) = previous {
            ensure!(previous == payload, ConflictSnafu { id: &payload.id });
            let previous_tools = conversation_payload_tools::table
                .filter(conversation_payload_tools::session_id.eq(&payload.session_id))
                .filter(conversation_payload_tools::payload_id.eq(&payload.id))
                .order(conversation_payload_tools::ordinal.asc())
                .select(Tool::as_select())
                .load::<Tool>(connection)
                .context(DatabaseSnafu {
                    operation: "verify interned tool payload",
                })?;
            ensure!(previous_tools == tools, ConflictSnafu { id: &payload.id });
        } else {
            diesel::insert_into(conversation_payloads::table)
                .values(&payload)
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "save immutable context payload",
                })?;
            if !tools.is_empty() {
                diesel::insert_into(conversation_payload_tools::table)
                    .values(&tools)
                    .execute(connection)
                    .context(DatabaseSnafu {
                        operation: "save immutable context tools",
                    })?;
            }
        }
        diesel::insert_into(conversation_state_entries::table)
            .values((
                conversation_state_entries::call_id.eq(&call.id),
                conversation_state_entries::session_id.eq(&call.session_id),
                conversation_state_entries::ordinal.eq(i64::try_from(ordinal).context(
                    IntegerSnafu {
                        operation: "order context version",
                    },
                )?),
                conversation_state_entries::payload_id.eq(&payload.id),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "save context membership",
            })?;
    }
    Ok(())
}

/// A transcript reply row is a display/audit index into the result, never reconstructed model history.
pub fn display_reply(
    connection: &mut SqliteConnection,
    invocation: &Invocation,
    messages: &[ChatMessage],
) -> Result<(), ChatError> {
    let Some(ChatMessage::Assistant {
        content, metadata, ..
    }) = messages.last()
    else {
        return Ok(());
    };
    if crate::message_log::load(connection, &invocation.call_id)?
        .as_ref()
        .and_then(|messages| messages.last())
        == messages.last()
    {
        return Ok(());
    }
    let owner = crate::session::load(connection, &invocation.session_id)?;
    let next = owner
        .next_sequence
        .checked_add(1)
        .context(ArithmeticSnafu {
            operation: "order context reply display",
        })?;
    diesel::insert_into(messages::table)
        .values(crate::Message {
            id: format!("{}:reply", invocation.call_id),
            session_id: invocation.session_id.clone(),
            turn_id: invocation.turn_id.clone(),
            sequence: owner.next_sequence,
            role: "assistant".into(),
            content: content.clone(),
            metadata: metadata.as_ref().map(|values| {
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.redacted()))
                    .collect()
            }),
            created_at: crate::session::now()?,
        })
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "index canonical context reply",
        })?;
    diesel::update(sessions::table.find(&owner.id))
        .set(sessions::next_sequence.eq(next))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "advance reply display order",
        })?;
    Ok(())
}

/// Bounded display projections for terminal-only Flows. Payload sharing supplies no execution provenance.
pub fn tool_projections(
    connection: &mut SqliteConnection,
    ids: &[String],
) -> Result<BTreeMap<String, Vec<crate::ChatToolUse>>, ChatError> {
    let states = conversation_states::table
        .filter(conversation_states::call_id.eq_any(ids))
        .select(conversation_states::call_id)
        .load::<String>(connection)
        .context(DatabaseSnafu {
            operation: "load context projection owners",
        })?;
    let tools = conversation_state_entries::table
        .inner_join(
            conversation_payload_tools::table.on(conversation_state_entries::session_id
                .eq(conversation_payload_tools::session_id)
                .and(
                    conversation_state_entries::payload_id
                        .eq(conversation_payload_tools::payload_id),
                )),
        )
        .filter(conversation_state_entries::call_id.eq_any(ids))
        .order((
            conversation_state_entries::call_id.asc(),
            conversation_state_entries::ordinal.asc(),
            conversation_payload_tools::ordinal.asc(),
        ))
        .select((
            conversation_state_entries::call_id,
            conversation_state_entries::ordinal,
            conversation_payload_tools::ordinal,
            conversation_payload_tools::provider_tool_call_id,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "substr(conversation_payload_tools.name, 1, 256)",
            ),
            conversation_payload_tools::public_arguments,
        ))
        .load::<(String, i64, i64, String, String, String)>(connection)
        .context(DatabaseSnafu {
            operation: "load context tool previews",
        })?;
    let results = conversation_state_entries::table
        .inner_join(
            conversation_payloads::table.on(conversation_state_entries::session_id
                .eq(conversation_payloads::session_id)
                .and(conversation_state_entries::payload_id.eq(conversation_payloads::id))),
        )
        .filter(conversation_state_entries::call_id.eq_any(ids))
        .filter(conversation_payloads::role.eq("tool"))
        .order((
            conversation_state_entries::call_id.asc(),
            conversation_state_entries::ordinal.asc(),
        ))
        .select((
            conversation_state_entries::call_id,
            conversation_state_entries::ordinal,
            conversation_payloads::tool_call_id,
            diesel::dsl::sql::<diesel::sql_types::Text>(
                "substr(conversation_payloads.content, 1, 2048)",
            ),
        ))
        .load::<(String, i64, Option<String>, String)>(connection)
        .context(DatabaseSnafu {
            operation: "load context result previews",
        })?;
    let mut projections: BTreeMap<_, Vec<_>> =
        states.into_iter().map(|id| (id, Vec::new())).collect();
    let mut positions: BTreeMap<(String, String), Vec<(i64, usize)>> = BTreeMap::new();
    for (call, message, ordinal, tool, name, arguments) in tools {
        let calls = projections.get_mut(&call).expect("context owns its tools");
        positions
            .entry((call.clone(), tool.clone()))
            .or_default()
            .push((message, calls.len()));
        calls.push(crate::ChatToolUse {
            context_only: true,
            id: format!("{call}:tool:{message}:{ordinal}"),
            assistant_message_id: format!("{call}:message:{message}"),
            model_call_id: None,
            tool_call_id: tool,
            name: name[..name.floor_char_boundary(name.len().min(crate::MAX_TOOL_NAME_BYTES))]
                .into(),
            arguments,
            output: None,
            execution_ids: Vec::new(),
        });
    }
    for (call, ordinal, tool, content) in results {
        if let Some(tool) = tool
            && let Some(index) = positions
                .get(&(call.clone(), tool))
                .and_then(|positions| {
                    positions
                        .iter()
                        .rev()
                        .find(|(message, _)| *message < ordinal)
                })
                .map(|(_, index)| *index)
        {
            projections.get_mut(&call).expect("context owns result")[index].output = Some(
                content
                    [..content.floor_char_boundary(content.len().min(crate::MAX_TOOL_TEXT_BYTES))]
                    .into(),
            );
        }
    }
    Ok(projections)
}

impl ChatDatabase {
    pub fn record_conversation_state(
        &self,
        invocation: &Invocation,
        messages: &[ChatMessage],
    ) -> Result<(), ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0;")
            .context(DatabaseSnafu {
                operation: "configure context checkpoint",
            })?;
        connection.immediate_transaction(|connection| save(connection, invocation, messages))
    }
}

#[cfg(test)]
mod tests;
