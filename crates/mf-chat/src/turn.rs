use crate::{
    ChatDatabase, ChatError, ModelCall, Turn,
    error::*,
    operation,
    schema::{messages, model_calls, sessions, turns},
    session, storage,
};
use diesel::prelude::*;
use mf_runtime::NodeValue;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct Invocation {
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
}

impl Invocation {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: uuid::Uuid::new_v4().to_string(),
            call_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ModelIdentity {
    pub profile: String,
    pub model: Option<String>,
    pub config_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usages: Option<Vec<mfn_llm::CallUsage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<mfn_llm::MessageMetadata>,
    pub text: String,
    pub usage: Option<BTreeMap<String, i64>>,
    pub finish_reason: Option<String>,
}

impl From<mfn_llm::LlmOutputs> for Reply {
    fn from(value: mfn_llm::LlmOutputs) -> Self {
        Self {
            usages: Some(value.usages),
            metadata: value.metadata,
            text: value.text,
            usage: value.usage,
            finish_reason: value.finish_reason,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct Completion {
    pub invocation: Invocation,
    pub reply: Reply,
}

#[derive(Clone, Debug, NodeValue)]
pub struct BeginTurnInputs {
    pub database_path: String,
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub prompt: String,
    pub model: ModelIdentity,
    pub retry_of: Option<String>,
    #[value(default = "session::default_history_bytes")]
    pub max_history_bytes: usize,
}

#[derive(Clone, Debug, NodeValue)]
pub struct TurnStarted {
    pub database_path: String,
    pub invocation: Invocation,
    pub request: mfn_llm::LlmInputs,
}

#[derive(Clone, Debug, NodeValue)]
pub struct CompleteTurnInputs {
    pub database_path: String,
    pub invocation: Invocation,
    pub response: Reply,
}

#[derive(Clone, Debug, NodeValue)]
pub struct CompletionOutputs {
    pub completion: Completion,
}

#[derive(Clone, Debug, NodeValue)]
pub struct FailTurnInputs {
    pub usages: Option<Vec<mfn_llm::CallUsage>>,
    pub database_path: String,
    pub invocation: Invocation,
    pub state: String,
    pub error: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct FailCallInputs {
    pub usages: Option<Vec<mfn_llm::CallUsage>>,
    pub database_path: String,
    pub call_id: String,
    pub state: String,
    pub error: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct Changed {
    pub changed: bool,
}

#[derive(Clone, Debug, NodeValue)]
pub struct RecordInputs {
    pub database_path: String,
    pub id: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct TurnOutputs {
    pub turn: Turn,
}

#[derive(Clone, Debug, NodeValue)]
pub struct TurnRequestOutputs {
    pub turn: Turn,
    pub prompt: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct CallOutputs {
    pub call: ModelCall,
}

#[derive(Clone, Debug, NodeValue)]
pub struct RecoveryInputs {
    pub database_path: String,
}

#[derive(Clone, Debug, NodeValue)]
pub struct RecoveryCounts {
    pub turns: i64,
    pub calls: i64,
}

#[derive(Clone, Debug, NodeValue)]
pub struct RecoveryOutputs {
    pub recovered: RecoveryCounts,
}

pub fn load_turn(connection: &mut SqliteConnection, id: &str) -> Result<Turn, ChatError> {
    turns::table
        .find(id)
        .select(Turn::as_select())
        .first(connection)
        .optional()
        .context(DatabaseSnafu {
            operation: "load chat turn",
        })?
        .context(TurnMissingSnafu { id })
}

pub fn load_call(connection: &mut SqliteConnection, id: &str) -> Result<ModelCall, ChatError> {
    let record: diesel::QueryResult<Option<ModelCall>> = model_calls::table
        .find(id)
        .first::<ModelCall>(connection)
        .optional();
    record
        .context(DatabaseSnafu {
            operation: "load model call",
        })?
        .context(CallMissingSnafu { id })
}

pub fn begin(input: BeginTurnInputs) -> Result<TurnStarted, ChatError> {
    ensure!(
        !input.prompt.is_empty(),
        InvalidSnafu {
            field: "prompt",
            reason: "must be nonempty"
        }
    );
    ensure!(
        !input.turn_id.is_empty() && !input.call_id.is_empty(),
        InvalidSnafu {
            field: "invocation",
            reason: "identifiers must be nonempty"
        }
    );
    ensure!(
        input
            .model
            .model
            .as_ref()
            .is_none_or(|model| !model.trim().is_empty())
            && !input.model.config_id.is_empty(),
        InvalidSnafu {
            field: "model",
            reason: "requires model and configuration identity"
        }
    );
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let owner = session::load(connection, &input.session_id)?;
        ensure!(!owner.archived, SessionArchivedSnafu { id: &owner.id });
        ensure!(
            owner.profile == input.model.profile,
            InvalidSnafu {
                field: "profile",
                reason: "does not match the session profile"
            }
        );
        let active = turns::table
            .filter(turns::session_id.eq(&owner.id))
            .filter(turns::state.eq("running"))
            .select(turns::id)
            .first::<String>(connection)
            .optional()
            .context(DatabaseSnafu {
                operation: "check active conversation turn",
            })?;
        ensure!(active.is_none(), SessionBusySnafu { id: &owner.id });
        if let Some(original) = &input.retry_of {
            let previous = load_turn(connection, original)?;
            ensure!(
                previous.session_id == owner.id
                    && matches!(
                        previous.state.as_str(),
                        "failed" | "cancelled" | "interrupted"
                    ),
                InvalidSnafu {
                    field: "retry_of",
                    reason: "must identify an unsuccessful turn in this session"
                }
            );
        }
        let context = session::history(connection, &owner.id, input.max_history_bytes)?;
        let timestamp = session::now()?;
        let next = owner
            .next_sequence
            .checked_add(2)
            .context(ArithmeticSnafu {
                operation: "reserve message order",
            })?;
        let turn = Turn {
            id: input.turn_id.clone(),
            session_id: owner.id.clone(),
            retry_of: input.retry_of.clone(),
            sequence: owner.next_sequence,
            state: "running".into(),
            created_at: timestamp,
            finished_at: None,
            error: None,
        };
        let message = crate::Message {
            metadata: None,
            id: uuid::Uuid::new_v4().to_string(),
            session_id: owner.id.clone(),
            turn_id: turn.id.clone(),
            sequence: turn.sequence,
            role: "user".into(),
            content: input.prompt.clone(),
            created_at: timestamp,
        };
        let call = ModelCall {
            usages: None,
            metadata: None,
            id: input.call_id.clone(),
            session_id: owner.id.clone(),
            turn_id: Some(turn.id.clone()),
            purpose: "conversation".into(),
            profile: input.model.profile.clone(),
            model: input.model.model.clone(),
            config_id: input.model.config_id.clone(),
            state: "running".into(),
            title_revision: None,
            started_at: Some(timestamp),
            finished_at: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            finish_reason: None,
            error: None,
            result_text: None,
            disposition: None,
        };
        diesel::insert_into(turns::table)
            .values(&turn)
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "begin chat turn",
            })?;
        diesel::insert_into(messages::table)
            .values(message)
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "save user message",
            })?;
        diesel::insert_into(model_calls::table)
            .values(call.clone())
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "begin conversation call",
            })?;
        diesel::update(sessions::table.find(&owner.id))
            .set((
                sessions::next_sequence.eq(next),
                sessions::updated_at.eq(timestamp),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "reserve session message order",
            })?;
        Ok(TurnStarted {
            database_path: input.database_path.clone(),
            invocation: Invocation {
                session_id: owner.id,
                turn_id: turn.id,
                call_id: call.id,
            },
            request: mfn_llm::LlmInputs {
                prompt: input.prompt.clone(),
                system: context.system,
                history: Some(context.history),
            },
        })
    })
}

