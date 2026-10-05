//! Caller-owned OTel instrumentation for one synchronous workflow invocation.

use crate::{
    ContractError, Count, EVENT_SCHEMA_VERSION, INSTRUMENTATION_SCOPE, LOOP_EVENT_SCHEMA_VERSION,
    description::WorkflowDescription,
    event::{
        Event, EventSequence, Failure, FailurePhase, LifecycleEvent, LoopPassOutcome,
        LoopPathEntry, LoopSummary, NodeIdentity, Outcome, SkipCause,
    },
    identity::{RunId, WorkflowId},
    wire::{TraceContext, WireRecord},
};
use opentelemetry::{
    Context, ContextGuard, KeyValue,
    global::BoxedTracer,
    logs::{AnyValue, LogRecord, Logger, LoggerProvider},
    trace::{Status, TraceContextExt, Tracer, TracerProvider},
};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "stream_observation.rs"]
mod stream_observation;
pub use stream_observation::{StreamCallback, StreamObservation};
use stream_observation::{StreamFrame, StreamInvocation};

type EmitLog = dyn Fn(&WireRecord) -> Result<(), ContractError> + Send + Sync;
type EmitNestedLog = dyn Fn(NestedLog) + Send + Sync;
type FailureState = Option<(Option<String>, Failure)>;

struct NestedLog {
    name: &'static str,
    attributes: Vec<(String, AnyValue)>,
    context: Context,
}

struct Backend {
    tracer: BoxedTracer,
    emit: Box<EmitLog>,
    emit_nested: Box<EmitNestedLog>,
}

/// Holds instruments obtained from providers owned and shut down by the caller.
#[derive(Clone)]
pub struct Observer(Arc<Backend>);

impl Observer {
    pub fn new<T, L>(traces: &T, logs: &L) -> Self
    where
        T: TracerProvider,
        T::Tracer: Send + Sync + 'static,
        <T::Tracer as Tracer>::Span: Send + Sync + 'static,
        L: LoggerProvider,
        L::Logger: Send + Sync + 'static,
    {
        let logger = logs.logger(INSTRUMENTATION_SCOPE);
        let nested_logger = logs.logger("mf.iteration");
        Self(Arc::new(Backend {
            tracer: BoxedTracer::new(Box::new(traces.tracer(INSTRUMENTATION_SCOPE))),
            emit: Box::new(move |wire| {
                let mut record = logger.create_log_record();
                wire.write_to(&mut record)?;
                // Lifecycle delivery is independent of diagnostic filtering and trace sampling.
                if Context::is_current_telemetry_suppressed() {
                    return Ok(());
                }
                // The SDK otherwise injects an invalid current NoopSpan when correlation is absent.
                let _context = wire
                    .trace_context
                    .is_none()
                    .then(|| Context::new().attach());
                logger.emit(record);
                Ok(())
            }),
            emit_nested: Box::new(move |log| {
                if Context::is_current_telemetry_suppressed() {
                    return;
                }
                let mut record = nested_logger.create_log_record();
                record.set_event_name(log.name);
                record.set_timestamp(SystemTime::now());
                record.set_body(AnyValue::String(log.name.into()));
                let mut attributes = log.attributes;
                if let Some(invocation) = log.context.get::<StreamInvocation>() {
                    if invocation.is_closed() {
                        return;
                    }
                    attributes.extend(invocation.attributes());
                }
                record.add_attributes(attributes);
                let span = log.context.span();
                let correlation = span.span_context();
                if correlation.is_valid() {
                    record.set_trace_context(
                        correlation.trace_id(),
                        correlation.span_id(),
                        Some(correlation.trace_flags()),
                    );
                }
                let _context = (!correlation.is_valid()).then(|| Context::new().attach());
                nested_logger.emit(record);
            }),
        }))
    }

