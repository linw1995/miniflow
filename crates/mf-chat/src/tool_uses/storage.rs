//! Row-oriented tool evidence and explicit, invocation-scoped message associations.

use super::*;
use crate::{ChatDatabase, schema::*, storage, turn};
use diesel::{connection::SimpleConnection, prelude::*};
use snafu::ResultExt;
use std::collections::BTreeMap;

#[derive(Queryable, Selectable, Insertable)]
#[diesel(table_name = tool_executions)]
struct Execution {
    call_id: String,
    id: String,
    ordinal: i64,
    name: String,
    phase: String,
    arguments: String,
    output: String,
    duration_ns: Option<i64>,
    model_call_id: Option<String>,
    provider_tool_call_id: Option<String>,
    semantic_call_id: Option<String>,
}

#[derive(Default, AsChangeset)]
#[diesel(table_name = tool_executions)]
struct Changes<'a> {
    name: Option<&'a str>,
    phase: Option<&'a str>,
    arguments: Option<&'a str>,
    output: Option<&'a str>,
    duration_ns: Option<Option<i64>>,
    model_call_id: Option<&'a str>,
    provider_tool_call_id: Option<&'a str>,
}

fn phase(value: ToolUsePhase) -> &'static str {
    match value {
        ToolUsePhase::Preparing => "preparing",
        ToolUsePhase::Running => "running",
        ToolUsePhase::Succeeded => "succeeded",
        ToolUsePhase::Failed => "failed",
        ToolUsePhase::Skipped => "skipped",
        ToolUsePhase::Interrupted => "interrupted",
        ToolUsePhase::Unknown => "unknown",
    }
}
fn decode_phase(value: &str) -> Result<ToolUsePhase, ChatError> {
    Ok(match value {
        "preparing" => ToolUsePhase::Preparing,
        "running" => ToolUsePhase::Running,
        "succeeded" => ToolUsePhase::Succeeded,
        "failed" => ToolUsePhase::Failed,
        "skipped" => ToolUsePhase::Skipped,
        "interrupted" => ToolUsePhase::Interrupted,
        "unknown" => ToolUsePhase::Unknown,
        _ => {
            return InvalidSnafu {
                field: "tool phase",
                reason: "unrecognized stored execution phase",
            }
            .fail();
        }
    })
}
fn clip(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// Loading a page performs a fixed number of batched queries, including canonical message projections.
pub fn load_histories(
    connection: &mut SqliteConnection,
    call_ids: &[String],
) -> Result<BTreeMap<String, ToolUseHistory>, ChatError> {
    let calls = model_calls::table
        .filter(model_calls::id.eq_any(call_ids))
        .filter(model_calls::purpose.eq("conversation"))
        .select((
            model_calls::id,
            model_calls::session_id,
            model_calls::turn_id,
            model_calls::state,
        ))
        .load::<(String, String, Option<String>, String)>(connection)
        .context(DatabaseSnafu {
            operation: "load tool evidence ownership",
        })?;
    let sources = tool_use_sources::table
        .filter(tool_use_sources::call_id.eq_any(call_ids))
        .load::<(String, bool, bool)>(connection)
        .context(DatabaseSnafu {
            operation: "load tool source checkpoints",
        })?;
    let streams = message_streams::table
        .filter(message_streams::call_id.eq_any(call_ids))
        .select(message_streams::call_id)
        .load::<String>(connection)
        .context(DatabaseSnafu {
            operation: "load message source ownership",
        })?;
    let mut histories: BTreeMap<_, _> = calls
        .into_iter()
        .filter_map(|(call_id, session_id, turn_id, state)| {
            turn_id.map(|turn_id| {
                (
                    call_id.clone(),
                    ToolUseHistory {
                        invocation: Invocation {
                            session_id,
                            turn_id,
                            call_id,
                        },
                        runs: Vec::new(),
                        calls: Vec::new(),
                        incomplete: false,
                        finished: state != "running",
                    },
                )
            })
        })
        .collect();
    let mut recorded: BTreeSet<_> = streams.into_iter().collect();
    for (id, incomplete, finished) in sources {
        recorded.insert(id.clone());
        if let Some(history) = histories.get_mut(&id) {
            history.incomplete = incomplete;
            history.finished = finished;
        }
    }
    let executions = tool_executions::table
        .filter(tool_executions::call_id.eq_any(call_ids))
        .order((
            tool_executions::call_id.asc(),
            tool_executions::ordinal.asc(),
        ))
        .select(Execution::as_select())
        .load::<Execution>(connection)
        .context(DatabaseSnafu {
            operation: "load tool execution rows",
        })?;
    let mut links: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for execution in executions {
        if let Some(id) = &execution.semantic_call_id {
            links
                .entry(id.clone())
                .or_default()
                .push(execution.id.clone());
        }
        if let Some(history) = histories.get_mut(&execution.call_id) {
            history.runs.push(ToolUse {
                id: execution.id,
                name: execution.name,
                phase: decode_phase(&execution.phase)?,
                arguments: execution.arguments,
                output: execution.output,
                duration_ns: execution.duration_ns,
                origin: execution
                    .model_call_id
                    .zip(execution.provider_tool_call_id)
                    .map(|(model_call_id, tool_call_id)| ToolOrigin {
                        model_call_id,
                        tool_call_id,
                    }),
            });
        }
    }
    // Read only bounded display copies here. Complete arguments/results remain in their canonical rows.
    let semantic = chat_tool_calls::table
        .inner_join(
            message_sources::table
                .on(chat_tool_calls::assistant_message_id.eq(message_sources::message_id)),
        )
        .filter(chat_tool_calls::call_id.eq_any(call_ids))
        .order((
            chat_tool_calls::call_id.asc(),
            message_sources::ordinal.asc(),
            chat_tool_calls::ordinal.asc(),
        ))
        .select((
            chat_tool_calls::call_id,
            chat_tool_calls::id,
            chat_tool_calls::assistant_message_id,
            message_sources::model_call_id,
            chat_tool_calls::provider_tool_call_id,
            chat_tool_calls::name,
            chat_tool_calls::public_arguments,
            chat_tool_calls::result_message_id,
        ))
        .load::<(
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
        )>(connection)
        .context(DatabaseSnafu {
            operation: "load semantic tool display rows",
        })?;
    let result_ids: Vec<_> = semantic.iter().filter_map(|row| row.7.as_ref()).collect();
    let results = messages::table
        .filter(messages::id.eq_any(result_ids))
        .select((
            messages::id,
            diesel::dsl::sql::<diesel::sql_types::Text>("substr(messages.content, 1, 2048)"),
        ))
        .load::<(String, String)>(connection)
        .context(DatabaseSnafu {
            operation: "load tool result display copies",
        })?;
    let results: BTreeMap<_, _> = results.into_iter().collect();
    for (call_id, id, assistant_message_id, model_call_id, tool_call_id, name, arguments, result) in
        semantic
    {
        if let Some(history) = histories.get_mut(&call_id) {
            if history.calls.len() == MAX_TOOL_USES {
                history.incomplete = true;
                continue;
            }
            history.calls.push(ChatToolUse {
                execution_ids: links.remove(&id).unwrap_or_default(),
                id,
                assistant_message_id,
                model_call_id,
                tool_call_id,
                name: clip(&name, MAX_TOOL_NAME_BYTES),
                arguments: clip(&arguments, MAX_TOOL_TEXT_BYTES),
                output: result
                    .and_then(|id| results.get(&id).map(|text| clip(text, MAX_TOOL_TEXT_BYTES))),
            });
        }
    }
    histories.retain(|id, _| recorded.contains(id));
    Ok(histories)
}

/// Attach only a unique, explicitly matching (conversation, model call, provider tool-call ID).
/// Names, parameters, ordering, timestamps and returned messages are not execution-success evidence.
pub fn link_executions(connection: &mut SqliteConnection, call_id: &str) -> Result<(), ChatError> {
    let targets = chat_tool_calls::table
        .inner_join(
            message_sources::table
                .on(chat_tool_calls::assistant_message_id.eq(message_sources::message_id)),
        )
        .filter(chat_tool_calls::call_id.eq(call_id))
        .select((
            chat_tool_calls::id,
            message_sources::model_call_id,
            chat_tool_calls::provider_tool_call_id,
        ))
        .load::<(String, Option<String>, String)>(connection)
        .context(DatabaseSnafu {
            operation: "resolve explicit tool message identities",
        })?;
    let mut identities: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for (id, model, tool) in targets {
        if let Some(model) = model {
            identities.entry((model, tool)).or_default().push(id);
        }
    }
    let executions = tool_executions::table
        .filter(tool_executions::call_id.eq(call_id))
        .select((
            tool_executions::id,
            tool_executions::model_call_id,
            tool_executions::provider_tool_call_id,
            tool_executions::semantic_call_id,
        ))
        .load::<(String, Option<String>, Option<String>, Option<String>)>(connection)
        .context(DatabaseSnafu {
            operation: "resolve tool execution associations",
        })?;
    for (id, model, tool, previous) in executions {
        let target = model
            .zip(tool)
            .and_then(|identity| identities.get(&identity))
            .and_then(|targets| match targets.as_slice() {
                [target] => Some(target),
                _ => None,
            });
        // Late provenance may remove uniqueness. Keep every fact, but never retain an ambiguous link.
        if previous.as_ref() != target {
            diesel::update(tool_executions::table.find((call_id, &id)))
                .set(tool_executions::semantic_call_id.eq(target))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "associate observed execution with canonical tool call",
                })?;
        }
    }
    Ok(())
}