pub fn validate_usages(usages: &[mfn_llm::CallUsage]) -> Result<(), ChatError> {
    let mut identities = std::collections::BTreeSet::new();
    for usage in usages {
        ensure!(
            !usage.call_id.trim().is_empty()
                && usage.started_at.is_none_or(|timestamp| timestamp >= 0)
                && identities.insert(&usage.call_id)
                && !usage.model.trim().is_empty()
                && usage
                    .error
                    .as_ref()
                    .is_none_or(|error| !error.trim().is_empty()),
            InvalidSnafu {
                field: "usages",
                reason: "requires unique call identities, model names, and nonblank errors"
            }
        );
        if let Some(node) = &usage.node {
            ensure!(
                !node.id.is_empty()
                    && !node.kind.is_empty()
                    && node
                        .path
                        .iter()
                        .all(|scope| !scope.node.is_empty() && scope.index >= 0),
                InvalidSnafu {
                    field: "usages",
                    reason: "requires valid node identities and scope indices"
                }
            );
        }
        validate_usage(usage.usage.as_ref())?;
    }
    Ok(())
}

fn validate_usage(usage: Option<&BTreeMap<String, i64>>) -> Result<(), ChatError> {
    if let Some(usage) = usage {
        ensure!(
            usage.iter().all(|(key, count)| matches!(
                key.as_str(),
                "input_tokens" | "output_tokens" | "total_tokens"
            ) && *count >= 0),
            InvalidSnafu {
                field: "usage",
                reason: "requires supported nonnegative counters"
            }
        );
    }
    Ok(())
}

