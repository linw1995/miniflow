//! Invocation-local tool execution projected from the records used by the execution panel.

use mf_runtime::ValueRef;
use mf_telemetry::{
    Count,
    event::{Event, LoopPassOutcome, LoopPathEntry, Outcome, validate_path},
    identity::RunId,
    iteration::{IterationEvent, IterationRecord},
    observation::LocalRecord,
    stream::StreamPayload,
};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::{Arc, Mutex},
};

const MAX_BINDINGS: usize = 256;
const MAX_SCOPES: usize = 4096;
const MAX_NODES: usize = 4096;

type Scope = (RunId, Vec<LoopPathEntry>);
type Status = (Option<ToolExecutionPhase>, Option<u64>);
const UNKNOWN: Status = (None, None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolExecutionPhase {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecution {
    pub name: String,
    /// Missing or conflicting evidence is not a successful execution.
    pub phase: Option<ToolExecutionPhase>,
    pub duration_ns: Option<u64>,
}

#[derive(Debug)]
struct Binding {
    scope: Scope,
    name: String,
    conflicted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Moment {
    sequence: Count,
    elapsed: Count,
}

impl Moment {
    fn since(self, start: Self) -> Option<u64> {
        (self.sequence > start.sequence && self.elapsed >= start.elapsed)
            .then(|| (self.elapsed.get() - start.elapsed.get()) as u64)
    }
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Start(Moment),
    Finish(Moment, Outcome),
    Skip(Moment),
}

#[derive(Debug, Default)]
struct Boundary {
    started: Option<Moment>,
    finished: Option<(Moment, Outcome)>,
    skipped: Option<Moment>,
    conflicted: bool,
}

impl Boundary {
    fn observe(&mut self, step: Step) {
        self.conflicted |= match step {
            Step::Start(start) => *self.started.get_or_insert(start) != start,
            Step::Finish(end, outcome) => {
                *self.finished.get_or_insert((end, outcome)) != (end, outcome)
            }
            Step::Skip(skip) => *self.skipped.get_or_insert(skip) != skip,
        };
    }

    fn conflicted(&self) -> bool {
        self.conflicted
            || (self.skipped.is_some() && (self.started.is_some() || self.finished.is_some()))
            || self
                .started
                .zip(self.finished)
                .is_some_and(|(start, (end, _))| end.since(start).is_none())
    }

    fn status(&self, closed: Option<bool>) -> Status {
        match (self.started, self.finished) {
            (Some(start), Some((end, outcome))) if !self.conflicted() => {
                terminal(end, start, outcome)
            }
            (Some(_), None) if !self.conflicted() => open_status(closed),
            _ => UNKNOWN,
        }
    }
}

fn terminal(end: Moment, start: Moment, outcome: Outcome) -> Status {
    end.since(start).map_or(UNKNOWN, |duration| {
        (
            Some(match outcome {
                Outcome::Succeeded => ToolExecutionPhase::Succeeded,
                Outcome::Failed => ToolExecutionPhase::Failed,
            }),
            Some(duration),
        )
    })
}

fn open_status(closed: Option<bool>) -> Status {
    (
        match closed {
            None => Some(ToolExecutionPhase::Running),
            Some(true) => Some(ToolExecutionPhase::Interrupted),
            Some(false) => None,
        },
        None,
    )
}

#[derive(Debug)]
struct CallEvidence {
    entry: LoopPathEntry,
    invocation: Option<u64>,
    boundary: Boundary,
}

#[derive(Debug)]
struct NodeEvidence {
    invocation: Count,
    boundary: Boundary,
}

#[derive(Debug, Default)]
struct ScopeEvidence {
    call_id: Option<String>,
    conflicted: bool,
    child_call: Option<CallEvidence>,
    nodes: BTreeMap<String, NodeEvidence>,
}

#[derive(Debug, Default)]
struct Executions {
    bindings: BTreeMap<String, Binding>,
    scopes: BTreeMap<Scope, ScopeEvidence>,
    node_count: usize,
    saturated: bool,
    closed: Option<bool>,
}

impl Executions {
    fn scope(&mut self, key: Scope) -> Option<&mut ScopeEvidence> {
        if self.closed.is_some() || self.saturated {
            return None;
        }
        let full = self.scopes.len() == MAX_SCOPES;
        match self.scopes.entry(key) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) if !full => Some(entry.insert(ScopeEvidence::default())),
            Entry::Vacant(_) => {
                self.saturated = true;
                None
            }
        }
    }

    fn status(&self, binding: &Binding) -> Option<Status> {
        let scope = &binding.scope;
        let evidence = &self.scopes[scope];
        if binding.conflicted || evidence.conflicted || self.saturated {
            return Some(UNKNOWN);
        }

        // The snapshot describes the caller's scope, not the tool body. Only its
        // direct runtime child can attest to this invocation; nested work cannot.
        let mut children = self
            .scopes
            .range(scope.clone()..)
            .take_while(|((run, path), _)| *run == scope.0 && path.starts_with(&scope.1))
            .filter_map(|((_, path), child)| {
                let [entry] = &path[scope.1.len()..] else {
                    return None;
                };
                (entry.is_call() && !child.nodes.is_empty()).then_some((entry, child))
            });
        let child = children.next();
        if children.next().is_some() {
            return Some(UNKNOWN);
        }
        if let Some(call) = &evidence.child_call {
            return Some(if child.is_some_and(|(entry, _)| *entry != call.entry) {
                UNKNOWN
            } else {
                call.boundary.status(self.closed)
            });
        }
        let (entry, child) = child?;
        Some(self.iteration_status(entry, evidence, child))
    }

    fn iteration_status(
        &self,
        entry: &LoopPathEntry,
        caller: &ScopeEvidence,
        body: &ScopeEvidence,
    ) -> Status {
        let Some((attempt, first)) = body
            .nodes
            .values()
            .filter_map(|node| Some((node.invocation, node.boundary.started?)))
            .min_by_key(|(_, start)| *start)
        else {
            return UNKNOWN;
        };
        if body
            .nodes
            .values()
            .any(|node| node.invocation != attempt || node.boundary.conflicted())
        {
            return UNKNOWN;
        }
        let owner = if entry.is_flow() {
            None
        } else {
            LoopPathEntry::binding_owner(entry.node_id())
        };
        let invoker = caller
            .nodes
            .get(owner.as_deref().unwrap_or_else(|| entry.node_id()));
        if invoker.is_some_and(|node| node.invocation != attempt || node.boundary.conflicted()) {
            return UNKNOWN;
        }
        let boundary = invoker.map(|node| &node.boundary);
        let start = boundary.and_then(|node| node.started);
        let end = boundary.and_then(|node| node.finished);
        if start.is_some_and(|start| first.since(start).is_none()) {
            return UNKNOWN;
        }
        let last = body
            .nodes
            .values()
            .flat_map(|node| {
                [
                    node.boundary.started,
                    node.boundary.finished.map(|(end, _)| end),
                    node.boundary.skipped,
                ]
            })
            .flatten()
            .max()
            .expect("a body start was observed");
        if end.is_some_and(|(end, _)| end.since(last).is_none()) {
            return UNKNOWN;
        }
        // A packed business error can look successful at the Router boundary.
        // Use failure in the selected body's own nodes, not arbitrary descendants.
        if let Some(failure) = body
            .nodes
            .values()
            .filter_map(|node| match node.boundary.finished {
                Some((end, Outcome::Failed)) => Some(end),
                _ => None,
            })
            .max()
        {
            return terminal(failure, first, Outcome::Failed);
        }
        match (start, end) {
            (Some(_), Some((end, outcome)))
                if body.nodes.values().all(|node| {
                    node.boundary.skipped.is_some()
                        || (node.boundary.started.is_some() && node.boundary.finished.is_some())
                }) =>
            {
                terminal(end, first, outcome)
            }
            (_, Some(_)) => UNKNOWN,
            (_, None) => open_status(self.closed),
        }
    }
}

/// Bounded execution evidence for one chat invocation, independent of dispatch and settlement.
#[derive(Clone, Debug, Default)]
pub struct ToolExecutionMetrics(Arc<Mutex<Executions>>);

impl ToolExecutionMetrics {
    /// Accepts the public IDs from an existing redacted ToolCall input, not its arguments.
    pub fn observe_snapshot(&self, run: RunId, scope: &[LoopPathEntry], inputs: &ValueRef) {
        let Some(call) = inputs.get("call").and_then(ValueRef::as_object) else {
            return;
        };
        if call.len() != 3 || !call.contains_key("arguments") {
            return;
        }
        let (Some(id), Some(name)) = (
            call.get("id").and_then(ValueRef::as_str),
            call.get("name").and_then(ValueRef::as_str),
        ) else {
            return;
        };
        if ![id, name].into_iter().all(valid_label) || validate_path(scope).is_err() {
            return;
        }
        let key = (run, scope.to_vec());
        let mut state = self.0.lock().expect("tool metrics were not poisoned");
        let Some(scope) = state.scope(key.clone()) else {
            return;
        };
        scope.conflicted |= scope.call_id.get_or_insert_with(|| id.to_owned()) != id;
        if state.bindings.len() == MAX_BINDINGS && !state.bindings.contains_key(id) {
            state.saturated = true;
            return;
        }
        let binding = state
            .bindings
            .entry(id.to_owned())
            .or_insert_with(|| Binding {
                scope: key.clone(),
                name: name.to_owned(),
                conflicted: false,
            });
        binding.conflicted |= binding.scope != key || binding.name != name;
    }

    /// Observe a local source record or an admitted receiver record. A receiver
    /// conflict invalidates the matching evidence rather than contributing a phase.
    pub fn observe(&self, record: LocalRecord<'_>, conflict: bool) {
        match record {
            LocalRecord::Finite(record) => self.observe_call(
                record.run_id,
                None,
                record.sequence,
                &record.event,
                conflict,
            ),
            LocalRecord::Stream(record) => {
                if let StreamPayload::Execution(event) = &record.payload {
                    self.observe_call(
                        record.run_id,
                        record.identity.as_ref().map(|id| id.invocation),
                        record.sequence,
                        event,
                        conflict,
                    );
                }
            }
            LocalRecord::Iteration(run, record) => self.observe_iteration(run, record, conflict),
        }
    }

    fn observe_call(
        &self,
        run: RunId,
        invocation: Option<u64>,
        sequence: Count,
        event: &Event,
        conflict: bool,
    ) {
        let moment = |elapsed| Moment { sequence, elapsed };
        let (path, step) = match event {
            Event::CallStarted { path, elapsed_ns } => (path, Step::Start(moment(*elapsed_ns))),
            Event::CallFinished {
                path,
                elapsed_ns,
                outcome,
            } => {
                let outcome = match outcome {
                    LoopPassOutcome::Completed => Outcome::Succeeded,
                    LoopPassOutcome::Failed => Outcome::Failed,
                    LoopPassOutcome::Exit => return,
                };
                (path, Step::Finish(moment(*elapsed_ns), outcome))
            }
            _ => return,
        };
        let Some((entry, parent)) = path.split_last().filter(|(entry, _)| entry.is_call()) else {
            return;
        };
        let mut state = self.0.lock().expect("tool metrics were not poisoned");
        let Some(scope) = state.scope((run, parent.to_vec())) else {
            return;
        };
        let call = scope.child_call.get_or_insert_with(|| CallEvidence {
            entry: entry.clone(),
            invocation,
            boundary: Boundary::default(),
        });
        call.boundary.conflicted |=
            conflict || call.entry != *entry || call.invocation != invocation;
        call.boundary.observe(step);
    }

    fn observe_iteration(&self, run: RunId, record: &IterationRecord, conflict: bool) {
        let context = &record.context;
        let moment = Moment {
            sequence: context.sequence,
            elapsed: context.elapsed_ns,
        };
        let (node, step) = match &record.event {
            IterationEvent::NodeStarted { node, .. } => (node, Step::Start(moment)),
            IterationEvent::NodeFinished { node, outcome, .. } => {
                (node, Step::Finish(moment, *outcome))
            }
            IterationEvent::NodeSkipped { node, .. } => (node, Step::Skip(moment)),
            _ => return,
        };
        let Some(loop_id) = context.iteration_path.last() else {
            return;
        };
        let Some(relative) = context.relative_scope(&node.path) else {
            return;
        };
        let mut path = context.parent_path.clone();
        path.push(LoopPathEntry::Loop {
            loop_id: loop_id.clone(),
            index: context.index,
        });
        path.extend(relative.iter().cloned());
        let mut state = self.0.lock().expect("tool metrics were not poisoned");
        let full = state.node_count == MAX_NODES;
        let Some(scope) = state.scope((run, path)) else {
            return;
        };
        let new = !scope.nodes.contains_key(&node.id);
        if new && full {
            state.saturated = true;
            return;
        }
        let observed = scope
            .nodes
            .entry(node.id.clone())
            .or_insert_with(|| NodeEvidence {
                invocation: context.invocation,
                boundary: Boundary::default(),
            });
        observed.boundary.conflicted |= conflict || observed.invocation != context.invocation;
        observed.boundary.observe(step);
        state.node_count += usize::from(new);
    }

    /// Only calls with execution evidence (or ambiguous identity) appear in the projection.
    pub fn snapshot(&self) -> BTreeMap<String, ToolExecution> {
        let state = self.0.lock().expect("tool metrics were not poisoned");
        state
            .bindings
            .iter()
            .filter_map(|(id, binding)| {
                let (phase, duration_ns) = state.status(binding)?;
                Some((
                    id.clone(),
                    ToolExecution {
                        name: binding.name.clone(),
                        phase,
                        duration_ns,
                    },
                ))
            })
            .collect()
    }

    /// A closed observation never invents a terminal outcome.
    pub fn close(&self, interrupted: bool) {
        self.0
            .lock()
            .expect("tool metrics were not poisoned")
            .closed
            .get_or_insert(interrupted);
    }
}

fn valid_label(value: &str) -> bool {
    value.len() <= 256
        && !value.trim().is_empty()
        && value != "[REDACTED]"
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests;