    pub fn start(
        &self,
        workflow_id: WorkflowId,
        run_id: RunId,
        nodes: Vec<NodeIdentity>,
    ) -> Result<RunObservation, ContractError> {
        self.start_inner(workflow_id, run_id, nodes, None)
    }

    pub fn start_with_description(
        &self,
        description: WorkflowDescription,
        run_id: RunId,
    ) -> Result<RunObservation, ContractError> {
        description.validate()?;
        crate::require(
            description.version.supports_loops() && !description.is_streaming(),
            "Loop observation requires the new description version",
        )?;
        let nodes = description
            .nodes
            .iter()
            .map(|node| NodeIdentity {
                id: node.id.clone(),
                kind: node.kind.clone(),
                path: Vec::new(),
            })
            .collect();
        self.start_inner(
            description.workflow_id.clone(),
            run_id,
            nodes,
            Some(description),
        )
    }

    fn start_inner(
        &self,
        workflow_id: WorkflowId,
        run_id: RunId,
        nodes: Vec<NodeIdentity>,
        description: Option<WorkflowDescription>,
    ) -> Result<RunObservation, ContractError> {
        let node_count = Count::try_from(
            i64::try_from(nodes.len()).map_err(|_| crate::invalid("too many nodes"))?,
        )?;
        let loop_schema = description.as_ref().is_some_and(|description| {
            description.version.supports_loops() && !description.is_streaming()
        });
        let static_count = if loop_schema {
            description
                .as_ref()
                .expect("Loop description exists")
                .static_node_count()?
        } else {
            node_count
        };
        let sequence = if loop_schema {
            EventSequence::with_maximum(crate::maximum_loop_event_count())
        } else {
            EventSequence::new(node_count)?
        };
        let mut index = BTreeMap::new();
        for (position, node) in nodes.into_iter().enumerate() {
            crate::require(
                !node.id.trim().is_empty() && !node.kind.trim().is_empty(),
                "blank observation node identity",
            )?;
            crate::require(
                index
                    .insert(node.id.clone(), (Count::try_from(position as i64)?, node))
                    .is_none(),
                "duplicate observation node ID",
            )?;
        }
        let started = Instant::now();
        let parent = Context::current();
        let span = self
            .0
            .tracer
            .span_builder("mf.workflow")
            .with_attributes([
                KeyValue::new("mf.workflow.id", workflow_id.to_string()),
                KeyValue::new("mf.run.id", run_id.to_string()),
            ])
            .start_with_context(&self.0.tracer, &parent);
        let context = parent.with_span(span);
        let mut run = RunObservation {
            backend: self.0.clone(),
            workflow_id,
            run_id,
            nodes: Arc::new(index),
            sequence: Arc::new(Mutex::new(sequence)),
            started,
            context,
            visited: Arc::new(AtomicI64::new(0)),
            visited_steps: Arc::new(AtomicI64::new(0)),
            description: description.map(Arc::new),
            failure: Arc::new(Mutex::new(None)),
            node_failures: Arc::new(Mutex::new(BTreeMap::new())),
            closed: Arc::new(AtomicBool::new(false)),
            stream: None,
            owner: true,
        };
        run.record(
            Event::WorkflowStarted {
                node_count: static_count,
                elapsed_ns: Count::ZERO,
            },
            None,
        );
        Ok(run)
    }
}

pub struct RunObservation {
    backend: Arc<Backend>,
    workflow_id: WorkflowId,
    run_id: RunId,
    nodes: Arc<BTreeMap<String, (Count, NodeIdentity)>>,
    sequence: Arc<Mutex<EventSequence>>,
    started: Instant,
    context: Context,
    visited: Arc<AtomicI64>,
    visited_steps: Arc<AtomicI64>,
    description: Option<Arc<WorkflowDescription>>,
    failure: Arc<Mutex<FailureState>>,
    node_failures: Arc<Mutex<BTreeMap<String, Failure>>>,
    closed: Arc<AtomicBool>,
    stream: Option<Arc<StreamFrame>>,
    owner: bool,
}