pub fn validate_reply(reply: &Reply) -> Result<(), ChatError> {
    ensure!(
        !reply.text.is_empty(),
        InvalidSnafu {
            field: "text",
            reason: "must be nonempty"
        }
    );
    validate_usage(reply.usage.as_ref())?;
    if let Some(usages) = &reply.usages {
        validate_usages(usages)?;
        ensure!(
            reply.usage.is_none() || (usages.len() == 1 && usages[0].usage == reply.usage),
            InvalidSnafu {
                field: "usage",
                reason: "scalar usage must match the single reported call"
            }
        );
    }

    Ok(())
}

pub fn reply_matches(call: &ModelCall, reply: &Reply) -> bool {
    let count = |key: &str| {
        reply
            .usage
            .as_ref()
            .and_then(|usage| usage.get(key))
            .copied()
    };
    call.result_text.as_deref() == Some(&reply.text)
        && call.input_tokens == count("input_tokens")
        && call.output_tokens == count("output_tokens")
        && call.total_tokens == count("total_tokens")
        && call.finish_reason == reply.finish_reason
        && call.metadata == reply.metadata
        && call.usages == reply.usages
}

pub fn saved_reply(call: &ModelCall) -> Result<Reply, ChatError> {
    let mut usage = BTreeMap::new();
    for (key, count) in [
        ("input_tokens", call.input_tokens),
        ("output_tokens", call.output_tokens),
        ("total_tokens", call.total_tokens),
    ] {
        if let Some(count) = count {
            usage.insert(key.into(), count);
        }
    }
    Ok(Reply {
        usages: call.usages.clone(),
        metadata: call.metadata.clone(),
        text: call
            .result_text
            .clone()
            .context(ConflictSnafu { id: &call.id })?,
        usage: (!usage.is_empty()).then_some(usage),
        finish_reason: call.finish_reason.clone(),
    })
}

pub fn save_response(
    connection: &mut SqliteConnection,
    call_id: &str,
    reply: &Reply,
    timestamp: i64,
    disposition: &str,
) -> Result<(), ChatError> {
    let count = |key: &str| {
        reply
            .usage
            .as_ref()
            .and_then(|usage| usage.get(key))
            .copied()
    };
    diesel::update(model_calls::table.find(call_id))
        .set((
            model_calls::state.eq("succeeded"),
            model_calls::finished_at.eq(Some(timestamp)),
            model_calls::input_tokens.eq(count("input_tokens")),
            model_calls::output_tokens.eq(count("output_tokens")),
            model_calls::total_tokens.eq(count("total_tokens")),
            model_calls::finish_reason.eq(&reply.finish_reason),
            model_calls::result_text.eq(Some(&reply.text)),
            model_calls::disposition.eq(Some(disposition)),
            model_calls::usages.eq(storage::StoredJson::from(reply.usages.clone())),
            model_calls::metadata.eq(storage::StoredJson::from(reply.metadata.clone())),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "save model response and usage",
        })?;
    Ok(())
}

