use crate::stream_limits::{StreamResources, add, output_bytes};
use crate::{
    EventContext, EventEffects, EventEmission, EventNode, ExecutionContext, FlowOutputs, NodeEvent,
    Outputs, PreparedStream, StreamExecution, StreamPlan, TimerUpdate, ValueRef, encoded_size,
};
use mf_telemetry::{
    event::{FailurePhase, SkipCause},
    observation::{StreamCallback, StreamObservation},
    stream::{StreamCounts, StreamFailure, StreamMessage, StreamTrigger},
};
use serde::Serialize;
use snafu::Snafu;
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Wake, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
pub enum StreamError {
    #[snafu(display("stream preparation failed: {message}"))]
    Preparation { message: String },
    #[snafu(display("invalid stream input: {message}"))]
    Input { message: String },
    #[snafu(display("stream output failed: {message}"))]
    Output { message: String },
    #[snafu(display("stream execution failed: {message}"))]
    Execution { message: String },
    #[snafu(display("stream resource limit: {message}"))]
    Resource { message: String },
    #[snafu(display("stream input capacity is full"))]
    Capacity,
    #[snafu(display("stream input is closed"))]
    Closed,
    #[snafu(display("stream was cancelled"))]
    Cancelled,
}

pub trait StreamClock: Send + Sync {
    fn now(&self) -> Duration;
    /// Custom clocks wake every registered instance after advancing logical time.
    fn register_waker(&self, _waker: Waker) {}
}

pub struct MonotonicClock(Instant);
impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl StreamClock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

pub struct StreamOptions {
    pub clock: Arc<dyn StreamClock>,
    pub observation: Option<StreamObservation>,
    pub snapshots: Option<crate::SnapshotRecorder>,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(MonotonicClock::default()),
            observation: None,
            snapshots: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MessageId {
    pub domain: usize,
    pub sequence: u64,
}

#[derive(Clone, Debug)]
pub struct StreamOutput {
    pub message: MessageId,
    pub outputs: FlowOutputs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamSummary {
    pub accepted_inputs: u64,
    pub emitted_messages: u64,
    pub completed_frames: u64,
    pub delivered_outputs: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamMetrics {
    pub pending_frames: usize,
    pub active_workers: usize,
    pub queued_and_buffered_bytes: usize,
    pub reserved_bytes: usize,
    pub accounted_bytes: usize,
}

struct Frame {
    message: MessageId,
    context: ExecutionContext,
    cursor: usize,
    credits: usize,
}

#[derive(Default)]
struct Domain {
    frame: Option<Frame>,
    occupied: bool,
    running: bool,
    closed: bool,
}

struct QueuedEmission {
    message: MessageId,
    emission: EventEmission,
    bytes: usize,
}

struct Operator {
    state: Box<dyn EventNode>,
    deadline: Option<Duration>,
    pending: VecDeque<QueuedEmission>,
    pending_bytes: usize,
    retained_bytes: usize,
    closed: bool,
}

struct Completion {
    frame: Frame,
    result: Result<(), StreamError>,
}

struct State {
    inputs: VecDeque<(MessageId, ValueRef, usize)>,
    output: Option<StreamOutput>,
    delivered: Option<usize>,
    delivery_pending: bool,
    completions: VecDeque<Completion>,
    domains: Vec<Domain>,
    operators: Vec<Option<Operator>>,
    sequences: Vec<u64>,
    root_live: usize,
    dynamic_bytes: usize,
    event_credits: usize,
    active_workers: usize,
    input_closed: bool,
    failure: Option<StreamError>,
    failure_node: Option<String>,
    done: bool,
    summary: StreamSummary,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    execution: StreamExecution,
    resources: StreamResources,
    input_capacity: usize,
    cancellation: Arc<AtomicBool>,
    observation: Option<StreamObservation>,
}

struct ClockWake(Weak<Shared>);
impl Wake for ClockWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(shared) = self.0.upgrade() {
            let _guard = shared.state.lock().unwrap();
            shared.changed.notify_all();
        }
    }
}

#[derive(Clone)]
pub struct StreamSender(Arc<Shared>);

impl StreamSender {
    pub fn send(&self, value: impl Into<ValueRef>) -> Result<(), StreamError> {
        self.admit(value.into(), true)
    }

    pub fn try_send(&self, value: &ValueRef) -> Result<(), StreamError> {
        self.admit(value.clone(), false)
    }

