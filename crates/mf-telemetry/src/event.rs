use crate::{
    ContractError, Count,
    identity::{RunId, WorkflowId},
    require,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Preparation,
    Dependency,
    Execution,
    Publication,
    OutputSelection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub phase: FailurePhase,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<LoopPathEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ScopePathEntry {
    Loop {
        loop_id: String,
        index: Count,
    },
    Invocation {
        call_id: String,
        binding: Count,
        target_id: String,
        invocation: Count,
    },
    Call {
        call_id: String,
    },
}

impl ScopePathEntry {
    pub fn node_id(&self) -> &str {
        match self {
            Self::Loop { loop_id, .. } => loop_id,
            Self::Call { call_id } | Self::Invocation { call_id, .. } => call_id,
        }
    }

    pub fn index(&self) -> Count {
        match self {
            Self::Loop { index, .. } => *index,
            Self::Call { .. } => Count::ZERO,
            Self::Invocation { invocation, .. } => *invocation,
        }
    }

    pub fn is_call(&self) -> bool {
        matches!(self, Self::Call { .. } | Self::Invocation { .. })
    }
}

pub type LoopPathEntry = ScopePathEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStopReason {
    Condition,
    Maximum,
    Exit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopSummary {
    pub pass_count: Count,
    pub reason: LoopStopReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPassOutcome {
    Completed,
    Exit,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum SkipCause {
    Output {
        source_node: String,
        source_output: String,
    },
    NodeSuccess {
        node_success: String,
    },
}

impl SkipCause {
    pub fn matches_dependency(
        &self,
        target: &str,
        data: &[crate::description::DataEdge],
        controls: &[crate::description::ControlEdge],
        orders: &[crate::description::OrderEdge],
    ) -> bool {
        match self {
            Self::Output {
                source_node,
                source_output,
            } => {
                data.iter().any(|edge| {
                    edge.to_node == target
                        && edge.from_node == *source_node
                        && edge.from_output == *source_output
                }) || controls.iter().any(|edge| {
                    edge.to_node == target
                        && edge.from_node == *source_node
                        && edge.from_output == *source_output
                })
            }
            Self::NodeSuccess { node_success } => orders
                .iter()
                .any(|edge| edge.to_node == target && edge.from_node == *node_success),
        }
    }

    pub fn source_node(&self) -> &str {
        match self {
            Self::Output { source_node, .. } => source_node,
            Self::NodeSuccess { node_success } => node_success,
        }
    }
    pub fn source_output(&self) -> Option<&str> {
        match self {
            Self::Output { source_output, .. } => Some(source_output),
            Self::NodeSuccess { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSelection {
    pub input: String,
    pub source_node: String,
    pub source_output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_name", content = "body")]
pub enum Event {
    #[serde(rename = "mf.workflow.started")]
    WorkflowStarted { elapsed_ns: Count },
    #[serde(rename = "mf.node.started")]
    NodeStarted {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selected_inputs: Vec<InputSelection>,
        node: NodeIdentity,
        position: Count,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.node.finished")]
    NodeFinished {
        node: NodeIdentity,
        position: Count,
        elapsed_ns: Count,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ns: Option<Count>,
        outcome: Outcome,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        loop_summary: Option<LoopSummary>,
    },
    #[serde(rename = "mf.node.skipped")]
    NodeSkipped {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selected_inputs: Vec<InputSelection>,
        node: NodeIdentity,
        position: Count,
        elapsed_ns: Count,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    },
    #[serde(rename = "mf.loop.pass.started")]
    LoopPassStarted {
        path: Vec<LoopPathEntry>,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.loop.pass.finished")]
    LoopPassFinished {
        path: Vec<LoopPathEntry>,
        elapsed_ns: Count,
        outcome: LoopPassOutcome,
    },
    #[serde(rename = "mf.call.started")]
    CallStarted {
        path: Vec<LoopPathEntry>,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.call.finished")]
    CallFinished {
        path: Vec<LoopPathEntry>,
        elapsed_ns: Count,
        outcome: LoopPassOutcome,
    },
    #[serde(rename = "mf.workflow.finished")]
    WorkflowFinished {
        elapsed_ns: Count,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure_node_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
    },
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Self::WorkflowStarted { .. } => "mf.workflow.started",
            Self::NodeStarted { .. } => "mf.node.started",
            Self::NodeFinished { .. } => "mf.node.finished",
            Self::NodeSkipped { .. } => "mf.node.skipped",
            Self::LoopPassStarted { .. } => "mf.loop.pass.started",
            Self::LoopPassFinished { .. } => "mf.loop.pass.finished",
            Self::WorkflowFinished { .. } => "mf.workflow.finished",
            Self::CallStarted { .. } => "mf.call.started",
            Self::CallFinished { .. } => "mf.call.finished",
        }
    }

    pub fn node(&self) -> Option<(&NodeIdentity, Count)> {
        match self {
            Self::NodeStarted { node, position, .. }
            | Self::NodeFinished { node, position, .. }
            | Self::NodeSkipped { node, position, .. } => Some((node, *position)),
            _ => None,
        }
    }

    pub fn selected_inputs(&self) -> &[InputSelection] {
        match self {
            Self::NodeStarted {
                selected_inputs, ..
            }
            | Self::NodeSkipped {
                selected_inputs, ..
            } => selected_inputs,
            _ => &[],
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        let mut inputs = BTreeSet::new();
        for selection in self.selected_inputs() {
            require(
                !selection.input.is_empty()
                    && !selection.source_node.is_empty()
                    && !selection.source_output.is_empty(),
                "blank selected input identity",
            )?;
            require(inputs.insert(&selection.input), "duplicate selected input")?;
        }
        if let Some((node, _)) = self.node() {
            require(
                !node.id.trim().is_empty() && !node.kind.trim().is_empty(),
                "blank node identity",
            )?;
            validate_path(&node.path)?;
        }
        match self {
            Self::WorkflowStarted { elapsed_ns } => {
                require(
                    *elapsed_ns == Count::ZERO,
                    "workflow start elapsed_ns must be zero",
                )?;
            }
            Self::NodeFinished {
                outcome,
                failure,
                produced_ports,
                skipped_ports,
                duration_ns,
                elapsed_ns,
                loop_summary,
                ..
            } => {
                validate_outcome(*outcome, failure)?;
                unique_ports(produced_ports)?;
                unique_ports(skipped_ports)?;
                require(
                    !produced_ports.iter().any(|p| skipped_ports.contains(p)),
                    "produced and skipped ports overlap",
                )?;
                if let Some(failure) = failure {
                    require(
                        failure.phase != FailurePhase::OutputSelection,
                        "output selection is a workflow failure",
                    )?;
                    require(
                        produced_ports.is_empty() && skipped_ports.is_empty(),
                        "failed node has no published ports",
                    )?;
                }
                let pre_invocation = failure.as_ref().is_some_and(|f| {
                    matches!(
                        f.phase,
                        FailurePhase::Preparation | FailurePhase::Dependency
                    )
                });
                require(
                    pre_invocation == duration_ns.is_none(),
                    "execution duration must be absent only for pre-invocation failure",
                )?;
                require(
                    duration_ns.is_none_or(|d| d <= *elapsed_ns),
                    "duration exceeds elapsed time",
                )?;
                if let Some(summary) = loop_summary {
                    require(
                        *outcome == Outcome::Succeeded && summary.pass_count.get() > 0,
                        "Loop summary requires a successful node and positive pass count",
                    )?;
                }
            }
            Self::NodeSkipped {
                causes,
                skipped_ports,
                ..
            } => {
                unique_ports(skipped_ports)?;
                let mut unique = BTreeSet::new();
                require(!causes.is_empty(), "skip requires at least one cause")?;
                for cause in causes {
                    require(
                        !cause.source_node().trim().is_empty()
                            && cause.source_output().is_none_or(|port| !port.is_empty())
                            && unique.insert(cause),
                        "invalid or duplicate skip cause",
                    )?;
                }
            }
            Self::WorkflowFinished {
                outcome,
                failure,
                failure_node_id,
                ..
            } => {
                validate_outcome(*outcome, failure)?;
                require(
                    failure_node_id
                        .as_ref()
                        .is_none_or(|id| !id.trim().is_empty()),
                    "blank failure node ID",
                )?;
                require(
                    *outcome != Outcome::Succeeded || failure_node_id.is_none(),
                    "successful workflow has a failure node",
                )?;
                if failure.as_ref().is_some_and(|failure| {
                    matches!(
                        failure.phase,
                        FailurePhase::Dependency
                            | FailurePhase::Execution
                            | FailurePhase::Publication
                    )
                }) {
                    require(
                        failure_node_id.is_some(),
                        "node failure requires a node identity",
                    )?;
                }
            }
            Self::CallStarted { path, .. } | Self::CallFinished { path, .. } => {
                validate_path(path)?;
                require(
                    path.last().is_some_and(LoopPathEntry::is_call),
                    "call scope requires a call path entry",
                )?;
                if let Self::CallFinished { outcome, .. } = self {
                    require(
                        *outcome != LoopPassOutcome::Exit,
                        "a call cannot exit a Loop",
                    )?;
                }
            }
            Self::LoopPassStarted { path, .. } | Self::LoopPassFinished { path, .. } => {
                validate_path(path)?;
                require(
                    path.last().is_some_and(|entry| !entry.is_call()),
                    "Loop pass requires a Loop path entry",
                )?;
            }
            Self::NodeStarted { .. } => {}
        }
        Ok(())
    }
}

fn validate_path(path: &[LoopPathEntry]) -> Result<(), ContractError> {
    require(
        path.len() <= crate::MAX_SCOPE_DEPTH,
        "Loop path exceeds nesting limit",
    )?;
    for entry in path {
        require(
            !entry.node_id().trim().is_empty()
                && (matches!(entry, ScopePathEntry::Invocation { .. })
                    || entry.index().get() < i64::from(crate::MAX_LOOP_ITERATIONS)),
            "invalid Loop path entry",
        )?;
        if let ScopePathEntry::Invocation { target_id, .. } = entry {
            require(!target_id.trim().is_empty(), "blank invocation target")?;
        }
    }
    Ok(())
}

fn validate_outcome(outcome: Outcome, failure: &Option<Failure>) -> Result<(), ContractError> {
    require(
        (outcome == Outcome::Failed) == failure.is_some(),
        "failure context must match outcome",
    )?;
    if let Some(failure) = failure {
        require(
            !failure.message.trim().is_empty(),
            "failure message is blank",
        )?;
    }
    Ok(())
}

fn unique_ports(ports: &[String]) -> Result<(), ContractError> {
    let mut names = BTreeSet::new();
    require(
        ports.iter().all(|p| !p.is_empty() && names.insert(p)),
        "empty or duplicate output name",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleEvent {
    pub workflow_id: WorkflowId,
    pub run_id: RunId,
    pub sequence: Count,
    pub event: Event,
}

impl LifecycleEvent {
    pub fn validate(&self) -> Result<(), ContractError> {
        require(self.sequence.get() > 0, "event sequence must be positive")?;
        self.event.validate()?;
        match &self.event {
            Event::WorkflowStarted { .. } => require(
                self.sequence.get() == 1,
                "workflow start sequence must be 1",
            ),
            _ => require(
                self.sequence.get() > 1,
                "node event must follow workflow start",
            ),
        }
    }

    /// Validates graph-relative bounds without inferring delivery or execution state.
    pub fn validate_for(
        &self,
        graph: &crate::description::WorkflowDescription,
    ) -> Result<(), ContractError> {
        graph.validate()?;
        self.validate_for_validated_description(graph)
    }

    /// Validates an event against a description already checked by `WorkflowDescription::validate`.
    pub fn validate_for_validated_description(
        &self,
        graph: &crate::description::WorkflowDescription,
    ) -> Result<(), ContractError> {
        self.validate()?;
        require(
            self.workflow_id == graph.workflow_id,
            "workflow identity mismatch",
        )?;
        let loop_schema = graph.version.supports_loops() && !graph.is_streaming();
        if let Some((identity, position)) = self.event.node() {
            graph.validate_scope_path(&identity.path)?;
            if let Some(ScopePathEntry::Invocation { target_id, .. }) = identity.path.last() {
                require(
                    identity.id == *target_id,
                    "node identity disagrees with selected invocation target",
                )?;
            }
            require(
                loop_schema || identity.path.is_empty(),
                "old event has a Loop path",
            )?;
            let static_path: Vec<_> = identity
                .path
                .iter()
                .map(|entry| entry.node_id().to_owned())
                .collect();
            let (nodes, data_edges, control_edges, order_edges, order) = if static_path.is_empty() {
                (
                    &graph.nodes,
                    &graph.data_edges,
                    &graph.control_edges,
                    &graph.order_edges,
                    &graph.execution_order,
                )
            } else {
                let body = graph
                    .body(&static_path)
                    .ok_or_else(|| crate::invalid("unknown Loop body path"))?;
                (
                    &body.nodes,
                    &body.data_edges,
                    &body.control_edges,
                    &body.order_edges,
                    &body.execution_order,
                )
            };
            let position = usize::try_from(position.get())
                .map_err(|_| crate::invalid("node position exceeds platform range"))?;
            require(
                order.get(position) == Some(&identity.id),
                "node position disagrees with execution order",
            )?;
            let node = nodes
                .iter()
                .find(|node| node.id == identity.id)
                .expect("validated graph contains ordered node");
            require(node.kind == identity.kind, "node kind mismatch")?;
            if let Event::NodeFinished {
                loop_summary: Some(summary),
                ..
            } = &self.event
            {
                require(
                    identity.kind == "workflow.loop",
                    "Loop summary belongs to a Loop node",
                )?;
                require(
                    summary.pass_count.get() <= i64::from(crate::MAX_LOOP_ITERATIONS),
                    "Loop pass count exceeds limit",
                )?;
            }
            for selection in self.event.selected_inputs() {
                require(
                    data_edges.iter().any(|edge| {
                        edge.exclusive
                            && edge.to_node == identity.id
                            && edge.to_input == selection.input
                            && edge.from_node == selection.source_node
                            && edge.from_output == selection.source_output
                    }),
                    "selected input is not an exclusive candidate",
                )?;
            }
            if let Event::NodeSkipped { causes, .. } = &self.event {
                for cause in causes {
                    require(
                        cause.matches_dependency(
                            &identity.id,
                            data_edges,
                            control_edges,
                            order_edges,
                        ),
                        "skip cause is not an incoming dependency",
                    )?;
                }
            }
        }
        match &self.event {
            Event::LoopPassStarted { path, .. }
            | Event::LoopPassFinished { path, .. }
            | Event::CallStarted { path, .. }
            | Event::CallFinished { path, .. } => {
                graph.validate_scope_path(path)?;
                require(loop_schema, "description does not support scope boundaries")?;
            }
            Event::WorkflowFinished {
                failure_node_id: Some(id),
                ..
            } => {
                require(graph.execution_order.contains(id), "unknown failure node")?;
            }
            _ => {}
        }
        Ok(())
    }
}
