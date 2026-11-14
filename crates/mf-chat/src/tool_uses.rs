//! Bounded public tool execution evidence, owned by the conversation host rather than its views.

use crate::{ChatError, Invocation, error::*};
use mf_runtime::{NodeEnum, NodeValue};
use serde::{Deserialize, Serialize};
use snafu::ensure;
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
pub struct ToolOrigin {
    pub model_call_id: String,
    pub tool_call_id: String,
}

/// Message semantics remain separate from the observed phase of each execution attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ChatToolUse {
    pub id: String,
    pub assistant_message_id: String,
    pub model_call_id: Option<String>,
    pub tool_call_id: String,
    pub name: String,
    pub arguments: String,
    pub output: Option<String>,
    pub execution_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ToolUse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ToolOrigin>,
    pub id: String,
    pub name: String,
    pub phase: ToolUsePhase,
    pub arguments: String,
    pub output: String,
    pub duration_ns: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ToolUseHistory {
    #[value(default)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<ChatToolUse>,
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
                        && run.origin.as_ref().is_none_or(|origin| {
                            !origin.model_call_id.trim().is_empty()
                                && !origin.tool_call_id.trim().is_empty()
                        })
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
    pub fn seal(&mut self, pending: ToolUsePhase) {
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

mod reducer;
mod storage;
pub use reducer::{ToolUseReducer, call_id, message_id};
pub use storage::{link_executions, load_histories, recover, seal_call};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatDatabase, storage};
    use diesel::connection::SimpleConnection;

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
            calls: Vec::new(),
            invocation: invocation.clone(),
            incomplete: false,
            finished,
            runs: vec![ToolUse {
                origin: None,
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
            reopened.tool_uses(&invocation.call_id).unwrap(),
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
                    .tool_uses(&retried.invocation.call_id)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                database.tool_uses(&invocation.call_id).unwrap(),
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
        let saved = reopened.tool_uses(&invocation.call_id).unwrap().unwrap();
        assert!(saved.finished && saved.incomplete);
        assert_eq!(saved.runs[0].phase, ToolUsePhase::Interrupted);
        assert_eq!(saved.runs[1].phase, ToolUsePhase::Succeeded);
        let legacy = begin(&reopened);
        assert!(reopened.tool_uses(&legacy.call_id).unwrap().is_none());
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
        assert!(database.tool_uses(&invocation.call_id).unwrap().is_none());
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
            database.tool_uses(&invocation.call_id).unwrap(),
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
