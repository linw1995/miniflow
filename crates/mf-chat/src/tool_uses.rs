//! Bounded public tool execution evidence, owned by the conversation host rather than its views.

use crate::{ChatDatabase, ChatError, Invocation, error::*, schema::model_calls, storage, turn};
use diesel::{connection::SimpleConnection, prelude::*};
use mf_runtime::{NodeEnum, NodeValue};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, ensure};
use std::collections::BTreeSet;

pub const MAX_TOOL_USES: usize = 64;
pub const MAX_TOOL_TEXT_BYTES: usize = 2048;
pub const MAX_TOOL_NAME_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum ToolUsePhase {
    #[value(rename = "preparing")]
    Preparing,
    #[value(rename = "running")]
    Running,
    #[value(rename = "succeeded")]
    Succeeded,
    #[value(rename = "failed")]
    Failed,
    #[value(rename = "skipped")]
    Skipped,
    #[value(rename = "interrupted")]
    Interrupted,
    #[value(rename = "unknown")]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub phase: ToolUsePhase,
    pub arguments: String,
    pub output: String,
    pub duration_ns: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ToolUseHistory {
    pub invocation: Invocation,
    pub runs: Vec<ToolUse>,
    pub incomplete: bool,
    pub finished: bool,
}

impl ToolUseHistory {
    pub fn validate(&self) -> Result<(), ChatError> {
        let mut ids = BTreeSet::new();
        ensure!(
            self.runs.len() <= MAX_TOOL_USES
                && !self.invocation.call_id.is_empty()
                && !self.invocation.session_id.is_empty()
                && !self.invocation.turn_id.is_empty()
                && self.runs.iter().all(|run| {
                    !run.id.is_empty()
                        && run.id.len() <= 256
                        && ids.insert(&run.id)
                        && !run.name.is_empty()
                        && run.name.len() <= MAX_TOOL_NAME_BYTES
                        && run.arguments.len() <= MAX_TOOL_TEXT_BYTES
                        && run.output.len() <= MAX_TOOL_TEXT_BYTES
                        && run.duration_ns.is_none_or(|duration| duration >= 0)
                        && (!self.finished
                            || !matches!(
                                run.phase,
                                ToolUsePhase::Preparing | ToolUsePhase::Running
                            ))
                }),
            InvalidSnafu {
                field: "tool uses",
                reason: "requires a valid invocation, bounded unique calls, and settled final phases"
            }
        );
        Ok(())
    }

    /// A host exit cannot prove that an unobserved tool succeeded.
    fn seal(&mut self, pending: ToolUsePhase) {
        if self.finished {
            return;
        }
        for run in &mut self.runs {
            if matches!(run.phase, ToolUsePhase::Preparing | ToolUsePhase::Running) {
                run.phase = pending;
            }
        }
        self.incomplete = true;
        self.finished = true;
    }
}

impl ChatDatabase {
    /// Checkpoints public evidence without waiting behind another storage operation.
    /// Complete final evidence is immutable; incomplete evidence may only refine known facts.
    pub fn record_tool_uses(
        &self,
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
        let mut connection = storage::connect(self.path())?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0;")
            .context(DatabaseSnafu {
                operation: "configure tool evidence checkpoint",
            })?;
        connection.immediate_transaction(|connection| {
            let call = turn::load_call(connection, &invocation.call_id)?;
            ensure!(
                call.session_id == invocation.session_id
                    && call.turn_id.as_deref() == Some(&invocation.turn_id)
                    && call.purpose == "conversation"
                    && (call.state == "running" || history.finished),
                ConflictSnafu { id: &call.id }
            );
            if let Some(previous) = &call.tool_uses
                && previous.finished
            {
                if previous == history {
                    return Ok(());
                }
                ensure!(
                    previous.incomplete
                        && history.finished
                        && previous
                            .runs
                            .iter()
                            .all(|old| history.runs.iter().any(|new| {
                                old.id == new.id
                                    && (matches!(
                                        old.phase,
                                        ToolUsePhase::Unknown | ToolUsePhase::Interrupted
                                    ) || old.phase == new.phase)
                            })),
                    ConflictSnafu { id: &call.id }
                );
            }
            diesel::update(model_calls::table.find(&call.id))
                .set(model_calls::tool_uses.eq(storage::StoredJson::from(Some(history.clone()))))
                .execute(connection)
                .context(DatabaseSnafu {
                    operation: "save tool execution evidence",
                })?;
            Ok(())
        })
    }
}

pub fn seal_call(
    connection: &mut SqliteConnection,
    call_id: &str,
    pending: ToolUsePhase,
) -> Result<(), ChatError> {
    let stored = model_calls::table
        .find(call_id)
        .select(model_calls::tool_uses)
        .first::<storage::StoredJson<ToolUseHistory>>(connection)
        .context(DatabaseSnafu {
            operation: "load unfinished tool evidence",
        })?;
    if let Some(mut history) = Option::<ToolUseHistory>::from(stored)
        && !history.finished
    {
        history.seal(pending);
        diesel::update(model_calls::table.find(call_id))
            .set(model_calls::tool_uses.eq(storage::StoredJson::from(Some(history))))
            .execute(connection)
            .context(DatabaseSnafu {
                operation: "settle unfinished tool evidence",
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
            operation: "find interrupted tool evidence",
        })?;
    for call in calls {
        seal_call(connection, &call, ToolUsePhase::Interrupted)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin(database: &ChatDatabase) -> Invocation {
        let session = database.create_session("default", None, None).unwrap();
        database
            .begin_turn(
                &session.id,
                "calculate",
                crate::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap()
            .invocation
    }

    fn history(invocation: &Invocation, phase: ToolUsePhase, finished: bool) -> ToolUseHistory {
        ToolUseHistory {
            invocation: invocation.clone(),
            incomplete: false,
            finished,
            runs: vec![ToolUse {
                id: "dispatch:0".into(),
                name: "calculator".into(),
                phase,
                arguments: r#"{"x":12,"y":8}"#.into(),
                output: "20".into(),
                duration_ns: Some(50_000),
            }],
        }
    }

    #[test]
    fn recorded_tools_survive_completion_and_reopen_without_entering_model_context() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let invocation = begin(&database);
        let evidence = history(&invocation, ToolUsePhase::Succeeded, true);
        database.record_tool_uses(&invocation, &evidence).unwrap();
        database
            .complete_turn(
                invocation.clone(),
                crate::ConversationResult {
                    text: "20".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        drop(database);
        let reopened = ChatDatabase::open(path).unwrap();
        assert_eq!(
            reopened.call(&invocation.call_id).unwrap().tool_uses,
            Some(evidence.clone())
        );
        let page = reopened.messages(&invocation.session_id, 10, None).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert!(
            page.messages
                .iter()
                .all(|record| record.tool_uses.as_ref() == Some(&evidence))
        );
        let context = reopened.history(&invocation.session_id).unwrap();
        assert_eq!(context.history.len(), 2);
        assert!(!format!("{:?}", context.history).contains("dispatch:0"));
    }

    #[test]
    fn failed_and_cancelled_turns_keep_tools_and_retries_do_not_overwrite_them() {
        for state in ["failed", "cancelled", "interrupted"] {
            let root = tempfile::tempdir().unwrap();
            let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
            let invocation = begin(&database);
            let evidence = history(&invocation, ToolUsePhase::Failed, true);
            database.record_tool_uses(&invocation, &evidence).unwrap();
            database
                .fail_turn(invocation.clone(), state, "execution ended")
                .unwrap();
            let page = database.messages(&invocation.session_id, 1, None).unwrap();
            assert_eq!(page.messages[0].tool_uses, Some(evidence.clone()));
            let retried = database
                .begin_turn(
                    &invocation.session_id,
                    "retry",
                    crate::ModelIdentity {
                        profile: "default".into(),
                        model: None,
                    },
                    Some(invocation.turn_id.clone()),
                )
                .unwrap();
            assert!(
                database
                    .call(&retried.invocation.call_id)
                    .unwrap()
                    .tool_uses
                    .is_none()
            );
            assert_eq!(
                database.call(&invocation.call_id).unwrap().tool_uses,
                Some(evidence)
            );
        }
    }

    #[test]
    fn interrupted_checkpoints_are_settled_without_falsifying_observed_success() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("chat.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let invocation = begin(&database);
        let mut evidence = history(&invocation, ToolUsePhase::Running, false);
        let mut completed = evidence.runs[0].clone();
        completed.id = "dispatch:1".into();
        completed.phase = ToolUsePhase::Succeeded;
        evidence.runs.push(completed);
        database.record_tool_uses(&invocation, &evidence).unwrap();
        drop(database);
        let reopened = ChatDatabase::open(path).unwrap();
        let saved = reopened
            .call(&invocation.call_id)
            .unwrap()
            .tool_uses
            .unwrap();
        assert!(saved.finished && saved.incomplete);
        assert_eq!(saved.runs[0].phase, ToolUsePhase::Interrupted);
        assert_eq!(saved.runs[1].phase, ToolUsePhase::Succeeded);
        let legacy = begin(&reopened);
        assert!(reopened.call(&legacy.call_id).unwrap().tool_uses.is_none());
    }

    #[test]
    fn checkpoint_lock_contention_fails_promptly_and_does_not_modify_evidence() {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let invocation = begin(&database);
        let evidence = history(&invocation, ToolUsePhase::Running, false);
        let mut other = storage::connect(database.path()).unwrap();
        other.batch_execute("BEGIN IMMEDIATE;").unwrap();
        let start = std::time::Instant::now();
        assert!(database.record_tool_uses(&invocation, &evidence).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        other.batch_execute("ROLLBACK;").unwrap();
        assert!(
            database
                .call(&invocation.call_id)
                .unwrap()
                .tool_uses
                .is_none()
        );
        database.record_tool_uses(&invocation, &evidence).unwrap();
    }

    #[test]
    fn completion_failure_rolls_back_checkpoint_settlement_and_bounds_are_checked() {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let invocation = begin(&database);
        let mut evidence = history(&invocation, ToolUsePhase::Running, false);
        database.record_tool_uses(&invocation, &evidence).unwrap();
        let mut connection = storage::connect(database.path()).unwrap();
        connection.batch_execute("CREATE TRIGGER reject_tool_completion BEFORE UPDATE ON model_calls WHEN NEW.state = 'succeeded' BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;").unwrap();
        assert!(
            database
                .complete_turn(
                    invocation.clone(),
                    crate::ConversationResult {
                        text: "answer".into(),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(
            database.call(&invocation.call_id).unwrap().tool_uses,
            Some(evidence.clone())
        );
        evidence.finished = true;
        assert!(
            evidence.validate().is_err(),
            "a running tool cannot be a final observation"
        );
        evidence.finished = false;
        evidence.runs[0].output = "界".repeat(MAX_TOOL_TEXT_BYTES);
        assert!(evidence.validate().is_err());
    }

    #[test]
    fn incomplete_exit_evidence_can_be_refined_without_rewriting_known_facts() {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let invocation = begin(&database);
        database
            .record_tool_uses(
                &invocation,
                &history(&invocation, ToolUsePhase::Running, false),
            )
            .unwrap();
        database
            .fail_turn(invocation.clone(), "cancelled", "cancelled")
            .unwrap();
        let mut final_evidence = history(&invocation, ToolUsePhase::Succeeded, true);
        database
            .record_tool_uses(&invocation, &final_evidence)
            .unwrap();
        final_evidence.runs[0].phase = ToolUsePhase::Failed;
        assert!(
            database
                .record_tool_uses(&invocation, &final_evidence)
                .is_err()
        );
    }

    #[test]
    fn evidence_rejects_cross_invocation_replays_duplicates_and_final_mutation() {
        let root = tempfile::tempdir().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let invocation = begin(&database);
        let mut evidence = history(&invocation, ToolUsePhase::Succeeded, true);
        let other = begin(&database);
        assert!(database.record_tool_uses(&other, &evidence).is_err());
        evidence.runs.push(evidence.runs[0].clone());
        assert!(database.record_tool_uses(&invocation, &evidence).is_err());
        evidence.runs.pop();
        database.record_tool_uses(&invocation, &evidence).unwrap();
        database.record_tool_uses(&invocation, &evidence).unwrap();
        evidence.runs[0].output = "changed".into();
        assert!(database.record_tool_uses(&invocation, &evidence).is_err());
    }
}