impl Clone for RunObservation {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            nodes: Arc::clone(&self.nodes),
            sequence: Arc::clone(&self.sequence),
            started: self.started,
            context: self.context.clone(),
            visited: Arc::clone(&self.visited),
            visited_steps: Arc::clone(&self.visited_steps),
            description: self.description.clone(),
            failure: Arc::clone(&self.failure),
            node_failures: Arc::clone(&self.node_failures),
            closed: Arc::clone(&self.closed),
            stream: self.stream.clone(),
            owner: false,
        }
    }
}

impl fmt::Debug for RunObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunObservation")
            .field("run_id", &self.run_id)
            .field("visited", &self.visited.load(Ordering::Acquire))
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl RunObservation {
    pub fn supports_loops(&self) -> bool {
        self.description.is_some()
    }

    pub fn enter(&self) -> ContextGuard {
        self.context.clone().attach()
    }

    pub fn iteration_observation(
        &self,
        id: &str,
        path: &[LoopPathEntry],
        body_nodes: Vec<NodeIdentity>,
    ) -> Option<IterationObservation> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        if path.is_empty() {
            self.nodes.get(id)?;
        } else {
            let static_path: Vec<_> = path.iter().map(|entry| entry.loop_id.clone()).collect();
            self.description
                .as_ref()?
                .loop_body(&static_path)?
                .nodes
                .iter()
                .find(|node| node.id == id)?;
        }
        Some(IterationObservation {
            backend: Arc::clone(&self.backend),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            iteration_id: id.into(),
            parent: Context::current(),
            body_nodes: Arc::new(
                body_nodes
                    .into_iter()
                    .map(|node| (node.id.clone(), node))
                    .collect(),
            ),
        })
    }

    fn elapsed(&self) -> Count {
        nanos(self.started.elapsed())
    }

    fn span(&self, id: &str) -> Option<NodeObservation> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let (position, node) = self.nodes.get(id)?;
        Some(self.span_for(node.clone(), *position))
    }

    fn span_for(&self, node: NodeIdentity, position: Count) -> NodeObservation {
        let parent = Context::current();
        let span = self
            .backend
            .tracer
            .span_builder("mf.node")
            .with_attributes([
                KeyValue::new("mf.workflow.id", self.workflow_id.to_string()),
                KeyValue::new("mf.run.id", self.run_id.to_string()),
                KeyValue::new("mf.node.id", node.id.clone()),
                KeyValue::new("mf.node.kind", node.kind.clone()),
            ])
            .start_with_context(&self.backend.tracer, &parent);
        let mut context = parent.with_span(span);
        if let Some(stream) = &self.stream {
            context = stream.attach(context);
        }
        NodeObservation {
            node,
            position,
            context,
            invoked: None,
            finished: false,
        }
    }

    /// Marks a scheduled step as visited before resolving dependencies.
    pub fn begin_node(&mut self, id: &str) -> Option<NodeObservation> {
        let step = self.span(id)?;
        self.visited
            .fetch_max(step.position.get() + 1, Ordering::AcqRel);
        self.visited_steps
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .ok()?;
        Some(step)
    }

    pub fn begin_invocation(
        &mut self,
        path: Vec<LoopPathEntry>,
        id: &str,
    ) -> Option<NodeObservation> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let description = self.description.as_ref()?;
        let static_path: Vec<_> = path.iter().map(|entry| entry.loop_id.clone()).collect();
        let body = description.loop_body(&static_path)?;
        let position = body.execution_order.iter().position(|node| node == id)?;
        let kind = body.nodes.iter().find(|node| node.id == id)?.kind.clone();
        let position = Count::try_from(position as i64).ok()?;
        let step = self.span_for(
            NodeIdentity {
                id: id.into(),
                kind,
                path,
            },
            position,
        );
        self.visited_steps
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .ok()?;
        Some(step)
    }

    pub fn node_started(&mut self, step: &mut NodeObservation) {
        step.invoked = Some(Instant::now());
        self.record(
            Event::NodeStarted {
                node: step.node.clone(),
                position: step.position,
                elapsed_ns: self.elapsed(),
            },
            Some(&step.context),
        );
    }

    pub fn node_succeeded(
        &mut self,
        step: NodeObservation,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
    ) {
        self.node_succeeded_with_loop_summary(step, produced_ports, skipped_ports, None);
    }

    pub fn node_succeeded_with_loop_summary(
        &mut self,
        mut step: NodeObservation,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        loop_summary: Option<LoopSummary>,
    ) {
        step.finished = true;
        step.context
            .span()
            .set_attribute(KeyValue::new("mf.outcome", "succeeded"));
        let duration_ns = step.invoked.map(|time| nanos(time.elapsed()));
        self.record(
            Event::NodeFinished {
                node: step.node.clone(),
                position: step.position,
                elapsed_ns: self.elapsed(),
                duration_ns,
                outcome: Outcome::Succeeded,
                produced_ports,
                skipped_ports,
                failure: None,
                loop_summary,
            },
            Some(&step.context),
        );
    }

    pub fn node_skipped(
        &mut self,
        mut step: NodeObservation,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    ) {
        step.finished = true;
        step.context
            .span()
            .set_attribute(KeyValue::new("mf.outcome", "skipped"));
        self.record(
            Event::NodeSkipped {
                node: step.node.clone(),
                position: step.position,
                elapsed_ns: self.elapsed(),
                causes,
                skipped_ports,
            },
            Some(&step.context),
        );
    }

    pub fn node_failed(&mut self, mut step: NodeObservation, phase: FailurePhase, message: String) {
        step.finished = true;
        let failure = Failure { phase, message };
        mark_failed(&step.context, &failure);
        self.node_failures
            .lock()
            .unwrap()
            .insert(step.node.id.clone(), failure.clone());
        *self.failure.lock().unwrap() = Some((Some(step.node.id.clone()), failure.clone()));
        let duration_ns = step.invoked.map(|time| nanos(time.elapsed()));
        self.record(
            Event::NodeFinished {
                node: step.node.clone(),
                position: step.position,
                elapsed_ns: self.elapsed(),
                duration_ns,
                outcome: Outcome::Failed,
                produced_ports: Vec::new(),
                skipped_ports: Vec::new(),
                failure: Some(failure),
                loop_summary: None,
            },
            Some(&step.context),
        );
    }

    pub fn preparation_failed(&mut self, node_id: &str, message: String) {
        if let Some(step) = self.span(node_id) {
            self.node_failed(step, FailurePhase::Preparation, message);
        } else {
            *self.failure.lock().unwrap() = Some((
                None,
                Failure {
                    phase: FailurePhase::Preparation,
                    message,
                },
            ));
        }
    }

    pub fn preparation_failed_unattributed(&mut self, message: String) {
        *self.failure.lock().unwrap() = Some((
            None,
            Failure {
                phase: FailurePhase::Preparation,
                message,
            },
        ));
    }

    pub fn output_selection_failed(&mut self, message: String) {
        *self.failure.lock().unwrap() = Some((
            None,
            Failure {
                phase: FailurePhase::OutputSelection,
                message,
            },
        ));
    }

    /// Selects the workflow-level failure after concurrent work has settled.
    pub fn select_failure(
        &mut self,
        node_id: Option<String>,
        phase: FailurePhase,
        message: String,
    ) {
        let failure = node_id
            .as_ref()
            .and_then(|node_id| self.node_failures.lock().unwrap().get(node_id).cloned())
            .unwrap_or(Failure { phase, message });
        *self.failure.lock().unwrap() = Some((node_id, failure));
    }

    pub fn loop_pass_started(&mut self, path: Vec<LoopPathEntry>) {
        let current = Context::current();
        self.record(
            Event::LoopPassStarted {
                path,
                elapsed_ns: self.elapsed(),
            },
            Some(&current),
        );
    }

    pub fn loop_pass_finished(
        &mut self,
        path: Vec<LoopPathEntry>,
        visited_node_count: Count,
        outcome: LoopPassOutcome,
    ) {
        let current = Context::current();
        self.record(
            Event::LoopPassFinished {
                path,
                elapsed_ns: self.elapsed(),
                visited_node_count,
                outcome,
            },
            Some(&current),
        );
    }

    /// Ends a handled run. Encoding or delivery failures never become workflow errors.
    pub fn finish(&mut self, error: Option<&dyn fmt::Display>) {
        if self.stream.is_some() {
            self.closed.store(true, Ordering::Release);
            return;
        }
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let failure = error.map(|error| {
            self.failure.lock().unwrap().take().unwrap_or_else(|| {
                (
                    None,
                    Failure {
                        phase: FailurePhase::Preparation,
                        message: error.to_string(),
                    },
                )
            })
        });
        let outcome = if failure.is_some() {
            Outcome::Failed
        } else {
            Outcome::Succeeded
        };
        if let Some((_, failure)) = &failure {
            mark_failed(&self.context, failure);
        } else {
            self.context
                .span()
                .set_attribute(KeyValue::new("mf.outcome", "succeeded"));
        }
        if let Ok(sequence) = self.sequence.lock().unwrap().finish() {
            let (failure_node_id, failure) = match failure {
                Some((id, failure)) => (id, Some(failure)),
                None => (None, None),
            };
            self.emit(
                sequence,
                Event::WorkflowFinished {
                    final_sequence: sequence,
                    elapsed_ns: self.elapsed(),
                    visited_node_count: if self.description.is_some() {
                        Count::try_from(self.visited_steps.load(Ordering::Acquire))
                            .expect("visited step count is bounded")
                    } else {
                        Count::try_from(self.visited.load(Ordering::Acquire))
                            .expect("visited node position is bounded")
                    },
                    top_level_visited_count: self.description.as_ref().map(|_| {
                        Count::try_from(self.visited.load(Ordering::Acquire))
                            .expect("visited node position is bounded")
                    }),
                    outcome,
                    failure_node_id,
                    failure,
                },
                &self.context,
            );
        }
        self.context.span().end();
    }

    fn record(&mut self, event: Event, context: Option<&Context>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Some(stream) = &self.stream {
            stream.record(event, context.unwrap_or(&self.context));
            return;
        }
        if let Ok(sequence) = self.sequence.lock().unwrap().reserve() {
            self.emit(sequence, event, context.unwrap_or(&self.context));
        }
    }

    fn emit(&self, sequence: Count, event: Event, context: &Context) {
        let span = context.span();
        let correlation = span.span_context();
        let trace_context = correlation.is_valid().then(|| TraceContext {
            trace_id: correlation.trace_id().to_string(),
            span_id: correlation.span_id().to_string(),
            trace_flags: correlation.trace_flags().to_u8(),
        });
        let Ok(timestamp) = SystemTime::now().duration_since(UNIX_EPOCH).map(|value| {
            // All practical timestamps fit the OTel u64 representation.
            value.as_nanos().min(u64::MAX as u128) as u64
        }) else {
            return;
        };
        let event = LifecycleEvent {
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            sequence,
            event,
        };
        let schema_version = if self.description.is_some() {
            LOOP_EVENT_SCHEMA_VERSION
        } else {
            EVENT_SCHEMA_VERSION
        };
        if let Ok(record) =
            WireRecord::from_event_with_version(&event, schema_version, timestamp, trace_context)
        {
            let _ = (self.backend.emit)(&record);
        }
    }
}

