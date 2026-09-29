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
    logs::{Logger, LoggerProvider},
    trace::{Status, TraceContextExt, Tracer, TracerProvider},
};
use std::{
    collections::BTreeMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type EmitLog = dyn Fn(&WireRecord) -> Result<(), ContractError> + Send + Sync;

struct Backend {
    tracer: BoxedTracer,
    emit: Box<EmitLog>,
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
            description.version == crate::description::WorkflowDescriptionVersion::V2026_09_29,
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
            description.version == crate::description::WorkflowDescriptionVersion::V2026_09_29
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
            EventSequence::new_loop()
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
            nodes: index,
            sequence,
            started,
            context,
            visited: Count::ZERO,
            visited_steps: Count::ZERO,
            description,
            failure: None,
            closed: false,
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
    nodes: BTreeMap<String, (Count, NodeIdentity)>,
    sequence: EventSequence,
    started: Instant,
    context: Context,
    visited: Count,
    visited_steps: Count,
    description: Option<WorkflowDescription>,
    failure: Option<(Option<String>, Failure)>,
    closed: bool,
}

impl fmt::Debug for RunObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunObservation")
            .field("run_id", &self.run_id)
            .field("visited", &self.visited)
            .field("closed", &self.closed)
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

    fn elapsed(&self) -> Count {
        nanos(self.started.elapsed())
    }

    fn span(&self, id: &str) -> Option<NodeObservation> {
        if self.closed {
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
        NodeObservation {
            node,
            position,
            context: parent.with_span(span),
            invoked: None,
        }
    }

    /// Marks a scheduled step as visited before resolving dependencies.
    pub fn begin_node(&mut self, id: &str) -> Option<NodeObservation> {
        let step = self.span(id)?;
        self.visited =
            Count::try_from(step.position.get() + 1).expect("node position is bounded at start");
        self.visited_steps = Count::try_from(self.visited_steps.get() + 1).ok()?;
        Some(step)
    }

    pub fn begin_invocation(
        &mut self,
        path: Vec<LoopPathEntry>,
        id: &str,
    ) -> Option<NodeObservation> {
        if self.closed {
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
        self.visited_steps = Count::try_from(self.visited_steps.get() + 1).ok()?;
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
        step: NodeObservation,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        loop_summary: Option<LoopSummary>,
    ) {
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
        step: NodeObservation,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    ) {
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

    pub fn node_failed(&mut self, step: NodeObservation, phase: FailurePhase, message: String) {
        let failure = Failure { phase, message };
        mark_failed(&step.context, &failure);
        self.failure = Some((Some(step.node.id.clone()), failure.clone()));
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
            self.failure = Some((
                None,
                Failure {
                    phase: FailurePhase::Preparation,
                    message,
                },
            ));
        }
    }

    pub fn preparation_failed_unattributed(&mut self, message: String) {
        self.failure = Some((
            None,
            Failure {
                phase: FailurePhase::Preparation,
                message,
            },
        ));
    }

    pub fn output_selection_failed(&mut self, message: String) {
        self.failure = Some((
            None,
            Failure {
                phase: FailurePhase::OutputSelection,
                message,
            },
        ));
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
        if self.closed {
            return;
        }
        self.closed = true;
        let failure = error.map(|error| {
            self.failure.take().unwrap_or_else(|| {
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
        if let Ok(sequence) = self.sequence.finish() {
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
                        self.visited_steps
                    } else {
                        self.visited
                    },
                    top_level_visited_count: self.description.as_ref().map(|_| self.visited),
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
        if let Ok(sequence) = self.sequence.reserve() {
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
        if !self.closed {
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
}

impl NodeObservation {
    pub fn enter(&self) -> ContextGuard {
        self.context.clone().attach()
    }
}

impl Drop for NodeObservation {
    fn drop(&mut self) {
        self.context.span().end();
    }
}

fn nanos(duration: Duration) -> Count {
    Count::try_from(duration.as_nanos().min(i64::MAX as u128) as i64)
        .expect("duration is clamped to the signed range")
}

fn mark_failed(context: &Context, failure: &Failure) {
    context
        .span()
        .set_attribute(KeyValue::new("mf.outcome", "failed"));
    let phase = match failure.phase {
        FailurePhase::Preparation => "preparation",
        FailurePhase::Dependency => "dependency",
        FailurePhase::Execution => "execution",
        FailurePhase::Publication => "publication",
        FailurePhase::OutputSelection => "output_selection",
    };
    context
        .span()
        .set_attribute(KeyValue::new("mf.failure.phase", phase));
    context
        .span()
        .set_status(Status::error(failure.message.clone()));
}