    fn admit(&self, value: ValueRef, wait: bool) -> Result<(), StreamError> {
        let mut state = self.0.state.lock().unwrap();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if state.input_closed || state.done {
            return Err(StreamError::Closed);
        }
        let validation = self
            .0
            .execution
            .input_type
            .validate_shared(&value)
            .map_err(|error| StreamError::Input {
                message: error.to_string(),
            })
            .and_then(|()| {
                encoded_size(&value, self.0.execution.limits.max_message_bytes)
                    .and_then(|bytes| add(bytes, self.0.resources.source_overhead))
            });
        let bytes = match validation {
            Ok(bytes) => bytes,
            Err(error) => {
                state.failure = Some(error.clone());
                self.0.changed.notify_all();
                return Err(error);
            }
        };
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.input_closed || state.done {
                return Err(StreamError::Closed);
            }
            if state.root_live < self.0.input_capacity
                && state
                    .dynamic_bytes
                    .checked_add(bytes)
                    .is_some_and(|total| total <= self.0.resources.soft_bytes)
            {
                let sequence =
                    take_sequence(&mut state.summary.accepted_inputs).inspect_err(|error| {
                        state.failure = Some(error.clone());
                        self.0.changed.notify_all();
                    })?;
                state.inputs.push_back((
                    MessageId {
                        domain: 0,
                        sequence,
                    },
                    value,
                    bytes,
                ));
                state.dynamic_bytes += bytes;
                state.root_live += 1;
                self.0.changed.notify_all();
                return Ok(());
            }
            if !wait {
                return Err(StreamError::Capacity);
            }
            state = self.0.changed.wait(state).unwrap();
        }
    }

    pub fn close(&self) {
        self.0.state.lock().unwrap().input_closed = true;
        self.0.changed.notify_all();
    }

    pub fn abort(&self, error: StreamError) {
        let mut state = self.0.state.lock().unwrap();
        if !state.done && state.failure.is_none() {
            state.failure = Some(error);
            self.0.cancellation.store(true, Ordering::Release);
        }
        self.0.changed.notify_all();
    }

    pub fn cancel(&self) {
        self.abort(StreamError::Cancelled);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancellation.load(Ordering::Acquire)
    }
}

pub struct StreamDelivery {
    output: Option<StreamOutput>,
    shared: Arc<Shared>,
}

impl StreamDelivery {
    pub fn output(&self) -> &StreamOutput {
        self.output.as_ref().expect("delivery is pending")
    }
    pub fn max_record_bytes(&self) -> usize {
        self.shared.resources.frame_bytes[self.output().message.domain]
    }
    pub fn acknowledge(mut self) -> Result<StreamOutput, StreamError> {
        let output = self.output.take().expect("delivery is acknowledged once");
        let mut state = self.shared.state.lock().unwrap();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        state.delivered = Some(output.message.domain);
        state.delivery_pending = false;
        take_sequence(&mut state.summary.delivered_outputs).inspect_err(|error| {
            state.failure = Some(error.clone());
            self.shared.changed.notify_all();
        })?;
        self.shared.changed.notify_all();
        Ok(output)
    }
    pub fn fail(mut self, error: StreamError) {
        self.output.take();
        StreamSender(Arc::clone(&self.shared)).abort(error);
    }
}

impl Drop for StreamDelivery {
    fn drop(&mut self) {
        if self.output.is_some() {
            StreamSender(Arc::clone(&self.shared)).abort(StreamError::Output {
                message: "delivery was dropped before acknowledgement".into(),
            });
        }
    }
}

pub struct StreamInstance {
    shared: Arc<Shared>,
    coordinator: Option<JoinHandle<()>>,
}

impl PreparedStream {
    pub fn start(self) -> Result<StreamInstance, StreamError> {
        self.start_with_options(StreamOptions::default())
    }

    pub fn start_with_options(self, options: StreamOptions) -> Result<StreamInstance, StreamError> {
        let observation = options.observation.clone();
        let result = self.start_inner(options);
        if let (Err(error), Some(observation)) = (&result, observation) {
            observation.preparation_failed(error.to_string());
        }
        result
    }