pub fn complete(input: CompleteTurnInputs) -> Result<CompletionOutputs, ChatError> {
    validate_reply(&input.response)?;
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let call = load_call(connection, &input.invocation.call_id)?;
        ensure!(
            call.session_id == input.invocation.session_id
                && call.turn_id.as_deref() == Some(&input.invocation.turn_id)
                && call.purpose == "conversation",
            ConflictSnafu { id: &call.id }
        );
        let turn = load_turn(connection, &input.invocation.turn_id)?;
        if call.state == "succeeded" {
            ensure!(
                turn.state == "succeeded" && reply_matches(&call, &input.response),
                ConflictSnafu { id: &call.id }
            );
            return Ok(CompletionOutputs {
                completion: Completion {
                    invocation: input.invocation.clone(),
                    reply: saved_reply(&call)?,
                },
            });
        }
        ensure!(
            call.state == "running" && turn.state == "running",
            ConflictSnafu { id: &call.id }
        );
        let owner = session::load(connection, &call.session_id)?;
        let revision = owner
            .conversation_revision
            .checked_add(1)
            .context(ArithmeticSnafu {
                operation: "advance conversation revision",
            })?;
        let timestamp = session::now()?;
        let message = crate::Message {
            metadata: input.response.metadata.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            session_id: call.session_id.clone(),
            turn_id: turn.id.clone(),
            sequence: turn.sequence.checked_add(1).context(ArithmeticSnafu {
                operation: "order assistant message",
            })?,
            role: "assistant".into(),
            content: input.response.text.clone(),
            created_at: timestamp,
        };
        diesel::insert_into(messages::table)
            .values(message)
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "save assistant message",
            })?;
        save_response(
            connection,
            &call.id,
            &input.response,
            timestamp,
            "committed",
        )?;
        diesel::update(turns::table.find(&turn.id))
            .set((
                turns::state.eq("succeeded"),
                turns::finished_at.eq(Some(timestamp)),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "complete chat turn",
            })?;
        diesel::update(sessions::table.find(&owner.id))
            .set((
                sessions::conversation_revision.eq(revision),
                sessions::updated_at.eq(timestamp),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "update completed session",
            })?;
        if owner.conversation_revision == 0 && owner.title_source == "provisional" {
            crate::title::schedule(
                connection,
                &owner.id,
                &turn.id,
                owner.title_revision,
                &owner.profile,
            )?;
        }
        Ok(CompletionOutputs {
            completion: Completion {
                invocation: input.invocation.clone(),
                reply: saved_reply(&load_call(connection, &call.id)?)?,
            },
        })
    })
}

pub fn failure_summary(text: &str) -> &str {
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub fn finalize_call(
    connection: &mut SqliteConnection,
    id: &str,
    state: &str,
    error: &str,
    usages: Option<&[mfn_llm::CallUsage]>,
) -> Result<Changed, ChatError> {
    if let Some(usages) = usages {
        validate_usages(usages)?;
    }
    ensure!(
        matches!(state, "failed" | "cancelled" | "interrupted"),
        InvalidSnafu {
            field: "state",
            reason: "requires a failure terminal state"
        }
    );
    let call = load_call(connection, id)?;
    if call.state != "running" && !(call.purpose == "title" && call.state == "pending") {
        if call.state != "succeeded"
            && let Some(usages) = usages
        {
            ensure!(call.usages.as_deref() == Some(usages), ConflictSnafu { id });
        }
        return Ok(Changed { changed: false });
    }
    let timestamp = session::now()?;
    diesel::update(model_calls::table.find(id))
        .set((
            model_calls::state.eq(state),
            model_calls::usages.eq(storage::StoredJson::from(
                usages.map(<[_]>::to_vec).or(call.usages.clone()),
            )),
            model_calls::finished_at.eq(Some(timestamp)),
            model_calls::error.eq(Some(failure_summary(error))),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "finalize failed model call",
        })?;
    if call.purpose == "conversation" {
        let turn_id = call.turn_id.as_deref().context(ConflictSnafu { id })?;
        diesel::update(
            turns::table
                .find(turn_id)
                .filter(turns::state.eq("running")),
        )
        .set((
            turns::state.eq(state),
            turns::finished_at.eq(Some(timestamp)),
            turns::error.eq(Some(failure_summary(error))),
        ))
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "finalize failed chat turn",
        })?;
    }
    Ok(Changed { changed: true })
}

