use super::*;
use crate::stream::{
    StreamCounts, StreamEvent, StreamFailure, StreamIdentity, StreamMessage, StreamOutcome,
    StreamPayload, StreamRecord, StreamTrigger,
};
use std::sync::Mutex;

struct Counters {
    sequence: i64,
    invocation: u64,
    closed: bool,
    exhausted: bool,
}
struct StreamInner {
    backend: Arc<Backend>,
    description: Arc<WorkflowDescription>,
    nodes: Arc<BTreeMap<String, (Count, NodeIdentity)>>,
    run_id: RunId,
    started: Instant,
    context: Context,
    counters: Mutex<Counters>,
}

#[derive(Clone)]
pub struct StreamObservation(Arc<StreamInner>);

impl fmt::Debug for StreamObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamObservation")
            .field("run_id", &self.0.run_id)
            .finish_non_exhaustive()
    }
}

impl Observer {
    pub fn start_stream(
        &self,
        description: WorkflowDescription,
        run_id: RunId,
    ) -> Result<StreamObservation, ContractError> {
        description.validate()?;
        crate::require(
            description.version.is_streaming(),
            "stream observation requires a streaming description",
        )?;
        let parent = Context::current();
        let span = self
            .0
            .tracer
            .span_builder("mf.workflow")
            .with_attributes([
                KeyValue::new("mf.workflow.id", description.workflow_id.to_string()),
                KeyValue::new("mf.run.id", run_id.to_string()),
            ])
            .start_with_context(&self.0.tracer, &parent);
        let count = description.static_node_count()?;
        let nodes = Arc::new(
            description
                .nodes
                .iter()
                .enumerate()
                .map(|(position, node)| {
                    (
                        node.id.clone(),
                        (
                            Count::try_from(position as i64).expect("validated description"),
                            NodeIdentity {
                                id: node.id.clone(),
                                kind: node.kind.clone(),
                                path: Vec::new(),
                            },
                        ),
                    )
                })
                .collect(),
        );
        let observation = StreamObservation(Arc::new(StreamInner {
            backend: Arc::clone(&self.0),
            description: Arc::new(description),
            nodes,
            run_id,
            started: Instant::now(),
            context: parent.with_span(span),
            counters: Mutex::new(Counters {
                sequence: 0,
                invocation: 0,
                closed: false,
                exhausted: false,
            }),
        }));
        observation.record(
            StreamPayload::Control(StreamEvent::Started {
                node_count: count,
                elapsed_ns: Count::ZERO,
            }),
            None,
            None,
            &observation.0.context,
        );
        Ok(observation)
    }
}

impl StreamObservation {
    pub fn enter(&self) -> ContextGuard {
        self.0.context.clone().attach()
    }
    pub fn description(&self) -> &WorkflowDescription {
        &self.0.description
    }
    pub fn elapsed(&self) -> Count {
        nanos(self.0.started.elapsed())
    }

    pub fn frame(&self, message: StreamMessage) -> RunObservation {
        self.frame_inner(Some(message), StreamTrigger::Message)
    }

    fn frame_inner(
        &self,
        message: Option<StreamMessage>,
        trigger: StreamTrigger,
    ) -> RunObservation {
        RunObservation {
            backend: Arc::clone(&self.0.backend),
            workflow_id: self.0.description.workflow_id.clone(),
            run_id: self.0.run_id,
            nodes: Arc::clone(&self.0.nodes),
            sequence: EventSequence::with_maximum(Count::try_from(i64::MAX).unwrap()),
            started: self.0.started,
            context: self.0.context.clone(),
            visited: Count::ZERO,
            visited_steps: Count::ZERO,
            description: Some(Arc::clone(&self.0.description)),
            failure: None,
            closed: false,
            stream: Some(StreamFrame {
                observation: self.clone(),
                message,
                trigger,
                emission_count: None,
            }),
        }
    }

    pub fn callback(
        &self,
        node: &str,
        message: Option<StreamMessage>,
        trigger: StreamTrigger,
    ) -> Option<StreamCallback> {
        if self.0.counters.lock().unwrap().closed {
            return None;
        }
        let mut run = self.frame_inner(message, trigger);
        let _parent = run.enter();
        let step = run.begin_node(node)?;
        Some(StreamCallback {
            run,
            step: Some(step),
        })
    }

