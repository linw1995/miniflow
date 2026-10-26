use super::*;
use mf_telemetry::stream::{
    StreamCounts, StreamEvent, StreamFailure, StreamIdentity, StreamPayload, StreamRecord,
};
use snafu::ResultExt;

const MAX_INVOCATIONS: usize = 4096;
const MAX_RECENT_INVOCATIONS: usize = 64;

type NodeKey = (Vec<String>, String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInvocationObservation {
    pub identity: StreamIdentity,
    pub path: Vec<LoopPathEntry>,
    pub node: NodeObservation,
    pub loop_summary: Option<LoopSummary>,
    pub completed_passes: u64,
    pub active_pass: Option<Count>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamNodeMetrics {
    pub id: String,
    pub observed_invocations: u64,
    pub observed_completions: u64,
    pub observed_results: u64,
    pub buffered_items: Option<Count>,
    pub last_flush_reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamSnapshot {
    pub invocations: Vec<StreamInvocationObservation>,
    pub nodes: Vec<StreamNodeMetrics>,
    pub counts: Option<StreamCounts>,
    pub failure: Option<StreamFailure>,
    pub hidden_invocations: u64,
}

struct Invocation {
    identity: StreamIdentity,
    path: Vec<LoopPathEntry>,
    node: NodeObservation,
    last_sequence: Count,
    loop_summary: Option<LoopSummary>,
    closed_prefix: i64,
    closed_passes: BTreeSet<Count>,
}

impl Invocation {
    fn is_finished(&self) -> bool {
        self.node.terminal_sequence.is_some()
    }
    fn has_closed_pass(&self, index: Count) -> bool {
        index.get() < self.closed_prefix || self.closed_passes.contains(&index)
    }
    fn key(&self) -> NodeKey {
        (
            self.path
                .iter()
                .map(|entry| entry.node_id().to_owned())
                .collect(),
            self.node.id.clone(),
        )
    }
    fn view(&self, closed: bool, final_seen: bool, newer: bool) -> StreamInvocationObservation {
        let mut node = self.node.clone();
        if !self.is_finished() && (closed || final_seen || newer) {
            node.last_known = Some(node.status);
            node.status = if closed && !final_seen && !newer && node.status == NodeStatus::Running {
                NodeStatus::Interrupted
            } else {
                NodeStatus::Unknown
            };
        }
        StreamInvocationObservation {
            identity: self.identity.clone(),
            path: self.path.clone(),
            node,
            loop_summary: self.loop_summary.clone(),
            completed_passes: self.closed_prefix as u64 + self.closed_passes.len() as u64,
            active_pass: None,
        }
    }
}

struct RootState {
    node: NodeObservation,
    invocation: Option<u64>,
    buffer_sequence: Count,
    flush_sequence: Count,
    metrics: StreamNodeMetrics,
    loop_summary: Option<LoopSummary>,
}

struct Boundary {
    outcome: Outcome,
    counts: StreamCounts,
    failure: Option<StreamFailure>,
}

pub struct StreamState {
    roots: Vec<RootState>,
    positions: BTreeMap<String, usize>,
    delivery: DeliveryTracker<[u8; 32]>,
    invocations: BTreeMap<u64, Invocation>,
    retired: BTreeMap<NodeKey, u64>,
    recent: VecDeque<StreamInvocationObservation>,
    final_boundary: Option<Boundary>,
    failed_invocation: bool,
    protocol_conflicts: u64,
    errors: u64,
    retention_overflows: u64,
    hidden_invocations: u64,
    diagnostics: VecDeque<String>,
    dropped_diagnostics: u64,
    passes: BTreeMap<(u64, Vec<LoopPathEntry>), StreamPass>,
    total_passes: usize,
    hidden_passes: usize,
}

impl StreamState {
    pub fn new(nodes: Vec<NodeObservation>, positions: BTreeMap<String, usize>) -> Self {
        Self {
            roots: nodes
                .into_iter()
                .map(|node| RootState {
                    invocation: None,
                    buffer_sequence: Count::ZERO,
                    flush_sequence: Count::ZERO,
                    metrics: StreamNodeMetrics {
                        id: node.id.clone(),
                        ..Default::default()
                    },
                    loop_summary: None,
                    node,
                })
                .collect(),
            positions,
            delivery: DeliveryTracker::new(MAX_WITNESSES),
            invocations: BTreeMap::new(),
            retired: BTreeMap::new(),
            recent: VecDeque::new(),
            final_boundary: None,
            failed_invocation: false,
            protocol_conflicts: 0,
            errors: 0,
            retention_overflows: 0,
            hidden_invocations: 0,
            diagnostics: VecDeque::new(),
            dropped_diagnostics: 0,
            passes: BTreeMap::new(),
            total_passes: 0,
            hidden_passes: 0,
        }
    }

    fn note(&mut self, message: &str) {
        let (message, dropped) = truncate_utf8(message, MAX_DIAGNOSTIC_ENTRY_BYTES);
        self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(dropped as u64);
        if self.diagnostics.len() == MAX_DIAGNOSTICS
            && let Some(old) = self.diagnostics.pop_front()
        {
            self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(old.len() as u64);
        }
        self.diagnostics.push_back(message);
    }

    fn conflict(&mut self, message: &str) -> Admission {
        self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
        self.note(message);
        Admission::Conflict
    }

    pub fn apply(
        &mut self,
        description: &WorkflowDescription,
        record: StreamRecord,
        correlation: Option<crate::traces::SpanKey>,
    ) -> Result<Admission, StateError> {
        let wire = record.to_wire(0, None).context(EventSnafu)?;
        let bytes = serde_json::to_vec(&wire).context(SerializeSnafu)?;
        if bytes.len() > MAX_EVENT_BYTES {
            return TooLargeSnafu {
                limit: MAX_EVENT_BYTES,
            }
            .fail();
        }
        let signature: [u8; 32] = Sha256::digest(bytes).into();
        let terminal = matches!(
            &record.payload,
            StreamPayload::Control(StreamEvent::Finished { .. })
        );
        let admission = self
            .delivery
            .receive(record.sequence, signature, terminal)?;
        match admission {
            DeliveryAdmission::Duplicate | DeliveryAdmission::UnverifiedDuplicate => {
                return Ok(Admission::Duplicate);
            }
            DeliveryAdmission::Conflict => {
                self.note("conflicting stream transport record");
                return Ok(Admission::Conflict);
            }
            DeliveryAdmission::New => {}
        }
        validate_graph(description, &record).inspect_err(|_| {
            self.errors = self.errors.saturating_add(1);
            self.note("stream event disagrees with the workflow description");
        })?;
        let admission = self
            .apply_payload(description, &record, correlation)
            .inspect_err(|_| {
                self.errors = self.errors.saturating_add(1);
            })?;
        self.retire();
        Ok(admission)
    }

    fn apply_payload(
        &mut self,
        description: &WorkflowDescription,
        record: &StreamRecord,
        correlation: Option<crate::traces::SpanKey>,
    ) -> Result<Admission, StateError> {
        match &record.payload {
            StreamPayload::Control(StreamEvent::Started { .. }) => Ok(Admission::Applied),
            StreamPayload::Control(StreamEvent::Finished {
                outcome,
                counts,
                failure,
                ..
            }) => {
                let failure = failure.as_ref().map(|failure| StreamFailure {
                    phase: failure.phase.clone(),
                    node: failure.node.clone(),
                    message: truncate_utf8(&failure.message, MAX_FAILURE_MESSAGE_BYTES).0,
                });
                self.final_boundary = Some(Boundary {
                    outcome: match outcome {
                        mf_telemetry::stream::StreamOutcome::Succeeded => Outcome::Succeeded,
                        mf_telemetry::stream::StreamOutcome::Failed => Outcome::Failed,
                    },
                    counts: counts.clone(),
                    failure,
                });
                if *outcome == mf_telemetry::stream::StreamOutcome::Succeeded
                    && self.failed_invocation
                {
                    return Ok(
                        self.conflict("successful stream boundary contradicts a failed invocation")
                    );
                }
                Ok(Admission::Applied)
            }
            StreamPayload::Execution(event) if event.node().is_some() => {
                self.apply_node(description, record, event, correlation)
            }
            StreamPayload::Control(
                StreamEvent::Buffered {
                    node, item_count, ..
                }
                | StreamEvent::Flushed {
                    node, item_count, ..
                },
            ) => {
                let identity = record.identity.as_ref().expect("validated identity");
                let admission = self.ensure_invocation(record, node)?;
                if admission != Admission::Applied {
                    return Ok(admission);
                }
                let root = &mut self.roots[self.positions[&node.id]];
                match &record.payload {
                    StreamPayload::Control(StreamEvent::Buffered { .. })
                        if record.sequence > root.buffer_sequence =>
                    {
                        root.buffer_sequence = record.sequence;
                        root.metrics.buffered_items = Some(*item_count);
                    }
                    StreamPayload::Control(StreamEvent::Flushed { reason, .. })
                        if record.sequence > root.flush_sequence =>
                    {
                        root.flush_sequence = record.sequence;
                        root.metrics.last_flush_reason = Some(reason.clone());
                    }
                    _ => {}
                }
                let invocation = self.invocations.get_mut(&identity.invocation).unwrap();
                invocation.last_sequence = invocation.last_sequence.max(record.sequence);
                Ok(Admission::Applied)
            }
            StreamPayload::Execution(_) => self.apply_pass(description, record),
        }
    }

    fn ensure_invocation(
        &mut self,
        record: &StreamRecord,
        node: &mf_telemetry::event::NodeIdentity,
    ) -> Result<Admission, StateError> {
        let identity = record.identity.as_ref().expect("validated identity");
        if node.path.is_empty() != identity.parent.is_none() {
            return Err(invalid(
                "stream invocation parent disagrees with its Loop scope",
            ));
        }
        if let Some(invocation) = self.invocations.get(&identity.invocation) {
            if invocation.identity != *identity
                || invocation.node.id != node.id
                || invocation.node.kind != node.kind
                || invocation.path != node.path
            {
                return Ok(self.conflict("stream invocation changed its node or message identity"));
            }
            return Ok(Admission::Applied);
        }
        let key = (
            node.path
                .iter()
                .map(|entry| entry.node_id().to_owned())
                .collect(),
            node.id.clone(),
        );
        if self
            .retired
            .get(&key)
            .is_some_and(|previous| identity.invocation <= *previous)
        {
            return Ok(self.conflict("a retired stream invocation received new lifecycle records"));
        }
        if self.invocations.len() == MAX_INVOCATIONS {
            self.retention_overflows = self.retention_overflows.saturating_add(1);
            self.note("stream active invocation limit exhausted");
            return Err(invalid("stream invocation limit exhausted"));
        }
        self.invocations.insert(
            identity.invocation,
            Invocation {
                identity: identity.clone(),
                path: node.path.clone(),
                node: NodeObservation::pending(&node.id, &node.kind),
                last_sequence: record.sequence,
                loop_summary: None,
                closed_prefix: 0,
                closed_passes: BTreeSet::new(),
            },
        );
        if node.path.is_empty() {
            let root = &mut self.roots[self.positions[&node.id]];
            root.metrics.observed_invocations = root.metrics.observed_invocations.saturating_add(1);
        }
        Ok(Admission::Applied)
    }

    fn apply_node(
        &mut self,
        description: &WorkflowDescription,
        record: &StreamRecord,
        event: &Event,
        correlation: Option<crate::traces::SpanKey>,
    ) -> Result<Admission, StateError> {
        let (node, _) = event.node().expect("node event");
        let admission = self.ensure_invocation(record, node)?;
        if admission != Admission::Applied {
            return Ok(admission);
        }
        let identity = record.identity.as_ref().unwrap();
        let invocation = self.invocations.get_mut(&identity.invocation).unwrap();
        let observed = &mut invocation.node;
        let violation = match event {
            Event::NodeStarted { elapsed_ns, .. } => {
                if observed.started_sequence.is_some()
                    || observed
                        .terminal_sequence
                        .is_some_and(|end| record.sequence >= end)
                    || (observed.terminal_sequence.is_some() && observed.duration_ns.is_none())
                {
                    Some("stream invocation started twice or contradicts its outcome")
                } else {
                    observed.started_sequence = Some(record.sequence);
                    observed.started_elapsed_ns = Some(*elapsed_ns);
                    observed.set_selections(event.selected_inputs());
                    if observed.terminal_sequence.is_none() {
                        observed.status = NodeStatus::Running;
                    }
                    None
                }
            }
            Event::NodeFinished {
                elapsed_ns,
                duration_ns,
                outcome,
                produced_ports,
                skipped_ports,
                failure,
                loop_summary,
                ..
            } => {
                let needs_start = *outcome == Outcome::Succeeded || duration_ns.is_some();
                if observed.terminal_sequence.is_some()
                    || observed
                        .started_sequence
                        .is_some_and(|start| record.sequence <= start)
                    || (!needs_start && observed.started_sequence.is_some())
                {
                    Some("stream invocation has conflicting terminal evidence")
                } else {
                    invocation.loop_summary = loop_summary.clone();
                    observed.terminal_sequence = Some(record.sequence);
                    observed.terminal_elapsed_ns = Some(*elapsed_ns);
                    observed.duration_ns = *duration_ns;
                    observed.status = if *outcome == Outcome::Succeeded {
                        NodeStatus::Succeeded
                    } else {
                        NodeStatus::Failed
                    };
                    let (ports, omitted) = bounded_ports(produced_ports);
                    observed.produced_ports = ports;
                    observed.omitted_port_names = omitted;
                    let (ports, omitted) = bounded_ports(skipped_ports);
                    observed.skipped_ports = ports;
                    observed.omitted_port_names += omitted;
                    observed.failure = failure.as_ref().map(bounded_failure);
                    None
                }
            }
            Event::NodeSkipped {
                elapsed_ns,
                causes,
                skipped_ports,
                ..
            } => {
                if observed.started_sequence.is_some() || observed.terminal_sequence.is_some() {
                    Some("stream invocation skipped after another lifecycle")
                } else {
                    observed.status = NodeStatus::Skipped;
                    observed.set_selections(event.selected_inputs());
                    observed.terminal_sequence = Some(record.sequence);
                    observed.terminal_elapsed_ns = Some(*elapsed_ns);
                    (observed.skip_causes, observed.omitted_skip_causes) = bounded_causes(causes);
                    (observed.skipped_ports, observed.omitted_port_names) =
                        bounded_ports(skipped_ports);
                    None
                }
            }
            _ => unreachable!(),
        };
        if let Some(message) = violation {
            return Ok(self.conflict(message));
        }
        if observed.started_sequence.max(observed.terminal_sequence) == Some(record.sequence) {
            observed.correlation = correlation;
        }
        invocation.last_sequence = invocation.last_sequence.max(record.sequence);
        if node.path.is_empty() {
            let root = &mut self.roots[self.positions[&node.id]];
            if root
                .invocation
                .is_none_or(|previous| identity.invocation >= previous)
            {
                root.node = observed.clone();
                root.loop_summary = invocation.loop_summary.clone();
                root.invocation = Some(identity.invocation);
            }
            if matches!(
                event,
                Event::NodeFinished { .. } | Event::NodeSkipped { .. }
            ) {
                root.metrics.observed_completions =
                    root.metrics.observed_completions.saturating_add(1);
                root.metrics.observed_results = root
                    .metrics
                    .observed_results
                    .saturating_add(record.emission_count.map_or(0, |count| count.get() as u64));
            }
        }
        let conflicts = self.protocol_conflicts;
        if matches!(
            event,
            Event::NodeFinished {
                outcome: Outcome::Failed,
                ..
            }
        ) {
            self.failed_invocation = true;
            if self
                .final_boundary
                .as_ref()
                .is_some_and(|boundary| boundary.outcome == Outcome::Succeeded)
            {
                self.conflict("failed invocation contradicts a successful stream boundary");
            }
        }
        self.update_pass_node(description, record, event);
        Ok(if self.protocol_conflicts == conflicts {
            Admission::Applied
        } else {
            Admission::Conflict
        })
    }

    fn retire(&mut self) {
        let mut ready: Vec<_> = self
            .invocations
            .iter()
            .filter(|(_, invocation)| {
                invocation.is_finished()
                    && invocation.last_sequence.get() <= self.delivery.watermark()
            })
            .map(|(id, invocation)| (invocation.last_sequence, *id))
            .collect();
        ready.sort();
        for (_, id) in ready {
            let invocation = self.invocations.remove(&id).unwrap();
            self.retired
                .entry(invocation.key())
                .and_modify(|previous| *previous = (*previous).max(id))
                .or_insert(id);
            self.recent.push_back(invocation.view(false, false, false));
            if self.recent.len() > MAX_RECENT_INVOCATIONS {
                self.recent.pop_front();
                self.hidden_invocations = self.hidden_invocations.saturating_add(1);
            }
        }
    }
}

fn validate_graph(
    description: &WorkflowDescription,
    record: &StreamRecord,
) -> Result<(), StateError> {
    if let Some(identity) = &record.identity
        && let Some(message) = identity.message
        && (message.domain == 0 || message.domain > description.nodes.len())
    {
        return Err(invalid(
            "stream message domain is outside the described graph",
        ));
    }
    match &record.payload {
        StreamPayload::Control(StreamEvent::Started { .. }) => {}
        StreamPayload::Control(StreamEvent::Finished { failure, .. }) => {
            if failure
                .as_ref()
                .and_then(|failure| failure.node.as_ref())
                .is_some_and(|id| !description.nodes.iter().any(|node| node.id == *id))
            {
                return Err(invalid("stream failure names an unknown node"));
            }
        }
        StreamPayload::Control(
            StreamEvent::Buffered { node, .. } | StreamEvent::Flushed { node, .. },
        ) => {
            if !node.path.is_empty()
                || !description
                    .nodes
                    .iter()
                    .any(|known| known.id == node.id && known.kind == node.kind)
            {
                return Err(invalid("batch record names an unknown node or scope"));
            }
            if let StreamPayload::Control(StreamEvent::Flushed { output, .. }) = &record.payload
                && output.domain > description.nodes.len()
            {
                return Err(invalid("batch output domain exceeds the graph"));
            }
        }
        StreamPayload::Execution(event) => {
            if let Some((node, position)) = event.node() {
                description
                    .validate_scope_path(&node.path)
                    .context(EventSnafu)?;
                let path: Vec<_> = node
                    .path
                    .iter()
                    .map(|entry| entry.node_id().to_owned())
                    .collect();
                let (nodes, order, data, control, orders) = if path.is_empty() {
                    (
                        &description.nodes,
                        &description.execution_order,
                        &description.data_edges,
                        &description.control_edges,
                        &description.order_edges,
                    )
                } else {
                    let body = description
                        .loop_body(&path)
                        .ok_or_else(|| invalid("unknown stream Loop body"))?;
                    (
                        &body.nodes,
                        &body.execution_order,
                        &body.data_edges,
                        &body.control_edges,
                        &body.order_edges,
                    )
                };
                if order.get(position.get() as usize) != Some(&node.id)
                    || !nodes
                        .iter()
                        .any(|known| known.id == node.id && known.kind == node.kind)
                {
                    return Err(invalid(
                        "stream node identity or position disagrees with description",
                    ));
                }
                if let Event::NodeFinished {
                    loop_summary: Some(_),
                    ..
                } = event
                    && node.kind != "workflow.loop"
                {
                    return Err(invalid("Loop summary belongs to a Loop node"));
                }
                for selection in event.selected_inputs() {
                    if !data.iter().any(|edge| {
                        edge.exclusive
                            && edge.to_node == node.id
                            && edge.to_input == selection.input
                            && edge.from_node == selection.source_node
                            && edge.from_output == selection.source_output
                    }) {
                        return Err(invalid("selected input is not an exclusive candidate"));
                    }
                }
                if let Event::NodeSkipped { causes, .. } = event {
                    for cause in causes {
                        if !cause.matches_dependency(&node.id, data, control, orders) {
                            return Err(invalid("stream skip cause is not an incoming dependency"));
                        }
                    }
                }
            }
            if let Event::LoopPassStarted { path, .. }
            | Event::LoopPassFinished { path, .. }
            | Event::CallStarted { path, .. }
            | Event::CallFinished { path, .. } = event
            {
                description.validate_scope_path(path).context(EventSnafu)?;
                let path: Vec<_> = path
                    .iter()
                    .map(|entry| entry.node_id().to_owned())
                    .collect();
                if description.loop_body(&path).is_none() {
                    return Err(invalid("unknown stream scope"));
                }
            }
        }
    }
    Ok(())
}

struct StreamPass {
    identity: Option<StreamIdentity>,
    message: Option<mf_telemetry::stream::StreamMessage>,
    trigger: mf_telemetry::stream::StreamTrigger,
    path: Vec<LoopPathEntry>,
    started: Option<Count>,
    finished: Option<Count>,
    outcome: Option<LoopPassOutcome>,
    latest: Count,
    statuses: Vec<Option<NodeStatus>>,
    invocations: Vec<Option<u64>>,
    details: BTreeMap<String, NodeObservation>,
}

impl StreamPass {
    fn is_finished(&self) -> bool {
        self.finished.is_some()
    }
}

impl StreamState {
    fn apply_pass(
        &mut self,
        description: &WorkflowDescription,
        record: &StreamRecord,
    ) -> Result<Admission, StateError> {
        let StreamPayload::Execution(event) = &record.payload else {
            unreachable!()
        };
        let path = match event {
            Event::LoopPassStarted { path, .. }
            | Event::LoopPassFinished { path, .. }
            | Event::CallStarted { path, .. }
            | Event::CallFinished { path, .. } => path,
            _ => return Err(invalid("unexpected stream execution event")),
        };
        let identity = record.identity.as_ref().unwrap();
        let (last, parent) = path.split_last().expect("validated path");
        let node = mf_telemetry::event::NodeIdentity {
            id: last.node_id().to_owned(),
            kind: if last.is_call() {
                "workflow.call"
            } else {
                "workflow.loop"
            }
            .into(),
            path: parent.to_vec(),
        };
        let admission = self.ensure_invocation(record, &node)?;
        if admission != Admission::Applied {
            return Ok(admission);
        }
        let body_path: Vec<_> = path
            .iter()
            .map(|entry| entry.node_id().to_owned())
            .collect();
        let body = description.loop_body(&body_path).expect("validated body");
        let key = (identity.invocation, path.clone());
        if !self.passes.contains_key(&key)
            && self.invocations[&identity.invocation].has_closed_pass(last.index())
        {
            return Ok(self.conflict("completed Loop pass received new lifecycle records"));
        }
        if !self.passes.contains_key(&key) && self.passes.len() >= MAX_RECENT_LOOP_PASSES {
            self.evict_pass()?;
        }
        if !self.passes.contains_key(&key) {
            self.total_passes = self.total_passes.saturating_add(1);
            self.passes.insert(
                key.clone(),
                StreamPass {
                    identity: Some(identity.clone()),
                    message: identity.message,
                    trigger: identity.trigger,
                    path: path.clone(),
                    started: None,
                    finished: None,
                    outcome: None,
                    latest: record.sequence,
                    statuses: vec![None; body.nodes.len()],
                    invocations: vec![None; body.nodes.len()],
                    details: BTreeMap::new(),
                },
            );
        }
        let pass = self.passes.get_mut(&key).unwrap();
        if pass
            .identity
            .as_ref()
            .is_some_and(|previous| previous != identity)
            || pass.message != identity.message
            || pass.trigger != identity.trigger
        {
            return Ok(self.conflict("Loop pass changed its stream identity"));
        }
        pass.identity = Some(identity.clone());
        let violation = match event {
            Event::LoopPassStarted { .. } | Event::CallStarted { .. } => {
                if pass.started.is_some() || pass.finished.is_some_and(|end| record.sequence >= end)
                {
                    true
                } else {
                    pass.started = Some(record.sequence);
                    false
                }
            }
            Event::LoopPassFinished { outcome, .. } | Event::CallFinished { outcome, .. } => {
                if pass.finished.is_some()
                    || pass.started.is_some_and(|start| record.sequence <= start)
                    || pass.latest > record.sequence
                {
                    true
                } else {
                    pass.finished = Some(record.sequence);
                    pass.outcome = Some(*outcome);
                    false
                }
            }
            _ => unreachable!(),
        };
        if violation {
            return Ok(self.conflict("Loop pass has conflicting boundaries"));
        }
        pass.latest = pass.latest.max(record.sequence);
        self.refresh_loop_counts();
        Ok(Admission::Applied)
    }

    fn update_pass_node(
        &mut self,
        description: &WorkflowDescription,
        record: &StreamRecord,
        event: &Event,
    ) {
        let (node, position) = event.node().unwrap();
        if node.path.is_empty() {
            return;
        }
        let identity = record.identity.as_ref().unwrap();
        let Some(owner) = identity.parent else {
            self.conflict("body invocation omitted its containing Loop invocation");
            return;
        };
        let key = (owner, node.path.clone());
        let (last, parent) = node.path.split_last().unwrap();
        let owner_key = (
            parent
                .iter()
                .map(|entry| entry.node_id().to_owned())
                .collect(),
            last.node_id().to_owned(),
        );
        if let Some(invocation) = self.invocations.get(&owner) {
            if invocation.node.id != last.node_id()
                || invocation.path != parent
                || invocation.identity.message != identity.message
                || invocation.identity.trigger != identity.trigger
            {
                self.conflict("body invocation names a different Loop owner");
                return;
            }
            if !self.passes.contains_key(&key) && invocation.has_closed_pass(last.index()) {
                self.conflict("body invocation followed a completed Loop pass");
                return;
            }
        } else if self
            .retired
            .get(&owner_key)
            .is_some_and(|previous| owner <= *previous)
        {
            self.conflict("body invocation followed a retired Loop invocation");
            return;
        }
        if !self.passes.contains_key(&key) {
            if self.passes.len() >= MAX_RECENT_LOOP_PASSES && self.evict_pass().is_err() {
                return;
            }
            let static_path: Vec<_> = node
                .path
                .iter()
                .map(|entry| entry.node_id().to_owned())
                .collect();
            let body = description.loop_body(&static_path).expect("validated body");
            self.total_passes = self.total_passes.saturating_add(1);
            self.passes.insert(
                key.clone(),
                StreamPass {
                    identity: None,
                    message: identity.message,
                    trigger: identity.trigger,
                    path: node.path.clone(),
                    started: None,
                    finished: None,
                    outcome: None,
                    latest: record.sequence,
                    statuses: vec![None; body.nodes.len()],
                    invocations: vec![None; body.nodes.len()],
                    details: BTreeMap::new(),
                },
            );
        }
        let invocation = &self.invocations[&identity.invocation];
        let observed = invocation.node.clone();
        if let Some(pass) = self.passes.get_mut(&key) {
            if pass.message != identity.message || pass.trigger != identity.trigger {
                self.conflict("body invocation disagrees with its pass message");
                return;
            }
            if pass.finished.is_some_and(|end| record.sequence > end) {
                self.conflict("body lifecycle followed its Loop pass boundary");
                return;
            }
            let index = position.get() as usize;
            if pass.invocations[index].is_some_and(|previous| previous != identity.invocation) {
                self.conflict("body invocation contradicts its Loop pass traversal");
                return;
            }
            pass.invocations[index] = Some(identity.invocation);
            pass.statuses[index] = Some(observed.status);
            pass.latest = pass.latest.max(record.sequence);
            pass.details.insert(node.id.clone(), observed);
            if pass.details.len() > MAX_RECENT_INVOCATIONS {
                let oldest = pass
                    .details
                    .iter()
                    .min_by_key(|(_, node)| node.terminal_sequence.or(node.started_sequence))
                    .map(|(id, _)| id.clone())
                    .unwrap();
                pass.details.remove(&oldest);
            }
            self.refresh_loop_counts();
        }
    }

    fn refresh_loop_counts(&mut self) {
        for ((owner, path), pass) in &self.passes {
            if pass.is_finished()
                && let Some(invocation) = self.invocations.get_mut(owner)
            {
                let index = path.last().unwrap().index();
                if index.get() >= invocation.closed_prefix
                    && invocation.closed_passes.len() < MAX_RECENT_LOOP_PASSES
                {
                    invocation.closed_passes.insert(index);
                } else if index.get() >= invocation.closed_prefix
                    && !invocation.closed_passes.contains(&index)
                {
                    self.retention_overflows = self.retention_overflows.saturating_add(1);
                }
                while invocation
                    .closed_passes
                    .first()
                    .is_some_and(|index| index.get() == invocation.closed_prefix)
                {
                    invocation.closed_passes.pop_first();
                    invocation.closed_prefix += 1;
                }
            }
        }
    }

    fn evict_pass(&mut self) -> Result<(), StateError> {
        let oldest = self
            .passes
            .iter()
            .filter(|(_, pass)| {
                pass.is_finished() && pass.latest.get() <= self.delivery.watermark()
            })
            .min_by_key(|(_, pass)| pass.latest)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.passes.remove(&key);
            self.hidden_passes = self.hidden_passes.saturating_add(1);
            Ok(())
        } else {
            self.retention_overflows = self.retention_overflows.saturating_add(1);
            self.note("unresolved Loop pass detail exceeded its retention limit");
            Err(invalid("stream Loop pass limit exhausted"))
        }
    }
}

impl StreamState {
    pub fn record_local_drop(&mut self, count: u64) {
        self.delivery.record_local_drop(count);
    }

    pub fn close(&mut self) {
        self.delivery.close();
    }

    pub fn integrity(&self) -> LifecycleIntegrity {
        self.delivery.snapshot(MAX_MISSING_RANGES)
    }

    pub fn snapshot(&self, session: &SessionState) -> StateSnapshot {
        let lifecycle = self.integrity();
        let mut nodes: Vec<_> = self.roots.iter().map(|root| root.node.clone()).collect();
        for node in &mut nodes {
            node.possibly_missing_events = (lifecycle.known_missing_count != 0
                || lifecycle.local_drops != 0
                || lifecycle.verification_lost
                || lifecycle.protocol_conflicts != 0)
                && matches!(
                    node.status,
                    NodeStatus::Pending
                        | NodeStatus::Running
                        | NodeStatus::Unknown
                        | NodeStatus::Interrupted
                );
            if session.closed || self.final_boundary.is_some() {
                if node.status == NodeStatus::Pending {
                    node.status = NodeStatus::Unknown;
                } else if node.status == NodeStatus::Running {
                    node.last_known = Some(NodeStatus::Running);
                    node.status = if self.final_boundary.is_none() {
                        NodeStatus::Interrupted
                    } else {
                        NodeStatus::Unknown
                    };
                }
            }
        }
        let mut invocations: Vec<_> = self.recent.iter().cloned().collect();
        invocations.extend(self.invocations.values().map(|invocation| {
            let newer = invocation.path.is_empty()
                && self.roots[self.positions[&invocation.node.id]]
                    .invocation
                    .is_some_and(|latest| latest > invocation.identity.invocation);
            let mut view = invocation.view(session.closed, self.final_boundary.is_some(), newer);
            view.node.possibly_missing_events = !invocation.is_finished()
                && (lifecycle.known_missing_count != 0
                    || lifecycle.local_drops != 0
                    || lifecycle.verification_lost
                    || lifecycle.protocol_conflicts != 0);
            view.active_pass = self
                .passes
                .iter()
                .filter(|((owner, _), pass)| {
                    *owner == invocation.identity.invocation && !pass.is_finished()
                })
                .max_by_key(|(_, pass)| pass.latest)
                .and_then(|((_, path), _)| path.last().map(|entry| entry.index()));
            view
        }));
        invocations.sort_by_key(|invocation| invocation.identity.invocation);
        let mut diagnostics: VecDeque<_> = session
            .diagnostics
            .iter()
            .chain(self.diagnostics.iter())
            .cloned()
            .collect();
        let mut dropped = session
            .diagnostic_bytes_dropped
            .saturating_add(self.dropped_diagnostics);
        while diagnostics.len() > MAX_DIAGNOSTICS {
            dropped = dropped.saturating_add(diagnostics.pop_front().unwrap().len() as u64);
        }
        let mut passes: Vec<_> = self.passes.iter().collect();
        passes.sort_by_key(|(_, pass)| pass.latest);
        let loop_passes = passes
            .iter()
            .map(|((owner, _), pass)| LoopPassObservation {
                path: pass.path.clone(),
                nodes: pass
                    .details
                    .values()
                    .map(|node| {
                        let invocation = pass
                            .invocations
                            .iter()
                            .flatten()
                            .filter_map(|id| self.invocations.get(id))
                            .find(|invocation| invocation.node.id == node.id);
                        invocation.map_or_else(
                            || node.clone(),
                            |invocation| {
                                invocation
                                    .view(
                                        session.closed,
                                        self.final_boundary.is_some() || pass.finished.is_some(),
                                        false,
                                    )
                                    .node
                            },
                        )
                    })
                    .collect(),
                started: pass.started.is_some(),
                outcome: pass.outcome,
                interrupted: session.closed && self.final_boundary.is_none(),
                stream_invocation: Some(*owner),
                compact_statuses: pass.statuses.clone(),
            })
            .collect();
        let mut overviews: BTreeMap<(Vec<LoopPathEntry>, String), (u64, LoopOverview)> =
            BTreeMap::new();
        for ((owner, path), pass) in &self.passes {
            let (last, parent) = path.split_last().unwrap();
            let key = (parent.to_vec(), last.node_id().to_owned());
            if overviews
                .get(&key)
                .is_none_or(|(current, _)| owner > current)
            {
                overviews.insert(
                    key.clone(),
                    (
                        *owner,
                        LoopOverview {
                            parent_path: parent.to_vec(),
                            loop_id: last.node_id().to_owned(),
                            completed_passes: 0,
                            active_index: None,
                            stop_reason: None,
                        },
                    ),
                );
            }
            let (current, overview) = overviews.get_mut(&key).unwrap();
            if *current == *owner {
                if pass.is_finished() {
                    overview.completed_passes += 1;
                } else {
                    overview.active_index = Some(last.index());
                }
            }
        }
        for root in &self.roots {
            if let Some(summary) = &root.loop_summary {
                let id = root.invocation.unwrap();
                let key = (Vec::new(), root.node.id.clone());
                if overviews
                    .get(&key)
                    .is_none_or(|(current, _)| id >= *current)
                {
                    overviews.insert(
                        key,
                        (
                            id,
                            LoopOverview {
                                parent_path: Vec::new(),
                                loop_id: root.node.id.clone(),
                                completed_passes: summary.pass_count.get() as usize,
                                active_index: None,
                                stop_reason: Some(summary.reason),
                            },
                        ),
                    );
                }
            }
        }
        for invocation in self.invocations.values() {
            if let Some(summary) = &invocation.loop_summary {
                let key = (invocation.path.clone(), invocation.node.id.clone());
                if overviews
                    .get(&key)
                    .is_none_or(|(current, _)| invocation.identity.invocation >= *current)
                {
                    overviews.insert(
                        key,
                        (
                            invocation.identity.invocation,
                            LoopOverview {
                                parent_path: invocation.path.clone(),
                                loop_id: invocation.node.id.clone(),
                                completed_passes: summary.pass_count.get() as usize,
                                active_index: None,
                                stop_reason: Some(summary.reason),
                            },
                        ),
                    );
                }
            }
        }
        StateSnapshot {
            latest_node_event: session.latest_node_event.clone(),
            nodes,
            workflow_outcome: self
                .final_boundary
                .as_ref()
                .map(|boundary| boundary.outcome),
            workflow_failure: None,
            lifecycle,
            observation: ObservationDiagnostics {
                protocol_conflicts: session
                    .protocol_conflicts
                    .saturating_add(self.protocol_conflicts),
                errors: session.observation_errors.saturating_add(self.errors),
                retention_overflows: self.retention_overflows,
            },
            traces: session.traces,
            diagnostic_bytes_dropped: dropped,
            diagnostics: diagnostics.into_iter().collect(),
            loop_passes,
            total_loop_passes: self.total_passes,
            hidden_loop_passes: self.hidden_passes,
            loop_overviews: overviews
                .into_values()
                .map(|(_, overview)| overview)
                .collect(),
            stream: Some(StreamSnapshot {
                invocations,
                nodes: self.roots.iter().map(|root| root.metrics.clone()).collect(),
                counts: self
                    .final_boundary
                    .as_ref()
                    .map(|boundary| boundary.counts.clone()),
                failure: self
                    .final_boundary
                    .as_ref()
                    .and_then(|boundary| boundary.failure.clone()),
                hidden_invocations: self.hidden_invocations,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_telemetry::{
        description::{DataEdge, ExecutionDescription, ExecutionMode, WorkflowDescriptionVersion},
        event::NodeIdentity,
        identity::WorkflowId,
        stream::{StreamMessage, StreamOutcome, StreamTrigger},
    };

    fn count(value: i64) -> Count {
        Count::try_from(value).unwrap()
    }
    fn description() -> WorkflowDescription {
        WorkflowDescription {
            order_edges: Vec::new(),
            version: WorkflowDescriptionVersion::V2026_10_03,
            workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
            execution: Some(ExecutionDescription {
                mode: ExecutionMode::Stream,
            }),
            nodes: vec![
                NodeDescription {
                    id: "source".into(),
                    kind: "test.source".into(),
                },
                NodeDescription {
                    id: "consume".into(),
                    kind: "test.consume".into(),
                },
            ],
            execution_order: vec!["source".into(), "consume".into()],
            control_edges: Vec::new(),
            loop_bodies: Vec::new(),
            clusters: Vec::new(),
            data_edges: vec![DataEdge {
                exclusive: false,
                from_node: "source".into(),
                from_output: "item".into(),
                to_node: "consume".into(),
                to_input: "item".into(),
            }],
        }
    }
    fn record(
        run: RunId,
        sequence: i64,
        identity: Option<StreamIdentity>,
        payload: StreamPayload,
    ) -> StreamRecord {
        StreamRecord {
            workflow_id: description().workflow_id,
            run_id: run,
            sequence: count(sequence),
            identity,
            payload,
            emission_count: None,
        }
    }
    fn start(run: RunId) -> StreamRecord {
        record(
            run,
            1,
            None,
            StreamPayload::Control(StreamEvent::Started {
                elapsed_ns: Count::ZERO,
            }),
        )
    }
    fn node(
        run: RunId,
        sequence: i64,
        invocation: u64,
        source: bool,
        finish: bool,
    ) -> StreamRecord {
        let node = NodeIdentity {
            id: if source { "source" } else { "consume" }.into(),
            kind: if source {
                "test.source"
            } else {
                "test.consume"
            }
            .into(),
            path: Vec::new(),
        };
        let identity = StreamIdentity {
            invocation,
            parent: None,
            trigger: if source {
                StreamTrigger::Startup
            } else {
                StreamTrigger::Message
            },
            message: (!source).then_some(StreamMessage {
                domain: 1,
                sequence: invocation - u64::from(!source),
            }),
        };
        let payload = if finish {
            Event::NodeFinished {
                node,
                position: count(i64::from(!source)),
                elapsed_ns: count(sequence),
                duration_ns: Some(count(1)),
                outcome: Outcome::Succeeded,
                produced_ports: vec!["item".into()],
                skipped_ports: Vec::new(),
                failure: None,
                loop_summary: None,
            }
        } else {
            Event::NodeStarted {
                selected_inputs: Vec::new(),
                node,
                position: count(i64::from(!source)),
                elapsed_ns: count(sequence),
            }
        };
        record(
            run,
            sequence,
            Some(identity),
            StreamPayload::Execution(payload),
        )
    }
    fn finish(run: RunId, sequence: i64, messages: u64) -> StreamRecord {
        record(
            run,
            sequence,
            None,
            StreamPayload::Control(StreamEvent::Finished {
                elapsed_ns: count(sequence),
                outcome: StreamOutcome::Succeeded,
                counts: StreamCounts {
                    startup_frames: 1,
                    emitted_messages: messages,
                    completed_frames: messages + 1,
                    delivered_outputs: messages,
                },
                failure: None,
            }),
        )
    }
    fn events(run: RunId) -> Vec<StreamRecord> {
        vec![
            start(run),
            node(run, 2, 0, true, false),
            node(run, 3, 1, false, false),
            node(run, 4, 1, false, true),
            node(run, 5, 2, false, false),
            node(run, 6, 2, false, true),
            node(run, 7, 0, true, true),
            finish(run, 8, 2),
        ]
    }

    #[test]
    fn repeated_invocations_retain_exact_correlation_despite_late_events() {
        let run = RunId::new();
        for reverse in [false, true] {
            let mut state = SessionState::new(description(), run).unwrap();
            let mut records = events(run);
            if reverse {
                records.reverse();
            }
            for record in records {
                let correlation = record
                    .identity
                    .as_ref()
                    .map(|_| (vec![1; 16], vec![record.sequence.get() as u8; 8]));
                state.apply_stream_observed(record, correlation).unwrap();
            }
            let snapshot = state.snapshot();
            assert_eq!(
                snapshot.nodes[1].correlation,
                Some((vec![1; 16], vec![6; 8]))
            );
            let invocations = &snapshot.stream.as_ref().unwrap().invocations;
            assert_eq!(
                invocations
                    .iter()
                    .find(|i| i.identity.invocation == 1)
                    .unwrap()
                    .node
                    .correlation,
                Some((vec![1; 16], vec![4; 8]))
            );
            assert_eq!(
                state
                    .apply_stream_observed(node(run, 4, 1, false, true), None)
                    .unwrap(),
                Admission::Duplicate
            );
            assert_eq!(
                state.snapshot().nodes[1].correlation,
                snapshot.nodes[1].correlation
            );
        }
    }

    #[test]
    fn latest_stream_node_focus_survives_reordering_and_completion() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        let records = events(run);
        state.apply_stream(records[6].clone()).unwrap();
        let focus = state.snapshot().latest_node_event.unwrap();
        assert_eq!(focus.sequence, count(7));
        for record in records.iter().rev() {
            state.apply_stream(record.clone()).unwrap();
            assert_eq!(state.snapshot().latest_node_event.as_ref(), Some(&focus));
        }
        state.close();
        assert_eq!(state.snapshot().latest_node_event.as_ref(), Some(&focus));
    }

    #[test]
    fn running_sources_and_repeated_consumers_remain_distinct() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        let records = events(run);
        for event in &records[..6] {
            assert_eq!(
                state.apply_stream(event.clone()).unwrap(),
                Admission::Applied
            );
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.nodes[0].status, NodeStatus::Running);
        assert_eq!(snapshot.nodes[1].status, NodeStatus::Succeeded);
        assert_eq!(
            snapshot.stream.as_ref().unwrap().nodes[1].observed_completions,
            2
        );
        for event in &records[6..] {
            state.apply_stream(event.clone()).unwrap();
        }
        assert_eq!(
            state.snapshot().lifecycle.completeness,
            Completeness::Complete
        );
        assert_eq!(
            state.apply_stream(records[3].clone()).unwrap(),
            Admission::Duplicate
        );
        assert_eq!(
            state.snapshot().stream.unwrap().nodes[1].observed_completions,
            2
        );
    }

    #[test]
    fn reverse_delivery_and_delayed_outcomes_repair_without_regression() {
        let run = RunId::new();
        let records = events(run);
        let mut reverse = SessionState::new(description(), run).unwrap();
        for event in records.iter().rev() {
            assert_eq!(
                reverse.apply_stream(event.clone()).unwrap(),
                Admission::Applied
            );
        }
        assert_eq!(
            reverse.snapshot().lifecycle.completeness,
            Completeness::Complete
        );
        let mut missing = SessionState::new(description(), run).unwrap();
        for event in records.iter().filter(|event| event.sequence.get() != 4) {
            missing.apply_stream(event.clone()).unwrap();
        }
        let snapshot = missing.snapshot();
        assert_eq!(snapshot.lifecycle.known_missing_count, 1);
        assert!(
            snapshot
                .stream
                .unwrap()
                .invocations
                .iter()
                .any(|invocation| invocation.identity.invocation == 1
                    && invocation.node.status == NodeStatus::Unknown)
        );
        missing.apply_stream(records[3].clone()).unwrap();
        assert_eq!(
            missing.snapshot().lifecycle.completeness,
            Completeness::Complete
        );
    }

    #[test]
    fn long_runs_bound_detail_and_ignore_old_retransmissions() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        state.apply_stream(start(run)).unwrap();
        let source = node(run, 2, 0, true, false);
        state.apply_stream(source.clone()).unwrap();
        for invocation in 1..=10_000 {
            state
                .apply_stream(node(
                    run,
                    2 * invocation as i64 + 1,
                    invocation,
                    false,
                    false,
                ))
                .unwrap();
            state
                .apply_stream(node(
                    run,
                    2 * invocation as i64 + 2,
                    invocation,
                    false,
                    true,
                ))
                .unwrap();
        }
        let stream = state.stream.as_ref().unwrap();
        assert_eq!(stream.recent.len(), MAX_RECENT_INVOCATIONS);
        assert_eq!(stream.invocations.len(), 1);
        assert_eq!(state.apply_stream(source).unwrap(), Admission::Duplicate);
        state
            .apply_stream(node(run, 20_003, 0, true, true))
            .unwrap();
        state.apply_stream(finish(run, 20_004, 10_000)).unwrap();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
        assert_eq!(
            snapshot.stream.as_ref().unwrap().nodes[1].observed_completions,
            10_000
        );
        assert_eq!(snapshot.lifecycle.unverified_retransmissions, 1);
    }

    #[test]
    fn malformed_identity_conflicts_and_missing_tail_remain_visible() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        state.apply_stream(start(run)).unwrap();
        let original = node(run, 2, 0, true, false);
        state.apply_stream(original.clone()).unwrap();
        let mut conflict = original.clone();
        if let StreamPayload::Execution(Event::NodeStarted { elapsed_ns, .. }) =
            &mut conflict.payload
        {
            *elapsed_ns = count(99);
        }
        assert_eq!(state.apply_stream(conflict).unwrap(), Admission::Conflict);
        let mut invalid = node(run, 3, 1, false, false);
        if let StreamPayload::Execution(Event::NodeStarted { position, .. }) = &mut invalid.payload
        {
            *position = count(0);
        }
        assert!(state.apply_stream(invalid).is_err());
        state.close();
        assert_eq!(
            state.snapshot().lifecycle.completeness,
            Completeness::UnverifiedTail
        );
        assert_eq!(state.snapshot().nodes[0].status, NodeStatus::Interrupted);
    }

    fn loop_records(
        run: RunId,
        pass_count: i64,
        messages: u64,
    ) -> (WorkflowDescription, Vec<StreamRecord>) {
        use mf_telemetry::description::LoopBodyDescription;
        let mut graph = description();
        graph.nodes[1] = NodeDescription {
            id: "repeat".into(),
            kind: "workflow.loop".into(),
        };
        graph.execution_order[1] = "repeat".into();
        graph.data_edges[0].to_node = "repeat".into();
        graph.loop_bodies = vec![LoopBodyDescription {
            order_edges: Vec::new(),
            path: vec!["repeat".into()],
            nodes: vec![NodeDescription {
                id: "%loop".into(),
                kind: "%loop".into(),
            }],
            execution_order: vec!["%loop".into()],
            data_edges: Vec::new(),
            control_edges: Vec::new(),
            clusters: Vec::new(),
        }];
        let mut records = vec![
            record(
                run,
                1,
                None,
                StreamPayload::Control(StreamEvent::Started {
                    elapsed_ns: Count::ZERO,
                }),
            ),
            node(run, 2, 0, true, false),
        ];
        let mut sequence = 3;
        for message in 0..messages {
            let owner = 1 + message * (pass_count as u64 + 1);
            let identity = StreamIdentity {
                invocation: owner,
                parent: None,
                trigger: StreamTrigger::Message,
                message: Some(StreamMessage {
                    domain: 1,
                    sequence: message,
                }),
            };
            let root = NodeIdentity {
                id: "repeat".into(),
                kind: "workflow.loop".into(),
                path: Vec::new(),
            };
            records.push(record(
                run,
                sequence,
                Some(identity.clone()),
                StreamPayload::Execution(Event::NodeStarted {
                    selected_inputs: Vec::new(),
                    node: root.clone(),
                    position: count(1),
                    elapsed_ns: count(sequence),
                }),
            ));
            sequence += 1;
            for index in 0..pass_count {
                let child = owner + 1 + index as u64;
                let path = vec![LoopPathEntry::Loop {
                    loop_id: "repeat".into(),
                    index: count(index),
                }];
                let events = [
                    (
                        identity.clone(),
                        Event::LoopPassStarted {
                            path: path.clone(),
                            elapsed_ns: count(sequence + 1),
                        },
                    ),
                    (
                        StreamIdentity {
                            invocation: child,
                            parent: Some(owner),
                            ..identity.clone()
                        },
                        Event::NodeStarted {
                            selected_inputs: Vec::new(),
                            node: NodeIdentity {
                                id: "%loop".into(),
                                kind: "%loop".into(),
                                path: path.clone(),
                            },
                            position: Count::ZERO,
                            elapsed_ns: count(sequence + 2),
                        },
                    ),
                    (
                        StreamIdentity {
                            invocation: child,
                            parent: Some(owner),
                            ..identity.clone()
                        },
                        Event::NodeFinished {
                            node: NodeIdentity {
                                id: "%loop".into(),
                                kind: "%loop".into(),
                                path: path.clone(),
                            },
                            position: Count::ZERO,
                            elapsed_ns: count(sequence + 3),
                            duration_ns: Some(count(1)),
                            outcome: Outcome::Succeeded,
                            produced_ports: vec!["item".into()],
                            skipped_ports: Vec::new(),
                            failure: None,
                            loop_summary: None,
                        },
                    ),
                    (
                        identity.clone(),
                        Event::LoopPassFinished {
                            path,
                            elapsed_ns: count(sequence + 4),
                            outcome: LoopPassOutcome::Completed,
                        },
                    ),
                ];
                for (identity, event) in events {
                    records.push(record(
                        run,
                        sequence,
                        Some(identity),
                        StreamPayload::Execution(event),
                    ));
                    sequence += 1;
                }
            }
            records.push(record(
                run,
                sequence,
                Some(identity),
                StreamPayload::Execution(Event::NodeFinished {
                    node: root,
                    position: count(1),
                    elapsed_ns: count(sequence),
                    duration_ns: Some(count(sequence - 3)),
                    outcome: Outcome::Succeeded,
                    produced_ports: vec!["item".into()],
                    skipped_ports: Vec::new(),
                    failure: None,
                    loop_summary: Some(LoopSummary {
                        pass_count: count(pass_count),
                        reason: LoopStopReason::Maximum,
                    }),
                }),
            ));
            sequence += 1;
        }
        records.push(node(run, sequence, 0, true, true));
        records.push(finish(run, sequence + 1, messages));
        (graph, records)
    }

    #[test]
    fn loop_passes_keep_their_owner_across_messages_and_reordered_delivery() {
        let run = RunId::new();
        let (graph, records) = loop_records(run, 1, 2);
        for reverse in [false, true] {
            let mut state = SessionState::new(graph.clone(), run).unwrap();
            let mut ordered = records.clone();
            if reverse {
                ordered.reverse();
            }
            for record in ordered {
                assert_eq!(state.apply_stream(record).unwrap(), Admission::Applied);
            }
            let snapshot = state.snapshot();
            assert_eq!(
                snapshot.lifecycle.completeness,
                Completeness::Complete,
                "{snapshot:?}"
            );
            assert_eq!(snapshot.loop_passes.len(), 2);
            assert_ne!(
                snapshot.loop_passes[0].stream_invocation,
                snapshot.loop_passes[1].stream_invocation
            );
            assert_eq!(snapshot.loop_overviews[0].completed_passes, 1);
        }
        let mut state = SessionState::new(graph, run).unwrap();
        for record in records.iter().filter(|record| record.sequence.get() != 6) {
            state.apply_stream(record.clone()).unwrap();
        }
        state.close();
        assert_eq!(
            state.snapshot().lifecycle.completeness,
            Completeness::Incomplete
        );
    }

    #[test]
    fn call_scopes_isolate_messages_and_retain_reordered_duplicate_evidence() {
        let run = RunId::new();
        let (mut graph, mut records) = loop_records(run, 1, 2);
        graph.version = WorkflowDescriptionVersion::V2026_10_23;
        graph.nodes[1].kind = "workflow.call".into();
        graph.loop_bodies[0].nodes[0] = NodeDescription {
            id: "leaf".into(),
            kind: "fixture.task".into(),
        };
        graph.loop_bodies[0].execution_order = vec!["leaf".into()];
        let path = vec![LoopPathEntry::Call {
            call_id: "repeat".into(),
        }];
        for record in &mut records {
            let StreamPayload::Execution(event) = &mut record.payload else {
                continue;
            };
            match event {
                Event::NodeStarted { node, .. } | Event::NodeFinished { node, .. } => {
                    if node.id == "repeat" {
                        node.kind = "workflow.call".into();
                    }
                    if !node.path.is_empty() {
                        node.path = path.clone();
                        node.id = "leaf".into();
                        node.kind = "fixture.task".into();
                    }
                    if let Event::NodeFinished { loop_summary, .. } = event {
                        *loop_summary = None;
                    }
                }
                Event::LoopPassStarted { elapsed_ns, .. } => {
                    *event = Event::CallStarted {
                        path: path.clone(),
                        elapsed_ns: *elapsed_ns,
                    };
                }
                Event::LoopPassFinished {
                    elapsed_ns,
                    outcome,
                    ..
                } => {
                    *event = Event::CallFinished {
                        path: path.clone(),
                        elapsed_ns: *elapsed_ns,
                        outcome: *outcome,
                    };
                }
                _ => {}
            }
        }
        graph.validate().unwrap();
        for reverse in [false, true] {
            let mut state = SessionState::new(graph.clone(), run).unwrap();
            let mut ordered = records.clone();
            if reverse {
                ordered.reverse();
            }
            for record in ordered {
                assert_eq!(
                    state.apply_stream(record.clone()).unwrap(),
                    Admission::Applied
                );
                assert_eq!(state.apply_stream(record).unwrap(), Admission::Duplicate);
            }
            let snapshot = state.snapshot();
            assert_eq!(
                snapshot.lifecycle.completeness,
                Completeness::Complete,
                "{snapshot:?}"
            );
            assert_eq!(snapshot.loop_passes.len(), 2);
            assert_ne!(
                snapshot.loop_passes[0].stream_invocation,
                snapshot.loop_passes[1].stream_invocation
            );
            for pass in &snapshot.loop_passes {
                assert_eq!(pass.path, path);
                assert_eq!(pass.nodes[0].status, NodeStatus::Succeeded);
            }
        }
        let mut forged = records.iter().find(|record| matches!(
            &record.payload,
            StreamPayload::Execution(Event::NodeStarted { node, .. }) if !node.path.is_empty()
        )).unwrap().clone();
        if let StreamPayload::Execution(Event::NodeStarted { node, .. }) = &mut forged.payload {
            node.path = vec![LoopPathEntry::Loop {
                loop_id: "repeat".into(),
                index: Count::ZERO,
            }];
        }
        assert!(
            SessionState::new(graph, run)
                .unwrap()
                .apply_stream(forged)
                .is_err()
        );
    }

    #[test]
    fn long_loop_runs_compact_verified_passes_without_lifecycle_loss() {
        let run = RunId::new();
        let (graph, records) = loop_records(run, 1000, 1);
        let mut state = SessionState::new(graph, run).unwrap();
        for record in &records {
            assert_eq!(
                state.apply_stream(record.clone()).unwrap(),
                Admission::Applied
            );
            let stream = state.stream.as_ref().unwrap();
            assert!(stream.passes.len() <= MAX_RECENT_LOOP_PASSES);
            assert!(
                stream
                    .invocations
                    .values()
                    .all(|invocation| invocation.closed_passes.len() <= MAX_RECENT_LOOP_PASSES)
            );
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
        assert_eq!(snapshot.loop_passes.len(), MAX_RECENT_LOOP_PASSES);
        assert_eq!(snapshot.hidden_loop_passes, 1000 - MAX_RECENT_LOOP_PASSES);
        assert_eq!(snapshot.loop_overviews[0].completed_passes, 1000);
        let mut replay = records[3].clone();
        replay.sequence = count(records.len() as i64 + 1);
        assert_eq!(state.apply_stream(replay).unwrap(), Admission::Conflict);
    }

    #[test]
    fn zero_emission_completion_needs_no_messages() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        state.apply_stream(start(run)).unwrap();
        state.apply_stream(node(run, 2, 0, true, false)).unwrap();
        let mut done = node(run, 3, 0, true, true);
        done.emission_count = Some(Count::ZERO);
        state.apply_stream(done).unwrap();
        state.apply_stream(finish(run, 4, 0)).unwrap();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
        assert_eq!(snapshot.nodes[1].status, NodeStatus::Unknown);
        assert_eq!(snapshot.stream.unwrap().nodes[0].observed_results, 0);
    }

    #[test]
    fn retention_limits_preserve_transport_evidence_and_reject_invalid_parents() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        state.apply_stream(start(run)).unwrap();
        for invocation in 0..MAX_INVOCATIONS {
            state
                .apply_stream(node(
                    run,
                    invocation as i64 + 2,
                    invocation as u64,
                    true,
                    false,
                ))
                .unwrap();
        }
        assert!(
            state
                .apply_stream(node(run, 4098, 4096, true, false))
                .is_err()
        );
        assert_eq!(
            state.stream.as_ref().unwrap().invocations.len(),
            MAX_INVOCATIONS
        );
        state.apply_stream(finish(run, 4099, 0)).unwrap();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
        assert_eq!(snapshot.lifecycle.known_missing_count, 0);
        assert_eq!(snapshot.observation.retention_overflows, 1);
        let (graph, records) = loop_records(run, 1, 1);
        let mut state = SessionState::new(graph, run).unwrap();
        let mut invalid = records[4].clone();
        invalid.identity.as_mut().unwrap().parent = None;
        assert!(state.apply_stream(invalid).is_err());
    }

    #[test]
    fn reordered_batch_fields_and_missing_body_outcomes_keep_independent_evidence() {
        let run = RunId::new();
        let mut state = SessionState::new(description(), run).unwrap();
        let begin = node(run, 3, 1, false, false);
        let identity = begin.identity.clone();
        let body = begin.payload.clone();
        let StreamPayload::Execution(Event::NodeStarted { node: batch, .. }) = body else {
            unreachable!()
        };
        let flushed = record(
            run,
            5,
            identity.clone(),
            StreamPayload::Control(StreamEvent::Flushed {
                node: batch.clone(),
                elapsed_ns: count(5),
                item_count: count(1),
                reason: "size_exceed".into(),
                output: StreamMessage {
                    domain: 2,
                    sequence: 0,
                },
            }),
        );
        let buffered = record(
            run,
            4,
            identity,
            StreamPayload::Control(StreamEvent::Buffered {
                node: batch,
                elapsed_ns: count(4),
                item_count: Count::ZERO,
            }),
        );
        for event in [node(run, 6, 1, false, true), flushed, buffered] {
            state.apply_stream(event).unwrap();
        }
        assert_eq!(
            state.stream.as_ref().unwrap().invocations[&1].last_sequence,
            count(6)
        );
        for event in [
            start(run),
            node(run, 2, 0, true, false),
            begin,
            node(run, 7, 0, true, true),
            finish(run, 8, 1),
        ] {
            state.apply_stream(event).unwrap();
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
        let metrics = &snapshot.stream.unwrap().nodes[1];
        assert_eq!(metrics.buffered_items, Some(Count::ZERO));
        assert_eq!(metrics.last_flush_reason.as_deref(), Some("size_exceed"));
        let (graph, records) = loop_records(run, 1, 1);
        let mut state = SessionState::new(graph, run).unwrap();
        for record in records.iter().filter(|record| record.sequence.get() != 6) {
            state.apply_stream(record.clone()).unwrap();
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.loop_passes[0].nodes[0].status, NodeStatus::Unknown);
    }

    #[test]
    fn reordered_body_events_cannot_change_their_parent_trigger() {
        let run = RunId::new();
        let (graph, records) = loop_records(run, 1, 1);
        let mut state = SessionState::new(graph, run).unwrap();
        let mut body = records[4].clone();
        body.identity.as_mut().unwrap().trigger = StreamTrigger::Input;
        assert_eq!(state.apply_stream(body).unwrap(), Admission::Applied);
        assert_eq!(
            state.apply_stream(records[3].clone()).unwrap(),
            Admission::Conflict
        );
        assert_eq!(state.snapshot().observation.protocol_conflicts, 1);
    }
}
