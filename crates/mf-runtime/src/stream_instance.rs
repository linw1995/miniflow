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
    error::Error,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard, Weak, mpsc},
    task::{Wake, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Snafu)]
pub enum StreamError {
    #[snafu(
        display("invalid stream runtime configuration: {message}"),
        visibility(pub)
    )]
    Configuration { message: String },
    #[snafu(display("could not claim stream stdio: {source}"), visibility(pub))]
    Stdio {
        #[snafu(source(from(std::io::Error, Arc::new)))]
        source: Arc<std::io::Error>,
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
    #[snafu(display("invalid stream input: {source}"), visibility(pub))]
    InputValidation {
        #[snafu(source(from(crate::WorkflowRunError, Arc::new)))]
        source: Arc<crate::WorkflowRunError>,
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
    #[snafu(display("stream execution failed: {message}"), visibility(pub))]
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
}

impl StreamError {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Configuration { .. }
            | Self::ThreadSpawn { .. }
            | Self::WorkerStartup { .. }
            | Self::Stdio { .. } => "preparation",
            Self::InputFailure { .. } | Self::InputRecord { .. } | Self::InputValidation { .. } => {
                "input"
            }
            Self::Output { .. } | Self::OutputWrite { .. } | Self::OutputEncode { .. } => "output",
            Self::Resource { .. } => "resource",
            Self::Producer { source, .. } | Self::Event { source, .. } => {
                if let NodeExecutionError::PluginFailed { source } = source.as_ref() {
                    let mut cause: &dyn Error = source.as_ref();
                    loop {
                        if let Some(error) = cause.downcast_ref::<Self>() {
                            return error.phase();
                        }
                        let Some(source) = cause.source() else {
                            break;
                        };
                        cause = source;
                    }
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
    pub stdin: Option<crate::TextInput>,
}
impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(MonotonicClock::default()),
            observation: None,
            snapshots: None,
            arguments: crate::WorkflowArguments::default(),
            stdin: None,
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
    execution_domains: Vec<ExecutionDomainState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecutionDomainState {
    Pending,
    Running,
    Complete,
}

impl Frame {
    fn new(
        message: MessageId,
        context: ExecutionContext,
        message_domain: usize,
        plan: &StreamPlan,
    ) -> Self {
        let mut execution_domains =
            vec![ExecutionDomainState::Complete; plan.execution_domains().len()];
        for &domain in plan.execution_domains_for_message(message_domain) {
            execution_domains[domain] = ExecutionDomainState::Pending;
        }
        Self {
            message,
            context,
            execution_domains,
        }
    }

    fn complete(&self, message_domain: usize, plan: &StreamPlan) -> bool {
        plan.execution_domains_for_message(message_domain)
            .iter()
            .all(|&domain| self.execution_domains[domain] == ExecutionDomainState::Complete)
    }
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
    message: MessageId,
    execution_domain: usize,
    context: ExecutionContext,
    detached: bool,
    inputs: Inputs,
    callback: Option<StreamCallback>,
}

struct DomainJob {
    message: MessageId,
    execution_domain: usize,
    context: ExecutionContext,
}

struct Completion {
    message: MessageId,
    execution_domain: usize,
    context: ExecutionContext,
    result: Result<(), StreamError>,
    failure_node: Option<String>,
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
    worker_limit: std::num::NonZeroUsize,
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
        Ok(self.admit(result)?)
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
        if self.worker.as_ref().unwrap().try_submit(job).is_err() {
            return ExecutionSnafu {
                message: "producer worker unavailable",
            }
            .fail();
        }
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
    let index = plan.execution_domain(job.execution_domain).nodes[0].index();
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
            node.execute(job.inputs, &mut job.context, &mut emitter)
        }))
        .unwrap_or_else(|payload| Err(panic_error(payload).into()))
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
        job.context = ExecutionContext::default();
    }
    let failure_node = result
        .is_err()
        .then(|| plan.nodes()[index].definition_id.to_string());
    shared
        .state
        .lock()
        .unwrap()
        .completions
        .push_back(Completion {
            message: job.message,
            execution_domain: job.execution_domain,
            context: job.context,
            result,
            failure_node,
            producer: true,
            detached: job.detached,
        });
    shared.changed.notify_all();
}