fn fail(input: FailTurnInputs) -> Result<Changed, ChatError> {
    if let Some(usages) = &input.usages {
        validate_usages(usages)?;
    }
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let call = load_call(connection, &input.invocation.call_id)?;
        ensure!(
            call.session_id == input.invocation.session_id
                && call.turn_id.as_deref() == Some(&input.invocation.turn_id)
                && call.purpose == "conversation",
            ConflictSnafu { id: &call.id }
        );
        finalize_call(
            connection,
            &call.id,
            &input.state,
            &input.error,
            input.usages.as_deref(),
        )
    })
}

fn fail_call(input: FailCallInputs) -> Result<Changed, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        finalize_call(
            connection,
            &input.call_id,
            &input.state,
            &input.error,
            input.usages.as_deref(),
        )
    })
}

fn get_turn(input: RecordInputs) -> Result<TurnOutputs, ChatError> {
    Ok(TurnOutputs {
        turn: load_turn(
            &mut storage::connect(Path::new(&input.database_path))?,
            &input.id,
        )?,
    })
}

fn get_call(input: RecordInputs) -> Result<CallOutputs, ChatError> {
    Ok(CallOutputs {
        call: load_call(
            &mut storage::connect(Path::new(&input.database_path))?,
            &input.id,
        )?,
    })
}

fn get_request(input: RecordInputs) -> Result<TurnRequestOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    let turn = load_turn(&mut connection, &input.id)?;
    let prompt = messages::table
        .filter(messages::turn_id.eq(&turn.id))
        .filter(messages::role.eq("user"))
        .select(messages::content)
        .first::<String>(&mut connection)
        .context(DatabaseSnafu {
            operation: "load original turn prompt",
        })?;
    Ok(TurnRequestOutputs { turn, prompt })
}

fn recover(input: RecoveryInputs) -> Result<RecoveryOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let timestamp = session::now()?;
        let calls = diesel::update(model_calls::table.filter(model_calls::state.eq("running")))
            .set((
                model_calls::state.eq("interrupted"),
                model_calls::finished_at.eq(Some(timestamp)),
                model_calls::error.eq(Some("previous owner exited")),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "recover interrupted model calls",
            })?;
        let turns = diesel::update(turns::table.filter(turns::state.eq("running")))
            .set((
                turns::state.eq("interrupted"),
                turns::finished_at.eq(Some(timestamp)),
                turns::error.eq(Some("previous owner exited")),
            ))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "recover interrupted chat turns",
            })?;
        Ok(RecoveryOutputs {
            recovered: RecoveryCounts {
                turns: i64::try_from(turns).context(IntegerSnafu {
                    operation: "count recovered turns",
                })?,
                calls: i64::try_from(calls).context(IntegerSnafu {
                    operation: "count recovered calls",
                })?,
            },
        })
    })
}

register_operation!(
    "mf_chat.turn.begin",
    BeginTurn,
    BeginTurnInputs,
    TurnStarted,
    begin
);
register_operation!(
    "mf_chat.turn.complete",
    CompleteTurn,
    CompleteTurnInputs,
    CompletionOutputs,
    complete
);
register_operation!("mf_chat.turn.fail", FailTurn, FailTurnInputs, Changed, fail);
register_operation!(
    "mf_chat.call.fail",
    FailCall,
    FailCallInputs,
    Changed,
    fail_call
);
register_operation!(
    "mf_chat.turn.load",
    LoadTurn,
    RecordInputs,
    TurnOutputs,
    get_turn
);
register_operation!(
    "mf_chat.call.load",
    LoadCall,
    RecordInputs,
    CallOutputs,
    get_call
);
register_operation!(
    "mf_chat.storage.recover",
    Recover,
    RecoveryInputs,
    RecoveryOutputs,
    recover
);

register_operation!(
    "mf_chat.turn.request",
    LoadTurnRequest,
    RecordInputs,
    TurnRequestOutputs,
    get_request
);

