use crate::{
    ContractError, Count,
    identity::{RunId, WorkflowId},
    maximum_event_count, require,
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
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkipCause {
    pub source_node: String,
    pub source_output: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_name", content = "body")]
pub enum Event {
    #[serde(rename = "mf.workflow.started")]
    WorkflowStarted {
        node_count: Count,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.node.started")]
    NodeStarted {
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
    },
    #[serde(rename = "mf.node.skipped")]
    NodeSkipped {
        node: NodeIdentity,
        position: Count,
        elapsed_ns: Count,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    },
    #[serde(rename = "mf.workflow.finished")]
    WorkflowFinished {
        final_sequence: Count,
        elapsed_ns: Count,
        visited_node_count: Count,
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
            Self::WorkflowFinished { .. } => "mf.workflow.finished",
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

    pub fn validate(&self) -> Result<(), ContractError> {
        if let Some((node, _)) = self.node() {
            require(
                !node.id.trim().is_empty() && !node.kind.trim().is_empty(),
                "blank node identity",
            )?;
        }
        match self {
            Self::WorkflowStarted {
                node_count,
                elapsed_ns,
            } => {
                maximum_event_count(*node_count)?;
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
                        !cause.source_node.trim().is_empty()
                            && !cause.source_output.is_empty()
                            && unique.insert(cause),
                        "invalid or duplicate skip cause",
                    )?;
                }
            }
            Self::WorkflowFinished {
                outcome,
                failure,
                final_sequence,
                failure_node_id,
                visited_node_count,
                ..
            } => {
                validate_outcome(*outcome, failure)?;
                require(
                    final_sequence.get() >= 2,
                    "workflow finish sequence must follow workflow start",
                )?;
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
                if let Some(failure) = failure {
                    if failure.phase == FailurePhase::Preparation {
                        require(
                            *visited_node_count == Count::ZERO,
                            "preparation failure cannot visit execution steps",
                        )?;
                    }
                    if matches!(
                        failure.phase,
                        FailurePhase::Dependency
                            | FailurePhase::Execution
                            | FailurePhase::Publication
                    ) {
                        require(
                            failure_node_id.is_some() && visited_node_count.get() > 0,
                            "node failure requires a visited node",
                        )?;
                    }
                }
            }
            Self::NodeStarted { .. } => {}
        }
        Ok(())
    }
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
            Event::WorkflowFinished { final_sequence, .. } => require(
                *final_sequence == self.sequence,
                "final sequence does not match event sequence",
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
        self.validate()?;
        require(
            self.workflow_id == graph.workflow_id,
            "workflow identity mismatch",
        )?;
        let count = graph.node_count()?;
        require(
            self.sequence <= maximum_event_count(count)?,
            "sequence exceeds graph event bound",
        )?;
        if let Some((identity, position)) = self.event.node() {
            let position = usize::try_from(position.get())
                .map_err(|_| crate::invalid("node position exceeds platform range"))?;
            require(
                graph.execution_order.get(position) == Some(&identity.id),
                "node position disagrees with execution order",
            )?;
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == identity.id)
                .expect("validated graph contains ordered node");
            require(node.kind == identity.kind, "node kind mismatch")?;
            if let Event::NodeSkipped { causes, .. } = &self.event {
                for cause in causes {
                    require(
                        graph.data_edges.iter().any(|e| {
                            e.to_node == identity.id
                                && e.from_node == cause.source_node
                                && e.from_output == cause.source_output
                        }) || graph.control_edges.iter().any(|e| {
                            e.to_node == identity.id
                                && e.from_node == cause.source_node
                                && e.from_output == cause.source_output
                        }),
                        "skip cause is not an incoming dependency",
                    )?;
                }
            }
        }
        match &self.event {
            Event::WorkflowStarted { node_count, .. } => {
                require(*node_count == count, "node count mismatch")?
            }
            Event::WorkflowFinished {
                visited_node_count,
                failure_node_id,
                failure,
                outcome,
                ..
            } => {
                require(*visited_node_count <= count, "visited prefix exceeds graph")?;
                if *outcome == Outcome::Succeeded
                    || failure
                        .as_ref()
                        .is_some_and(|f| f.phase == FailurePhase::OutputSelection)
                {
                    require(
                        *visited_node_count == count,
                        "output selection requires all steps visited",
                    )?;
                }
                if let Some(id) = failure_node_id {
                    require(graph.execution_order.contains(id), "unknown failure node")?;
                    if failure.as_ref().is_some_and(|f| {
                        matches!(
                            f.phase,
                            FailurePhase::Dependency
                                | FailurePhase::Execution
                                | FailurePhase::Publication
                        )
                    }) {
                        let index = usize::try_from(visited_node_count.get() - 1)
                            .map_err(|_| crate::invalid("invalid visited prefix"))?;
                        require(
                            graph.execution_order.get(index) == Some(id),
                            "failure node must end the visited prefix",
                        )?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Reserves identity before serialization/enqueueing; callers never roll it back.
#[derive(Debug)]
pub struct EventSequence {
    next: i64,
    maximum: i64,
    closed: bool,
}

impl EventSequence {
    pub fn new(node_count: Count) -> Result<Self, ContractError> {
        Ok(Self {
            next: 1,
            maximum: maximum_event_count(node_count)?.get(),
            closed: false,
        })
    }

    pub fn reserve(&mut self) -> Result<Count, ContractError> {
        // Keep one slot for a terminal boundary, even when every node emits two records.
        require(
            !self.closed && self.next < self.maximum,
            "lifecycle sequence is exhausted or closed",
        )?;
        let sequence = Count::try_from(self.next)?;
        self.next += 1;
        Ok(sequence)
    }

    pub fn finish(&mut self) -> Result<Count, ContractError> {
        require(
            !self.closed && self.next > 1,
            "lifecycle sequence is closed or has not started",
        )?;
        self.closed = true;
        Count::try_from(self.next)
    }
}