    fn invocation(
        &self,
        message: Option<StreamMessage>,
        trigger: StreamTrigger,
        parent: Option<u64>,
    ) -> Option<StreamInvocation> {
        let mut counters = self.0.counters.lock().unwrap();
        if counters.closed || counters.exhausted {
            return None;
        }
        let id = counters.invocation;
        let Some(next) = id.checked_add(1) else {
            counters.exhausted = true;
            return None;
        };
        counters.invocation = next;
        Some(StreamInvocation {
            observation: self.clone(),
            identity: StreamIdentity {
                invocation: id,
                message,
                trigger,
                parent,
            },
        })
    }

    fn record(
        &self,
        payload: StreamPayload,
        identity: Option<StreamIdentity>,
        emission_count: Option<Count>,
        context: &Context,
    ) {
        let mut counters = self.0.counters.lock().unwrap();
        if counters.closed || counters.exhausted {
            return;
        }
        let Some(sequence) = counters.sequence.checked_add(1) else {
            counters.exhausted = true;
            return;
        };
        counters.sequence = sequence;
        self.emit(
            Count::try_from(sequence).unwrap(),
            payload,
            identity,
            emission_count,
            context,
        );
    }

    fn emit(
        &self,
        sequence: Count,
        payload: StreamPayload,
        identity: Option<StreamIdentity>,
        emission_count: Option<Count>,
        context: &Context,
    ) {
        let span = context.span();
        let correlation = span.span_context();
        let trace = correlation.is_valid().then(|| TraceContext {
            trace_id: correlation.trace_id().to_string(),
            span_id: correlation.span_id().to_string(),
            trace_flags: correlation.trace_flags().to_u8(),
        });
        let Ok(time) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return;
        };
        let record = StreamRecord {
            workflow_id: self.0.description.workflow_id.clone(),
            run_id: self.0.run_id,
            sequence,
            identity,
            payload,
            emission_count,
        };
        if let Ok(wire) = record.to_wire(time.as_nanos().min(u64::MAX as u128) as u64, trace) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.0.backend.emit)(&wire)
            }));
        }
    }

    pub fn finish(&self, counts: StreamCounts, failure: Option<StreamFailure>, cancelled: bool) {
        let mut counters = self.0.counters.lock().unwrap();
        if counters.closed {
            return;
        }
        counters.closed = true;
        let outcome = if cancelled {
            StreamOutcome::Cancelled
        } else if failure.is_some() {
            StreamOutcome::Failed
        } else {
            StreamOutcome::Succeeded
        };
        if let Some(failure) = &failure {
            self.0
                .context
                .span()
                .set_status(Status::error(failure.message.clone()));
        }
        self.0.context.span().set_attribute(KeyValue::new(
            "mf.outcome",
            match outcome {
                StreamOutcome::Succeeded => "succeeded",
                StreamOutcome::Failed => "failed",
                StreamOutcome::Cancelled => "cancelled",
            },
        ));
        if !counters.exhausted
            && let Some(sequence) = counters.sequence.checked_add(1)
        {
            counters.sequence = sequence;
            let sequence = Count::try_from(sequence).unwrap();
            self.emit(
                sequence,
                StreamPayload::Control(StreamEvent::Finished {
                    final_sequence: sequence,
                    elapsed_ns: self.elapsed(),
                    outcome,
                    counts,
                    failure,
                }),
                None,
                None,
                &self.0.context,
            );
        }
        self.0.context.span().end();
    }

    pub fn preparation_failed(&self, message: impl Into<String>) {
        self.finish(
            StreamCounts::default(),
            Some(StreamFailure {
                phase: "preparation".into(),
                message: message.into(),
                node: None,
            }),
            false,
        );
    }
}

impl Drop for StreamInner {
    fn drop(&mut self) {
        if !self.counters.get_mut().unwrap().closed {
            self.context
                .span()
                .set_status(Status::error("stream observation abandoned"));
            self.context.span().end();
        }
    }
}

