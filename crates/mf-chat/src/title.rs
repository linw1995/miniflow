use crate::{
    ChatDatabase, ChatError, Invocation, ModelCall, ModelIdentity, Reply,
    error::*,
    operation,
    schema::{messages, model_calls, sessions, turns},
    session, storage, turn,
};
use diesel::prelude::*;
use mf_runtime::{NodeEnum, NodeValue};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
use std::path::Path;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum CallPurpose {
    #[value(rename = "conversation")]
    Conversation,
    #[value(rename = "title")]
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum TitleOutcome {
    #[value(rename = "applied")]
    Applied,
    #[value(rename = "stale")]
    Stale,
    #[value(rename = "rejected")]
    Rejected,
    #[value(rename = "skipped")]
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct TitleCompletion {
    pub invocation: Invocation,
    pub title: Option<String>,
    pub outcome: TitleOutcome,
}

#[derive(Clone, Debug, NodeValue)]
pub struct PrepareTitleInputs {
    pub database_path: String,
    pub call_id: String,
    pub model: ModelIdentity,
    #[value(default = "session::default_history_bytes")]
    pub max_bytes: usize,
}
#[derive(Clone, Debug, NodeValue)]
pub struct TitleStarted {
    pub database_path: String,
    pub invocation: Invocation,
    pub title_revision: i64,
    pub request: Option<mfn_llm::LlmInputs>,
}
#[derive(Clone, Debug, NodeValue)]
pub struct CompleteTitleInputs {
    pub database_path: String,
    pub invocation: Invocation,
    pub title_revision: i64,
    pub response: Reply,
}
#[derive(Clone, Debug, NodeValue)]
pub struct TitleCompletionOutputs {
    pub result: TitleCompletion,
}
#[derive(Clone, Debug, NodeValue)]
pub struct PendingTitleInputs {
    pub database_path: String,
    #[value(default = "session::default_page_size")]
    pub limit: usize,
}
#[derive(Clone, Debug, NodeValue)]
pub struct PendingTitles {
    pub calls: Vec<ModelCall>,
}

pub fn schedule(
    connection: &mut SqliteConnection,
    session_id: &str,
    turn_id: &str,
    revision: i64,
    profile: &str,
) -> Result<ModelCall, ChatError> {
    let call = ModelCall {
        usages: None,
        metadata: None,
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.into(),
        turn_id: Some(turn_id.into()),
        purpose: "title".into(),
        profile: profile.into(),
        model: None,
        state: "pending".into(),
        title_revision: Some(revision),
        started_at: None,
        finished_at: None,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        finish_reason: None,
        error: None,
        result_text: None,
        disposition: None,
    };
    diesel::insert_into(model_calls::table)
        .values(call.clone())
        .execute(connection)
        .context(DatabaseSnafu {
            operation: "schedule session title",
        })?;
    Ok(call)
}

pub fn prepare(input: PrepareTitleInputs) -> Result<TitleStarted, ChatError> {
    ensure!(
        input.max_bytes > 0,
        InvalidSnafu {
            field: "max_bytes",
            reason: "must be positive"
        }
    );
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let call = turn::load_call(connection, &input.call_id)?;
        ensure!(call.purpose == "title" && call.state == "pending", ConflictSnafu { id: &call.id });
        let owner = session::load(connection, &call.session_id)?;
        let turn_id = call.turn_id.clone().context(ConflictSnafu { id: &call.id })?;
        let revision = call.title_revision.context(ConflictSnafu { id: &call.id })?;
        let invocation = Invocation { session_id: owner.id.clone(), turn_id: turn_id.clone(), call_id: call.id.clone() };
        if owner.title_source != "provisional" || owner.title_revision != revision {
            diesel::update(model_calls::table.find(&call.id)).set((model_calls::state.eq("skipped"),
                model_calls::finished_at.eq(Some(session::now()?)), model_calls::disposition.eq(Some("stale"))))
                .execute(connection).context(DatabaseSnafu { operation: "skip obsolete title request" })?;
            return Ok(TitleStarted { database_path: input.database_path.clone(), invocation, title_revision: revision, request: None });
        }
        let source = turn::load_turn(connection, &turn_id)?;
        ensure!(source.state == "succeeded" && source.session_id == owner.id, ConflictSnafu { id: &call.id });
        let transcript: Vec<(String, String)> = messages::table.filter(messages::turn_id.eq(&turn_id))
            .order(messages::sequence.asc()).select((messages::role, messages::content)).load(connection)
            .context(DatabaseSnafu { operation: "load title source turn" })?;
        let prompt = transcript.into_iter().map(|(role, content)| format!("{role}: {content}")).collect::<Vec<_>>().join("\n");
        ensure!(prompt.len() <= input.max_bytes, HistoryTooLargeSnafu { limit: input.max_bytes });
        diesel::update(model_calls::table.find(&call.id)).set((model_calls::state.eq("running"),
            model_calls::started_at.eq(Some(session::now()?)), model_calls::profile.eq(&input.model.profile),
            model_calls::model.eq(&input.model.model)))
            .execute(connection).context(DatabaseSnafu { operation: "begin session title call" })?;
        Ok(TitleStarted { database_path: input.database_path.clone(), invocation, title_revision: revision,
            request: Some(mfn_llm::LlmInputs { prompt,
                system: Some("Summarize this conversation as a concise single-line title in its language. Return only the title, at most 120 characters.".into()),
                history: None }) })
    })
}