    fn start_inner(self, options: StreamOptions) -> Result<StreamInstance, StreamError> {
        if options.snapshots.is_some() {
            return Err(StreamError::Preparation {
                message: "snapshot capture is unsupported for streaming instances".into(),
            });
        }
        let (prepared, event_states) = self.into_parts();
        let domain_count = prepared.domains().len();
        let input_capacity = prepared
            .execution()
            .limits
            .max_pending_messages
            .checked_sub(domain_count - 1)
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| StreamError::Preparation {
                message: format!(
                    "max_pending_messages must reserve at least {domain_count} domain slots"
                ),
            })?;
        let resources = StreamResources::new(&prepared)?;
        let mut dynamic_bytes = 0usize;
        let mut operators = Vec::with_capacity(prepared.nodes().len());
        for event_state in event_states {
            operators.push(if let Some(event_state) = event_state {
                let retained_bytes = event_state.retained_bytes();
                dynamic_bytes = add(dynamic_bytes, retained_bytes)?;
                Some(Operator {
                    state: event_state,
                    retained_bytes,
                    deadline: None,
                    pending: VecDeque::new(),
                    pending_bytes: 0,
                    closed: false,
                })
            } else {
                None
            });
        }
        if add(dynamic_bytes, resources.source_bytes)? > resources.soft_bytes {
            return Err(StreamError::Preparation {
                message: "initial retained node state exceeds available byte capacity".into(),
            });
        }
        let plan = Arc::new(prepared);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                inputs: VecDeque::new(),
                output: None,
                delivered: None,
                delivery_pending: false,
                completions: VecDeque::new(),
                domains: (0..domain_count).map(|_| Domain::default()).collect(),
                operators,
                sequences: vec![0; domain_count],
                root_live: 0,
                dynamic_bytes,
                event_credits: 0,
                active_workers: 0,
                input_closed: false,
                failure: None,
                failure_node: None,
                done: false,
                summary: StreamSummary::default(),
            }),
            changed: Condvar::new(),
            execution: plan.execution().clone(),
            resources,
            input_capacity,
            cancellation: Arc::new(AtomicBool::new(false)),
            observation: options.observation,
        });
        options
            .clock
            .register_waker(Waker::from(Arc::new(ClockWake(Arc::downgrade(&shared)))));
        let coordinator_shared = Arc::clone(&shared);
        let workers = Workers::new(&shared, &plan)?;
        let coordinator = thread::Builder::new()
            .name("workflow-stream".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    coordinate(&coordinator_shared, &plan, options.clock.as_ref(), workers)
                }));
                if let Err(payload) = result {
                    let mut state = coordinator_shared
                        .state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    coordinator_shared.state.clear_poison();
                    state.failure.get_or_insert_with(|| panic_error(payload));
                    clear_retained(&mut state);
                    state.done = true;
                    finish_observation(&coordinator_shared, &state);
                    coordinator_shared.changed.notify_all();
                }
            })
            .map_err(|error| StreamError::Preparation {
                message: error.to_string(),
            })?;
        Ok(StreamInstance {
            shared,
            coordinator: Some(coordinator),
        })
    }
}

impl StreamInstance {
    pub fn input(&self) -> StreamSender {
        StreamSender(Arc::clone(&self.shared))
    }
    pub fn close_input(&self) {
        self.input().close();
    }
    pub fn cancel(&self) {
        self.input().cancel();
    }

    pub fn recv(&self) -> Result<Option<StreamOutput>, StreamError> {
        self.receive()?.map(StreamDelivery::acknowledge).transpose()
    }

    pub fn receive(&self) -> Result<Option<StreamDelivery>, StreamError> {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if let Some(output) = state.output.take() {
                state.delivery_pending = true;
                drop(state);
                return Ok(Some(StreamDelivery {
                    output: Some(output),
                    shared: Arc::clone(&self.shared),
                }));
            }
            if state.done {
                return Ok(None);
            }
            state = self.shared.changed.wait(state).unwrap();
        }
    }

    pub fn summary(&self) -> StreamSummary {
        self.shared.state.lock().unwrap().summary.clone()
    }

    pub fn execution(&self) -> &StreamExecution {
        &self.shared.execution
    }

    pub fn failure(&self) -> Option<StreamError> {
        self.shared.state.lock().unwrap().failure.clone()
    }

    pub fn metrics(&self) -> StreamMetrics {
        let state = self.shared.state.lock().unwrap();
        if state.done {
            return StreamMetrics::default();
        }
        let reserved_bytes = self.shared.resources.fixed_bytes + state.event_credits;
        StreamMetrics {
            pending_frames: state.root_live
                + state
                    .domains
                    .iter()
                    .skip(1)
                    .filter(|domain| domain.occupied)
                    .count(),
            active_workers: state.active_workers,
            queued_and_buffered_bytes: state.dynamic_bytes,
            reserved_bytes,
            accounted_bytes: reserved_bytes + state.dynamic_bytes,
        }
    }

    pub fn join(mut self) -> Result<StreamSummary, StreamError> {
        if let Some(coordinator) = self.coordinator.take() {
            coordinator.join().map_err(panic_error)?;
        }
        let state = self.shared.state.lock().unwrap();
        match &state.failure {
            Some(error) => Err(error.clone()),
            None => Ok(state.summary.clone()),
        }
    }
}

impl Drop for StreamInstance {
    fn drop(&mut self) {
        if let Some(coordinator) = self.coordinator.take() {
            self.cancel();
            let _ = coordinator.join();
        }
    }
}

struct Workers {
    sender: Option<mpsc::SyncSender<Frame>>,
    threads: Vec<JoinHandle<()>>,
    count: usize,
}