impl Drop for RunObservation {
    fn drop(&mut self) {
        if self.stream.is_some() {
            return;
        }
        if self.owner && !self.closed.load(Ordering::Acquire) {
            // An abandoned scope has no trustworthy terminal event, including during unwinding.
            self.context
                .span()
                .set_status(Status::error("workflow observation scope abandoned"));
            self.context.span().end();
        }
    }
}

pub struct NodeObservation {
    node: NodeIdentity,
    position: Count,
    context: Context,
    invoked: Option<Instant>,
    finished: bool,
}

impl NodeObservation {
    pub fn enter(&self) -> ContextGuard {
        self.context.clone().attach()
    }
}

impl Drop for NodeObservation {
    fn drop(&mut self) {
        if !self.finished
            && let Some(invocation) = self.context.get::<StreamInvocation>()
        {
            invocation.abandoned(&self.node, self.position, self.invoked, &self.context);
        }
        self.context.span().end();
    }
}

pub struct IterationObservation {
    backend: Arc<Backend>,
    workflow_id: WorkflowId,
    run_id: RunId,
    iteration_id: String,
    parent: Context,
    body_nodes: Arc<BTreeMap<String, NodeIdentity>>,
}

impl IterationObservation {
    pub fn begin_item(&self, index: usize) -> Option<ItemObservation> {
        let index = i64::try_from(index).ok()?;
        let span = self
            .backend
            .tracer
            .span_builder("mf.iteration.item")
            .with_attributes([
                KeyValue::new("mf.workflow.id", self.workflow_id.to_string()),
                KeyValue::new("mf.run.id", self.run_id.to_string()),
                KeyValue::new("mf.iteration.id", self.iteration_id.clone()),
                KeyValue::new("mf.iteration.index", index),
            ])
            .start_with_context(&self.backend.tracer, &self.parent);
        let item = ItemObservation {
            backend: Arc::clone(&self.backend),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            iteration_id: self.iteration_id.clone(),
            index,
            context: self.parent.with_span(span),
            body_nodes: Arc::clone(&self.body_nodes),
            started: Instant::now(),
            finished: false,
        };
        if let Some(invocation) = item.context.get::<StreamInvocation>() {
            invocation.annotate(&item.context);
        }
        item.emit("mf.iteration.item.started", Vec::new());
        Some(item)
    }
}