fn outcome(call: &ModelCall) -> Result<TitleCompletion, ChatError> {
    let outcome = match call.disposition.as_deref() {
        Some("applied") => TitleOutcome::Applied,
        Some("stale") => TitleOutcome::Stale,
        Some("rejected") => TitleOutcome::Rejected,
        _ => return ConflictSnafu { id: &call.id }.fail(),
    };
    Ok(TitleCompletion {
        invocation: Invocation {
            session_id: call.session_id.clone(),
            turn_id: call
                .turn_id
                .clone()
                .context(ConflictSnafu { id: &call.id })?,
            call_id: call.id.clone(),
        },
        title: if outcome == TitleOutcome::Applied {
            Some(turn::saved_reply(call)?.text.trim().into())
        } else {
            None
        },
        outcome,
    })
}

pub fn complete(input: CompleteTitleInputs) -> Result<TitleCompletionOutputs, ChatError> {
    turn::validate_reply(&input.response)?;
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let call = turn::load_call(connection, &input.invocation.call_id)?;
        ensure!(
            call.purpose == "title"
                && call.session_id == input.invocation.session_id
                && call.turn_id.as_deref() == Some(input.invocation.turn_id.as_str())
                && call.title_revision == Some(input.title_revision),
            ConflictSnafu { id: &call.id }
        );
        if call.state == "succeeded" {
            ensure!(
                turn::reply_matches(&call, &input.response),
                ConflictSnafu { id: &call.id }
            );
            return Ok(TitleCompletionOutputs {
                result: outcome(&call)?,
            });
        }
        ensure!(call.state == "running", ConflictSnafu { id: &call.id });
        let owner = session::load(connection, &call.session_id)?;
        let text = input.response.text.trim();
        let valid =
            !text.is_empty() && text.chars().count() <= 120 && !text.chars().any(char::is_control);
        let disposition = if !valid {
            "rejected"
        } else if owner.title_source != "provisional"
            || owner.title_revision != input.title_revision
        {
            "stale"
        } else {
            "applied"
        };
        let timestamp = session::now()?;
        turn::save_response(
            connection,
            &call.id,
            &input.response,
            timestamp,
            disposition,
        )?;
        if disposition == "applied" {
            let revision = owner
                .title_revision
                .checked_add(1)
                .context(ArithmeticSnafu {
                    operation: "advance generated title revision",
                })?;
            diesel::update(sessions::table.find(&owner.id))
                .set((
                    sessions::title.eq(text),
                    sessions::title_source.eq("generated"),
                    sessions::title_revision.eq(revision),
                    sessions::updated_at.eq(timestamp),
                ))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "apply generated session title",
                })?;
        }
        Ok(TitleCompletionOutputs {
            result: outcome(&turn::load_call(connection, &call.id)?)?,
        })
    })
}