fn workflow_error_node(error: &crate::WorkflowRunError) -> String {
    match error {
        crate::WorkflowRunError::Dependency { definition_id, .. }
        | crate::WorkflowRunError::InputType { definition_id, .. }
        | crate::WorkflowRunError::OutputType { definition_id, .. }
        | crate::WorkflowRunError::OutputSelection { definition_id, .. }
        | crate::WorkflowRunError::Context { definition_id, .. }
        | crate::WorkflowRunError::NodeExecution { definition_id, .. } => definition_id.to_string(),
        crate::WorkflowRunError::WorkflowInputs { .. }
        | crate::WorkflowRunError::WorkerPool { .. } => "<workflow>".to_owned(),
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
        self.start_with_runtime_options(options, crate::RuntimeOptions::default())
    }

    pub fn start_with_runtime_options(
        self,
        options: StreamOptions,
        runtime_options: crate::RuntimeOptions,
    ) -> Result<StreamInstance, StreamError> {
        let observation = options.observation.clone();
        let result = self.start_inner(options, runtime_options);
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

    fn start_inner(
        self,
        options: StreamOptions,
        runtime_options: crate::RuntimeOptions,
    ) -> Result<StreamInstance, StreamError> {
        ensure!(
            options.snapshots.is_none(),
            ConfigurationSnafu {
                message: "snapshot capture is unsupported for streaming instances",
            }
        );
        if let Some(observation) = &options.observation {
            ensure!(
                observation.description().event_schema_version()
                    == mf_telemetry::STREAM_EVENT_SCHEMA_VERSION,
                ConfigurationSnafu {
                    message: "stream execution requires observation schema 4"
                }
            );
        }
        let (prepared, operator_states) = self.into_parts();
        let domain_limit = prepared
            .execution()
            .limits
            .workers
            .min(runtime_options.max_parallel_domains.get());
        let worker_limit = std::num::NonZeroUsize::new(domain_limit)
            .expect("validated stream worker limit is positive");
        let mut context = ExecutionContext::default();
        if let Some(input) = options.stdin {
            context.set_stdin(input);
        }
        let cancellation = context.cancellation();
        context.set_workflow_arguments(options.arguments);
        context.configure_worker_limit(worker_limit);
        context
            .bind_workflow_inputs(prepared.input_schema())
            .context(InputValidationSnafu)?;
        if let Some(observation) = &options.observation {
            context.set_frame_observation(observation.startup_frame());
        }
        let domain_count = prepared.message_sources().len();
        let mut domains: Vec<_> = (0..domain_count).map(|_| Domain::default()).collect();
        domains[0].frame = Some(Frame::new(
            MessageId {
                domain: 0,
                sequence: 0,
            },
            context,
            0,
            &prepared,
        ));
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
            worker_limit,
        });
        shared
            .cancellation
            .register(Waker::from(Arc::new(FailureWake(Arc::downgrade(&shared)))));
        options
            .clock
            .register_waker(Waker::from(Arc::new(ClockWake(Arc::downgrade(&shared)))));
        let coordinator_shared = Arc::clone(&shared);
        let workers = start_workers(&plan, runtime_options)?;
        if let Some(frame) = shared.state.lock().unwrap().domains[0].frame.as_mut() {
            frame.context.set_worker_handle(workers.handle());
        }
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
    plan: &Arc<StreamPlan>,
    runtime_options: crate::RuntimeOptions,
) -> Result<WorkerPool<crate::worker::WorkerJob>, StreamError> {
    let active_domains = plan
        .execution_domains()
        .domains()
        .iter()
        .filter(|domain| {
            domain
                .nodes
                .iter()
                .any(|node_id| plan.nodes()[node_id.index()].node.is_some())
        })
        .count();
    let count = plan
        .execution()
        .limits
        .workers
        .min(runtime_options.max_parallel_domains.get())
        .min(active_domains);
    WorkerPool::new(count, crate::worker::WorkerJob::run).context(WorkerStartupSnafu)
}

