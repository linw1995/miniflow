//! Tool evidence from actual dispatch observations, independent of rendering.
use crate::{
    run::LocalObservation,
    state::{NodeObservation, NodeStatus, StateSnapshot},
};
use mf_chat::{
    ChatDatabase, Invocation, MAX_TOOL_NAME_BYTES, MAX_TOOL_TEXT_BYTES, MAX_TOOL_USES,
    ToolUse as ToolRun, ToolUseHistory, ToolUsePhase as Phase,
};
use mf_runtime::{NodeSnapshot, ValueRef};
use mf_telemetry::event::Outcome;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn bounded(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    format!(
        "{}…",
        &text[..text.floor_char_boundary(bytes.saturating_sub('…'.len_utf8()))]
    )
}

fn public_text(value: Option<&ValueRef>, limit: usize) -> String {
    value
        .map(|value| {
            let public = value.redacted();
            let text = public.as_str().map(str::to_owned).unwrap_or_else(|| {
                serde_json::to_string(&public).unwrap_or_else(|_| "[unavailable]".into())
            });
            bounded(&text, limit)
        })
        .unwrap_or_default()
}

fn phase(
    arguments_error: bool,
    execution: Option<NodeStatus>,
    encoding: Option<NodeStatus>,
    outcome: Option<Outcome>,
    finished: bool,
) -> Phase {
    if arguments_error
        || execution == Some(NodeStatus::Failed)
        || encoding == Some(NodeStatus::Failed)
        || outcome == Some(Outcome::Failed)
    {
        Phase::Failed
    } else if execution == Some(NodeStatus::Succeeded) {
        Phase::Succeeded
    } else if execution == Some(NodeStatus::Skipped) {
        Phase::Skipped
    } else if execution == Some(NodeStatus::Interrupted) {
        Phase::Interrupted
    } else if execution == Some(NodeStatus::Unknown) || finished {
        Phase::Unknown
    } else if execution == Some(NodeStatus::Running) {
        Phase::Running
    } else {
        Phase::Preparing
    }
}

fn summarize(
    id: String,
    arguments: Option<&NodeSnapshot>,
    execution: Option<(&NodeObservation, Option<Arc<NodeSnapshot>>)>,
    encoding: Option<(&NodeObservation, Option<Arc<NodeSnapshot>>)>,
    outcome: Option<Outcome>,
    finished: bool,
) -> ToolRun {
    let call = arguments.and_then(|snapshot| snapshot.inputs.get("call"));
    let name = public_text(
        arguments
            .and_then(|snapshot| snapshot.outputs.get("name"))
            .or_else(|| call.and_then(|call| call.get("name"))),
        MAX_TOOL_NAME_BYTES,
    );
    let request = public_text(
        arguments
            .and_then(|snapshot| snapshot.outputs.get("arguments"))
            .or_else(|| call.and_then(|call| call.get("arguments"))),
        MAX_TOOL_TEXT_BYTES,
    );
    let arguments_error = arguments.and_then(|snapshot| snapshot.outputs.get("error"));
    let encoded = encoding.as_ref().and_then(|(_, data)| data.as_ref());
    let executed = execution.as_ref().and_then(|(_, data)| data.as_ref());
    let output = public_text(
        arguments_error
            .or_else(|| {
                encoded
                    .and_then(|snapshot| snapshot.outputs.get("result"))
                    .and_then(|result| result.get("content"))
            })
            .or_else(|| executed.and_then(|snapshot| snapshot.outputs.get("result"))),
        MAX_TOOL_TEXT_BYTES,
    );
    ToolRun {
        id,
        name: if name.is_empty() {
            "tool (name unobserved)".into()
        } else {
            name
        },
        phase: phase(
            arguments_error.is_some(),
            execution.as_ref().map(|(node, _)| node.status),
            encoding.as_ref().map(|(node, _)| node.status),
            outcome,
            finished,
        ),
        arguments: request,
        output,
        duration_ns: execution
            .and_then(|(node, _)| node.duration_ns.map(|duration| duration.get())),
    }
}

pub fn collect(
    snapshot: &StateSnapshot,
    finished: bool,
    mut data: impl FnMut(&NodeObservation) -> Option<Arc<NodeSnapshot>>,
) -> Vec<ToolRun> {
    snapshot
        .iterations
        .items
        .iter()
        .filter_map(|item| {
            // This is the provider's actual tool-dispatch boundary, not an arbitrary router or model text.
            let arguments = item
                .nodes
                .iter()
                .find(|node| node.node.kind == "llm.tool_arguments")?;
            let execution = item
                .nodes
                .iter()
                .find(|node| node.node.kind == "builtin.router");
            let encoding = item
                .nodes
                .iter()
                .find(|node| node.node.kind == "llm.tool_result");
            let arguments_data = data(&arguments.node);
            let execution_data = execution.map(|node| (&node.node, data(&node.node)));
            let encoding_data = encoding.map(|node| (&node.node, data(&node.node)));
            Some(summarize(
                format!(
                    "{}:{}",
                    item.identity.invocation.get(),
                    item.identity.index.get()
                ),
                arguments_data.as_deref(),
                execution_data,
                encoding_data,
                item.outcome,
                finished || item.closed || item.interrupted,
            ))
        })
        .take(MAX_TOOL_USES)
        .collect()
}