impl ChatDatabase {
    pub fn begin_turn(
        &self,
        session_id: &str,
        prompt: &str,
        model: ModelIdentity,
        retry_of: Option<String>,
        max_history_bytes: usize,
    ) -> Result<TurnStarted, ChatError> {
        self.begin_turn_with_invocation(
            Invocation::new(session_id),
            prompt,
            model,
            retry_of,
            max_history_bytes,
        )
    }

    pub fn begin_turn_with_invocation(
        &self,
        invocation: Invocation,
        prompt: &str,
        model: ModelIdentity,
        retry_of: Option<String>,
        max_history_bytes: usize,
    ) -> Result<TurnStarted, ChatError> {
        operation::execute(
            self.path(),
            "mf_chat.turn.begin",
            BeginTurnInputs {
                database_path: String::new(),
                session_id: invocation.session_id,
                turn_id: invocation.turn_id,
                call_id: invocation.call_id,
                prompt: prompt.into(),
                model,
                retry_of,
                max_history_bytes,
            },
        )
    }

    pub fn complete_turn(
        &self,
        invocation: Invocation,
        response: Reply,
    ) -> Result<Completion, ChatError> {
        let result: CompletionOutputs = operation::execute(
            self.path(),
            "mf_chat.turn.complete",
            CompleteTurnInputs {
                database_path: String::new(),
                invocation,
                response,
            },
        )?;
        Ok(result.completion)
    }

    pub fn fail_turn_with_usages(
        &self,
        invocation: Invocation,
        error: &str,
        usages: Vec<mfn_llm::CallUsage>,
    ) -> Result<bool, ChatError> {
        let result: Changed = operation::execute(
            self.path(),
            "mf_chat.turn.fail",
            FailTurnInputs {
                usages: Some(usages),
                database_path: String::new(),
                invocation,
                state: "failed".into(),
                error: error.into(),
            },
        )?;
        Ok(result.changed)
    }

    pub fn fail_turn(
        &self,
        invocation: Invocation,
        state: &str,
        error: &str,
    ) -> Result<bool, ChatError> {
        let result: Changed = operation::execute(
            self.path(),
            "mf_chat.turn.fail",
            FailTurnInputs {
                usages: None,
                database_path: String::new(),
                invocation,
                state: state.into(),
                error: error.into(),
            },
        )?;
        Ok(result.changed)
    }

    pub fn fail_call(&self, call_id: &str, state: &str, error: &str) -> Result<bool, ChatError> {
        self.fail_call_with_usages(call_id, state, error, None)
    }

    pub fn fail_call_with_usages(
        &self,
        call_id: &str,
        state: &str,
        error: &str,
        usages: Option<Vec<mfn_llm::CallUsage>>,
    ) -> Result<bool, ChatError> {
        let result: Changed = operation::execute(
            self.path(),
            "mf_chat.call.fail",
            FailCallInputs {
                usages,
                database_path: String::new(),
                call_id: call_id.into(),
                state: state.into(),
                error: error.into(),
            },
        )?;
        Ok(result.changed)
    }

    pub fn turn(&self, id: &str) -> Result<Turn, ChatError> {
        let result: TurnOutputs = operation::execute(
            self.path(),
            "mf_chat.turn.load",
            RecordInputs {
                database_path: String::new(),
                id: id.into(),
            },
        )?;
        Ok(result.turn)
    }