fn run_domain_job(mut job: DomainJob, shared: Arc<Shared>, plan: Arc<StreamPlan>) {
    let _context = job.context.observation().map(crate::RunObservation::enter);
    let domain = plan.execution_domain(job.execution_domain);
    let first_node = domain
        .nodes
        .first()
        .map(|node_id| plan.nodes()[node_id.index()].definition_id.to_string());
    let execution_result: (Result<(), StreamError>, Option<String>) =
        if let Some(error) = shared.state.lock().unwrap().failure.clone() {
            (Err(error), first_node.clone())
        } else {
            match catch_unwind(AssertUnwindSafe(|| {
                plan.execute_domain(job.execution_domain, &mut job.context)
            })) {
                Ok(result) => {
                    let failure_node = result.as_ref().err().map(workflow_error_node);
                    let result = result.context(WorkflowSnafu {
                        message: job.message,
                    });
                    (result, failure_node)
                }
                Err(payload) => (Err(panic_error(payload)), first_node.clone()),
            }
        };
    let (result, failure_node) = execution_result;
    if result.is_err() {
        job.context = ExecutionContext::default();
    }
    shared
        .state
        .lock()
        .unwrap()
        .completions
        .push_back(Completion {
            message: job.message,
            execution_domain: job.execution_domain,
            context: job.context,
            result,
            failure_node,
            producer: false,
            detached: false,
        });
    shared.changed.notify_all();
}

fn coordinate(
    shared: &Arc<Shared>,
    plan: &Arc<StreamPlan>,
    clock: &dyn StreamClock,
    workers: &WorkerPool<crate::worker::WorkerJob>,
) {
    let mut guard = shared.state.lock().unwrap();
    loop {
        while let Some(completion) = guard.completions.pop_front() {
            if !completion.producer {
                guard.active_workers -= 1;
            }
            let message_domain = completion.message.domain;
            let node = plan.execution_domain(completion.execution_domain).nodes[0].index();
            if completion.producer {
                let operator = guard.operators[node].as_mut().unwrap();
                if let OperatorExecutor::Producer(producer) = &mut operator.executor {
                    producer.active = false;
                }
                if completion.detached {
                    operator.closed = true;
                }
            }
            if let Err(error) = completion.result {
                if guard.failure.is_none() {
                    guard.failure_node = completion
                        .failure_node
                        .or_else(|| Some(plan.nodes()[node].definition_id.to_string()));
                }
                guard.failure.get_or_insert(error);
            }
            if guard.failure.is_none()
                && !completion.detached
                && let Some(frame) = guard.domains[message_domain].frame.as_mut()
            {
                frame.context.merge_domain_outputs(
                    &completion.context,
                    plan.execution_domain(completion.execution_domain)
                        .nodes
                        .iter()
                        .map(|node_id| &plan.nodes()[node_id.index()]),
                );
                frame.execution_domains[completion.execution_domain] =
                    ExecutionDomainState::Complete;
            }
        }
        if let Err(failure) = check_failure(&mut guard, shared) {
            drop(guard);
            shared.cancellation.cancel(failure);
            shared.changed.notify_all();
            break;
        }
        if let Some(domain) = guard.delivered.take() {
            release_domain(&mut guard, domain);
        }
        match tick(guard, plan, workers, clock, shared) {
            Ok((next_guard, progress)) => {
                guard = next_guard;
                shared.changed.notify_all();
                if guard.domains.iter().all(|domain| domain.closed) && guard.output.is_none() {
                    break;
                }
                if progress {
                    drop(guard);
                    thread::yield_now();
                    guard = shared.state.lock().unwrap();
                    continue;
                }
            }
            Err(error) => {
                guard = shared.state.lock().unwrap();
                guard.failure.get_or_insert(error);
                shared.changed.notify_all();
                continue;
            }
        }
        let wait = plan
            .message_sources()
            .iter()
            .skip(1)
            .filter_map(|domain| guard.operators[domain.expect("emission domain")].as_ref())
            .filter(|operator| {
                operator.pending.len() < plan.execution().limits.max_pending_messages
            })
            .filter_map(|operator| {
                operator
                    .deadline
                    .map(|deadline| deadline.saturating_sub(clock.now()))
            })
            .min();
        guard = match wait {
            Some(wait) => shared.changed.wait_timeout(guard, wait).unwrap().0,
            None => shared.changed.wait(guard).unwrap(),
        };
    }
}