pub(super) struct Capture {
    observation: Option<LocalObservation>,
    history: ToolUseHistory,
    next_poll: Instant,
    checkpointed: Option<ToolUseHistory>,
}

impl Capture {
    pub fn new(invocation: Invocation) -> Self {
        Self {
            observation: None,
            history: ToolUseHistory {
                invocation,
                runs: Vec::new(),
                incomplete: false,
                finished: false,
            },
            next_poll: Instant::now(),
            checkpointed: None,
        }
    }

    pub fn observe(&mut self, observation: LocalObservation) {
        self.observation = Some(observation);
    }

    fn refresh(&mut self, finished: bool) {
        let Some(observation) = &self.observation else {
            self.history.incomplete = true;
            return;
        };
        let snapshot = observation.reader.snapshot_shared();
        let runs = collect(&snapshot, finished, |node| {
            let (trace, span) = node.correlation.as_ref()?;
            observation.reader.span_data(trace, span).node
        });
        self.history.incomplete |= snapshot.iterations.retention_overflows > 0
            || snapshot.iterations.protocol_conflicts > 0
            || snapshot.iterations.known_missing_count > 0
            || snapshot
                .iterations
                .items
                .iter()
                .filter(|item| {
                    item.nodes
                        .iter()
                        .any(|node| node.node.kind == "llm.tool_arguments")
                })
                .count()
                > MAX_TOOL_USES
            || (finished
                && snapshot.lifecycle.completeness != crate::state::Completeness::Complete);
        merge(&mut self.history, runs);
    }

    pub fn checkpoint(&mut self, database: &ChatDatabase) -> Result<(), mf_chat::ChatError> {
        let now = Instant::now();
        if now < self.next_poll || self.observation.is_none() {
            return Ok(());
        }
        self.next_poll = now + Duration::from_millis(100);
        self.refresh(false);
        if self.checkpointed.as_ref() != Some(&self.history) {
            database.record_tool_uses(&self.history.invocation, &self.history)?;
            self.checkpointed = Some(self.history.clone());
        }
        Ok(())
    }

    pub fn finish(mut self, interrupted: bool) -> ToolUseHistory {
        let pending: Vec<_> = self
            .history
            .runs
            .iter()
            .filter(|run| matches!(run.phase, Phase::Preparing | Phase::Running))
            .map(|run| run.id.clone())
            .collect();
        self.refresh(true);
        for run in &mut self.history.runs {
            if matches!(run.phase, Phase::Preparing | Phase::Running)
                || (interrupted && run.phase == Phase::Unknown && pending.contains(&run.id))
            {
                run.phase = if interrupted {
                    Phase::Interrupted
                } else {
                    Phase::Unknown
                };
                self.history.incomplete = true;
            }
        }
        self.history.incomplete |= self
            .history
            .runs
            .iter()
            .any(|run| run.phase == Phase::Unknown);
        self.history.finished = true;
        self.history
    }
}