pub struct ItemObservation {
    backend: Arc<Backend>,
    workflow_id: WorkflowId,
    run_id: RunId,
    iteration_id: String,
    index: i64,
    context: Context,
    body_nodes: Arc<BTreeMap<String, NodeIdentity>>,
    started: Instant,
    finished: bool,
}

impl ItemObservation {
    pub fn enter(&self) -> ContextGuard {
        self.context.clone().attach()
    }

    pub fn body_observation(&self) -> BodyObservation {
        BodyObservation {
            backend: Arc::clone(&self.backend),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            iteration_id: self.iteration_id.clone(),
            index: self.index,
            context: self.context.clone(),
            body_nodes: Arc::clone(&self.body_nodes),
        }
    }

    pub fn finish(&mut self, error: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut attributes = vec![(
            "mf.duration.ns".into(),
            AnyValue::Int(nanos(self.started.elapsed()).get()),
        )];
        match error {
            Some(message) => {
                self.context
                    .span()
                    .set_attribute(KeyValue::new("mf.outcome", "failed"));
                self.context
                    .span()
                    .set_status(Status::error(message.to_owned()));
                attributes.push(("mf.outcome".into(), AnyValue::String("failed".into())));
                attributes.push((
                    "mf.failure.message".into(),
                    AnyValue::String(message.to_owned().into()),
                ));
            }
            None => {
                self.context
                    .span()
                    .set_attribute(KeyValue::new("mf.outcome", "succeeded"));
                attributes.push(("mf.outcome".into(), AnyValue::String("succeeded".into())));
            }
        }
        self.emit("mf.iteration.item.finished", attributes);
    }