fn tick<'a>(
    mut guard: MutexGuard<'a, State>,
    plan: &Arc<StreamPlan>,
    workers: &WorkerPool<crate::worker::WorkerJob>,
    clock: &dyn StreamClock,
    shared: &'a Arc<Shared>,
) -> Result<(MutexGuard<'a, State>, bool), StreamError> {
    let mut progress = false;
    for source in plan.message_sources().iter().skip(1) {
        let index = source.expect("emission domain");
        let due = guard.operators[index]
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
            guard.operators[index].as_mut().unwrap().deadline = None;
            let callback;
            (guard, callback) =
                event_callback(guard, shared, plan, index, None, StreamTrigger::Timer)?;
            guard = invoke_event(
                guard,
                shared,
                plan,
                index,
                NodeEvent::Timer,
                EventContext {
                    now: clock.now(),
                    input: None,
                },
                callback,
            )?;
            progress = true;
        }
    }
    for domain in 1..guard.domains.len() {
        if guard.domains[domain].occupied {
            continue;
        }
        let source = plan.message_sources()[domain].expect("emission domain");
        let Some(queued) = guard.operators[source]
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
        context.configure_worker_limit(shared.worker_limit);
        context.set_worker_handle(workers.handle());
        if let Some(observation) = &shared.observation {
            context.set_frame_observation(observation.frame(stream_message(queued.message)));
        }
        guard.domains[domain].frame = Some(Frame::new(queued.message, context, domain, plan));
        guard.domains[domain].occupied = true;
        progress = true;
    }
    for message_domain in 0..guard.domains.len() {
        let Some(mut frame) = guard.domains[message_domain].frame.take() else {
            continue;
        };
        if frame.complete(message_domain, plan) {
            take_sequence(&mut guard.summary.completed_frames)?;
            if plan.selected_domain() == Some(message_domain) {
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
                        outputs.insert(output.name.clone().into_owned(), value);
                    }
                }
                guard.output = Some(StreamOutput {
                    message: frame.message,
                    outputs,
                });
            } else {
                release_domain(&mut guard, message_domain);
            }
            progress = true;
            continue;
        }

        let ready_domains = plan.execution_domains_for_message(message_domain).to_vec();
        for execution_domain in ready_domains {
            if frame.execution_domains[execution_domain] != ExecutionDomainState::Pending {
                continue;
            }
            let execution = plan.execution_domain(execution_domain);
            if execution.predecessors.iter().any(|predecessor| {
                frame.execution_domains[*predecessor] != ExecutionDomainState::Complete
            }) {
                continue;
            }
            let index = execution.nodes[0].index();
            if let Some(operator) = guard.operators[index].as_ref() {
                if !operator.pending.is_empty() {
                    continue;
                }
                let is_producer = matches!(&operator.executor, OperatorExecutor::Producer(_));
                let _ = operator;
                let callback;
                (guard, callback) = event_callback(
                    guard,
                    shared,
                    plan,
                    index,
                    (message_domain != 0).then_some(frame.message),
                    if message_domain == 0 {
                        StreamTrigger::Startup
                    } else {
                        StreamTrigger::Input
                    },
                )?;
                let _context = callback.as_ref().map(StreamCallback::enter);
                let inputs = match frame
                    .context
                    .event_inputs(&plan.nodes()[index], plan.dependencies(index))
                {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        guard.failure_node = Some(plan.nodes()[index].definition_id.to_string());
                        drop(guard);
                        if let Some(callback) = callback {
                            callback.failed(FailurePhase::Dependency, error.to_string());
                        }
                        guard = shared.state.lock().unwrap();
                        check_failure(&mut guard, shared)?;
                        return Err(error).context(WorkflowSnafu {
                            message: frame.message,
                        });
                    }
                };
                if let Some(inputs) = inputs {
                    if is_producer {
                        let observation = shared.observation.as_ref().map(|observation| {
                            if message_domain == 0 {
                                observation.startup_frame()
                            } else {
                                observation.frame(stream_message(frame.message))
                            }
                        });
                        let context = frame.context.fork_stream(observation);
                        let detached = message_domain == 0;
                        let operator = guard.operators[index].as_mut().unwrap();
                        let OperatorExecutor::Producer(producer) = &mut operator.executor else {
                            unreachable!("producer kind was checked above")
                        };
                        producer.submit(
                            ProducerJob {
                                message: frame.message,
                                execution_domain,
                                context,
                                inputs,
                                callback,
                                detached,
                            },
                            shared,
                            plan,
                        )?;
                        frame.execution_domains[execution_domain] = if detached {
                            ExecutionDomainState::Complete
                        } else {
                            ExecutionDomainState::Running
                        };
                        progress = true;
                        continue;
                    }
                    guard = invoke_event(
                        guard,
                        shared,
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
                            source_node: dependency.source_node.clone().into_owned(),
                            source_output: dependency.source_output.clone().into_owned(),
                        })
                        .collect();
                    drop(guard);
                    callback.skipped(causes.into_iter().collect());
                    guard = shared.state.lock().unwrap();
                    check_failure(&mut guard, shared)?;
                }
                if is_producer && message_domain == 0 {
                    guard.operators[index].as_mut().unwrap().closed = true;
                }
                frame.execution_domains[execution_domain] = ExecutionDomainState::Complete;
                progress = true;
            } else if guard.active_workers < workers.worker_count() {
                let observation = shared.observation.as_ref().map(|observation| {
                    if frame.message.domain == 0 {
                        observation.startup_frame()
                    } else {
                        observation.frame(stream_message(frame.message))
                    }
                });
                let visible_outputs = plan.visible_outputs_for_execution(execution_domain);
                let context = frame
                    .context
                    .fork_domain_with_observation_and_visible_outputs(
                        observation,
                        &visible_outputs,
                    );
                let job_shared = Arc::clone(shared);
                let job_plan = Arc::clone(plan);
                let job = DomainJob {
                    message: frame.message,
                    execution_domain,
                    context,
                };
                match workers.try_submit(crate::worker::WorkerJob::new(move || {
                    run_domain_job(job, job_shared, job_plan)
                })) {
                    Ok(()) => {
                        frame.execution_domains[execution_domain] = ExecutionDomainState::Running;
                        guard.active_workers += 1;
                        progress = true;
                    }
                    Err(mpsc::TrySendError::Full(_)) => {}
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        return ExecutionSnafu {
                            message: "domain worker queue unavailable",
                        }
                        .fail();
                    }
                }
            }
        }

        if frame.complete(message_domain, plan) {
            take_sequence(&mut guard.summary.completed_frames)?;
            if plan.selected_domain() == Some(message_domain) {
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
                        outputs.insert(output.name.clone().into_owned(), value);
                    }
                }
                guard.output = Some(StreamOutput {
                    message: frame.message,
                    outputs,
                });
            } else {
                release_domain(&mut guard, message_domain);
            }
        } else {
            guard.domains[message_domain].frame = Some(frame);
        }
    }
    if !guard.domains[0].occupied && !guard.domains[0].closed {
        guard.domains[0].closed = true;
        progress = true;
    }
    for domain in 0..guard.domains.len() {
        if guard.domains[domain].closed {
            for &index in plan
                .execution_domains_for_message(domain)
                .iter()
                .flat_map(|&id| plan.execution_domain(id).positions.iter())
            {
                if guard.operators[index]
                    .as_ref()
                    .is_some_and(|operator| !operator.closed && !matches!(&operator.executor, OperatorExecutor::Producer(producer) if producer.active))
                {
                    if matches!(
                        guard.operators[index].as_ref().unwrap().executor,
                        OperatorExecutor::Event(_)
                    ) {
                        let callback;
                        (guard, callback) = event_callback(
                            guard,
                            shared,
                            plan,
                            index,
                            None,
                            StreamTrigger::UpstreamClosed,
                        )?;
                        guard = invoke_event(
                            guard,
                            shared,
                            plan,
                            index,
                            NodeEvent::UpstreamClosed,
                            EventContext {
                                now: clock.now(),
                                input: None,
                            },
                            callback,
                        )?;
                    }
                    let operator = guard.operators[index].as_mut().unwrap();
                    operator.closed = true;
                    operator.deadline = None;
                    progress = true;
                }
            }
        } else if domain > 0 && !guard.domains[domain].occupied {
            let operator = guard.operators
                [plan.message_sources()[domain].expect("emission domain")]
            .as_ref()
            .unwrap();
            if operator.closed && operator.pending.is_empty() {
                guard.domains[domain].closed = true;
                progress = true;
            }
        }
    }
    Ok((guard, progress))
}