fn merge(history: &mut ToolUseHistory, runs: Vec<ToolRun>) {
    for mut run in runs {
        if let Some(previous) = history.runs.iter_mut().find(|old| old.id == run.id) {
            if run.arguments.is_empty() {
                run.arguments = previous.arguments.clone();
            }
            if run.output.is_empty() {
                run.output = previous.output.clone();
            }
            if run.duration_ns.is_none() {
                run.duration_ns = previous.duration_ns;
            }
            if matches!(
                previous.phase,
                Phase::Succeeded | Phase::Failed | Phase::Skipped | Phase::Interrupted
            ) && matches!(
                run.phase,
                Phase::Unknown | Phase::Preparing | Phase::Running
            ) {
                run.phase = previous.phase;
            }
            *previous = run;
        } else if history.runs.len() < MAX_TOOL_USES {
            history.runs.push(run);
        } else {
            history.incomplete = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::SnapshotOutcome;
    use serde_json::json;

    fn captured(inputs: serde_json::Value, outputs: serde_json::Value) -> NodeSnapshot {
        NodeSnapshot {
            inputs: inputs.into(),
            outputs: outputs.into(),
            skipped: Arc::from([]),
            outcome: SnapshotOutcome::Succeeded,
            error: None,
        }
    }

    #[test]
    fn status_requires_observed_tool_execution_not_conversation_success() {
        assert_eq!(
            phase(false, None, None, Some(Outcome::Succeeded), true),
            Phase::Unknown
        );
        assert_eq!(
            phase(false, Some(NodeStatus::Running), None, None, false),
            Phase::Running
        );
        assert_eq!(
            phase(false, Some(NodeStatus::Running), None, None, true),
            Phase::Unknown
        );
        assert_eq!(
            phase(false, Some(NodeStatus::Succeeded), None, None, true),
            Phase::Succeeded
        );
        assert_eq!(
            phase(false, Some(NodeStatus::Interrupted), None, None, true),
            Phase::Interrupted
        );
    }

    #[test]
    fn captured_tool_argument_failure_is_not_a_successful_tool() {
        let arguments = captured(
            json!({"call":{"name":"calculator","arguments":{"x":1}}}),
            json!({"error":"Invalid arguments"}),
        );
        let node = NodeObservation::pending("execute", "builtin.router");
        let tool = summarize(
            "one".into(),
            Some(&arguments),
            Some((&node, None)),
            None,
            Some(Outcome::Succeeded),
            true,
        );
        assert_eq!(tool.name, "calculator");
        assert_eq!(tool.phase, Phase::Failed);
        assert_eq!(tool.output, "Invalid arguments");
    }

    #[test]
    fn repeated_names_keep_identity_and_only_public_bounded_values_are_displayed() {
        let mut arguments = captured(
            json!({"call":{"name":"calculator"}}),
            json!({"name":"calculator","arguments":{"public":12,"private":"credential"}}),
        );
        arguments.outputs = ValueRef::object([
            ("name".into(), ValueRef::from("calculator")),
            (
                "arguments".into(),
                ValueRef::object([
                    ("public".into(), ValueRef::from(12_i64)),
                    ("private".into(), ValueRef::from("credential").into_secret()),
                ]),
            ),
        ]);
        let first = summarize("one".into(), Some(&arguments), None, None, None, false);
        let second = summarize("two".into(), Some(&arguments), None, None, None, false);
        assert_ne!(first.id, second.id);
        assert_eq!(first.name, second.name);
        assert!(first.arguments.contains("[REDACTED]"));
        assert!(!first.arguments.contains("credential"));
        assert!(
            serde_json::from_str::<serde_json::Value>(&first.arguments).is_ok(),
            "redaction notices must not corrupt the native JSON arguments panel"
        );
        let long = ValueRef::from("界".repeat(4096));
        let shown = public_text(Some(&long), 2048);
        assert!(shown.len() <= 2051);
        assert!(shown.ends_with('…'));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("redacted.sqlite");
        let database = ChatDatabase::open(&path).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let begun = database
            .begin_turn(
                &session.id,
                "tool evidence",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        let evidence = ToolUseHistory {
            invocation: begun.invocation.clone(),
            runs: vec![first, second],
            incomplete: false,
            finished: false,
        };
        database
            .record_tool_uses(&begun.invocation, &evidence)
            .unwrap();
        drop(database);
        let reopened = ChatDatabase::open(&path).unwrap();
        let saved = reopened
            .call(&begun.invocation.call_id)
            .unwrap()
            .tool_uses
            .unwrap();
        assert!(saved.runs[0].arguments.contains("[REDACTED]"));
        for file in [path.clone(), path.with_file_name("redacted.sqlite-wal")] {
            if file.exists() {
                let bytes = std::fs::read(file).unwrap();
                assert!(
                    !bytes
                        .windows(b"credential".len())
                        .any(|part| part == b"credential")
                );
            }
        }
    }

    #[test]
    fn checkpoints_merge_retained_calls_and_report_capacity_without_regressing_known_states() {
        let invocation = Invocation::new("fixture");
        let mut capture = Capture::new(invocation);
        let make = |id, phase| ToolRun {
            id: format!("dispatch:{id}"),
            name: "calculator".into(),
            phase,
            arguments: "12 + 8".into(),
            output: "20".into(),
            duration_ns: Some(40),
        };
        merge(
            &mut capture.history,
            vec![make(0, Phase::Succeeded), make(1, Phase::Running)],
        );
        let mut expired = make(0, Phase::Unknown);
        expired.arguments.clear();
        expired.output.clear();
        expired.duration_ns = None;
        merge(&mut capture.history, vec![expired]);
        assert_eq!(capture.history.runs[0].phase, Phase::Succeeded);
        assert_eq!(capture.history.runs[0].output, "20");
        merge(
            &mut capture.history,
            (2..MAX_TOOL_USES + 2)
                .map(|id| make(id, Phase::Succeeded))
                .collect(),
        );
        assert_eq!(capture.history.runs.len(), MAX_TOOL_USES);
        assert!(capture.history.incomplete);
        let final_history = capture.finish(true);
        assert_eq!(final_history.runs[0].phase, Phase::Succeeded);
        assert_eq!(final_history.runs[1].phase, Phase::Interrupted);
        assert!(final_history.finished && final_history.incomplete);
        final_history.validate().unwrap();
    }
}