    pub fn call(&self, id: &str) -> Result<ModelCall, ChatError> {
        let result: CallOutputs = operation::execute(
            self.path(),
            "mf_chat.call.load",
            RecordInputs {
                database_path: String::new(),
                id: id.into(),
            },
        )?;
        Ok(result.call)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;

    fn identity() -> ModelIdentity {
        ModelIdentity {
            profile: "default".into(),
            model: Some("test-model".into()),
            config_id: "test-config".into(),
        }
    }
    fn reply(text: &str) -> Reply {
        Reply {
            usages: None,
            metadata: None,
            text: text.into(),
            usage: Some(BTreeMap::from([
                ("input_tokens".into(), 2),
                ("output_tokens".into(), 1),
            ])),
            finish_reason: Some("stop".into()),
        }
    }

    #[test]
    fn completion_is_atomic_idempotent_and_rejects_conflicting_replay() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let begun = database
            .begin_turn(&session.id, "question", identity(), None, 1024)
            .unwrap();
        let mut connection = storage::connect(database.path()).unwrap();
        connection.batch_execute("CREATE TRIGGER reject_completion BEFORE UPDATE ON model_calls WHEN NEW.state = 'succeeded' BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;").unwrap();
        assert!(
            database
                .complete_turn(begun.invocation.clone(), reply("answer"))
                .is_err()
        );
        assert_eq!(
            database
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert_eq!(
            database.turn(&begun.invocation.turn_id).unwrap().state,
            "running"
        );
        assert_eq!(
            database
                .call(&begun.invocation.call_id)
                .unwrap()
                .output_tokens,
            None
        );
        connection
            .batch_execute("DROP TRIGGER reject_completion;")
            .unwrap();
        let first = database
            .complete_turn(begun.invocation.clone(), reply("answer"))
            .unwrap();
        let second = database
            .complete_turn(begun.invocation.clone(), reply("answer"))
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            database
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            2
        );
        assert_eq!(
            database.session(&session.id).unwrap().conversation_revision,
            1
        );
        assert!(
            database
                .complete_turn(begun.invocation.clone(), reply("different"))
                .is_err()
        );
        assert!(
            !database
                .fail_turn(begun.invocation, "cancelled", "late cancellation")
                .unwrap()
        );
    }