pub(super) struct StreamFrame {
    observation: StreamObservation,
    message: Option<StreamMessage>,
    trigger: StreamTrigger,
    emission_count: Option<Count>,
}

impl StreamFrame {
    pub(super) fn attach(&self, context: Context) -> Context {
        let parent = context
            .get::<StreamInvocation>()
            .filter(|invocation| Arc::ptr_eq(&invocation.observation.0, &self.observation.0))
            .map(|invocation| invocation.identity.invocation);
        match self
            .observation
            .invocation(self.message, self.trigger, parent)
        {
            Some(invocation) => invocation.attach(context),
            None => context,
        }
    }
    pub(super) fn record(&self, event: Event, context: &Context) {
        let identity = context
            .get::<StreamInvocation>()
            .map(|invocation| invocation.identity.clone());
        let count = match event {
            Event::NodeFinished {
                outcome: Outcome::Succeeded,
                ..
            } => self
                .emission_count
                .or_else(|| Some(Count::try_from(1).unwrap())),
            Event::NodeSkipped { .. } => Some(Count::ZERO),
            _ => None,
        };
        self.observation
            .record(StreamPayload::Execution(event), identity, count, context);
    }
}

#[derive(Clone)]
pub(super) struct StreamInvocation {
    observation: StreamObservation,
    identity: StreamIdentity,
}
impl StreamInvocation {
    fn attach(self, context: Context) -> Context {
        self.annotate(&context);
        context.with_value(self)
    }

    pub(super) fn annotate(&self, context: &Context) {
        for (name, value) in self.attributes() {
            match value {
                AnyValue::String(value) => context
                    .span()
                    .set_attribute(KeyValue::new(name, value.to_string())),
                AnyValue::Int(value) => context.span().set_attribute(KeyValue::new(name, value)),
                _ => {}
            }
        }
    }
    pub(super) fn child_context(&self, context: Context) -> Context {
        match self.observation.invocation(
            self.identity.message,
            StreamTrigger::Message,
            Some(self.identity.invocation),
        ) {
            Some(child) => child.attach(context),
            None => context,
        }
    }
    pub(super) fn abandoned(
        &self,
        node: &NodeIdentity,
        position: Count,
        invoked: Option<Instant>,
        context: &Context,
    ) {
        let failure = Failure {
            phase: if invoked.is_some() {
                FailurePhase::Execution
            } else {
                FailurePhase::Dependency
            },
            message: "node execution unwound before completion".into(),
        };
        mark_failed(context, &failure);
        self.observation.record(
            StreamPayload::Execution(Event::NodeFinished {
                node: node.clone(),
                position,
                elapsed_ns: self.observation.elapsed(),
                duration_ns: invoked.map(|time| nanos(time.elapsed())),
                outcome: Outcome::Failed,
                produced_ports: Vec::new(),
                skipped_ports: Vec::new(),
                failure: Some(failure),
                loop_summary: None,
            }),
            Some(self.identity.clone()),
            None,
            context,
        );
    }
    pub(super) fn is_closed(&self) -> bool {
        let counters = self.observation.0.counters.lock().unwrap();
        counters.closed || counters.exhausted
    }
    pub(super) fn attributes(&self) -> Vec<(String, AnyValue)> {
        let mut values = vec![(
            "mf.stream.invocation".into(),
            AnyValue::String(self.identity.invocation.to_string().into()),
        )];
        if let Some(message) = self.identity.message {
            values.push((
                "mf.stream.domain".into(),
                AnyValue::Int(message.domain as i64),
            ));
            values.push((
                "mf.stream.message".into(),
                AnyValue::String(message.sequence.to_string().into()),
            ));
        }
        if let Some(parent) = self.identity.parent {
            values.push((
                "mf.stream.parent_invocation".into(),
                AnyValue::String(parent.to_string().into()),
            ));
        }
        values
    }
}

