//! Tool evidence from actual dispatch observations, independent of rendering.
use crate::{
    run::LocalObservation,
    state::{IterationItemObservation, NodeObservation, NodeStatus, StateSnapshot},
};
use mf_chat::{
    ChatDatabase, Invocation, MAX_TOOL_NAME_BYTES, MAX_TOOL_TEXT_BYTES, MAX_TOOL_USES,
    ToolUse as ToolRun, ToolUseHistory, ToolUsePhase as Phase,
};
use mf_runtime::{NodeSnapshot, ValueRef};
use mf_telemetry::{
    description::{LoopBodyDescription, WorkflowDescription},
    event::Outcome,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const UNOBSERVED_NAME: &str = "tool (name unobserved)";

struct Dispatch<'a> {
    arguments: &'a str,
    execution: &'a str,
    encoding: &'a str,
}

fn dispatch(body: &LoopBodyDescription) -> Option<Dispatch<'_>> {
    let source = body.source.as_deref()?;
    let connects = |from: &str, output: &str, to: &str, input: &str| {
        body.data_edges.iter().any(|edge| {
            edge.from_node == from
                && edge.from_output == output
                && edge.to_node == to
                && edge.to_input == input
        })
    };
    body.nodes
        .iter()
        .filter(|node| node.kind == "llm.tool_result")
        .find_map(|encoding| {
            let arguments = &body
                .data_edges
                .iter()
                .find(|edge| edge.to_node == encoding.id && edge.to_input == "plain_content")?
                .from_node;
            let execution = body.nodes.iter().find(|node| {
                node.kind == "builtin.router"
                    && connects(arguments, "name", &node.id, "route")
                    && connects(arguments, "arguments", &node.id, "arguments")
                    && connects(&node.id, "result", &encoding.id, "result")
            })?;
            (connects(source, "item", arguments, "call")
                && connects(source, "item", &encoding.id, "call"))
            .then_some(Dispatch {
                arguments,
                execution: &execution.id,
                encoding: &encoding.id,
            })
        })
}