    #[test]
    fn failed_turns_remain_inspectable_and_model_retries_get_new_identities() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let first = database
            .begin_turn(&session.id, "first", identity(), None, 1024)
            .unwrap();
        database
            .fail_turn(first.invocation.clone(), "failed", "provider failure")
            .unwrap();
        let second = database
            .begin_turn(
                &session.id,
                "retry",
                identity(),
                Some(first.invocation.turn_id.clone()),
                1024,
            )
            .unwrap();
        assert_ne!(first.invocation.call_id, second.invocation.call_id);
        assert_ne!(first.invocation.turn_id, second.invocation.turn_id);
        assert!(second.request.history.unwrap().is_empty());
        database
            .complete_turn(second.invocation.clone(), reply("answer"))
            .unwrap();
        let history = database.history(&session.id, 1024).unwrap();
        assert_eq!(history.history.len(), 2);
        assert_eq!(history.history[0].content, "retry");
        assert_eq!(
            database.turn(&second.invocation.turn_id).unwrap().retry_of,
            Some(first.invocation.turn_id)
        );
        assert_eq!(
            database
                .messages(&session.id, 10, None)
                .unwrap()
                .messages
                .len(),
            3
        );
        database.archive_session(&session.id).unwrap();
        assert!(
            database
                .begin_turn(&session.id, "archived", identity(), None, 1024)
                .is_err()
        );
    }

    #[test]
    fn concurrent_turn_admission_is_per_session_and_releases_the_database() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let results = std::thread::scope(|scope| {
            let first =
                scope.spawn(|| database.begin_turn(&session.id, "a", identity(), None, 1024));
            let second =
                scope.spawn(|| database.begin_turn(&session.id, "b", identity(), None, 1024));
            [
                first.join().unwrap().is_ok(),
                second.join().unwrap().is_ok(),
            ]
        });
        assert_eq!(results.into_iter().filter(|success| *success).count(), 1);
        let other = database.create_session("default", None, None).unwrap();
        assert!(
            database
                .begin_turn(&other.id, "other", identity(), None, 1024)
                .is_ok()
        );
    }

    #[test]
    fn recovery_requires_ownership_and_never_retries_abandoned_calls() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let begun = database
            .begin_turn(&session.id, "pending", identity(), None, 1024)
            .unwrap();
        assert!(matches!(
            ChatDatabase::open(&path),
            Err(ChatError::Busy { .. })
        ));
        assert_eq!(
            database.turn(&begun.invocation.turn_id).unwrap().state,
            "running"
        );
        drop(database);
        let reopened = ChatDatabase::open(&path).unwrap();
        assert_eq!(
            reopened.turn(&begun.invocation.turn_id).unwrap().state,
            "interrupted"
        );
        let call = reopened.call(&begun.invocation.call_id).unwrap();
        assert_eq!(call.state, "interrupted");
        assert_eq!(call.total_tokens, None);
        assert!(
            reopened
                .history(&session.id, 1024)
                .unwrap()
                .history
                .is_empty()
        );
    }

    #[test]
    fn invalid_begin_requests_do_not_reserve_message_order() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let owner = database.create_session("default", None, None).unwrap();
        let other = database.create_session("default", None, None).unwrap();
        let previous = database
            .begin_turn(&other.id, "other", identity(), None, 1024)
            .unwrap();
        database
            .fail_turn(previous.invocation.clone(), "failed", "failed")
            .unwrap();
        let base = BeginTurnInputs {
            database_path: database.path().to_str().unwrap().into(),
            session_id: owner.id.clone(),
            turn_id: "turn".into(),
            call_id: "call".into(),
            prompt: "question".into(),
            model: identity(),
            retry_of: None,
            max_history_bytes: 1024,
        };
        let mut prompt = base.clone();
        prompt.prompt.clear();
        let mut invocation = base.clone();
        invocation.call_id.clear();
        let mut model = base.clone();
        model.model.model = Some(String::new());
        let mut profile = base.clone();
        profile.model.profile = "different".into();
        let mut retry = base.clone();
        retry.retry_of = Some(previous.invocation.turn_id);
        for (input, expected) in [
            (prompt, "prompt"),
            (invocation, "invocation"),
            (model, "model"),
            (profile, "profile"),
            (retry, "retry_of"),
        ] {
            assert!(
                matches!(begin(input), Err(ChatError::Invalid { field, .. }) if field == expected)
            );
        }
        assert_eq!(database.session(&owner.id).unwrap().next_sequence, 0);
        assert!(
            database
                .messages(&owner.id, 10, None)
                .unwrap()
                .messages
                .is_empty()
        );
        assert_eq!(begin(base).unwrap().invocation.call_id, "call");
    }

    #[test]
    fn invalid_completions_preserve_running_state_and_unicode_errors_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(directory.path().join("chat.sqlite")).unwrap();
        let owner = database.create_session("default", None, None).unwrap();
        let begun = database
            .begin_turn(&owner.id, "question", identity(), None, 1024)
            .unwrap();
        let base = CompleteTurnInputs {
            database_path: database.path().to_str().unwrap().into(),
            invocation: begun.invocation.clone(),
            response: reply("answer"),
        };
        let mut empty = base.clone();
        empty.response.text.clear();
        let mut negative = base.clone();
        negative.response.usage = Some(BTreeMap::from([("input_tokens".into(), -1)]));
        let mut unsupported = base.clone();
        unsupported.response.usage = Some(BTreeMap::from([("cost".into(), 1)]));
        for input in [empty, negative, unsupported] {
            assert!(matches!(complete(input), Err(ChatError::Invalid { .. })));
        }
        let mut foreign = base.clone();
        foreign.invocation.session_id = "different".into();
        assert!(matches!(
            complete(foreign.clone()),
            Err(ChatError::Conflict { .. })
        ));
        assert!(matches!(
            fail(FailTurnInputs {
                usages: None,
                database_path: base.database_path.clone(),
                invocation: foreign.invocation,
                state: "failed".into(),
                error: "wrong identity".into(),
            }),
            Err(ChatError::Conflict { .. })
        ));
        assert!(matches!(
            fail_call(FailCallInputs {
                usages: None,
                database_path: base.database_path.clone(),
                call_id: begun.invocation.call_id.clone(),
                state: "running".into(),
                error: "invalid terminal state".into(),
            }),
            Err(ChatError::Invalid { field: "state", .. })
        ));
        assert_eq!(
            database.turn(&begun.invocation.turn_id).unwrap().state,
            "running"
        );
        assert_eq!(
            database
                .messages(&owner.id, 10, None)
                .unwrap()
                .messages
                .len(),
            1
        );
        let error = format!("{}{}", "x".repeat(4095), "\u{1f4ac}".repeat(3));
        database
            .fail_turn(begun.invocation.clone(), "failed", &error)
            .unwrap();
        let saved = database.turn(&begun.invocation.turn_id).unwrap();
        assert_eq!(saved.error.unwrap(), "x".repeat(4095));
        assert!(matches!(complete(base), Err(ChatError::Conflict { .. })));
    }
}