pub fn save(
    connection: &mut SqliteConnection,
    invocation: &Invocation,
    history: &ToolUseHistory,
) -> Result<(), ChatError> {
    history.validate()?;
    ensure!(
        history.invocation == *invocation,
        ConflictSnafu {
            id: &invocation.call_id
        }
    );
    let call = turn::load_call(connection, &invocation.call_id)?;
    ensure!(
        call.session_id == invocation.session_id
            && call.turn_id.as_deref() == Some(&invocation.turn_id)
            && call.purpose == "conversation"
            && (call.state == "running" || history.finished),
        ConflictSnafu { id: &call.id }
    );
    let source = tool_use_sources::table
        .find(&call.id)
        .select((tool_use_sources::incomplete, tool_use_sources::finished))
        .first::<(bool, bool)>(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "read tool source checkpoint",
        })?;
    let previous = tool_executions::table
        .filter(tool_executions::call_id.eq(&call.id))
        .order(tool_executions::ordinal.asc())
        .select(Execution::as_select())
        .load::<Execution>(connection)
        .context(DatabaseSnafu {
            operation: "read prior tool execution rows",
        })?;
    if let Some((incomplete, true)) = source {
        ensure!(
            history.finished
                && previous
                    .iter()
                    .all(|old| history.runs.iter().any(|new| old.id == new.id
                        && (old.phase == phase(new.phase)
                            || incomplete
                                && matches!(old.phase.as_str(), "unknown" | "interrupted")))),
            ConflictSnafu { id: &call.id }
        );
        if !incomplete {
            ensure!(
                !history.incomplete && previous.len() == history.runs.len(),
                ConflictSnafu { id: &call.id }
            );
        }
    }
    ensure!(
        previous
            .iter()
            .all(|old| history.runs.iter().any(|new| old.id == new.id)),
        ConflictSnafu { id: &call.id }
    );
    match source {
        None => {
            diesel::insert_into(tool_use_sources::table)
                .values((
                    tool_use_sources::call_id.eq(&call.id),
                    tool_use_sources::incomplete.eq(history.incomplete),
                    tool_use_sources::finished.eq(history.finished),
                ))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "begin tool source checkpoint",
                })?;
        }
        Some((incomplete, finished))
            if (incomplete, finished) != (history.incomplete, history.finished) =>
        {
            diesel::update(tool_use_sources::table.find(&call.id))
                .set((
                    tool_use_sources::incomplete.eq(history.incomplete),
                    tool_use_sources::finished.eq(history.finished),
                ))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "advance tool source checkpoint",
                })?;
        }
        _ => {}
    }
    let previous: BTreeMap<_, _> = previous
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    for (index, run) in history.runs.iter().enumerate() {
        let origin = run.origin.as_ref();
        let model = origin.map(|origin| origin.model_call_id.as_str());
        let tool = origin.map(|origin| origin.tool_call_id.as_str());
        if let Some(old) = previous.get(&run.id) {
            ensure!(
                old.model_call_id
                    .as_deref()
                    .is_none_or(|id| Some(id) == model)
                    && old
                        .provider_tool_call_id
                        .as_deref()
                        .is_none_or(|id| Some(id) == tool),
                ConflictSnafu { id: &run.id }
            );
            if source.is_some_and(|(_, finished)| finished) {
                ensure!(
                    (old.name == run.name || old.name == "tool (name unobserved)")
                        && (old.arguments.is_empty() || old.arguments == run.arguments)
                        && (old.output.is_empty() || old.output == run.output)
                        && old
                            .duration_ns
                            .is_none_or(|duration| Some(duration) == run.duration_ns),
                    ConflictSnafu { id: &run.id }
                );
            }
            let changes = Changes {
                name: (old.name != run.name).then_some(run.name.as_str()),
                phase: (old.phase != phase(run.phase)).then_some(phase(run.phase)),
                arguments: (old.arguments != run.arguments).then_some(run.arguments.as_str()),
                output: (old.output != run.output).then_some(run.output.as_str()),
                duration_ns: (old.duration_ns != run.duration_ns).then_some(run.duration_ns),
                model_call_id: if old.model_call_id.is_none() {
                    model
                } else {
                    None
                },
                provider_tool_call_id: if old.provider_tool_call_id.is_none() {
                    tool
                } else {
                    None
                },
            };
            let payload_dirty = changes.name.is_some()
                || changes.phase.is_some()
                || changes.arguments.is_some()
                || changes.output.is_some()
                || changes.duration_ns.is_some();
            let dirty = payload_dirty
                || changes.model_call_id.is_some()
                || changes.provider_tool_call_id.is_some();
            // Complete observations cannot be overwritten by a different replay, even within the same phase.
            ensure!(
                !source.is_some_and(|(incomplete, finished)| finished && !incomplete)
                    || !payload_dirty,
                ConflictSnafu { id: &run.id }
            );
            if dirty {
                diesel::update(tool_executions::table.find((&call.id, &run.id)))
                    .set(changes)
                    .execute(connection)
                    .context(DatabaseSnafu {
                        operation: "update dirty tool execution fields",
                    })?;
            }
        } else {
            diesel::insert_into(tool_executions::table)
                .values(Execution {
                    call_id: call.id.clone(),
                    id: run.id.clone(),
                    ordinal: i64::try_from(index).context(IntegerSnafu {
                        operation: "order tool executions",
                    })?,
                    name: run.name.clone(),
                    phase: phase(run.phase).into(),
                    arguments: run.arguments.clone(),
                    output: run.output.clone(),
                    duration_ns: run.duration_ns,
                    model_call_id: model.map(str::to_owned),
                    provider_tool_call_id: tool.map(str::to_owned),
                    semantic_call_id: None,
                })
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "insert observed tool execution",
                })?;
        }
    }
    link_executions(connection, &call.id)
}