impl Workers {
    fn new(shared: &Arc<Shared>, plan: &Arc<StreamPlan>) -> Result<Self, StreamError> {
        let active_domains = plan
            .domains()
            .iter()
            .filter(|domain| {
                domain
                    .steps
                    .iter()
                    .any(|&index| plan.nodes()[index].node.is_some())
            })
            .count();
        let count = plan.execution().limits.workers.min(active_domains);
        let (sender, receiver) = mpsc::sync_channel::<Frame>(count);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Self {
            sender: Some(sender),
            threads: Vec::new(),
            count,
        };
        for index in 0..count {
            let receiver = Arc::clone(&receiver);
            let shared = Arc::clone(shared);
            let plan = Arc::clone(plan);
            workers.threads.push(
                thread::Builder::new()
                    .name(format!("workflow-worker-{index}"))
                    .spawn(move || {
                        loop {
                            let job = { receiver.lock().unwrap().recv() };
                            let Ok(mut frame) = job else { break };
                            let index = plan.domains()[frame.message.domain].steps[frame.cursor];
                            let _context = frame
                                .context
                                .observation()
                                .map(crate::RunObservation::enter);
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                plan.execute_step(index, &mut frame.context)
                            }))
                            .map_err(panic_error)
                            .and_then(|result| {
                                result.map_err(|error| workflow_error(error, frame.message))
                            });
                            if result.is_ok() {
                                frame.cursor += 1;
                            } else {
                                frame.context = ExecutionContext::default();
                            }
                            shared
                                .state
                                .lock()
                                .unwrap()
                                .completions
                                .push_back(Completion { frame, result });
                            shared.changed.notify_all();
                        }
                    })
                    .map_err(|error| StreamError::Preparation {
                        message: error.to_string(),
                    })?,
            );
        }
        Ok(workers)
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}

fn coordinate(shared: &Arc<Shared>, plan: &StreamPlan, clock: &dyn StreamClock, workers: Workers) {
    let mut state = shared.state.lock().unwrap();
    loop {
        while let Some(completion) = state.completions.pop_front() {
            state.active_workers -= 1;
            let domain = completion.frame.message.domain;
            state.domains[domain].running = false;
            if let Err(error) = completion.result {
                if state.failure.is_none() {
                    let index = plan.domains()[domain].steps[completion.frame.cursor];
                    state.failure_node = Some(plan.nodes()[index].definition_id.to_string());
                }
                state.failure.get_or_insert(error);
            }
            if state.failure.is_none() {
                state.domains[domain].frame = Some(completion.frame);
            }
        }
        if state.failure.is_some() {
            shared.cancellation.store(true, Ordering::Release);
            state.input_closed = true;
            state.inputs.clear();
            state.output = None;
            for domain in &mut state.domains {
                domain.frame = None;
                domain.occupied = domain.running;
            }
            for operator in &mut state.operators {
                *operator = None;
            }
            state.dynamic_bytes = 0;
            state.event_credits = 0;
            state.root_live = usize::from(state.domains[0].running);
            state.delivered = None;
            state.delivery_pending = false;
            if state.active_workers == 0 {
                state.done = true;
                finish_observation(shared, &state);
                shared.changed.notify_all();
                break;
            }
            state = shared.changed.wait(state).unwrap();
            continue;
        }
        if let Some(domain) = state.delivered.take() {
            release_domain(&mut state, domain);
        }
        match tick(&mut state, plan, &workers, clock, shared) {
            Ok(progress) => {
                shared.changed.notify_all();
                if state.domains.iter().all(|domain| domain.closed)
                    && state.active_workers == 0
                    && state.output.is_none()
                {
                    clear_retained(&mut state);
                    state.done = true;
                    finish_observation(shared, &state);
                    shared.changed.notify_all();
                    break;
                }
                if progress {
                    drop(state);
                    thread::yield_now();
                    state = shared.state.lock().unwrap();
                    continue;
                }
            }
            Err(error) => {
                state.failure = Some(error);
                shared.changed.notify_all();
                continue;
            }
        }
        let wait = plan
            .domains()
            .iter()
            .skip(1)
            .filter_map(|domain| state.operators[domain.source].as_ref())
            .filter(|operator| {
                operator.pending.len() < plan.execution().limits.max_pending_messages
            })
            .filter_map(|operator| {
                operator
                    .deadline
                    .map(|deadline| deadline.saturating_sub(clock.now()))
            })
            .min();
        if wait.is_none()
            && state.active_workers == 0
            && state.output.is_none()
            && !state.delivery_pending
            && (state.dynamic_bytes > shared.resources.soft_bytes
                || !state.inputs.is_empty()
                || state.domains.iter().any(|domain| domain.frame.is_some())
                || state
                    .operators
                    .iter()
                    .flatten()
                    .any(|operator| !operator.pending.is_empty()))
        {
            state.failure = Some(StreamError::Resource {
                message: "retained state prevents progress within the configured byte budget"
                    .into(),
            });
            continue;
        }
        state = match wait {
            Some(wait) => shared.changed.wait_timeout(state, wait).unwrap().0,
            None => shared.changed.wait(state).unwrap(),
        };
    }
    drop(state);
    drop(workers);
}

