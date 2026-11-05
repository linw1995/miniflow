//! Bounded iteration detail reduction, independent of outer lifecycle delivery.

use super::*;
use mf_telemetry::{
    event::InputSelection,
    iteration::{ItemIdentity, IterationContext},
};
use serde::Serialize;

const MAX_ITEMS: usize = 64;
const MAX_INVOCATIONS: usize = 64;
const MAX_NODES: usize = 4096;
const MAX_DETAIL_WITNESSES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum IterationEvent {
    InvocationFinished,
    ItemStarted,
    ItemFinished {
        outcome: Outcome,
        duration_ns: Option<Count>,
    },
    NodeStarted {
        node: NodeIdentity,
        selected_inputs: Vec<InputSelection>,
    },
    NodeFinished {
        node: NodeIdentity,
        outcome: Outcome,
        duration_ns: Option<Count>,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        failure: Option<Failure>,
    },
    NodeSkipped {
        node: NodeIdentity,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    },
}

impl IterationEvent {
    fn node(&self) -> Option<&NodeIdentity> {
        match self {
            Self::NodeStarted { node, .. }
            | Self::NodeFinished { node, .. }
            | Self::NodeSkipped { node, .. } => Some(node),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IterationRecord {
    pub context: IterationContext,
    pub event: IterationEvent,
    pub correlation: Option<crate::traces::SpanKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IterationNodeObservation {
    pub path: Vec<String>,
    pub node: NodeObservation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IterationItemObservation {
    pub identity: ItemIdentity,
    pub iteration_path: Vec<String>,
    pub parent_path: Vec<LoopPathEntry>,
    pub parent_stream_invocation: Option<u64>,
    pub started: bool,
    pub outcome: Option<Outcome>,
    pub interrupted: bool,
    pub closed: bool,
    pub latest: Count,
    pub nodes: Vec<IterationNodeObservation>,
}

impl IterationItemObservation {
    pub fn display_nodes<'a>(
        &self,
        path: &[String],
        order: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<NodeObservation> {
        let observed: BTreeMap<_, _> = self
            .nodes
            .iter()
            .filter(|node| node.path == path)
            .map(|node| (node.node.id.as_str(), &node.node))
            .collect();
        order
            .into_iter()
            .map(|(id, kind)| {
                let mut node = observed.get(id).map_or_else(
                    || NodeObservation::pending(id, kind),
                    |node| (*node).clone(),
                );
                if node.kind == "builtin.iteration_input"
                    && self.started
                    && node.status == NodeStatus::Pending
                {
                    node.status = NodeStatus::Succeeded;
                    node.produced_ports = vec!["item".into(), "key".into()];
                }
                if self.closed || self.interrupted || self.outcome.is_some() {
                    if node.status == NodeStatus::Running {
                        node.last_known = Some(NodeStatus::Running);
                        node.status = if self.interrupted {
                            NodeStatus::Interrupted
                        } else {
                            NodeStatus::Unknown
                        };
                        node.possibly_missing_events = true;
                    } else if node.status == NodeStatus::Pending {
                        node.status = NodeStatus::Unknown;
                    }
                } else if !self.started && node.status == NodeStatus::Pending {
                    node.status = NodeStatus::Unknown;
                }
                node
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IterationNodeFocus {
    pub revision: u64,
    pub context: IterationContext,
    pub node: NodeIdentity,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IterationSnapshot {
    pub items: Vec<IterationItemObservation>,
    pub latest_node_event: Option<IterationNodeFocus>,
    pub retention_overflows: u64,
    pub protocol_conflicts: u64,
    pub known_missing_count: u64,
}

#[derive(Default)]
pub struct IterationState {
    invocations: BTreeMap<Count, InvocationState>,
    items: BTreeMap<ItemIdentity, ItemState>,
    pub latest_node_event: Option<IterationNodeFocus>,
    retention_overflows: u64,
    protocol_conflicts: u64,
    retired_invocation: Count,
}

struct InvocationState {
    context: IterationContext,
    delivery: DeliveryTracker<[u8; 32]>,
    finished: Option<Count>,
    retired_prefix: i64,
    retired_items: BTreeSet<Count>,
}

struct ItemState {
    view: IterationItemObservation,
    started: Option<Count>,
    finished: Option<Count>,
    nodes: BTreeMap<(Vec<String>, String), NodeObservation>,
}

impl IterationState {
    pub fn apply(
        &mut self,
        description: &WorkflowDescription,
        record: IterationRecord,
    ) -> Result<(Admission, bool), StateError> {
        let context = &record.context;
        context.validate().context(IterationSnafu)?;
        ensure!(
            record
                .correlation
                .as_ref()
                .is_none_or(|(trace, span)| trace.len() == 16 && span.len() == 8),
            InvalidIterationSnafu {
                message: "invalid iteration trace correlation"
            }
        );
        description
            .validate_scope_path(&context.parent_path)
            .context(IterationSnafu)?;
        let (owner, parent) = context
            .iteration_path
            .split_last()
            .expect("validated iteration path");
        let owners = if parent.is_empty() {
            &description.nodes
        } else {
            &description
                .body(parent)
                .context(InvalidIterationSnafu {
                    message: "iteration owner scope is absent",
                })?
                .nodes
        };
        ensure!(
            owners
                .iter()
                .any(|node| node.id == *owner && node.kind == "builtin.iteration"),
            InvalidIterationSnafu {
                message: "iteration owner is absent or has the wrong kind"
            }
        );
        let body = description
            .body(&context.body_path)
            .context(InvalidIterationSnafu {
                message: "iteration body scope is absent",
            })?;
        if let Some(node) = record.event.node() {
            ensure!(
                node.path.is_empty()
                    && body
                        .nodes
                        .iter()
                        .any(|expected| expected.id == node.id && expected.kind == node.kind),
                InvalidIterationSnafu {
                    message: "iteration node disagrees with its described scope"
                }
            );
        } else {
            ensure!(
                context.body_path == context.iteration_path,
                InvalidIterationSnafu {
                    message: "item boundary is outside its iteration body"
                }
            );
        }
        let encoded = serde_json::to_vec(&record).context(SerializeSnafu)?;
        ensure!(
            encoded.len() <= MAX_EVENT_BYTES,
            TooLargeSnafu {
                limit: MAX_EVENT_BYTES
            }
        );
        if !self.invocations.contains_key(&context.invocation) {
            if context.invocation <= self.retired_invocation {
                return Ok((Admission::Duplicate, false));
            }
            if self.invocations.len() >= MAX_INVOCATIONS {
                let removable = self.invocations.iter().find_map(|(identity, invocation)| {
                    invocation.finished.is_some().then_some(*identity)
                });
                let Some(invocation) = removable else {
                    self.retention_overflows = self.retention_overflows.saturating_add(1);
                    return Ok((Admission::Conflict, false));
                };
                self.invocations.remove(&invocation);
                self.retention_overflows = self.retention_overflows.saturating_add(1);
                self.retired_invocation = self.retired_invocation.max(invocation);
                self.items.retain(|key, _| key.invocation != invocation);
            }
            self.invocations.insert(
                context.invocation,
                InvocationState {
                    context: context.clone(),
                    delivery: DeliveryTracker::new(MAX_DETAIL_WITNESSES),
                    finished: None,
                    retired_prefix: -1,
                    retired_items: BTreeSet::new(),
                },
            );
        }
        let invocation = self
            .invocations
            .get_mut(&context.invocation)
            .expect("invocation was admitted");
        if invocation.context.iteration_path != context.iteration_path
            || invocation.context.parent_path != context.parent_path
            || invocation.context.parent_stream_invocation != context.parent_stream_invocation
        {
            self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
            return Ok((Admission::Conflict, false));
        }
        match invocation.delivery.receive(
            context.sequence,
            Sha256::digest(&encoded).into(),
            matches!(record.event, IterationEvent::InvocationFinished),
        )? {
            DeliveryAdmission::Duplicate | DeliveryAdmission::UnverifiedDuplicate => {
                return Ok((Admission::Duplicate, false));
            }
            DeliveryAdmission::Conflict => {
                self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
                return Ok((Admission::Conflict, false));
            }
            DeliveryAdmission::New => {}
        }
        if matches!(record.event, IterationEvent::InvocationFinished) {
            invocation.finished = Some(context.sequence);
            return Ok((Admission::Applied, false));
        }
        if context.index.get() <= invocation.retired_prefix
            || invocation.retired_items.contains(&context.index)
        {
            return Ok((Admission::Duplicate, false));
        }
        let key = context.item();
        let new_node = record.event.node().is_some_and(|node| {
            self.items.get(&key).is_none_or(|item| {
                !item
                    .nodes
                    .contains_key(&(context.body_path.clone(), node.id.clone()))
            })
        });
        while (!self.items.contains_key(&key) && self.items.len() >= MAX_ITEMS)
            || (new_node
                && self
                    .items
                    .values()
                    .map(|item| item.nodes.len())
                    .sum::<usize>()
                    >= MAX_NODES)
        {
            let oldest = self
                .items
                .iter()
                .filter(|(identity, item)| {
                    **identity != key
                        && (item.finished.is_some()
                            || self
                                .invocations
                                .get(&identity.invocation)
                                .is_some_and(|invocation| invocation.finished.is_some()))
                })
                .min_by_key(|(_, item)| item.view.latest)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                self.retention_overflows = self.retention_overflows.saturating_add(1);
                return Ok((Admission::Conflict, false));
            };
            let invocation = self
                .invocations
                .get_mut(&oldest.invocation)
                .expect("retained item has an invocation");
            if invocation.retired_items.len() >= MAX_DETAIL_WITNESSES
                && oldest.index.get() != invocation.retired_prefix.saturating_add(1)
            {
                self.retention_overflows = self.retention_overflows.saturating_add(1);
                return Ok((Admission::Conflict, false));
            }
            invocation.retired_items.insert(oldest.index);
            while let Some(next) = invocation
                .retired_prefix
                .checked_add(1)
                .and_then(|next| Count::try_from(next).ok())
            {
                if !invocation.retired_items.remove(&next) {
                    break;
                }
                invocation.retired_prefix = next.get();
            }
            self.items.remove(&oldest);
            self.retention_overflows = self.retention_overflows.saturating_add(1);
        }
        let item = self.items.entry(key.clone()).or_insert_with(|| ItemState {
            view: IterationItemObservation {
                identity: key,
                iteration_path: context.iteration_path.clone(),
                parent_path: context.parent_path.clone(),
                parent_stream_invocation: context.parent_stream_invocation,
                started: false,
                outcome: None,
                interrupted: false,
                closed: false,
                latest: context.elapsed_ns,
                nodes: Vec::new(),
            },
            started: None,
            finished: None,
            nodes: BTreeMap::new(),
        });
        if item.finished.is_some_and(|end| context.sequence > end)
            || (record.event.node().is_some()
                && item.started.is_some_and(|start| start >= context.sequence))
        {
            self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
            return Ok((Admission::Conflict, false));
        }
        let accepted = match &record.event {
            IterationEvent::ItemStarted
                if item.started.is_none()
                    && item.nodes.values().all(|node| {
                        node.started_sequence
                            .or(node.terminal_sequence)
                            .is_none_or(|child| context.sequence < child)
                    }) =>
            {
                item.started = Some(context.sequence);
                item.view.started = true;
                true
            }
            IterationEvent::ItemFinished { outcome, .. }
                if item.finished.is_none()
                    && item.started.is_none_or(|start| start < context.sequence)
                    && item.nodes.values().all(|node| {
                        node.started_sequence
                            .max(node.terminal_sequence)
                            .is_none_or(|child| child < context.sequence)
                    }) =>
            {
                item.finished = Some(context.sequence);
                item.view.outcome = Some(*outcome);
                true
            }
            IterationEvent::ItemStarted | IterationEvent::ItemFinished { .. } => false,
            IterationEvent::InvocationFinished => {
                unreachable!("invocation boundaries were handled")
            }
            event => {
                let identity = event.node().expect("node event has an identity");
                let node = item
                    .nodes
                    .entry((context.body_path.clone(), identity.id.clone()))
                    .or_insert_with(|| NodeObservation::pending(&identity.id, &identity.kind));
                match event {
                    IterationEvent::NodeStarted {
                        selected_inputs, ..
                    } if node.started_sequence.is_none()
                        && node
                            .terminal_sequence
                            .is_none_or(|end| context.sequence < end) =>
                    {
                        node.started_sequence = Some(context.sequence);
                        node.started_elapsed_ns = Some(context.elapsed_ns);
                        node.set_selections(selected_inputs);
                        if node.terminal_sequence.is_none() {
                            node.status = NodeStatus::Running;
                            node.correlation = record.correlation.clone();
                        }
                        true
                    }
                    IterationEvent::NodeFinished {
                        outcome,
                        duration_ns,
                        produced_ports,
                        skipped_ports,
                        failure,
                        ..
                    } if node.terminal_sequence.is_none()
                        && node
                            .started_sequence
                            .is_none_or(|start| start < context.sequence) =>
                    {
                        node.status = if *outcome == Outcome::Succeeded {
                            NodeStatus::Succeeded
                        } else {
                            NodeStatus::Failed
                        };
                        node.terminal_sequence = Some(context.sequence);
                        node.terminal_elapsed_ns = Some(context.elapsed_ns);
                        node.terminal_received_at = Some(std::time::Instant::now());
                        node.duration_ns = *duration_ns;
                        (node.produced_ports, node.omitted_port_names) =
                            bounded_ports(produced_ports);
                        let (ports, omitted) = bounded_ports(skipped_ports);
                        node.skipped_ports = ports;
                        node.omitted_port_names += omitted;
                        node.failure = failure.as_ref().map(bounded_failure);
                        node.correlation = record.correlation.clone();
                        true
                    }
                    IterationEvent::NodeSkipped {
                        causes,
                        skipped_ports,
                        ..
                    } if node.terminal_sequence.is_none() && node.started_sequence.is_none() => {
                        node.status = NodeStatus::Skipped;
                        node.terminal_sequence = Some(context.sequence);
                        node.terminal_elapsed_ns = Some(context.elapsed_ns);
                        node.terminal_received_at = Some(std::time::Instant::now());
                        (node.skip_causes, node.omitted_skip_causes) = bounded_causes(causes);
                        (node.skipped_ports, node.omitted_port_names) =
                            bounded_ports(skipped_ports);
                        node.correlation = record.correlation.clone();
                        true
                    }
                    _ => {
                        node.conflicted = true;
                        false
                    }
                }
            }
        };
        if !accepted {
            self.protocol_conflicts = self.protocol_conflicts.saturating_add(1);
            return Ok((Admission::Conflict, false));
        }
        item.view.latest = item.view.latest.max(context.elapsed_ns);
        let focus = record.event.node().filter(|_| {
            self.latest_node_event.as_ref().is_none_or(|last| {
                (context.elapsed_ns, context.invocation, context.sequence)
                    > (
                        last.context.elapsed_ns,
                        last.context.invocation,
                        last.context.sequence,
                    )
            })
        });
        if let Some(node) = focus {
            self.latest_node_event = Some(IterationNodeFocus {
                revision: 0,
                context: context.clone(),
                node: node.clone(),
            });
        }
        Ok((Admission::Applied, focus.is_some()))
    }

    pub fn snapshot(&self, closed: bool) -> IterationSnapshot {
        let mut items: Vec<_> = self
            .items
            .values()
            .map(|item| {
                let mut view = item.view.clone();
                let ended = self
                    .invocations
                    .get(&view.identity.invocation)
                    .is_some_and(|invocation| invocation.finished.is_some());
                view.interrupted = closed && !ended && item.finished.is_none();
                view.closed = ended || closed || item.finished.is_some();
                let missing =
                    self.invocations
                        .get(&view.identity.invocation)
                        .is_some_and(|invocation| {
                            invocation.delivery.snapshot(0).known_missing_count != 0
                        });
                view.nodes = item
                    .nodes
                    .iter()
                    .map(|((path, _), node)| {
                        let mut node = node.clone();
                        node.possibly_missing_events |= missing && !node.status.is_terminal();
                        IterationNodeObservation {
                            path: path.clone(),
                            node,
                        }
                    })
                    .collect();
                view
            })
            .collect();
        items.sort_by_key(|item| (item.latest, item.identity.clone()));
        IterationSnapshot {
            items,
            latest_node_event: self.latest_node_event.clone(),
            retention_overflows: self.retention_overflows,
            protocol_conflicts: self.protocol_conflicts,
            known_missing_count: self
                .invocations
                .values()
                .map(|invocation| invocation.delivery.snapshot(0).known_missing_count)
                .fold(0, u64::saturating_add),
        }
    }
}