fn event_callback<'a>(
    guard: MutexGuard<'a, State>,
    shared: &'a Shared,
    plan: &StreamPlan,
    index: usize,
    message: Option<MessageId>,
    trigger: StreamTrigger,
) -> Result<(MutexGuard<'a, State>, Option<StreamCallback>), StreamError> {
    let Some(observation) = &shared.observation else {
        return Ok((guard, None));
    };
    // Span creation synchronously invokes processors that may reenter cancellation.
    drop(guard);
    let callback = observation.callback(
        plan.nodes()[index].definition_id.as_str(),
        message.map(stream_message),
        trigger,
    );
    let mut guard = shared.state.lock().unwrap();
    if let Err(error) = check_failure(&mut guard, shared) {
        drop(guard);
        if let Some(callback) = callback {
            callback.failed(FailurePhase::Dependency, error.to_string());
        }
        return Err(error);
    }
    Ok((guard, callback))
}

fn check_failure(state: &mut State, shared: &Shared) -> Result<(), StreamError> {
    // Cancellation may be recorded before its waker acquires the scheduler lock.
    if let Some(error) = state
        .failure
        .clone()
        .or_else(|| shared.cancellation.failure())
    {
        state.failure.get_or_insert(error.clone());
        return Err(error);
    }
    Ok(())
}