fn tick(
    state: &mut State,
    plan: &StreamPlan,
    workers: &Workers,
    clock: &dyn StreamClock,
    shared: &Shared,
) -> Result<bool, StreamError> {
    let resources = &shared.resources;
    let mut progress = false;
    for source in plan.domains().iter().skip(1) {
        let index = source.source;
        let due = state.operators[index]
            .as_ref()
            .filter(|operator| {
                !operator.closed
                    && operator.pending.len() < plan.execution().limits.max_pending_messages
            })
            .and_then(|operator| {
                operator
                    .deadline
                    .filter(|deadline| *deadline <= clock.now())
            });
        if due.is_some() {
            state.operators[index].as_mut().unwrap().deadline = None;
            invoke_event(
                state,
                plan,
                index,
                NodeEvent::Timer,
                EventContext {
                    now: clock.now(),
                    input: None,
                },
                resources,
                event_callback(shared, plan, index, None, StreamTrigger::Timer),
            )?;
            progress = true;
        }
    }
    if !state.domains[0].occupied
        && let Some((_, _, bytes)) = state.inputs.front()
        && can_begin(state, resources, 0, *bytes)?
    {
        let (message, value, bytes) = state.inputs.pop_front().unwrap();
        state.dynamic_bytes -= bytes;
        let credits = resources.domain_credits[0];
        state.event_credits = add(state.event_credits, credits)?;
        let context_domain = 0;
        let mut context = ExecutionContext::for_message(
            &plan.nodes()[0],
            Outputs::from([("item".into(), value)]).into(),
        )
        .map_err(execution_error)?;
        context.set_cancellation(Arc::clone(&shared.cancellation));
        context.set_stream_limits(
            plan.execution().limits.max_message_bytes,
            resources.frame_bytes[context_domain],
        );
        if let Some(observation) = &shared.observation {
            context.set_frame_observation(observation.frame(stream_message(message)));
        }
        context.retained_bytes(resources.frame_bytes[0])?;
        state.domains[0].frame = Some(Frame {
            message,
            context,
            cursor: 0,
            credits,
        });
        state.domains[0].occupied = true;
        progress = true;
    }
    for domain in 1..state.domains.len() {
        if state.domains[domain].occupied {
            continue;
        }
        let source = plan.domains()[domain].source;
        let operator = state.operators[source].as_ref().unwrap();
        let Some(queued) = operator.pending.front() else {
            continue;
        };
        let remaining = operator.pending_bytes - queued.bytes;
        let release = operator
            .pending_bytes
            .saturating_sub(resources.seal_bytes[source])
            - remaining.saturating_sub(resources.seal_bytes[source]);
        if !can_begin(state, resources, domain, release)? {
            continue;
        }
        let operator = state.operators[source].as_mut().unwrap();
        let queued = operator.pending.pop_front().unwrap();
        operator.pending_bytes = remaining;
        state.dynamic_bytes -= release;
        let credits = resources.domain_credits[domain];
        state.event_credits = add(state.event_credits, credits)?;
        let context_domain = domain;
        let mut context =
            ExecutionContext::for_message(&plan.nodes()[source], queued.emission.result)
                .map_err(execution_error)?;
        context.set_cancellation(Arc::clone(&shared.cancellation));
        context.set_stream_limits(
            plan.execution().limits.max_message_bytes,
            resources.frame_bytes[context_domain],
        );
        if let Some(observation) = &shared.observation {
            context.set_frame_observation(observation.frame(stream_message(queued.message)));
        }
        context.retained_bytes(resources.frame_bytes[domain])?;
        state.domains[domain].frame = Some(Frame {
            message: queued.message,
            context,
            cursor: 0,
            credits,
        });
        state.domains[domain].occupied = true;
        progress = true;
    }
    for domain in 0..state.domains.len() {
        let Some(mut frame) = state.domains[domain].frame.take() else {
            continue;
        };
        let steps = &plan.domains()[domain].steps;
        if frame.cursor == steps.len() {
            state.event_credits -= frame.credits;
            take_sequence(&mut state.summary.completed_frames)?;
            if plan.selected_domain() == Some(domain) {
                let mut outputs = FlowOutputs::new();
                for output in plan.outputs() {
                    if let Some(value) = frame
                        .context
                        .select_output(
                            &output.name,
                            output.node.as_str(),
                            &output.port,
                            output.optional,
                        )
                        .map_err(execution_error)?
                    {
                        outputs.insert(output.name.clone(), value);
                    }
                }
                output_bytes(&outputs, plan.execution().limits.max_message_bytes).map_err(
                    |error| StreamError::Resource {
                        message: format!(
                            "selected outputs for domain {domain} message {}: {error}",
                            frame.message.sequence
                        ),
                    },
                )?;
                state.output = Some(StreamOutput {
                    message: frame.message,
                    outputs,
                });
            } else {
                release_domain(state, domain);
            }
            progress = true;
            continue;
        }
        let index = steps[frame.cursor];
        if let Some(operator) = &state.operators[index] {
            if !operator.pending.is_empty() {
                state.domains[domain].frame = Some(frame);
                continue;
            }
            state.event_credits -= resources.event_bytes[index];
            frame.credits -= resources.event_bytes[index];
            let callback = event_callback(
                shared,
                plan,
                index,
                Some(frame.message),
                StreamTrigger::Input,
            );
            let _context = callback.as_ref().map(StreamCallback::enter);
            match frame
                .context
                .event_inputs(&plan.nodes()[index], plan.dependencies(index))
            {
                Ok(Some(inputs)) => invoke_event(
                    state,
                    plan,
                    index,
                    NodeEvent::Input(inputs),
                    EventContext {
                        now: clock.now(),
                        input: Some(&frame.context),
                    },
                    resources,
                    callback,
                )?,
                Ok(None) => {
                    if let Some(callback) = callback {
                        let causes: std::collections::BTreeSet<_> = plan
                            .dependencies(index)
                            .iter()
                            .filter(|dependency| {
                                matches!(
                                    frame.context.output(&crate::output_id(
                                        &dependency.source_node,
                                        &dependency.source_output
                                    )),
                                    Ok(crate::ContextValue::Skipped)
                                )
                            })
                            .map(|dependency| SkipCause {
                                source_node: dependency.source_node.clone(),
                                source_output: dependency.source_output.clone(),
                            })
                            .collect();
                        callback.skipped(causes.into_iter().collect());
                    }
                }
                Err(error) => {
                    if let Some(callback) = callback {
                        callback.failed(FailurePhase::Dependency, error.to_string());
                    }
                    state.failure_node = Some(plan.nodes()[index].definition_id.to_string());
                    return Err(workflow_error(error, frame.message));
                }
            }
            frame.cursor += 1;
            state.domains[domain].frame = Some(frame);
            progress = true;
        } else if state.active_workers < workers.count {
            workers
                .sender
                .as_ref()
                .unwrap()
                .try_send(frame)
                .map_err(|_| StreamError::Execution {
                    message: "worker queue unavailable".into(),
                })?;
            state.active_workers += 1;
            state.domains[domain].running = true;
            progress = true;
        } else {
            state.domains[domain].frame = Some(frame);
        }
    }
    if state.input_closed
        && state.inputs.is_empty()
        && !state.domains[0].occupied
        && !state.domains[0].closed
    {
        state.domains[0].closed = true;
        progress = true;
    }
    for domain in 0..state.domains.len() {
        if state.domains[domain].closed {
            for &index in &plan.domains()[domain].steps {
                if state.operators[index]
                    .as_ref()
                    .is_some_and(|operator| !operator.closed)
                {
                    invoke_event(
                        state,
                        plan,
                        index,
                        NodeEvent::UpstreamClosed,
                        EventContext {
                            now: clock.now(),
                            input: None,
                        },
                        resources,
                        event_callback(shared, plan, index, None, StreamTrigger::UpstreamClosed),
                    )?;
                    let operator = state.operators[index].as_mut().unwrap();
                    operator.closed = true;
                    operator.deadline = None;
                    progress = true;
                }
            }
        } else if domain > 0 && !state.domains[domain].occupied {
            let operator = state.operators[plan.domains()[domain].source]
                .as_ref()
                .unwrap();
            if operator.closed && operator.pending.is_empty() {
                state.domains[domain].closed = true;
                progress = true;
            }
        }
    }
    Ok(progress)
}