    fn emit(&self, name: &'static str, attributes: Vec<(String, AnyValue)>) {
        let mut common = nested_attributes(
            &self.workflow_id,
            self.run_id,
            &self.iteration_id,
            self.index,
        );
        common.extend(attributes);
        (self.backend.emit_nested)(NestedLog {
            name,
            attributes: common,
            context: self.context.clone(),
        });
    }
}

impl Drop for ItemObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.context
                .span()
                .set_status(Status::error("iteration item observation abandoned"));
        }
        self.context.span().end();
    }
}

#[derive(Clone)]
pub struct BodyObservation {
    backend: Arc<Backend>,
    workflow_id: WorkflowId,
    run_id: RunId,
    iteration_id: String,
    index: i64,
    context: Context,
    body_nodes: Arc<BTreeMap<String, NodeIdentity>>,
}

impl fmt::Debug for BodyObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BodyObservation")
            .field("run_id", &self.run_id)
            .field("iteration_id", &self.iteration_id)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl BodyObservation {
    pub fn begin_node(&self, id: &str) -> Option<BodyNodeObservation> {
        let node = self.body_nodes.get(id)?.clone();
        let span = self
            .backend
            .tracer
            .span_builder("mf.iteration.node")
            .with_attributes([
                KeyValue::new("mf.workflow.id", self.workflow_id.to_string()),
                KeyValue::new("mf.run.id", self.run_id.to_string()),
                KeyValue::new("mf.iteration.id", self.iteration_id.clone()),
                KeyValue::new("mf.iteration.index", self.index),
                KeyValue::new("mf.node.id", node.id.clone()),
                KeyValue::new("mf.node.kind", node.kind.clone()),
            ])
            .start_with_context(&self.backend.tracer, &self.context);
        let mut context = self.context.with_span(span);
        if let Some(parent) = self.context.get::<StreamInvocation>() {
            context = parent.child_context(context);
        }
        Some(BodyNodeObservation {
            backend: Arc::clone(&self.backend),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            iteration_id: self.iteration_id.clone(),
            index: self.index,
            node,
            context,
            invoked: None,
            finished: false,
        })
    }
}

