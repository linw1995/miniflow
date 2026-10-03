use crate::{
    EventContext, EventEffects, EventEmission, EventNode, ExecutionContext, FlowOutputs, Inputs,
    NodeEvent, NodeExecution, NodeExecutionError, NodeResult, PreparedStream, StreamExecution,
    StreamNode, StreamPlan, TimerUpdate, WorkerPool,
};
use mf_telemetry::{
    event::{FailurePhase, SkipCause},
    observation::{StreamCallback, StreamObservation},
    stream::{StreamCounts, StreamFailure, StreamMessage, StreamTrigger},
};
use serde::Serialize;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Wake, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Snafu)]
pub enum StreamError {
    #[snafu(display("stream preparation failed: {message}"), visibility(pub))]
    Preparation { message: String },
    #[snafu(display("could not claim stream stdio: {source}"), visibility(pub))]
    Stdio {
        #[snafu(source(from(std::io::Error, Arc::new)))]
        source: Arc<std::io::Error>,
    },
    #[snafu(display("stream preparation failed: {source}"), visibility(pub))]
    Compilation {
        #[snafu(source(from(Box<dyn std::error::Error + Send + Sync>, Arc::from)))]
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
    #[snafu(
        display("could not start stream thread `{thread}`: {source}"),
        visibility(pub)
    )]
    ThreadSpawn {
        thread: String,
        #[snafu(source(from(std::io::Error, Arc::new)))]
        source: Arc<std::io::Error>,
    },
    #[snafu(display("stream worker preparation failed: {source}"))]
    WorkerStartup {
        #[snafu(source(from(crate::WorkerPoolError, Arc::new)))]
        source: Arc<crate::WorkerPoolError>,
    },
    #[snafu(display("invalid stream input: {source}"))]
    Input { source: crate::TypeMismatch },
    #[snafu(display("invalid stream input: {message}"), visibility(pub))]
    InputFailure { message: String },
    #[snafu(
        display("invalid stream input at line {line}: {source}"),
        visibility(pub)
    )]
    InputRecord {
        line: u64,
        #[snafu(source(from(Box<dyn std::error::Error + Send + Sync>, Arc::from)))]
        source: Arc<dyn std::error::Error + Send + Sync>,
    },
    #[snafu(display("stream output failed: {message}"))]
    Output { message: String },
    #[snafu(display("stream output failed: {source}"), visibility(pub))]
    OutputWrite {
        #[snafu(source(from(std::io::Error, Arc::new)))]
        source: Arc<std::io::Error>,
    },
    #[snafu(display("stream output failed: {source}"), visibility(pub))]
    OutputEncode {
        #[snafu(source(from(serde_json::Error, Arc::new)))]
        source: Arc<serde_json::Error>,
    },
    #[snafu(display("stream execution failed: {message}"))]
    Execution { message: String },
    #[snafu(display(
        "stream execution failed in domain {} message {}: {source}",
        message.domain, message.sequence
    ))]
    Workflow {
        message: MessageId,
        #[snafu(source(from(crate::WorkflowRunError, Arc::new)))]
        source: Arc<crate::WorkflowRunError>,
    },
    #[snafu(display("stream event node `{definition_id}` failed: {source}"))]
    Event {
        definition_id: crate::DefinitionId,
        #[snafu(source(from(crate::NodeExecutionError, Arc::new)))]
        source: Arc<crate::NodeExecutionError>,
    },
    #[snafu(display("stream producer node `{definition_id}` failed: {source}"))]
    Producer {
        definition_id: crate::DefinitionId,
        #[snafu(source(from(NodeExecutionError, Arc::new)))]
        source: Arc<NodeExecutionError>,
    },
    #[snafu(display("stream resource limit: {message}"), visibility(pub))]
    Resource { message: String },
    #[snafu(display("stream input capacity is full"))]
    Capacity,
    #[snafu(display("stream input is closed"))]
    Closed,
}