fn can_begin(
    state: &State,
    resources: &StreamResources,
    domain: usize,
    released: usize,
) -> Result<bool, StreamError> {
    let bytes = state
        .dynamic_bytes
        .checked_sub(released)
        .ok_or_else(|| execution_error("invalid retained-byte accounting"))?;
    Ok(add(
        add(bytes, state.event_credits)?,
        resources.domain_credits[domain],
    )? <= resources.hard_bytes)
}

fn event_callback(
    shared: &Shared,
    plan: &StreamPlan,
    index: usize,
    message: Option<MessageId>,
    trigger: StreamTrigger,
) -> Option<StreamCallback> {
    shared.observation.as_ref()?.callback(
        plan.nodes()[index].definition_id.as_str(),
        message.map(stream_message),
        trigger,
    )
}

fn invoke_event(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    event: NodeEvent,
    context: EventContext<'_>,
    resources: &StreamResources,
    mut callback: Option<StreamCallback>,
) -> Result<(), StreamError> {
    let _span = callback.as_ref().map(StreamCallback::enter);
    if let Some(callback) = callback.as_mut() {
        callback.started();
    }
    let mut phase = FailurePhase::Execution;
    let old_pending = state.operators[index].as_ref().unwrap().pending.len();
    let result = (|| -> Result<(usize, Vec<String>), StreamError> {
        let operator = state.operators[index].as_ref().unwrap();
        let previous = add(
            operator.retained_bytes,
            operator
                .pending_bytes
                .saturating_sub(resources.seal_bytes[index]),
        )?;
        let effects = catch_unwind(AssertUnwindSafe(|| {
            state.operators[index]
                .as_mut()
                .unwrap()
                .state
                .on_event(event, &context)
        }))
        .map_err(panic_error)?
        .map_err(|error| StreamError::Execution {
            message: format!("node `{}`: {error}", plan.nodes()[index].definition_id),
        })?;
        phase = FailurePhase::Publication;
        let count = effects.emissions.len();
        let ports: std::collections::BTreeSet<_> = effects
            .emissions
            .iter()
            .flat_map(|emission| emission.result.outputs.keys().cloned())
            .collect();
        apply_effects(state, plan, index, effects, context.now, resources)?;
        let operator = state.operators[index].as_mut().unwrap();
        operator.retained_bytes = operator.state.retained_bytes();
        let current = add(
            operator.retained_bytes,
            operator
                .pending_bytes
                .saturating_sub(resources.seal_bytes[index]),
        )?;
        state.dynamic_bytes = add(
            state
                .dynamic_bytes
                .checked_sub(previous)
                .ok_or_else(|| execution_error("invalid operator byte accounting"))?,
            current,
        )?;
        if add(state.dynamic_bytes, state.event_credits)? > resources.hard_bytes {
            return Err(StreamError::Resource {
                message: format!(
                    "node `{}` retained more data than the available byte budget",
                    plan.nodes()[index].definition_id
                ),
            });
        }
        Ok((count, ports.into_iter().collect()))
    })();
    match result {
        Ok((count, ports)) => {
            if let Some(callback) = callback {
                let operator = state.operators[index].as_ref().unwrap();
                if let Some(items) = operator.state.buffered_items() {
                    callback.buffered(items);
                }
                for emitted in operator.pending.iter().skip(old_pending) {
                    if let Some(batch) = emitted.emission.batch {
                        let reason = match batch.reason {
                            crate::FlushReason::SizeExceed => "size_exceed",
                            crate::FlushReason::TimeoutExceed => "timeout_exceed",
                            crate::FlushReason::UpstreamClosed => "upstream_closed",
                        };
                        callback.flushed(stream_message(emitted.message), batch.item_count, reason);
                    }
                }
                callback.succeeded(count, ports);
            }
            Ok(())
        }
        Err(error) => {
            state.failure_node = Some(plan.nodes()[index].definition_id.to_string());
            if let Some(callback) = callback {
                callback.failed(phase, error.to_string());
            }
            Err(error)
        }
    }
}