pub struct BodyNodeObservation {
    backend: Arc<Backend>,
    workflow_id: WorkflowId,
    run_id: RunId,
    iteration_id: String,
    index: i64,
    node: NodeIdentity,
    context: Context,
    invoked: Option<Instant>,
    finished: bool,
}

impl BodyNodeObservation {
    pub fn enter(&self) -> ContextGuard {
        self.context.clone().attach()
    }

    pub fn started(&mut self) {
        self.invoked = Some(Instant::now());
        self.emit("mf.iteration.node.started", Vec::new());
    }

    pub fn succeeded(mut self, produced_ports: Vec<String>, skipped_ports: Vec<String>) {
        self.finished = true;
        self.context
            .span()
            .set_attribute(KeyValue::new("mf.outcome", "succeeded"));
        let mut attributes = vec![("mf.outcome".into(), AnyValue::String("succeeded".into()))];
        if let Some(started) = self.invoked {
            attributes.push((
                "mf.duration.ns".into(),
                AnyValue::Int(nanos(started.elapsed()).get()),
            ));
        }
        attributes.push(("mf.produced.ports".into(), port_names(produced_ports)));
        attributes.push(("mf.skipped.ports".into(), port_names(skipped_ports)));
        self.emit("mf.iteration.node.finished", attributes);
    }