impl StreamError {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Preparation { .. }
            | Self::Compilation { .. }
            | Self::ThreadSpawn { .. }
            | Self::WorkerStartup { .. }
            | Self::Stdio { .. } => "preparation",
            Self::Input { .. } | Self::InputFailure { .. } | Self::InputRecord { .. } => "input",
            Self::Output { .. } | Self::OutputWrite { .. } | Self::OutputEncode { .. } => "output",
            Self::Resource { .. } | Self::Capacity => "resource",
            Self::Producer { source, .. } | Self::Event { source, .. } => {
                if let NodeExecutionError::PluginFailed { source } = source.as_ref()
                    && let Some(error) = source.downcast_ref::<Self>()
                {
                    return error.phase();
                }
                "execution"
            }
            _ => "execution",
        }
    }
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
    pub arguments: crate::WorkflowArguments,
    pub resources: crate::ExecutionResources,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(MonotonicClock::default()),
            observation: None,
            snapshots: None,
            arguments: crate::WorkflowArguments::default(),
            resources: crate::ExecutionResources::default(),
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
    pub startup_frames: u64,
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
    closed: bool,
}

struct QueuedEmission {
    message: MessageId,
    emission: EventEmission,
}

struct Operator {
    executor: OperatorExecutor,
    deadline: Option<Duration>,
    pending: VecDeque<QueuedEmission>,
    closed: bool,
}

enum OperatorExecutor {
    Event(Box<dyn EventNode>),
    Producer(Producer),
}

struct Producer {
    node: Option<Box<dyn StreamNode>>,
    worker: Option<WorkerPool<ProducerJob>>,
    active: bool,
}

struct ProducerJob {
    frame: Frame,
    detached: bool,
    inputs: Inputs,
    callback: Option<StreamCallback>,
}

struct Completion {
    frame: Frame,
    result: Result<(), StreamError>,
    producer: bool,
    detached: bool,
}

struct State {
    output: Option<StreamOutput>,
    delivered: Option<usize>,
    delivery_pending: bool,
    completions: VecDeque<Completion>,
    domains: Vec<Domain>,
    operators: Vec<Option<Operator>>,
    sequences: Vec<u64>,
    active_workers: usize,
    failure: Option<StreamError>,
    failure_node: Option<String>,
    done: bool,
    summary: StreamSummary,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    execution: StreamExecution,
    observation: Option<StreamObservation>,
    cancellation: crate::StreamCancellation,
}

impl Shared {
    fn fail(&self, error: StreamError) {
        let mut state = self.state.lock().unwrap();
        if !state.done && state.failure.is_none() {
            state.failure = Some(error);
        }
        let failure = state.failure.clone();
        drop(state);
        if let Some(failure) = failure {
            self.cancellation.cancel(failure);
        }
        self.changed.notify_all();
    }
}

/// A borrowed, non-cloneable output sink for one producer invocation.
pub struct Emitter<'a> {
    shared: &'a Shared,
    plan: &'a StreamPlan,
    index: usize,
    emitted: usize,
    ports: BTreeSet<String>,
    publication_failed: bool,
}

impl Emitter<'_> {
    /// Wait for queue capacity and transfer one validated result to the runtime.
    /// Failure wakes waiting senders. Success does not imply downstream completion.
    pub fn send(&mut self, result: NodeResult) -> Result<(), NodeExecutionError> {
        self.admit(result)
            .map_err(|source| NodeExecutionError::PluginFailed {
                source: Box::new(source),
            })
    }

    fn admit(&mut self, result: NodeResult) -> Result<(), StreamError> {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.operators[self.index].as_ref().unwrap().pending.len()
                < self.plan.execution().limits.max_pending_messages
            {
                break;
            }
            state = self.shared.changed.wait(state).unwrap();
        }
        let ports = result.outputs.keys().cloned().collect::<Vec<_>>();
        let admitted: Result<(), StreamError> = (|| {
            let emitted = self.emitted.checked_add(1).context(ResourceSnafu {
                message: "producer emission counter exhausted",
            })?;
            enqueue_emission(&mut state, self.plan, self.index, result.into())?;
            self.emitted = emitted;
            self.ports.extend(ports);
            Ok(())
        })();
        if let Err(error) = &admitted {
            self.publication_failed = true;
            state.failure = Some(error.clone());
            state.failure_node = Some(self.plan.nodes()[self.index].definition_id.to_string());
        }
        self.shared.changed.notify_all();
        admitted
    }
}