fn apply_effects(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    effects: EventEffects,
    now: Duration,
    resources: &StreamResources,
) -> Result<(), StreamError> {
    let domain = plan.output_domain(index);
    let operator = state.operators[index].as_mut().unwrap();
    if effects
        .emissions
        .len()
        .saturating_add(operator.pending.len())
        > plan.execution().limits.max_pending_messages
    {
        return Err(StreamError::Resource {
            message: format!(
                "node `{}` emitted too many pending messages",
                plan.nodes()[index].definition_id
            ),
        });
    }
    for emission in effects.emissions {
        output_bytes(
            &emission.result.outputs,
            plan.execution().limits.max_message_bytes,
        )
        .map_err(|error| StreamError::Resource {
            message: format!(
                "node `{}` output: {error}",
                plan.nodes()[index].definition_id
            ),
        })?;
        let context = ExecutionContext::for_message(&plan.nodes()[index], emission.result.clone())
            .map_err(execution_error)?;
        let bytes = context.retained_bytes(resources.seal_bytes[index] / 2)?;
        let sequence = take_sequence(&mut state.sequences[domain])?;
        take_sequence(&mut state.summary.emitted_messages)?;
        operator.pending_bytes = add(operator.pending_bytes, bytes)?;
        operator.pending.push_back(QueuedEmission {
            message: MessageId { domain, sequence },
            emission,
            bytes,
        });
    }
    match effects.timer {
        TimerUpdate::Keep => {}
        TimerUpdate::Cancel => operator.deadline = None,
        TimerUpdate::Set(deadline) => {
            if deadline <= now {
                return Err(StreamError::Execution {
                    message: format!(
                        "node `{}` must request a future timer deadline",
                        plan.nodes()[index].definition_id
                    ),
                });
            }
            if Instant::now()
                .checked_add(deadline.saturating_sub(now))
                .is_none()
            {
                return Err(StreamError::Resource {
                    message: format!(
                        "node `{}` deadline exceeds the monotonic clock",
                        plan.nodes()[index].definition_id
                    ),
                });
            }
            operator.deadline = Some(deadline);
        }
    }
    Ok(())
}

