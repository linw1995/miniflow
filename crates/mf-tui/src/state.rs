//! Bounded, transport-independent state reduction for one workflow run.

use mf_telemetry::{
    ContractError, Count, EVENT_SCHEMA_VERSION, LOOP_EVENT_SCHEMA_VERSION,
    description::{NodeDescription, WorkflowDescription, WorkflowDescriptionVersion},
    event::{
        Event, Failure, FailurePhase, LifecycleEvent, LoopPassOutcome, LoopPathEntry,
        LoopStopReason, LoopSummary, Outcome, SkipCause,
    },
    identity::RunId,
    maximum_event_count, maximum_loop_event_count,
};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

const MAX_DIAGNOSTICS: usize = 64;
const MAX_DIAGNOSTIC_ENTRY_BYTES: usize = 1024;
const MAX_EVENT_BYTES: usize = 128 * 1024;
const MAX_VISIBLE_PORTS: usize = 32;
const MAX_PORT_NAME_BYTES: usize = 128;
const MAX_FAILURE_MESSAGE_BYTES: usize = 4096;
const MAX_MISSING_RANGES: usize = 64;
pub const MAX_SESSION_NODES: usize = 10_000;
const MAX_RECENT_LOOP_PASSES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    NotRun,
    Unknown,
    Interrupted,
}