impl Producer {
    fn submit(
        &mut self,
        job: ProducerJob,
        shared: &Arc<Shared>,
        plan: &Arc<StreamPlan>,
    ) -> Result<(), StreamError> {
        if self.worker.is_none() {
            let node = Mutex::new(self.node.take().expect("producer has not started"));
            let shared = Arc::clone(shared);
            let plan = Arc::clone(plan);
            self.worker = Some(
                WorkerPool::new(1, move |job| {
                    run_producer(&mut **node.lock().unwrap(), job, &shared, &plan);
                })
                .context(WorkerStartupSnafu)?,
            );
        }
        self.worker.as_ref().unwrap().try_submit(job).map_err(|_| {
            ExecutionSnafu {
                message: "producer worker unavailable",
            }
            .build()
        })?;
        self.active = true;
        Ok(())
    }
}

fn run_producer(
    node: &mut dyn StreamNode,
    mut job: ProducerJob,
    shared: &Arc<Shared>,
    plan: &Arc<StreamPlan>,
) {
    let index = plan.domains()[job.frame.message.domain].steps[job.frame.cursor];
    let _span = job.callback.as_ref().map(StreamCallback::enter);
    let mut emitter = Emitter {
        shared,
        plan,
        index,
        emitted: 0,
        ports: BTreeSet::new(),
        publication_failed: false,
    };
    let failure = shared.state.lock().unwrap().failure.clone();
    let result = if let Some(error) = failure {
        Err(error)
    } else {
        if let Some(callback) = job.callback.as_mut() {
            callback.started();
        }
        catch_unwind(AssertUnwindSafe(|| {
            job.frame
                .context
                .with_node(plan.nodes()[index].definition_id.as_str(), |context| {
                    node.execute(job.inputs, context, &mut emitter)
                })
        }))
        .unwrap_or_else(|payload| {
            Err(NodeExecutionError::ExecutionFailed {
                message: panic_error(payload).to_string(),
            })
        })
        .context(ProducerSnafu {
            definition_id: plan.nodes()[index].definition_id.clone(),
        })
        .and_then(|()| {
            shared
                .state
                .lock()
                .unwrap()
                .failure
                .clone()
                .map_or(Ok(()), Err)
        })
    };
    if let Some(callback) = job.callback {
        match &result {
            Ok(()) => callback.succeeded(emitter.emitted, emitter.ports.into_iter().collect()),
            Err(error) => callback.failed(
                if emitter.publication_failed {
                    FailurePhase::Publication
                } else {
                    FailurePhase::Execution
                },
                error.to_string(),
            ),
        }
    }
    if result.is_err() {
        job.frame.context = ExecutionContext::default();
    }
    shared
        .state
        .lock()
        .unwrap()
        .completions
        .push_back(Completion {
            frame: job.frame,
            result,
            producer: true,
            detached: job.detached,
        });
    shared.changed.notify_all();
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

struct FailureWake(Weak<Shared>);
impl Wake for FailureWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(shared) = self.0.upgrade()
            && let Some(failure) = shared.cancellation.failure()
        {
            shared.fail(failure);
        }
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
            self.shared.fail(
                OutputSnafu {
                    message: "delivery was dropped before acknowledgement",
                }
                .build(),
            );
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
            observation.finish(
                StreamCounts::default(),
                Some(StreamFailure {
                    phase: error.phase().into(),
                    message: error.to_string(),
                    node: None,
                }),
            );
        }
        result
    }

    fn start_inner(self, options: StreamOptions) -> Result<StreamInstance, StreamError> {
        ensure!(
            options.snapshots.is_none(),
            PreparationSnafu {
                message: "snapshot capture is unsupported for streaming instances",
            }
        );
        if let Some(observation) = &options.observation {
            ensure!(
                observation.description().event_schema_version()
                    == mf_telemetry::STREAM_EVENT_SCHEMA_VERSION,
                PreparationSnafu {
                    message: "stream execution requires observation schema 4"
                }
            );
        }
        let (prepared, operator_states, mut resources) = self.into_parts();
        resources.extend(options.resources)?;
        prepared
            .input_schema()
            .validate(&options.arguments)
            .map_err(|error| StreamError::InputFailure {
                message: error.to_string(),
            })?;
        let cancellation = crate::StreamCancellation::default();
        let mut context = ExecutionContext::default();
        context.set_execution_resources(resources, cancellation.clone());
        if let Some(failure) = cancellation.failure() {
            return Err(failure);
        }
        context.set_workflow_arguments(options.arguments);
        context
            .bind_workflow_inputs(prepared.input_schema())
            .map_err(|error| StreamError::Preparation {
                message: error.to_string(),
            })?;
        if let Some(observation) = &options.observation {
            context.set_frame_observation(observation.startup_frame());
        }
        let domain_count = prepared.domains().len();
        let mut domains: Vec<_> = (0..domain_count).map(|_| Domain::default()).collect();
        domains[0].frame = Some(Frame {
            message: MessageId {
                domain: 0,
                sequence: 0,
            },
            context,
            cursor: 0,
        });
        domains[0].occupied = true;
        let operators = operator_states
            .into_iter()
            .map(|state| {
                state.map(|state| Operator {
                    executor: match state {
                        NodeExecution::Event(state) => OperatorExecutor::Event(state),
                        NodeExecution::Stream(node) => OperatorExecutor::Producer(Producer {
                            node: Some(node),
                            worker: None,
                            active: false,
                        }),
                        NodeExecution::Task(_) => unreachable!("tasks remain in the plan"),
                    },
                    deadline: None,
                    pending: VecDeque::new(),
                    closed: false,
                })
            })
            .collect();
        let plan = Arc::new(prepared);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                output: None,
                delivered: None,
                delivery_pending: false,
                completions: VecDeque::new(),
                domains,
                operators,
                sequences: vec![0; domain_count],
                active_workers: 0,
                failure: None,
                failure_node: None,
                done: false,
                summary: StreamSummary {
                    startup_frames: 1,
                    ..Default::default()
                },
            }),
            changed: Condvar::new(),
            execution: plan.execution().clone(),
            observation: options.observation,
            cancellation,
        });
        shared
            .cancellation
            .register_failure(Waker::from(Arc::new(FailureWake(Arc::downgrade(&shared)))));
        options
            .clock
            .register_waker(Waker::from(Arc::new(ClockWake(Arc::downgrade(&shared)))));
        let coordinator_shared = Arc::clone(&shared);
        let workers = start_workers(&shared, &plan)?;
        let coordinator = thread::Builder::new()
            .name("workflow-stream".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    coordinate(&coordinator_shared, &plan, options.clock.as_ref(), &workers)
                }));
                if let Err(payload) = result {
                    let mut state = coordinator_shared
                        .state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    coordinator_shared.state.clear_poison();
                    let failure = state
                        .failure
                        .get_or_insert_with(|| panic_error(payload))
                        .clone();
                    drop(state);
                    coordinator_shared.cancellation.cancel(failure);
                    coordinator_shared.changed.notify_all();
                }
                drop(workers);
                finish_instance(&coordinator_shared);
            })
            .context(ThreadSpawnSnafu {
                thread: "workflow-stream",
            })?;
        Ok(StreamInstance {
            shared,
            coordinator: Some(coordinator),
        })
    }
}