fn pending(input: PendingTitleInputs) -> Result<PendingTitles, ChatError> {
    session::validate_page(input.limit)?;
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    let calls = model_calls::table
        .filter(model_calls::purpose.eq("title"))
        .filter(model_calls::state.eq("pending"))
        .order(model_calls::id.asc())
        .limit(input.limit as i64)
        .load::<ModelCall>(&mut connection)
        .context(DatabaseSnafu {
            operation: "list pending title requests",
        })?;
    Ok(PendingTitles { calls })
}

fn retry(input: crate::nodes::SessionInputs) -> Result<crate::nodes::CallOutputs, ChatError> {
    let mut connection = storage::connect(Path::new(&input.database_path))?;
    connection.immediate_transaction(|connection| {
        let owner = session::load(connection, &input.session_id)?;
        ensure!(
            owner.title_source == "provisional",
            InvalidSnafu {
                field: "title",
                reason: "must remain provisional to retry generation"
            }
        );
        let active = model_calls::table
            .filter(model_calls::session_id.eq(&owner.id))
            .filter(model_calls::purpose.eq("title"))
            .filter(model_calls::state.eq_any(["pending", "running"]))
            .count()
            .get_result::<i64>(connection)
            .context(DatabaseSnafu {
                operation: "check active title requests",
            })?;
        ensure!(active == 0, TitleBusySnafu { id: &owner.id });
        let first = turns::table
            .filter(turns::session_id.eq(&owner.id))
            .filter(turns::state.eq("succeeded"))
            .order(turns::sequence.asc())
            .first::<crate::Turn>(connection)
            .context(DatabaseSnafu {
                operation: "load first successful title source",
            })?;
        Ok(crate::nodes::CallOutputs {
            call: schedule(
                connection,
                &owner.id,
                &first.id,
                owner.title_revision,
                &owner.profile,
            )?,
        })
    })
}

struct PrepareTitle;
impl mf_runtime::TypedTaskNode for PrepareTitle {
    type Input = PrepareTitleInputs;
    type Output = TitleStarted;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let output = prepare(input)?;
        let skipped = output.request.is_none();
        let mut result: mf_runtime::TypedNodeResult<_> = output.into();
        if skipped {
            result.skipped.insert("request".into());
        }
        Ok(result)
    }
}
register_task!("mf_chat.title.prepare", PrepareTitle);
register_operation!(
    "mf_chat.title.complete",
    CompleteTitle,
    CompleteTitleInputs,
    TitleCompletionOutputs,
    complete
);
register_operation!(
    "mf_chat.title.pending",
    PendingTitle,
    PendingTitleInputs,
    PendingTitles,
    pending
);
register_operation!(
    "mf_chat.title.retry",
    RetryTitle,
    crate::nodes::SessionInputs,
    crate::nodes::CallOutputs,
    retry
);

impl ChatDatabase {
    pub fn pending_titles(&self, limit: usize) -> Result<Vec<ModelCall>, ChatError> {
        let result: PendingTitles = operation::execute(
            self.path(),
            "mf_chat.title.pending",
            PendingTitleInputs {
                database_path: String::new(),
                limit,
            },
        )?;
        Ok(result.calls)
    }
    pub fn retry_title(&self, session_id: &str) -> Result<ModelCall, ChatError> {
        let result: crate::nodes::CallOutputs = operation::execute(
            self.path(),
            "mf_chat.title.retry",
            crate::nodes::SessionInputs {
                database_path: String::new(),
                session_id: session_id.into(),
            },
        )?;
        Ok(result.call)
    }
    pub fn complete_title(
        &self,
        invocation: Invocation,
        title_revision: i64,
        response: Reply,
    ) -> Result<TitleCompletion, ChatError> {
        let result: TitleCompletionOutputs = operation::execute(
            self.path(),
            "mf_chat.title.complete",
            CompleteTitleInputs {
                database_path: String::new(),
                invocation,
                title_revision,
                response,
            },
        )?;
        Ok(result.result)
    }
}