fn invoke_event<'a>(
    mut guard: MutexGuard<'a, State>,
    shared: &'a Shared,
    plan: &StreamPlan,
    index: usize,
    event: NodeEvent,
    context: EventContext<'_>,
    mut callback: Option<StreamCallback>,
) -> Result<MutexGuard<'a, State>, StreamError> {
    let _span = callback.as_ref().map(StreamCallback::enter);
    // Only the coordinator accesses event operators; workers retain their own producer slots.
    let mut operator = guard.operators[index].take().unwrap();
    drop(guard);
    if let Some(callback) = callback.as_mut() {
        callback.started();
    }
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let OperatorExecutor::Event(node) = &mut operator.executor else {
            unreachable!("only event nodes receive callbacks");
        };
        let effects = node.on_event(event, &context)?;
        let buffered = callback.as_ref().and_then(|_| node.buffered_items());
        let ports: BTreeSet<_> = effects
            .emissions
            .iter()
            .flat_map(|emission| emission.result.outputs.keys().cloned())
            .collect();
        Ok((effects, buffered, ports.into_iter().collect()))
    }))
    .map_err(panic_error)
    .and_then(|result| {
        result.with_context(|_| EventSnafu {
            definition_id: plan.nodes()[index].definition_id.clone(),
        })
    });
    let mut guard = shared.state.lock().unwrap();
    guard.operators[index] = Some(operator);
    if let Err(error) = check_failure(&mut guard, shared) {
        drop(guard);
        drop(execution);
        if let Some(callback) = callback {
            callback.failed(FailurePhase::Execution, error.to_string());
        }
        return Err(error);
    }

    let mut phase = FailurePhase::Execution;
    let old_pending = guard.operators[index].as_ref().unwrap().pending.len();
    let result = (|| -> Result<_, StreamError> {
        let (effects, buffered, ports) = execution?;
        phase = FailurePhase::Publication;
        let count = effects.emissions.len();
        apply_effects(&mut guard, plan, index, effects, context.now)?;
        let flushed: Vec<_> = guard.operators[index]
            .as_ref()
            .unwrap()
            .pending
            .iter()
            .skip(old_pending)
            .filter_map(|emitted| {
                let batch = emitted.emission.batch?;
                let reason = match batch.reason {
                    crate::FlushReason::SizeExceed => "size_exceed",
                    crate::FlushReason::TimeoutExceed => "timeout_exceed",
                    crate::FlushReason::UpstreamClosed => "upstream_closed",
                };
                Some((stream_message(emitted.message), batch.item_count, reason))
            })
            .collect();
        Ok((count, ports, buffered, flushed))
    })();
    match result {
        Ok((count, ports, buffered, flushed)) => {
            if let Some(callback) = callback {
                drop(guard);
                if let Some(items) = buffered {
                    callback.buffered(items);
                }
                for (message, count, reason) in flushed {
                    callback.flushed(message, count, reason);
                }
                callback.succeeded(count, ports);
                guard = shared.state.lock().unwrap();
            }
            check_failure(&mut guard, shared)?;
            Ok(guard)
        }
        Err(error) => {
            guard.failure_node = Some(plan.nodes()[index].definition_id.to_string());
            guard.failure.get_or_insert(error.clone());
            drop(guard);
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
        let plan = PreparedStream::from_plan(
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
                vec![crate::FlowDependency {
                    input: None,
                    source_node: "start".into(),
                    source_output: "item".into(),
                }],
            ]
            .into_iter()
            .map(std::borrow::Cow::Owned)
            .collect::<Vec<_>>()
            .into(),
            crate::MessageDomains::from_parts(
                vec![None, Some(1)].into(),
                vec![0, 1].into(),
                None,
                crate::ExecutionDomains::from_parts(
                    vec![
                        crate::ExecutionDomain {
                            id: 0,
                            nodes: vec![crate::NodeId::new(0)].into(),
                            positions: vec![0].into(),
                            predecessors: vec![].into(),
                            successors: vec![1].into(),
                            first_position: 0,
                        },
                        crate::ExecutionDomain {
                            id: 1,
                            nodes: vec![crate::NodeId::new(1)].into(),
                            positions: vec![1].into(),
                            predecessors: vec![0].into(),
                            successors: vec![].into(),
                            first_position: 1,
                        },
                    ]
                    .into(),
                    vec![
                        std::borrow::Cow::Owned(vec![]),
                        std::borrow::Cow::Owned(vec![0]),
                    ]
                    .into(),
                ),
                vec![
                    std::borrow::Cow::Owned(vec![0, 1]),
                    std::borrow::Cow::Owned(vec![]),
                ]
                .into(),
                vec![0, 0].into(),
            ),
            Vec::new().into(),
            Default::default(),
        );
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