impl StreamInstance {
    pub fn cancellation(&self) -> crate::StreamCancellation {
        self.shared.cancellation.clone()
    }
    pub fn fail(&self, error: StreamError) {
        self.shared.fail(error);
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
            pending_frames: state
                .domains
                .iter()
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
            self.shared.fail(
                ExecutionSnafu {
                    message: "stream instance dropped before completion",
                }
                .build(),
            );
            let _ = coordinator.join();
        }
    }
}

fn start_workers(
    shared: &Arc<Shared>,
    plan: &Arc<StreamPlan>,
) -> Result<WorkerPool<Frame>, StreamError> {
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
    let shared = Arc::clone(shared);
    let plan = Arc::clone(plan);
    WorkerPool::new(count, move |mut frame: Frame| {
        let _context = frame
            .context
            .observation()
            .map(crate::RunObservation::enter);
        let steps = &plan.domains()[frame.message.domain].steps;
        let result = (|| -> Result<(), StreamError> {
            while let Some(&index) = steps.get(frame.cursor) {
                if plan.nodes()[index].node.is_none() {
                    break;
                }
                if let Some(error) = shared.state.lock().unwrap().failure.clone() {
                    return Err(error);
                }
                catch_unwind(AssertUnwindSafe(|| {
                    plan.execute_step(index, &mut frame.context)
                }))
                .map_err(panic_error)?
                .context(WorkflowSnafu {
                    message: frame.message,
                })?;
                frame.cursor += 1;
            }
            Ok(())
        })();
        if result.is_err() {
            frame.context = ExecutionContext::default();
        }
        shared
            .state
            .lock()
            .unwrap()
            .completions
            .push_back(Completion {
                frame,
                result,
                producer: false,
                detached: false,
            });
        shared.changed.notify_all();
    })
    .context(WorkerStartupSnafu)
}