impl NodeStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::NotRun
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureSummary {
    pub phase: FailurePhase,
    pub message: String,
    pub dropped_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeObservation {
    pub id: String,
    pub kind: String,
    pub status: NodeStatus,
    pub last_known: Option<NodeStatus>,
    pub conflicted: bool,
    pub possibly_missing_events: bool,
    pub started_elapsed_ns: Option<Count>,
    pub terminal_elapsed_ns: Option<Count>,
    pub duration_ns: Option<Count>,
    pub produced_ports: Vec<String>,
    pub skipped_ports: Vec<String>,
    pub skip_causes: Vec<SkipCause>,
    pub failure: Option<FailureSummary>,
    pub omitted_port_names: usize,
    pub omitted_skip_causes: usize,
    started_sequence: Option<Count>,
    terminal_sequence: Option<Count>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Collecting,
    Complete,
    Incomplete,
    UnverifiedTail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingRange {
    pub first: i64,
    pub last: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleIntegrity {
    pub completeness: Completeness,
    pub known_missing_count: u64,
    pub known_missing_ranges: Vec<MissingRange>,
    pub omitted_missing_ranges: usize,
    pub final_sequence: Option<Count>,
    pub unresolved_visited_nodes: usize,
    pub local_drops: u64,
    pub protocol_conflicts: u64,
    pub observation_errors: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceAvailability {
    pub observed_spans: u64,
    pub local_drops: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateSnapshot {
    pub nodes: Vec<NodeObservation>,
    pub workflow_outcome: Option<Outcome>,
    pub workflow_failure: Option<FailureSummary>,
    pub lifecycle: LifecycleIntegrity,
    pub traces: TraceAvailability,
    pub diagnostic_bytes_dropped: u64,
    pub diagnostics: Vec<String>,
    pub loop_passes: Vec<LoopPassObservation>,
    pub total_loop_passes: usize,
    pub hidden_loop_passes: usize,
    pub loop_overviews: Vec<LoopOverview>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopPassObservation {
    pub path: Vec<LoopPathEntry>,
    pub nodes: Vec<NodeObservation>,
    pub started: bool,
    pub outcome: Option<LoopPassOutcome>,
    pub visited_node_count: Option<Count>,
    pub interrupted: bool,
}

impl LoopPassObservation {
    pub fn display_nodes<'a>(
        &self,
        order: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<NodeObservation> {
        let observed: BTreeMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        order
            .into_iter()
            .enumerate()
            .map(|(position, (id, kind))| {
                let mut node = observed.get(id).map_or_else(
                    || NodeObservation::pending(id, kind),
                    |node| (*node).clone(),
                );
                if self
                    .visited_node_count
                    .is_some_and(|visited| position as i64 >= visited.get())
                    && node.status == NodeStatus::Pending
                {
                    node.status = NodeStatus::NotRun;
                } else if self.interrupted {
                    if node.status == NodeStatus::Pending {
                        node.status = NodeStatus::Unknown;
                    } else if node.status == NodeStatus::Running {
                        node.last_known = Some(NodeStatus::Running);
                        node.status = NodeStatus::Interrupted;
                    }
                }
                node
            })
            .collect()
    }
}

impl NodeObservation {
    pub fn pending(id: &str, kind: &str) -> Self {
        pending_node_parts(id, kind)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopOverview {
    pub parent_path: Vec<LoopPathEntry>,
    pub loop_id: String,
    pub completed_passes: usize,
    pub active_index: Option<Count>,
    pub stop_reason: Option<LoopStopReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Applied,
    Duplicate,
    Conflict,
    UnrelatedRun,
}

#[derive(Debug, Snafu)]
pub enum StateError {
    #[snafu(display("streaming observation requires a compatible streaming consumer"))]
    UnsupportedStream,
    #[snafu(display("invalid workflow description: {source}"))]
    Description { source: ContractError },
    #[snafu(display("invalid lifecycle event: {source}"))]
    Event { source: ContractError },
    #[snafu(display("could not encode lifecycle event: {source}"))]
    Serialize { source: serde_json::Error },
    #[snafu(display("lifecycle event exceeded the {limit}-byte admission limit"))]
    TooLarge { limit: usize },
    #[snafu(display("workflow description exceeded the {limit}-node session limit"))]
    TooManyNodes { limit: usize },
    #[snafu(display("workflow observation has finished collecting"))]
    Closed,
}

pub struct SessionState {
    description: WorkflowDescription,
    run_id: RunId,
    node_positions: BTreeMap<String, usize>,
    nodes: Vec<NodeObservation>,
    max_sequence: Count,
    received: BTreeMap<i64, SequenceWitness>,
    final_boundary: Option<FinalBoundary>,
    local_lifecycle_drops: u64,
    protocol_conflicts: u64,
    observation_errors: u64,
    traces: TraceAvailability,
    diagnostic_bytes_dropped: u64,
    diagnostics: VecDeque<String>,
    pass_states: BTreeMap<Vec<LoopPathEntry>, PassState>,
    pass_order: BTreeSet<(Count, Vec<LoopPathEntry>)>,
    loop_summaries: BTreeMap<(Vec<LoopPathEntry>, String), LoopSummary>,
    closed: bool,
    snapshot_cache: Mutex<Option<Arc<StateSnapshot>>>,
}

#[derive(Default)]
struct PassState {
    first_sequence: Option<Count>,
    started_sequence: Option<Count>,
    terminal_sequence: Option<Count>,
    visited_node_count: Option<Count>,
    outcome: Option<LoopPassOutcome>,
    nodes: BTreeMap<String, NodeObservation>,
}

#[derive(Clone)]
struct FinalBoundary {
    sequence: Count,
    visited: Count,
    visited_steps: Option<Count>,
    outcome: Outcome,
    failure_node_id: Option<String>,
    failure: Option<Failure>,
}

struct SequenceWitness {
    signature: [u8; 32],
    node: Option<(Vec<LoopPathEntry>, String)>,
}

impl SessionState {
    pub fn new(description: WorkflowDescription, run_id: RunId) -> Result<Self, StateError> {
        if description.version.is_streaming() {
            return Err(StateError::UnsupportedStream);
        }
        description
            .validate()
            .map_err(|source| StateError::Description { source })?;
        if description
            .static_node_count()
            .map_err(|source| StateError::Description { source })?
            .get()
            > MAX_SESSION_NODES as i64
        {
            return Err(StateError::TooManyNodes {
                limit: MAX_SESSION_NODES,
            });
        }
        let max_sequence = if description.version == WorkflowDescriptionVersion::V2026_09_29 {
            maximum_loop_event_count()
        } else {
            maximum_event_count(
                description
                    .node_count()
                    .map_err(|source| StateError::Description { source })?,
            )
            .map_err(|source| StateError::Description { source })?
        };
        let described: BTreeMap<_, _> = description
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let mut node_positions = BTreeMap::new();
        let nodes = description
            .execution_order
            .iter()
            .enumerate()
            .map(|(position, id)| {
                let node = described[id.as_str()];
                node_positions.insert(id.clone(), position);
                pending_node(node)
            })
            .collect();
        Ok(Self {
            description,
            run_id,
            node_positions,
            nodes,
            max_sequence,
            received: BTreeMap::new(),
            final_boundary: None,
            local_lifecycle_drops: 0,
            protocol_conflicts: 0,
            observation_errors: 0,
            traces: TraceAvailability::default(),
            diagnostic_bytes_dropped: 0,
            diagnostics: VecDeque::new(),
            pass_states: BTreeMap::new(),
            pass_order: BTreeSet::new(),
            loop_summaries: BTreeMap::new(),
            closed: false,
            snapshot_cache: Mutex::new(None),
        })
    }

    pub fn expected_event_schema_version(&self) -> i64 {
        if self.description.version == WorkflowDescriptionVersion::V2026_09_29 {
            LOOP_EVENT_SCHEMA_VERSION
        } else {
            EVENT_SCHEMA_VERSION
        }
    }

    pub fn apply(&mut self, event: LifecycleEvent) -> Result<Admission, StateError> {
        if self.closed {
            return Err(StateError::Closed);
        }
        if event.run_id != self.run_id || event.workflow_id != self.description.workflow_id {
            self.note("ignored event from another workflow or run");
            return Ok(Admission::UnrelatedRun);
        }
        if event.sequence > self.max_sequence {
            return Err(StateError::Event {
                source: ContractError::Invalid {
                    message: "sequence exceeds graph event bound".into(),
                },
            });
        }
        event
            .validate_for_validated_description(&self.description)
            .map_err(|source| StateError::Event { source })?;
        let encoded =
            serde_json::to_vec(&event.event).map_err(|source| StateError::Serialize { source })?;
        if encoded.len() > MAX_EVENT_BYTES {
            return Err(StateError::TooLarge {
                limit: MAX_EVENT_BYTES,
            });
        }
        let signature: [u8; 32] = Sha256::digest(&encoded).into();
        let sequence = event.sequence.get();
        if let Some(existing) = self.received.get(&sequence) {
            if existing.signature == signature {
                return Ok(Admission::Duplicate);
            }
            let previous_node = existing.node.clone();
            self.conflict(
                "same sequence carried conflicting lifecycle records",
                event.event.node(),
            );
            if let Some((path, id)) = previous_node {
                if path.is_empty() {
                    if let Some(&position) = self.node_positions.get(&id) {
                        self.nodes[position].conflicted = true;
                    }
                } else if let Some(node) = self
                    .pass_states
                    .get_mut(&path)
                    .and_then(|pass| pass.nodes.get_mut(&id))
                {
                    node.conflicted = true;
                }
            }
            return Ok(Admission::Conflict);
        }
        if self
            .final_boundary
            .as_ref()
            .is_some_and(|boundary| event.sequence > boundary.sequence)
        {
            self.conflict("event followed the final sequence", event.event.node());
            return Ok(Admission::Conflict);
        }
        self.invalidate_snapshot();
        self.received.insert(
            sequence,
            SequenceWitness {
                signature,
                node: event
                    .event
                    .node()
                    .map(|(node, _)| (node.path.clone(), node.id.clone())),
            },
        );
        let conflicted = if matches!(
            &event.event,
            Event::LoopPassStarted { .. } | Event::LoopPassFinished { .. }
        ) || event
            .event
            .node()
            .is_some_and(|(node, _)| !node.path.is_empty())
        {
            self.apply_loop_event(event.sequence, &event.event)
        } else {
            self.apply_event(event.sequence, &event.event)
        };
        if !conflicted
            && let Event::NodeFinished {
                node,
                loop_summary: Some(summary),
                ..
            } = &event.event
        {
            self.loop_summaries
                .insert((node.path.clone(), node.id.clone()), summary.clone());
        }
        if conflicted {
            Ok(Admission::Conflict)
        } else {
            Ok(Admission::Applied)
        }
    }

    pub fn record_local_lifecycle_drop(&mut self, count: u64, reason: &str) {
        self.invalidate_snapshot();
        self.local_lifecycle_drops = self.local_lifecycle_drops.saturating_add(count);
        self.note(reason);
    }

    pub fn record_trace_span(&mut self) {
        self.invalidate_snapshot();
        self.traces.observed_spans = self.traces.observed_spans.saturating_add(1);
    }

    pub fn record_local_trace_drop(&mut self, count: u64, reason: &str) {
        self.invalidate_snapshot();
        self.traces.local_drops = self.traces.local_drops.saturating_add(count);
        self.note(reason);
    }

    pub fn record_diagnostic_truncation(&mut self, bytes: u64) {
        let total = self.diagnostic_bytes_dropped.saturating_add(bytes);
        if total != self.diagnostic_bytes_dropped {
            self.invalidate_snapshot();
            self.diagnostic_bytes_dropped = total;
        }
    }

    pub fn record_diagnostic(&mut self, message: &str) {
        self.invalidate_snapshot();
        self.note(message);
    }

    pub fn record_observation_error(&mut self, reason: &str) {
        self.invalidate_snapshot();
        self.observation_errors = self.observation_errors.saturating_add(1);
        self.note(reason);
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.invalidate_snapshot();
        self.closed = true;
        if self.final_boundary.is_none() {
            for node in &mut self.nodes {
                match node.status {
                    NodeStatus::Pending => node.status = NodeStatus::Unknown,
                    NodeStatus::Running => {
                        node.status = NodeStatus::Interrupted;
                        node.last_known = Some(NodeStatus::Running);
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn snapshot(&self) -> StateSnapshot {
        self.build_snapshot()
    }

    /// Shares the last immutable snapshot until new evidence changes the session.
    pub fn snapshot_shared(&self) -> Arc<StateSnapshot> {
        let mut cached = self
            .snapshot_cache
            .lock()
            .expect("snapshot cache was not poisoned");
        Arc::clone(cached.get_or_insert_with(|| Arc::new(self.build_snapshot())))
    }

    fn invalidate_snapshot(&mut self) {
        self.snapshot_cache
            .get_mut()
            .expect("snapshot cache was not poisoned")
            .take();
    }

    fn build_snapshot(&self) -> StateSnapshot {
        let lifecycle = self.integrity();
        let has_gap = lifecycle.known_missing_count != 0;
        let mut nodes = self.nodes.clone();
        if has_gap {
            for node in &mut nodes {
                node.possibly_missing_events =
                    matches!(node.status, NodeStatus::Pending | NodeStatus::Running);
            }
        }
        StateSnapshot {
            nodes,
            workflow_outcome: self
                .final_boundary
                .as_ref()
                .map(|boundary| boundary.outcome),
            workflow_failure: self
                .final_boundary
                .as_ref()
                .and_then(|boundary| boundary.failure.as_ref())
                .map(bounded_failure),
            lifecycle,
            traces: self.traces,
            diagnostic_bytes_dropped: self.diagnostic_bytes_dropped,
            diagnostics: self.diagnostics.iter().cloned().collect(),
            loop_passes: self.recent_loop_pass_snapshots(),
            total_loop_passes: self.pass_states.len(),
            hidden_loop_passes: self
                .pass_states
                .len()
                .saturating_sub(MAX_RECENT_LOOP_PASSES),
            loop_overviews: self.loop_overviews(),
        }
    }

    fn recent_loop_pass_snapshots(&self) -> Vec<LoopPassObservation> {
        let recent: Vec<_> = self
            .pass_order
            .iter()
            .rev()
            .take(MAX_RECENT_LOOP_PASSES)
            .collect();
        recent
            .into_iter()
            .rev()
            .map(|(_, path)| {
                let state = &self.pass_states[path];
                LoopPassObservation {
                    path: path.clone(),
                    nodes: state.nodes.values().cloned().collect(),
                    started: state.started_sequence.is_some(),
                    outcome: state.outcome,
                    visited_node_count: state.visited_node_count,
                    interrupted: self.closed && self.final_boundary.is_none(),
                }
            })
            .collect()
    }

    fn loop_overviews(&self) -> Vec<LoopOverview> {
        let mut overviews: BTreeMap<(Vec<LoopPathEntry>, String), LoopOverview> = BTreeMap::new();
        for (path, state) in &self.pass_states {
            let Some((last, parent)) = path.split_last() else {
                continue;
            };
            let key = (parent.to_vec(), last.loop_id.clone());
            let overview = overviews.entry(key).or_insert_with(|| LoopOverview {
                parent_path: parent.to_vec(),
                loop_id: last.loop_id.clone(),
                completed_passes: 0,
                active_index: None,
                stop_reason: None,
            });
            if state.terminal_sequence.is_some() {
                overview.completed_passes += 1;
            } else if overview.active_index.is_none_or(|index| last.index > index) {
                overview.active_index = Some(last.index);
            }
        }
        for ((parent, id), summary) in &self.loop_summaries {
            let key = (parent.clone(), id.clone());
            let overview = overviews.entry(key).or_insert_with(|| LoopOverview {
                parent_path: parent.clone(),
                loop_id: id.clone(),
                completed_passes: 0,
                active_index: None,
                stop_reason: None,
            });
            overview.stop_reason = Some(summary.reason);
            overview.completed_passes = summary.pass_count.get() as usize;
            overview.active_index = None;
        }
        overviews.into_values().collect()
    }

    pub fn integrity(&self) -> LifecycleIntegrity {
        let upper = self.final_boundary.as_ref().map_or_else(
            || {
                self.received
                    .last_key_value()
                    .map_or(0, |(sequence, _)| *sequence)
            },
            |boundary| boundary.sequence.get(),
        );
        let mut cursor = 1;
        let mut known_missing_count = 0u64;
        let mut ranges = Vec::new();
        let mut omitted_missing_ranges = 0;
        for &sequence in self.received.keys().filter(|&&sequence| sequence <= upper) {
            if sequence > cursor {
                add_gap(
                    cursor,
                    sequence - 1,
                    &mut ranges,
                    &mut omitted_missing_ranges,
                    &mut known_missing_count,
                );
            }
            cursor = sequence.saturating_add(1);
        }
        if cursor <= upper {
            add_gap(
                cursor,
                upper,
                &mut ranges,
                &mut omitted_missing_ranges,
                &mut known_missing_count,
            );
        }
        let mut unresolved_visited_nodes = self.final_boundary.as_ref().map_or(0, |boundary| {
            self.nodes
                .iter()
                .take(boundary.visited.get() as usize)
                .filter(|node| {
                    !matches!(
                        node.status,
                        NodeStatus::Succeeded | NodeStatus::Failed | NodeStatus::Skipped
                    )
                })
                .count()
        });
        if self.final_boundary.is_some() {
            for (path, state) in &self.pass_states {
                if state.started_sequence.is_none()
                    || (path.len() > 1 && !self.pass_states.contains_key(&path[..path.len() - 1]))
                {
                    unresolved_visited_nodes += 1;
                }
                let static_path: Vec<_> = path.iter().map(|entry| entry.loop_id.clone()).collect();
                let body = self
                    .description
                    .loop_body(&static_path)
                    .expect("validated pass path has a body");
                if let Some(visited) = state.visited_node_count {
                    unresolved_visited_nodes += body
                        .execution_order
                        .iter()
                        .take(visited.get() as usize)
                        .filter(|id| {
                            state.nodes.get(*id).is_none_or(|node| {
                                !matches!(
                                    node.status,
                                    NodeStatus::Succeeded
                                        | NodeStatus::Failed
                                        | NodeStatus::Skipped
                                )
                            })
                        })
                        .count();
                } else {
                    unresolved_visited_nodes += 1;
                }
            }
            if let Some(boundary) = &self.final_boundary
                && let Some(expected) = boundary.visited_steps
            {
                let observed = boundary.visited.get()
                    + self
                        .pass_states
                        .values()
                        .filter_map(|pass| pass.visited_node_count)
                        .map(Count::get)
                        .sum::<i64>();
                unresolved_visited_nodes += expected.get().abs_diff(observed) as usize;
            }
        }
        let completeness = match (&self.final_boundary, self.closed) {
            (None, true) => Completeness::UnverifiedTail,
            (None, false) => Completeness::Collecting,
            (Some(_), _)
                if known_missing_count == 0
                    && unresolved_visited_nodes == 0
                    && self.local_lifecycle_drops == 0
                    && self.protocol_conflicts == 0
                    && self.observation_errors == 0 =>
            {
                Completeness::Complete
            }
            (Some(_), true) => Completeness::Incomplete,
            (Some(_), false) => Completeness::Collecting,
        };
        LifecycleIntegrity {
            completeness,
            known_missing_count,
            known_missing_ranges: ranges,
            omitted_missing_ranges,
            final_sequence: self
                .final_boundary
                .as_ref()
                .map(|boundary| boundary.sequence),
            unresolved_visited_nodes,
            local_drops: self.local_lifecycle_drops,
            protocol_conflicts: self.protocol_conflicts,
            observation_errors: self.observation_errors,
        }
    }

    fn ensure_pass(&mut self, path: &[LoopPathEntry], sequence: Count) -> &mut PassState {
        let pass = self.pass_states.entry(path.to_vec()).or_default();
        let first = pass
            .first_sequence
            .map_or(sequence, |first| first.min(sequence));
        if pass.first_sequence != Some(first) {
            if let Some(previous) = pass.first_sequence {
                self.pass_order.remove(&(previous, path.to_vec()));
            }
            self.pass_order.insert((first, path.to_vec()));
            pass.first_sequence = Some(first);
        }
        pass
    }

    fn apply_loop_event(&mut self, sequence: Count, event: &Event) -> bool {
        let path = match event {
            Event::LoopPassStarted { path, .. } | Event::LoopPassFinished { path, .. } => path,
            Event::NodeStarted { node, .. }
            | Event::NodeFinished { node, .. }
            | Event::NodeSkipped { node, .. } => &node.path,
            _ => return false,
        };
        let body_path: Vec<_> = path.iter().map(|entry| entry.loop_id.clone()).collect();
        let body_len = self
            .description
            .loop_body(&body_path)
            .expect("validated event has a body")
            .nodes
            .len();
        let mut violation = None;
        let pass = self.ensure_pass(path, sequence);
        match event {
            Event::LoopPassStarted { .. } => {
                if pass.started_sequence.is_some()
                    || pass
                        .terminal_sequence
                        .is_some_and(|finished| sequence > finished)
                {
                    violation = Some("Loop pass started more than once or after completion");
                } else {
                    pass.started_sequence = Some(sequence);
                }
            }
            Event::LoopPassFinished {
                visited_node_count,
                outcome,
                ..
            } => {
                if pass.terminal_sequence.is_some()
                    || pass
                        .started_sequence
                        .is_some_and(|started| sequence < started)
                    || (*outcome == LoopPassOutcome::Completed
                        && visited_node_count.get() != body_len as i64)
                {
                    violation = Some("Loop pass has a conflicting finish boundary");
                } else {
                    pass.terminal_sequence = Some(sequence);
                    pass.visited_node_count = Some(*visited_node_count);
                    pass.outcome = Some(*outcome);
                }
            }
            Event::NodeStarted {
                node, elapsed_ns, ..
            } => {
                if pass
                    .terminal_sequence
                    .is_some_and(|finished| sequence > finished)
                {
                    violation = Some("body node started after pass completion");
                } else {
                    let observed = pass
                        .nodes
                        .entry(node.id.clone())
                        .or_insert_with(|| pending_node_parts(&node.id, &node.kind));
                    if observed.started_sequence.is_some()
                        || observed
                            .terminal_sequence
                            .is_some_and(|finished| sequence > finished)
                    {
                        violation = Some("body node started more than once or after completion");
                    } else {
                        observed.started_sequence = Some(sequence);
                        observed.started_elapsed_ns = Some(*elapsed_ns);
                        if observed.terminal_sequence.is_none() {
                            observed.status = NodeStatus::Running;
                        }
                    }
                }
            }
            Event::NodeFinished {
                node,
                elapsed_ns,
                duration_ns,
                outcome,
                produced_ports,
                skipped_ports,
                failure,
                ..
            } => {
                if pass
                    .terminal_sequence
                    .is_some_and(|finished| sequence > finished)
                {
                    violation = Some("body node finished after pass completion");
                } else {
                    let observed = pass
                        .nodes
                        .entry(node.id.clone())
                        .or_insert_with(|| pending_node_parts(&node.id, &node.kind));
                    if observed.terminal_sequence.is_some()
                        || observed
                            .started_sequence
                            .is_some_and(|started| sequence < started)
                    {
                        violation = Some("body node received conflicting outcomes");
                    } else {
                        observed.status = if *outcome == Outcome::Succeeded {
                            NodeStatus::Succeeded
                        } else {
                            NodeStatus::Failed
                        };
                        observed.terminal_sequence = Some(sequence);
                        observed.terminal_elapsed_ns = Some(*elapsed_ns);
                        observed.duration_ns = *duration_ns;
                        let (produced, omitted_produced) = bounded_ports(produced_ports);
                        let (skipped, omitted_skipped) = bounded_ports(skipped_ports);
                        observed.produced_ports = produced;
                        observed.skipped_ports = skipped;
                        observed.omitted_port_names = omitted_produced + omitted_skipped;
                        observed.failure = failure.as_ref().map(bounded_failure);
                    }
                }
            }
            Event::NodeSkipped {
                node,
                elapsed_ns,
                causes,
                skipped_ports,
                ..
            } => {
                if pass
                    .terminal_sequence
                    .is_some_and(|finished| sequence > finished)
                {
                    violation = Some("body node skipped after pass completion");
                } else {
                    let observed = pass
                        .nodes
                        .entry(node.id.clone())
                        .or_insert_with(|| pending_node_parts(&node.id, &node.kind));
                    if observed.started_sequence.is_some() || observed.terminal_sequence.is_some() {
                        violation = Some("body node skipped after another outcome");
                    } else {
                        observed.status = NodeStatus::Skipped;
                        observed.terminal_sequence = Some(sequence);
                        observed.terminal_elapsed_ns = Some(*elapsed_ns);
                        let (visible_causes, omitted_causes) = bounded_causes(causes);
                        observed.skip_causes = visible_causes;
                        observed.omitted_skip_causes = omitted_causes;
                        let (ports, omitted) = bounded_ports(skipped_ports);
                        observed.skipped_ports = ports;
                        observed.omitted_port_names = omitted;
                    }
                }
            }
            _ => {}
        }
        if let Some(message) = violation {
            self.conflict(message, event.node());
            true
        } else {
            false
        }
    }

    fn apply_event(&mut self, sequence: Count, event: &Event) -> bool {
        match event {
            Event::WorkflowStarted { .. } => false,
            Event::LoopPassStarted { .. } | Event::LoopPassFinished { .. } => false,
            Event::NodeStarted {
                node, elapsed_ns, ..
            } => {
                let position = self.node_positions[&node.id];
                let final_sequence = self
                    .final_boundary
                    .as_ref()
                    .map(|boundary| boundary.sequence);
                let observed = &mut self.nodes[position];
                if observed.started_sequence.is_some() {
                    self.conflict("node started more than once", event.node());
                    return true;
                }
                observed.started_sequence = Some(sequence);
                observed.started_elapsed_ns = Some(*elapsed_ns);
                match observed.status {
                    NodeStatus::Pending => observed.status = NodeStatus::Running,
                    NodeStatus::Unknown if observed.terminal_sequence.is_none() => {
                        observed.last_known = Some(NodeStatus::Running);
                    }
                    NodeStatus::Failed
                        if observed.terminal_sequence.is_none()
                            && final_sequence
                                .is_some_and(|final_sequence| sequence < final_sequence) =>
                    {
                        observed.last_known = Some(NodeStatus::Running);
                    }
                    _ if observed
                        .terminal_sequence
                        .is_some_and(|terminal| sequence < terminal) => {}
                    _ => {
                        self.conflict("node started after a terminal outcome", event.node());
                        return true;
                    }
                }
                false
            }
            Event::NodeFinished {
                node,
                elapsed_ns,
                duration_ns,
                outcome,
                produced_ports,
                skipped_ports,
                failure,
                ..
            } => {
                let position = self.node_positions[&node.id];
                let contradicts_final = *outcome == Outcome::Failed
                    && self.final_boundary.as_ref().is_some_and(|boundary| {
                        boundary.failure_node_id.as_deref() != Some(node.id.as_str())
                    });
                let observed = &mut self.nodes[position];
                if observed.terminal_sequence.is_some()
                    || observed.status.is_terminal() && observed.status != NodeStatus::Failed
                    || observed.status == NodeStatus::Failed && failure.is_none()
                    || observed
                        .started_sequence
                        .is_some_and(|started| sequence < started)
                {
                    self.conflict("node received conflicting terminal outcomes", event.node());
                    return true;
                }
                observed.status = match outcome {
                    Outcome::Succeeded => NodeStatus::Succeeded,
                    Outcome::Failed => NodeStatus::Failed,
                };
                observed.last_known = None;
                observed.terminal_sequence = Some(sequence);
                observed.terminal_elapsed_ns = Some(*elapsed_ns);
                observed.duration_ns = *duration_ns;
                let (produced, omitted_produced) = bounded_ports(produced_ports);
                let (skipped, omitted_skipped) = bounded_ports(skipped_ports);
                observed.produced_ports = produced;
                observed.skipped_ports = skipped;
                observed.omitted_port_names = omitted_produced + omitted_skipped;
                observed.failure = failure.as_ref().map(bounded_failure);
                if contradicts_final {
                    self.conflict(
                        "node failure contradicts the workflow final boundary",
                        event.node(),
                    );
                    true
                } else {
                    false
                }
            }
            Event::NodeSkipped {
                node,
                elapsed_ns,
                causes,
                skipped_ports,
                ..
            } => {
                let position = self.node_positions[&node.id];
                let observed = &mut self.nodes[position];
                if observed.status != NodeStatus::Pending && observed.status != NodeStatus::Unknown
                {
                    self.conflict("node skipped after another lifecycle state", event.node());
                    return true;
                }
                observed.status = NodeStatus::Skipped;
                observed.last_known = None;
                observed.terminal_sequence = Some(sequence);
                observed.terminal_elapsed_ns = Some(*elapsed_ns);
                let (visible_causes, omitted_causes) = bounded_causes(causes);
                observed.skip_causes = visible_causes;
                observed.omitted_skip_causes = omitted_causes;
                let (ports, omitted) = bounded_ports(skipped_ports);
                observed.skipped_ports = ports;
                observed.omitted_port_names = omitted;
                false
            }
            Event::WorkflowFinished {
                final_sequence,
                visited_node_count,
                top_level_visited_count,
                outcome,
                failure_node_id,
                failure,
                ..
            } => {
                if self.final_boundary.is_some() {
                    self.conflict("workflow finished more than once", None);
                    return true;
                }
                self.final_boundary = Some(FinalBoundary {
                    sequence: *final_sequence,
                    visited: top_level_visited_count.unwrap_or(*visited_node_count),
                    visited_steps: top_level_visited_count.map(|_| *visited_node_count),
                    outcome: *outcome,
                    failure_node_id: failure_node_id.clone(),
                    failure: failure.clone(),
                });
                self.apply_final_boundary();
                false
            }
        }
    }

    fn apply_final_boundary(&mut self) {
        let Some(boundary) = &self.final_boundary else {
            return;
        };
        let mut contradictions = 0u64;
        for (position, node) in self.nodes.iter_mut().enumerate() {
            if node.status == NodeStatus::Failed
                && boundary.failure_node_id.as_deref() != Some(node.id.as_str())
            {
                node.conflicted = true;
                self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
                contradictions += 1;
            }
            let failure_here = boundary.failure_node_id.as_deref() == Some(&node.id);
            if failure_here
                && boundary
                    .failure
                    .as_ref()
                    .is_some_and(|failure| failure.phase != FailurePhase::OutputSelection)
            {
                if matches!(
                    node.status,
                    NodeStatus::Pending | NodeStatus::Running | NodeStatus::Unknown
                ) {
                    node.last_known = Some(node.status);
                    node.status = NodeStatus::Failed;
                    node.failure = boundary.failure.as_ref().map(bounded_failure);
                } else if node.status != NodeStatus::Failed {
                    node.conflicted = true;
                    self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
                    contradictions += 1;
                }
                continue;
            }
            if position as i64 >= boundary.visited.get() {
                if node.status == NodeStatus::Pending {
                    node.status = NodeStatus::NotRun;
                } else {
                    node.conflicted = true;
                    self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
                    contradictions += 1;
                }
            } else if matches!(node.status, NodeStatus::Pending | NodeStatus::Running) {
                node.last_known = Some(node.status);
                node.status = NodeStatus::Unknown;
            }
        }
        if contradictions != 0 {
            self.note("final visited prefix contradicted node evidence");
        }
    }

    fn conflict(
        &mut self,
        message: &str,
        node: Option<(&mf_telemetry::event::NodeIdentity, Count)>,
    ) {
        self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
        if let Some((identity, _)) = node {
            if identity.path.is_empty() {
                if let Some(&position) = self.node_positions.get(&identity.id) {
                    self.nodes[position].conflicted = true;
                }
            } else if let Some(observed) = self
                .pass_states
                .get_mut(&identity.path)
                .and_then(|pass| pass.nodes.get_mut(&identity.id))
            {
                observed.conflicted = true;
            }
        }
        self.note(message);
    }

    fn note(&mut self, message: &str) {
        self.invalidate_snapshot();
        let (message, dropped) = truncate_utf8(message, MAX_DIAGNOSTIC_ENTRY_BYTES);
        self.diagnostic_bytes_dropped =
            self.diagnostic_bytes_dropped.saturating_add(dropped as u64);
        if self.diagnostics.len() == MAX_DIAGNOSTICS
            && let Some(evicted) = self.diagnostics.pop_front()
        {
            self.diagnostic_bytes_dropped = self
                .diagnostic_bytes_dropped
                .saturating_add(evicted.len() as u64);
        }
        self.diagnostics.push_back(message);
    }
}

fn pending_node(node: &NodeDescription) -> NodeObservation {
    pending_node_parts(&node.id, &node.kind)
}

fn pending_node_parts(id: &str, kind: &str) -> NodeObservation {
    NodeObservation {
        id: id.into(),
        kind: kind.into(),
        status: NodeStatus::Pending,
        last_known: None,
        conflicted: false,
        possibly_missing_events: false,
        started_elapsed_ns: None,
        terminal_elapsed_ns: None,
        duration_ns: None,
        produced_ports: Vec::new(),
        skipped_ports: Vec::new(),
        skip_causes: Vec::new(),
        failure: None,
        omitted_port_names: 0,
        omitted_skip_causes: 0,
        started_sequence: None,
        terminal_sequence: None,
    }
}

fn add_gap(
    first: i64,
    last: i64,
    ranges: &mut Vec<MissingRange>,
    omitted: &mut usize,
    count: &mut u64,
) {
    *count = count.saturating_add((last - first + 1) as u64);
    if ranges.len() == MAX_MISSING_RANGES {
        *omitted += 1;
    } else {
        ranges.push(MissingRange { first, last });
    }
}

fn bounded_ports(ports: &[String]) -> (Vec<String>, usize) {
    let mut omitted = ports.len().saturating_sub(MAX_VISIBLE_PORTS);
    let visible = ports
        .iter()
        .take(MAX_VISIBLE_PORTS)
        .map(|port| {
            let (name, dropped) = truncate_utf8(port, MAX_PORT_NAME_BYTES);
            omitted += usize::from(dropped != 0);
            name
        })
        .collect();
    (visible, omitted)
}

fn bounded_causes(causes: &[SkipCause]) -> (Vec<SkipCause>, usize) {
    let mut visible = Vec::new();
    let mut omitted = causes.len().saturating_sub(MAX_VISIBLE_PORTS);
    for cause in causes.iter().take(MAX_VISIBLE_PORTS) {
        if cause.source_node.len() > MAX_PORT_NAME_BYTES
            || cause.source_output.len() > MAX_PORT_NAME_BYTES
        {
            omitted += 1;
        } else {
            visible.push(cause.clone());
        }
    }
    (visible, omitted)
}

fn bounded_failure(failure: &Failure) -> FailureSummary {
    let (message, dropped_bytes) = truncate_utf8(&failure.message, MAX_FAILURE_MESSAGE_BYTES);
    FailureSummary {
        phase: failure.phase,
        message,
        dropped_bytes,
    }
}

fn truncate_utf8(value: &str, limit: usize) -> (String, usize) {
    if value.len() <= limit {
        return (value.into(), 0);
    }
    let end = value.floor_char_boundary(limit);
    (value[..end].into(), value.len() - end)
}