fn observed<'a>(item: &'a IterationItemObservation, id: &str) -> Option<&'a NodeObservation> {
    item.nodes
        .iter()
        .find(|node| {
            node.path == item.iteration_path && node.runtime_path.is_empty() && node.node.id == id
        })
        .map(|node| &node.node)
}

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
    // The Flow classifier publishes an error object even on success. Only ok=false
    // proves rejection; legacy runners omit their error port on success.
    let rejected = arguments.is_some_and(|snapshot| match snapshot.outputs.get("ok") {
        Some(ok) => ok.as_bool() == Some(false),
        None => snapshot.outputs.get("error").is_some(),
    });
    let arguments_error = arguments
        .filter(|_| rejected)
        .and_then(|snapshot| snapshot.outputs.get("error"));
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
            UNOBSERVED_NAME.into()
        } else {
            name
        },
        phase: phase(
            rejected,
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
    description: &WorkflowDescription,
    mut data: impl FnMut(&NodeObservation) -> Option<Arc<NodeSnapshot>>,
) -> Vec<ToolRun> {
    snapshot
        .iterations
        .items
        .iter()
        .filter_map(|item| {
            // Anchor the classifier/router to the tool encoder's validated dataflow,
            // including before the encoder starts. Never match an arbitrary code/router
            // node or a nested invocation just because its kind or name looks similar.
            let boundary = dispatch(description.body(&item.iteration_path)?)?;
            let arguments_data = observed(item, boundary.arguments).and_then(&mut data);
            let execution_data = observed(item, boundary.execution).map(|node| (node, data(node)));
            let encoding_data = observed(item, boundary.encoding).map(|node| (node, data(node)));
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

/// Refresh bounded public evidence without writing storage. Both the host recorder
/// and terminal presentation use this projection; a final journal stays frozen.
pub fn refresh(history: &mut ToolUseHistory, observation: &LocalObservation, finished: bool) {
    if history.finished {
        return;
    }
    let snapshot = observation.reader.snapshot_shared();
    let description = observation.reader.description();
    let runs = collect(&snapshot, finished, &description, |node| {
        let (trace, span) = node.correlation.as_ref()?;
        observation.reader.span_data(trace, span).node
    });
    let tool_bodies: Vec<_> = snapshot
        .iterations
        .items
        .iter()
        .filter_map(|item| description.body(&item.iteration_path))
        .filter(|body| body.nodes.iter().any(|node| node.kind == "llm.tool_result"))
        .collect();
    history.incomplete |= snapshot.iterations.retention_overflows > 0
        || snapshot.iterations.protocol_conflicts > 0
        || snapshot.iterations.known_missing_count > 0
        || tool_bodies.len() > MAX_TOOL_USES
        || tool_bodies.iter().any(|body| dispatch(body).is_none())
        || (finished && runs.iter().any(|run| run.name == UNOBSERVED_NAME))
        || (finished && snapshot.lifecycle.completeness != crate::state::Completeness::Complete);
    merge(history, runs);
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
        if let Some(observation) = &self.observation {
            refresh(&mut self.history, observation, finished);
        } else {
            self.history.incomplete = true;
        }
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
            if run.name == UNOBSERVED_NAME {
                run.name = previous.name.clone();
            }
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

    fn topology(classifier: &str) -> WorkflowDescription {
        serde_json::from_value(json!({
            "version":"2026-11-11", "workflow_id":format!("sha256:{}", "b".repeat(64)),
            "execution":{"mode":"single"},
            "nodes":[{"id":"dispatch", "kind":"builtin.iteration"}],
            "execution_order":["dispatch"], "data_edges":[], "control_edges":[],
            "loop_bodies":[{
                "path":["dispatch"], "source":"item-input",
                "nodes":[
                    {"id":"item-input", "kind":"builtin.iteration_input"},
                    {"id":"parse-request", "kind":classifier},
                    {"id":"invoke-capability", "kind":"builtin.router"},
                    {"id":"encode-reply", "kind":"llm.tool_result"}
                ],
                "execution_order":["item-input", "parse-request", "invoke-capability", "encode-reply"],
                "data_edges":[
                    {"from_node":"item-input", "from_output":"item", "to_node":"parse-request", "to_input":"call"},
                    {"from_node":"item-input", "from_output":"item", "to_node":"encode-reply", "to_input":"call"},
                    {"from_node":"parse-request", "from_output":"name", "to_node":"invoke-capability", "to_input":"route"},
                    {"from_node":"parse-request", "from_output":"arguments", "to_node":"invoke-capability", "to_input":"arguments"},
                    {"from_node":"parse-request", "from_output":"plain", "to_node":"encode-reply", "to_input":"plain_content"},
                    {"from_node":"invoke-capability", "from_output":"result", "to_node":"encode-reply", "to_input":"result"}
                ], "control_edges":[]
            }]
        })).unwrap()
    }

    fn item_snapshot(description: &WorkflowDescription) -> StateSnapshot {
        use crate::state::{IterationEvent, IterationRecord, SessionState};
        use mf_telemetry::{Count, identity::RunId, iteration::IterationContext};
        let mut state = SessionState::new(description.clone(), RunId::new()).unwrap();
        state
            .apply_iteration(IterationRecord {
                context: IterationContext {
                    invocation: Count::try_from(1).unwrap(),
                    index: Count::ZERO,
                    sequence: Count::try_from(1).unwrap(),
                    elapsed_ns: Count::ZERO,
                    iteration_path: vec!["dispatch".into()],
                    body_path: vec!["dispatch".into()],
                    parent_path: vec![],
                    parent_stream_invocation: None,
                },
                event: IterationEvent::ItemStarted,
                correlation: None,
            })
            .unwrap();
        state.snapshot()
    }

    fn observe_node(snapshot: &mut StateSnapshot, id: &str, kind: &str, status: NodeStatus) {
        let mut node = NodeObservation::pending(id, kind);
        node.status = status;
        snapshot.iterations.items[0]
            .nodes
            .push(crate::state::IterationNodeObservation {
                path: vec!["dispatch".into()],
                runtime_path: vec![],
                node,
            });
    }

    #[test]
    fn flow_classifier_and_legacy_dispatch_are_scoped_and_visible_before_encoding() {
        for classifier in ["builtin.code", "llm.tool_arguments"] {
            let description = topology(classifier);
            let mut snapshot = item_snapshot(&description);
            observe_node(
                &mut snapshot,
                "parse-request",
                classifier,
                NodeStatus::Succeeded,
            );
            observe_node(
                &mut snapshot,
                "invoke-capability",
                "builtin.router",
                NodeStatus::Running,
            );
            // A nested router with the same ID cannot prove the outer tool succeeded.
            let mut nested = snapshot.iterations.items[0].nodes[1].clone();
            nested
                .runtime_path
                .push(mf_telemetry::event::LoopPathEntry::Call {
                    call_id: "nested".into(),
                });
            nested.node.status = NodeStatus::Succeeded;
            snapshot.iterations.items[0].nodes.insert(0, nested);
            let arguments = Arc::new(captured(
                json!({}),
                json!({
                    "name":"calculator", "arguments":{"left":12}, "ok":true,
                    "error":{"error":""}, "plain":true
                }),
            ));
            let runs = collect(&snapshot, false, &description, |node| {
                (node.id == "parse-request").then(|| arguments.clone())
            });
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].name, "calculator");
            assert_eq!(runs[0].phase, Phase::Running);
            assert!(
                runs[0].output.is_empty(),
                "successful classifiers publish a nonfailure error object"
            );
            assert!(
                snapshot.iterations.items[0]
                    .nodes
                    .iter()
                    .all(|node| node.node.kind != "llm.tool_result")
            );

            for node in &mut snapshot.iterations.items[0].nodes {
                if node.runtime_path.is_empty() && node.node.id == "invoke-capability" {
                    node.node.status = NodeStatus::Succeeded;
                }
            }
            observe_node(
                &mut snapshot,
                "encode-reply",
                "llm.tool_result",
                NodeStatus::Succeeded,
            );
            let result = Arc::new(captured(json!({}), json!({"result":{"content":"20"}})));
            let runs = collect(&snapshot, true, &description, |node| {
                match node.id.as_str() {
                    "parse-request" => Some(arguments.clone()),
                    "encode-reply" => Some(result.clone()),
                    _ => None,
                }
            });
            assert_eq!(runs[0].phase, Phase::Succeeded);
            assert_eq!(runs[0].output, "20");
        }
    }

    #[test]
    fn classifier_rejection_is_failed_but_arbitrary_code_and_router_pairs_are_not_tools() {
        let mut description = topology("builtin.code");
        let mut snapshot = item_snapshot(&description);
        observe_node(
            &mut snapshot,
            "parse-request",
            "builtin.code",
            NodeStatus::Succeeded,
        );
        observe_node(
            &mut snapshot,
            "invoke-capability",
            "builtin.router",
            NodeStatus::Skipped,
        );
        let arguments = Arc::new(captured(
            json!({}),
            json!({
                "name":"missing", "arguments":{}, "ok":false, "error":{"error":"unknown tool"}
            }),
        ));
        let runs = collect(&snapshot, true, &description, |_| Some(arguments.clone()));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].phase, Phase::Failed);
        assert!(runs[0].output.contains("unknown tool"));
        description.loop_bodies[0]
            .data_edges
            .retain(|edge| edge.to_input != "route");
        assert!(collect(&snapshot, true, &description, |_| Some(arguments.clone())).is_empty());
        description.loop_bodies[0]
            .nodes
            .retain(|node| node.kind != "llm.tool_result");
        assert!(collect(&snapshot, true, &description, |_| Some(arguments.clone())).is_empty());
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
        let without_detail = captured(json!({}), json!({"name":"calculator", "ok":false}));
        let tool = summarize("two".into(), Some(&without_detail), None, None, None, true);
        assert_eq!(
            tool.phase,
            Phase::Failed,
            "known rejection survives missing error detail"
        );
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
        expired.name = UNOBSERVED_NAME.into();
        expired.arguments.clear();
        expired.output.clear();
        expired.duration_ns = None;
        merge(&mut capture.history, vec![expired]);
        assert_eq!(capture.history.runs[0].phase, Phase::Succeeded);
        assert_eq!(capture.history.runs[0].name, "calculator");
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