    pub fn skipped(mut self, causes: Vec<SkipCause>, skipped_ports: Vec<String>) {
        self.finished = true;
        self.context
            .span()
            .set_attribute(KeyValue::new("mf.outcome", "skipped"));
        self.emit(
            "mf.iteration.node.skipped",
            vec![
                ("mf.outcome".into(), AnyValue::String("skipped".into())),
                ("mf.skipped.ports".into(), port_names(skipped_ports)),
                (
                    "mf.skip.causes".into(),
                    AnyValue::ListAny(Box::new(
                        causes
                            .into_iter()
                            .map(|cause| {
                                AnyValue::Map(Box::new(
                                    [
                                        (
                                            "source_node".into(),
                                            AnyValue::String(cause.source_node.into()),
                                        ),
                                        (
                                            "source_output".into(),
                                            AnyValue::String(cause.source_output.into()),
                                        ),
                                    ]
                                    .into_iter()
                                    .collect(),
                                ))
                            })
                            .collect(),
                    )),
                ),
            ],
        );
    }

    pub fn failed(mut self, phase: FailurePhase, message: String) {
        self.finished = true;
        let failure = Failure {
            phase,
            message: message.clone(),
        };
        mark_failed(&self.context, &failure);
        let mut attributes = vec![
            ("mf.outcome".into(), AnyValue::String("failed".into())),
            (
                "mf.failure.phase".into(),
                AnyValue::String(phase_name(phase).into()),
            ),
            (
                "mf.failure.message".into(),
                AnyValue::String(message.into()),
            ),
        ];
        if let Some(started) = self.invoked {
            attributes.push((
                "mf.duration.ns".into(),
                AnyValue::Int(nanos(started.elapsed()).get()),
            ));
        }
        self.emit("mf.iteration.node.finished", attributes);
    }

    fn emit(&self, name: &'static str, attributes: Vec<(String, AnyValue)>) {
        let mut common = nested_attributes(
            &self.workflow_id,
            self.run_id,
            &self.iteration_id,
            self.index,
        );
        common.push((
            "mf.node.id".into(),
            AnyValue::String(self.node.id.clone().into()),
        ));
        common.push((
            "mf.node.kind".into(),
            AnyValue::String(self.node.kind.clone().into()),
        ));
        common.extend(attributes);
        (self.backend.emit_nested)(NestedLog {
            name,
            attributes: common,
            context: self.context.clone(),
        });
    }
}

impl Drop for BodyNodeObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.context
                .span()
                .set_status(Status::error("iteration body node observation abandoned"));
        }
        self.context.span().end();
    }
}

fn nested_attributes(
    workflow_id: &WorkflowId,
    run_id: RunId,
    iteration_id: &str,
    index: i64,
) -> Vec<(String, AnyValue)> {
    vec![
        ("mf.schema.version".into(), AnyValue::Int(1)),
        (
            "mf.workflow.id".into(),
            AnyValue::String(workflow_id.to_string().into()),
        ),
        (
            "mf.run.id".into(),
            AnyValue::String(run_id.to_string().into()),
        ),
        (
            "mf.iteration.id".into(),
            AnyValue::String(iteration_id.to_owned().into()),
        ),
        ("mf.iteration.index".into(), AnyValue::Int(index)),
    ]
}

fn port_names(names: Vec<String>) -> AnyValue {
    AnyValue::ListAny(Box::new(
        names
            .into_iter()
            .map(|name| AnyValue::String(name.into()))
            .collect(),
    ))
}

fn nanos(duration: Duration) -> Count {
    Count::try_from(duration.as_nanos().min(i64::MAX as u128) as i64)
        .expect("duration is clamped to the signed range")
}

fn mark_failed(context: &Context, failure: &Failure) {
    context
        .span()
        .set_attribute(KeyValue::new("mf.outcome", "failed"));
    let phase = phase_name(failure.phase);
    context
        .span()
        .set_attribute(KeyValue::new("mf.failure.phase", phase));
    context
        .span()
        .set_status(Status::error(failure.message.clone()));
}

fn phase_name(phase: FailurePhase) -> &'static str {
    match phase {
        FailurePhase::Preparation => "preparation",
        FailurePhase::Dependency => "dependency",
        FailurePhase::Execution => "execution",
        FailurePhase::Publication => "publication",
        FailurePhase::OutputSelection => "output_selection",
    }
}