pub struct StreamCallback {
    run: RunObservation,
    step: Option<NodeObservation>,
}
impl StreamCallback {
    pub fn enter(&self) -> ContextGuard {
        self.step.as_ref().unwrap().enter()
    }
    pub fn started(&mut self) {
        self.run.node_started(self.step.as_mut().unwrap());
    }
    pub fn succeeded(mut self, emissions: usize, ports: Vec<String>) {
        self.run.stream.as_mut().unwrap().emission_count = i64::try_from(emissions)
            .ok()
            .and_then(|value| Count::try_from(value).ok());
        self.run
            .node_succeeded(self.step.take().unwrap(), ports, Vec::new());
    }
    pub fn failed(mut self, phase: FailurePhase, message: String) {
        self.run
            .node_failed(self.step.take().unwrap(), phase, message);
    }
    pub fn skipped(mut self, causes: Vec<SkipCause>) {
        self.run
            .node_skipped(self.step.take().unwrap(), causes, Vec::new());
    }
    pub fn buffered(&self, count: usize) {
        let Ok(count) = i64::try_from(count) else {
            return;
        };
        let Ok(count) = Count::try_from(count) else {
            return;
        };
        self.control(StreamEvent::Buffered {
            node: self.step.as_ref().unwrap().node.clone(),
            item_count: count,
            elapsed_ns: self.run.elapsed(),
        });
    }
    pub fn flushed(&self, output: StreamMessage, count: usize, reason: &str) {
        let Ok(count) = i64::try_from(count) else {
            return;
        };
        let Ok(count) = Count::try_from(count) else {
            return;
        };
        self.control(StreamEvent::Flushed {
            node: self.step.as_ref().unwrap().node.clone(),
            output,
            item_count: count,
            reason: reason.into(),
            elapsed_ns: self.run.elapsed(),
        });
    }
    fn control(&self, event: StreamEvent) {
        let step = self.step.as_ref().unwrap();
        let identity = step
            .context
            .get::<StreamInvocation>()
            .map(|invocation| invocation.identity.clone());
        self.run.stream.as_ref().unwrap().observation.record(
            StreamPayload::Control(event),
            identity,
            None,
            &step.context,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn observation(records: Arc<AtomicUsize>) -> StreamObservation {
        let observer = Observer(Arc::new(Backend {
            tracer: opentelemetry::global::tracer("stream-test"),
            emit: Box::new(move |_| {
                records.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            emit_nested: Box::new(|_| {}),
        }));
        observer
            .start_stream(
                WorkflowDescription {
                    version: crate::description::WorkflowDescriptionVersion::V2026_10_02,
                    workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64)))
                        .unwrap(),
                    nodes: vec![crate::description::NodeDescription {
                        id: "%input".into(),
                        kind: "%input".into(),
                    }],
                    data_edges: Vec::new(),
                    control_edges: Vec::new(),
                    execution_order: vec!["%input".into()],
                    loop_bodies: Vec::new(),
                },
                RunId::new(),
            )
            .unwrap()
    }

    #[test]
    fn exhausted_counters_never_wrap_or_claim_a_complete_terminal_stream() {
        let records = Arc::new(AtomicUsize::new(0));
        let observation = observation(Arc::clone(&records));
        observation.0.counters.lock().unwrap().sequence = i64::MAX;
        let mut callback = observation
            .callback(
                "%input",
                Some(StreamMessage {
                    domain: 0,
                    sequence: 0,
                }),
                StreamTrigger::Input,
            )
            .unwrap();
        callback.started();
        callback.succeeded(0, Vec::new());
        observation.finish(StreamCounts::default(), None, false);
        assert_eq!(records.load(Ordering::SeqCst), 1);
        assert_eq!(observation.0.counters.lock().unwrap().sequence, i64::MAX);

        let records = Arc::new(AtomicUsize::new(0));
        let observation = self::observation(Arc::clone(&records));
        observation.0.counters.lock().unwrap().invocation = u64::MAX;
        assert!(
            observation
                .invocation(None, StreamTrigger::Timer, None)
                .is_none()
        );
        observation.finish(StreamCounts::default(), None, false);
        assert_eq!(records.load(Ordering::SeqCst), 1);
        assert_eq!(observation.0.counters.lock().unwrap().invocation, u64::MAX);
    }
}