fn coordinate(
    shared: &Arc<Shared>,
    plan: &Arc<StreamPlan>,
    clock: &dyn StreamClock,
    workers: &WorkerPool<Frame>,
) {
    let mut state = shared.state.lock().unwrap();
    loop {
        while let Some(mut completion) = state.completions.pop_front() {
            if !completion.producer {
                state.active_workers -= 1;
            }
            let domain = completion.frame.message.domain;
            if completion.producer {
                let index = plan.domains()[domain].steps[completion.frame.cursor];
                let operator = state.operators[index].as_mut().unwrap();
                if let OperatorExecutor::Producer(producer) = &mut operator.executor {
                    producer.active = false;
                }
                if completion.detached {
                    operator.closed = true;
                }
            }
            if let Err(error) = completion.result {
                if state.failure.is_none() {
                    let index = plan.domains()[domain].steps[completion.frame.cursor];
                    state.failure_node = Some(plan.nodes()[index].definition_id.to_string());
                }
                state.failure.get_or_insert(error);
            }
            if state.failure.is_none() && !completion.detached {
                if completion.producer {
                    completion.frame.cursor += 1;
                }
                state.domains[domain].frame = Some(completion.frame);
            }
        }
        if let Some(failure) = state.failure.clone() {
            drop(state);
            shared.cancellation.cancel(failure);
            shared.changed.notify_all();
            break;
        }
        if let Some(domain) = state.delivered.take() {
            release_domain(&mut state, domain);
        }
        match tick(&mut state, plan, workers, clock, shared) {
            Ok(progress) => {
                shared.changed.notify_all();
                if state.domains.iter().all(|domain| domain.closed) && state.output.is_none() {
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
            .filter_map(|domain| state.operators[domain.source.expect("emission domain")].as_ref())
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
}

fn tick(
    state: &mut State,
    plan: &Arc<StreamPlan>,
    workers: &WorkerPool<Frame>,
    clock: &dyn StreamClock,
    shared: &Arc<Shared>,
) -> Result<bool, StreamError> {
    let mut progress = false;
    for source in plan.domains().iter().skip(1) {
        let index = source.source.expect("emission domain");
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
                event_callback(shared, plan, index, None, StreamTrigger::Timer),
            )?;
            progress = true;
        }
    }
    for domain in 1..state.domains.len() {
        if state.domains[domain].occupied {
            continue;
        }
        let source = plan.domains()[domain].source.expect("emission domain");
        let Some(queued) = state.operators[source]
            .as_mut()
            .unwrap()
            .pending
            .pop_front()
        else {
            continue;
        };
        let mut context =
            ExecutionContext::for_message(&plan.nodes()[source], queued.emission.result).context(
                WorkflowSnafu {
                    message: queued.message,
                },
            )?;
        context.set_cancellation(shared.cancellation.clone());
        if let Some(observation) = &shared.observation {
            context.set_frame_observation(observation.frame(stream_message(queued.message)));
        }
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
                        .context(WorkflowSnafu {
                            message: frame.message,
                        })?
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
            let mut callback = event_callback(
                shared,
                plan,
                index,
                (domain != 0).then_some(frame.message),
                if domain == 0 {
                    StreamTrigger::Startup
                } else {
                    StreamTrigger::Input
                },
            );
            let _context = callback.as_ref().map(StreamCallback::enter);
            let inputs = frame
                .context
                .event_inputs(&plan.nodes()[index], plan.dependencies(index))
                .inspect_err(|error| {
                    if let Some(callback) = callback.take() {
                        callback.failed(FailurePhase::Dependency, error.to_string());
                    }
                    state.failure_node = Some(plan.nodes()[index].definition_id.to_string());
                })
                .context(WorkflowSnafu {
                    message: frame.message,
                })?;
            if let Some(inputs) = inputs {
                if let OperatorExecutor::Producer(producer) =
                    &mut state.operators[index].as_mut().unwrap().executor
                {
                    if domain == 0 {
                        let context = frame.context.fork_stream(
                            shared
                                .observation
                                .as_ref()
                                .map(StreamObservation::startup_frame),
                        );
                        let invocation = Frame {
                            message: frame.message,
                            context,
                            cursor: frame.cursor,
                        };
                        producer.submit(
                            ProducerJob {
                                frame: invocation,
                                inputs,
                                callback,
                                detached: true,
                            },
                            shared,
                            plan,
                        )?;
                        frame.cursor += 1;
                        state.domains[domain].frame = Some(frame);
                    } else {
                        producer.submit(
                            ProducerJob {
                                frame,
                                inputs,
                                callback,
                                detached: false,
                            },
                            shared,
                            plan,
                        )?;
                    }
                    progress = true;
                    continue;
                }
                invoke_event(
                    state,
                    plan,
                    index,
                    NodeEvent::Input(inputs),
                    EventContext {
                        now: clock.now(),
                        input: Some(&frame.context),
                    },
                    callback,
                )?;
            } else if let Some(callback) = callback {
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
            if domain == 0
                && matches!(
                    state.operators[index].as_ref().unwrap().executor,
                    OperatorExecutor::Producer(_)
                )
            {
                state.operators[index].as_mut().unwrap().closed = true;
            }
            frame.cursor += 1;
            state.domains[domain].frame = Some(frame);
            progress = true;
        } else if state.active_workers < workers.worker_count() {
            workers
                .try_submit(frame)
                // A failed send owns the frame; release its values instead of retaining them.
                .map_err(|_| {
                    ExecutionSnafu {
                        message: "worker queue unavailable",
                    }
                    .build()
                })?;
            state.active_workers += 1;
            progress = true;
        } else {
            state.domains[domain].frame = Some(frame);
        }
    }
    if !state.domains[0].occupied && !state.domains[0].closed {
        state.domains[0].closed = true;
        progress = true;
    }
    for domain in 0..state.domains.len() {
        if state.domains[domain].closed {
            for &index in &plan.domains()[domain].steps {
                if state.operators[index]
                    .as_ref()
                    .is_some_and(|operator| !operator.closed && !matches!(&operator.executor, OperatorExecutor::Producer(producer) if producer.active))
                {
                    if matches!(
                        state.operators[index].as_ref().unwrap().executor,
                        OperatorExecutor::Event(_)
                    ) {
                        invoke_event(
                            state,
                            plan,
                            index,
                            NodeEvent::UpstreamClosed,
                            EventContext {
                                now: clock.now(),
                                input: None,
                            },
                            event_callback(
                                shared,
                                plan,
                                index,
                                None,
                                StreamTrigger::UpstreamClosed,
                            ),
                        )?;
                    }
                    let operator = state.operators[index].as_mut().unwrap();
                    operator.closed = true;
                    operator.deadline = None;
                    progress = true;
                }
            }
        } else if domain > 0 && !state.domains[domain].occupied {
            let operator = state.operators[plan.domains()[domain].source.expect("emission domain")]
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
    mut callback: Option<StreamCallback>,
) -> Result<(), StreamError> {
    let _span = callback.as_ref().map(StreamCallback::enter);
    if let Some(callback) = callback.as_mut() {
        callback.started();
    }
    let mut phase = FailurePhase::Execution;
    let old_pending = state.operators[index].as_ref().unwrap().pending.len();
    let result = (|| -> Result<(usize, Vec<String>), StreamError> {
        let effects = catch_unwind(AssertUnwindSafe(|| {
            let OperatorExecutor::Event(node) =
                &mut state.operators[index].as_mut().unwrap().executor
            else {
                unreachable!("only event nodes receive callbacks");
            };
            node.on_event(event, &context)
        }))
        .map_err(panic_error)?
        .with_context(|_| EventSnafu {
            definition_id: plan.nodes()[index].definition_id.clone(),
        })?;
        phase = FailurePhase::Publication;
        let count = effects.emissions.len();
        let ports: std::collections::BTreeSet<_> = effects
            .emissions
            .iter()
            .flat_map(|emission| emission.result.outputs.keys().cloned())
            .collect();
        apply_effects(state, plan, index, effects, context.now)?;
        Ok((count, ports.into_iter().collect()))
    })();
    match result {
        Ok((count, ports)) => {
            if let Some(callback) = callback {
                let operator = state.operators[index].as_ref().unwrap();
                if let OperatorExecutor::Event(node) = &operator.executor
                    && let Some(items) = node.buffered_items()
                {
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

fn enqueue_emission(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    emission: EventEmission,
) -> Result<(), StreamError> {
    let domain = plan.output_domain(index);
    ExecutionContext::for_message(&plan.nodes()[index], emission.result.clone()).context(
        WorkflowSnafu {
            message: MessageId {
                domain,
                sequence: state.sequences[domain],
            },
        },
    )?;
    let sequence = take_sequence(&mut state.sequences[domain])?;
    take_sequence(&mut state.summary.emitted_messages)?;
    state.operators[index]
        .as_mut()
        .unwrap()
        .pending
        .push_back(QueuedEmission {
            message: MessageId { domain, sequence },
            emission,
        });
    Ok(())
}

fn apply_effects(
    state: &mut State,
    plan: &StreamPlan,
    index: usize,
    effects: EventEffects,
    now: Duration,
) -> Result<(), StreamError> {
    let operator = state.operators[index].as_mut().unwrap();
    ensure!(
        effects
            .emissions
            .len()
            .saturating_add(operator.pending.len())
            <= plan.execution().limits.max_pending_messages,
        ResourceSnafu {
            message: format!(
                "node `{}` emitted too many pending messages",
                plan.nodes()[index].definition_id
            ),
        }
    );
    for emission in effects.emissions {
        enqueue_emission(state, plan, index, emission)?;
    }
    let operator = state.operators[index].as_mut().unwrap();
    match effects.timer {
        TimerUpdate::Keep => {}
        TimerUpdate::Cancel => operator.deadline = None,
        TimerUpdate::Set(deadline) => {
            ensure!(
                deadline > now,
                ExecutionSnafu {
                    message: format!(
                        "node `{}` must request a future timer deadline",
                        plan.nodes()[index].definition_id
                    ),
                }
            );
            ensure!(
                Instant::now()
                    .checked_add(deadline.saturating_sub(now))
                    .is_some(),
                ResourceSnafu {
                    message: format!(
                        "node `{}` deadline exceeds the monotonic clock",
                        plan.nodes()[index].definition_id
                    ),
                }
            );
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

fn finish_observation(
    shared: &Shared,
    summary: StreamSummary,
    error: Option<StreamError>,
    node: Option<String>,
) {
    if let Some(observation) = &shared.observation {
        let failure = error.map(|error| StreamFailure {
            phase: error.phase().into(),
            message: error.to_string(),
            node,
        });
        observation.finish(
            StreamCounts {
                startup_frames: summary.startup_frames,
                emitted_messages: summary.emitted_messages,
                completed_frames: summary.completed_frames,
                delivered_outputs: summary.delivered_outputs,
                ..Default::default()
            },
            failure,
        );
    }
}

fn release_domain(state: &mut State, domain: usize) {
    state.domains[domain].occupied = false;
}

fn finish_instance(shared: &Shared) {
    let operators = {
        let mut state = shared.state.lock().unwrap();
        std::mem::take(&mut state.operators)
    };
    // Joining workers can run plugin destructors that call back into the instance.
    drop(operators);
    let mut state = shared.state.lock().unwrap();
    state.output = None;
    state.delivered = None;
    state.delivery_pending = false;
    state.completions.clear();
    for domain in &mut state.domains {
        *domain = Domain::default();
        domain.closed = true;
    }
    state.active_workers = 0;
    state.done = true;
    let summary = state.summary.clone();
    let failure = state.failure.clone();
    let node = state.failure_node.clone();
    drop(state);
    finish_observation(shared, summary, failure, node);
    shared.changed.notify_all();
}

fn take_sequence(sequence: &mut u64) -> Result<u64, StreamError> {
    let current = *sequence;
    *sequence = current.checked_add(1).context(ResourceSnafu {
        message: "sequence counter exhausted",
    })?;
    Ok(current)
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
    ExecutionSnafu { message }.build()
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
        struct Start;
        impl crate::TaskNode for Start {
            fn execute(
                &self,
                _: Inputs,
                _: &mut ExecutionContext,
            ) -> Result<NodeResult, NodeExecutionError> {
                Ok(crate::Outputs::from([("item".into(), serde_json::json!(1).into())]).into())
            }
        }
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
                limits: Default::default(),
            },
            vec![
                crate::FlowNode::new(
                    "start",
                    crate::PreparedNode::new(
                        Start,
                        crate::NodePorts {
                            inputs: Vec::new(),
                            outputs: vec![crate::PortSpec::new(
                                "item",
                                crate::ValueType::Int64,
                                true,
                            )],
                        },
                    ),
                ),
                crate::FlowNode::new(
                    "timer",
                    crate::PreparedNode::event(Timer, crate::NodeMetadata::default()),
                ),
            ],
            vec![
                Vec::new(),
                vec![crate::StreamDependency {
                    input: None,
                    source_node: "start".into(),
                    source_output: "item".into(),
                }],
            ],
            Vec::new(),
        )
        .unwrap();
        let instance = plan.start().unwrap();

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