impl ChatDatabase {
    pub fn tool_uses(&self, call_id: &str) -> Result<Option<ToolUseHistory>, ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection.transaction(|connection| {
            Ok(load_histories(connection, &[call_id.to_owned()])?.remove(call_id))
        })
    }
    /// This subscriber's writes never wait on frontend rendering or rewrite model-call payloads.
    pub fn record_tool_uses(
        &self,
        invocation: &Invocation,
        history: &ToolUseHistory,
    ) -> Result<(), ChatError> {
        let mut connection = storage::connect(self.path())?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0;")
            .context(DatabaseSnafu {
                operation: "configure tool evidence checkpoint",
            })?;
        connection.immediate_transaction(|connection| save(connection, invocation, history))
    }
}

pub fn seal_call(
    connection: &mut SqliteConnection,
    call_id: &str,
    pending: ToolUsePhase,
) -> Result<(), ChatError> {
    let source = tool_use_sources::table
        .find(call_id)
        .select(tool_use_sources::finished)
        .first::<bool>(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "load unfinished tool source",
        })?;
    if source == Some(false) {
        diesel::update(
            tool_executions::table
                .filter(tool_executions::call_id.eq(call_id))
                .filter(tool_executions::phase.eq_any(["preparing", "running"])),
        )
        .set(tool_executions::phase.eq(phase(pending)))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "settle pending tool executions",
        })?;
        diesel::update(tool_use_sources::table.find(call_id))
            .set((
                tool_use_sources::incomplete.eq(true),
                tool_use_sources::finished.eq(true),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "settle unfinished tool source",
            })?;
    }
    Ok(())
}

pub fn recover(connection: &mut SqliteConnection) -> Result<(), ChatError> {
    let calls = model_calls::table
        .filter(model_calls::state.eq("running"))
        .select(model_calls::id)
        .load::<String>(connection)
        .context(DatabaseSnafu {
            operation: "find interrupted tool sources",
        })?;
    for call in calls {
        seal_call(connection, &call, ToolUsePhase::Interrupted)?;
    }
    Ok(())
}