fn stream_message(message: MessageId) -> StreamMessage {
    StreamMessage {
        domain: message.domain,
        sequence: message.sequence,
    }
}

fn finish_observation(shared: &Shared, state: &State) {
    if let Some(observation) = &shared.observation {
        let failure = state.failure.as_ref().map(|error| StreamFailure {
            phase: match error {
                StreamError::Preparation { .. } => "preparation",
                StreamError::Input { .. } => "input",
                StreamError::Output { .. } => "output",
                StreamError::Resource { .. } | StreamError::Capacity => "resource",
                StreamError::Cancelled => "cancellation",
                _ => "execution",
            }
            .into(),
            message: error.to_string(),
            node: state.failure_node.clone(),
        });
        observation.finish(
            StreamCounts {
                accepted_inputs: state.summary.accepted_inputs,
                emitted_messages: state.summary.emitted_messages,
                completed_frames: state.summary.completed_frames,
                delivered_outputs: state.summary.delivered_outputs,
            },
            failure,
            matches!(state.failure, Some(StreamError::Cancelled)),
        );
    }
}

fn release_domain(state: &mut State, domain: usize) {
    state.domains[domain].occupied = false;
    if domain == 0 {
        state.root_live -= 1;
    }
}

fn clear_retained(state: &mut State) {
    state.inputs.clear();
    state.output = None;
    state.delivered = None;
    state.delivery_pending = false;
    state.completions.clear();
    for domain in &mut state.domains {
        *domain = Domain::default();
        domain.closed = true;
    }
    for operator in &mut state.operators {
        *operator = None;
    }
    state.dynamic_bytes = 0;
    state.event_credits = 0;
    state.root_live = 0;
}

fn take_sequence(sequence: &mut u64) -> Result<u64, StreamError> {
    let current = *sequence;
    *sequence = current
        .checked_add(1)
        .ok_or_else(|| StreamError::Resource {
            message: "sequence counter exhausted".into(),
        })?;
    Ok(current)
}

fn workflow_error(error: crate::WorkflowRunError, message: MessageId) -> StreamError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    let mut resource = false;
    while let Some(error) = source {
        if matches!(
            error.downcast_ref::<crate::WorkflowRunError>(),
            Some(crate::WorkflowRunError::Resource { .. })
        ) {
            resource = true;
        }
        source = error.source();
    }
    let message = format!(
        "domain {} message {}: {error}",
        message.domain, message.sequence
    );
    if resource {
        StreamError::Resource { message }
    } else {
        StreamError::Execution { message }
    }
}

fn execution_error(error: impl std::fmt::Display) -> StreamError {
    StreamError::Execution {
        message: error.to_string(),
    }
}

fn panic_error(payload: Box<dyn std::any::Any + Send>) -> StreamError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .unwrap_or_else(|| "node panicked".into());
    execution_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identities_fail_before_wraparound_and_size_counting_stops_at_the_limit() {
        let mut sequence = u64::MAX;
        assert!(take_sequence(&mut sequence).is_err());
        assert_eq!(sequence, u64::MAX);
        assert_eq!(encoded_size(&"hello", 7).unwrap(), 7);
        assert!(encoded_size(&"hello", 6).is_err());
    }

    #[test]
    fn nonfuture_timer_requests_fail_without_spinning() {
        struct Timer;

        impl EventNode for Timer {
            fn on_event(
                &mut self,
                event: NodeEvent,
                context: &EventContext<'_>,
            ) -> Result<EventEffects, crate::NodeExecutionError> {
                Ok(EventEffects {
                    emissions: Vec::new(),
                    timer: if matches!(event, NodeEvent::UpstreamClosed) {
                        TimerUpdate::Cancel
                    } else {
                        TimerUpdate::Set(context.now)
                    },
                })
            }
        }
        let plan = PreparedStream::new(
            StreamExecution {
                mode: crate::StreamMode::Stream,
                input_type: crate::ValueType::Int64,
                limits: Default::default(),
            },
            vec![
                crate::stream_input_node(crate::ValueType::Int64),
                crate::FlowNode::new(
                    "timer",
                    crate::PreparedNode::event(Timer, crate::NodeMetadata::default()),
                ),
            ],
            vec![
                Vec::new(),
                vec![crate::StreamDependency {
                    input: None,
                    source_node: crate::STREAM_INPUT_ID.into(),
                    source_output: "item".into(),
                }],
            ],
            Vec::new(),
        )
        .unwrap();
        let instance = plan.start().unwrap();
        instance.input().send(serde_json::json!(1)).unwrap();
        instance.close_input();
        assert!(
            instance
                .recv()
                .unwrap_err()
                .to_string()
                .contains("future timer")
        );
        assert!(instance.join().is_err());
    }
}
