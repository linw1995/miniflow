use crate::{
    EventContext, EventEffects, EventEmission, EventNode, ExecutionContext, FlowOutputs, NodeEvent,
    Outputs, PreparedStream, StreamExecution, StreamPlan, TimerUpdate, ValueRef,
};
use serde::Serialize;
use snafu::Snafu;
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, Weak, mpsc},
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
    pub snapshots: Option<crate::SnapshotRecorder>,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(MonotonicClock::default()),
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
}

struct Frame {
    message: MessageId,
    context: ExecutionContext,
    cursor: usize,
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
}

struct Operator {
    state: Box<dyn EventNode>,
    deadline: Option<Duration>,
    pending: VecDeque<QueuedEmission>,
    closed: bool,
}

struct Completion {
    frame: Frame,
    result: Result<(), StreamError>,
}

struct State {
    inputs: VecDeque<(MessageId, ValueRef)>,
    output: Option<StreamOutput>,
    delivered: Option<usize>,
    delivery_pending: bool,
    completions: VecDeque<Completion>,
    domains: Vec<Domain>,
    operators: Vec<Option<Operator>>,
    sequences: Vec<u64>,
    root_live: usize,
    active_workers: usize,
    input_closed: bool,
    failure: Option<StreamError>,
    done: bool,
    summary: StreamSummary,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    execution: StreamExecution,
    input_capacity: usize,
}

impl Shared {
    fn fail(&self, error: StreamError) {
        let mut state = self.state.lock().unwrap();
        if !state.done && state.failure.is_none() {
            state.failure = Some(error);
        }
        self.changed.notify_all();
    }
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
        if let Err(error) = self
            .0
            .execution
            .input_type
            .validate_shared(&value)
            .map_err(|error| StreamError::Input {
                message: error.to_string(),
            })
        {
            state.failure = Some(error.clone());
            self.0.changed.notify_all();
            return Err(error);
        }
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.input_closed || state.done {
                return Err(StreamError::Closed);
            }
            if state.root_live < self.0.input_capacity {
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
                ));
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

    pub(super) fn fail(&self, error: StreamError) {
        self.0.fail(error);
    }

    pub(super) fn failure(&self) -> Option<StreamError> {
        self.0.state.lock().unwrap().failure.clone()
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
        self.shared.fail(error);
    }
}

impl Drop for StreamDelivery {
    fn drop(&mut self) {
        if self.output.is_some() {
            self.shared.fail(StreamError::Output {
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
        let operators = event_states
            .into_iter()
            .map(|state| {
                state.map(|state| Operator {
                    state,
                    deadline: None,
                    pending: VecDeque::new(),
                    closed: false,
                })
            })
            .collect();
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
                active_workers: 0,
                input_closed: false,
                failure: None,
                done: false,
                summary: StreamSummary::default(),
            }),
            changed: Condvar::new(),
            execution: plan.execution().clone(),
            input_capacity,
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
        StreamMetrics {
            pending_frames: state.root_live
                + state
                    .domains
                    .iter()
                    .skip(1)
                    .filter(|domain| domain.occupied)
                    .count(),
            active_workers: state.active_workers,
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
            self.shared.fail(StreamError::Execution {
                message: "stream instance dropped before completion".into(),
            });
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
                            let failure = shared.state.lock().unwrap().failure.clone();
                            let result = if let Some(error) = failure {
                                Err(error)
                            } else {
                                catch_unwind(AssertUnwindSafe(|| {
                                    plan.execute_step(index, &mut frame.context)
                                }))
                                .map_err(panic_error)
                                .and_then(|result| {
                                    result.map_err(|error| workflow_error(error, frame.message))
                                })
                            };
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
                state.failure.get_or_insert(error);
            }
            if state.failure.is_none() {
                state.domains[domain].frame = Some(completion.frame);
            }
        }
        if state.failure.is_some() {
            state.input_closed = true;
            if state.active_workers == 0 {
                clear_retained(&mut state);
                state.done = true;

                shared.changed.notify_all();
                break;
            }
            state = shared.changed.wait(state).unwrap();
            continue;
        }
        if let Some(domain) = state.delivered.take() {
            release_domain(&mut state, domain);
        }
        match tick(&mut state, plan, &workers, clock) {
            Ok(progress) => {
                shared.changed.notify_all();
                if state.domains.iter().all(|domain| domain.closed)
                    && state.active_workers == 0
                    && state.output.is_none()
                {
                    clear_retained(&mut state);
                    state.done = true;

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
) -> Result<bool, StreamError> {
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
            )?;
            progress = true;
        }
    }
    if !state.domains[0].occupied
        && let Some((message, value)) = state.inputs.pop_front()
    {
        let context = ExecutionContext::for_message(
            &plan.nodes()[0],
            Outputs::from([("item".into(), value)]).into(),
        )
        .map_err(execution_error)?;
        state.domains[0].frame = Some(Frame {
            message,
            context,
            cursor: 0,
        });
        state.domains[0].occupied = true;
        progress = true;
    }
    for domain in 1..state.domains.len() {
        if state.domains[domain].occupied {
            continue;
        }
        let source = plan.domains()[domain].source;
        let Some(queued) = state.operators[source]
            .as_mut()
            .unwrap()
            .pending
            .pop_front()
        else {
            continue;
        };
        let context = ExecutionContext::for_message(&plan.nodes()[source], queued.emission.result)
            .map_err(execution_error)?;
        state.domains[domain].frame = Some(Frame {
            message: queued.message,
            context,
            cursor: 0,
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
                )?,
                Ok(None) => {}
                Err(error) => {
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

fn invoke_event(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    event: NodeEvent,
    context: EventContext<'_>,
) -> Result<(), StreamError> {
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
    apply_effects(state, plan, index, effects, context.now)?;
    Ok(())
}

fn apply_effects(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    effects: EventEffects,
    now: Duration,
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
        ExecutionContext::for_message(&plan.nodes()[index], emission.result.clone())
            .map_err(execution_error)?;
        let sequence = take_sequence(&mut state.sequences[domain])?;
        take_sequence(&mut state.summary.emitted_messages)?;
        operator.pending.push_back(QueuedEmission {
            message: MessageId { domain, sequence },
            emission,
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
    execution_error(format!(
        "domain {} message {}: {error}",
        message.domain, message.sequence
    ))
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
    fn identities_fail_before_wraparound() {
        let mut sequence = u64::MAX;
        assert!(take_sequence(&mut sequence).is_err());
        assert_eq!(sequence, u64::MAX);
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
