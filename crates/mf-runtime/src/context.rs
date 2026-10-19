use crate::ValueRef as Value;
use crate::node::LoopVariableTypeSnafu;
use crate::runner::{ContextSnafu, NodeExecutionSnafu, OutputSelectionSnafu, OutputTypeSnafu};
use crate::worker::{RuntimeWorkerHandle, WorkerJob, WorkerPool};
use crate::{FlowNode, Inputs, NodeExecutionError, Outputs, WorkflowRunError, output_id};
use mf_telemetry::{
    event::{FailurePhase, LoopPathEntry, LoopSummary, SkipCause},
    observation::{BodyNodeObservation, BodyObservation, NodeObservation, RunObservation},
};
use snafu::ResultExt;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Debug)]
pub struct ExecutionScope {
    node_id: String,
    source_id: String,
    variables: Outputs,
    types: BTreeMap<String, crate::ValueType>,
    index: usize,
    exit_requested: bool,
    visited_steps: Arc<AtomicUsize>,
}

impl ExecutionScope {
    pub fn new(
        node_id: impl Into<String>,
        source_id: impl Into<String>,
        index: usize,
        variables: Outputs,
        types: BTreeMap<String, crate::ValueType>,
    ) -> Result<Self, NodeExecutionError> {
        i64::try_from(index).map_err(|_| NodeExecutionError::ExecutionFailed {
            message: "scope index exceeds the signed 64-bit range".into(),
        })?;
        Ok(Self {
            node_id: node_id.into(),
            source_id: source_id.into(),
            variables,
            types,
            index,
            exit_requested: false,
            visited_steps: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[derive(Clone, Debug)]
struct StepBudget(Arc<AtomicUsize>);

impl StepBudget {
    fn new(remaining: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(remaining)))
    }

    fn reserve(&self, id: &str) -> Result<(), WorkflowRunError> {
        match self
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            }) {
            Ok(_) => Ok(()),
            Err(_) => ContextSnafu {
                definition_id: crate::DefinitionId::from(id),
                message: format!(
                    "scheduled-step budget of {} exhausted",
                    crate::MAX_SCHEDULED_STEPS
                ),
            }
            .fail(),
        }
    }
}

struct ScopeGuard<'a> {
    context: &'a mut ExecutionContext,
    parent_outputs: BTreeMap<String, Option<Value>>,
    parent_typed_outputs: BTreeSet<String>,
    parent_exit_cutoff: Arc<AtomicUsize>,
    depth: usize,
}

impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        self.context.scopes.truncate(self.depth);
        self.context.outputs = std::mem::take(&mut self.parent_outputs);
        self.context.typed_outputs = std::mem::take(&mut self.parent_typed_outputs);
        self.context.scope_exit_cutoff = self.parent_exit_cutoff.clone();
        self.context.pending_loop_write = None;
    }
}

struct NodeInvocationGuard<'a> {
    context: &'a mut ExecutionContext,
    previous: Option<crate::DefinitionId>,
}

impl Drop for NodeInvocationGuard<'_> {
    fn drop(&mut self) {
        self.context.current_node = self.previous.take();
    }
}

/// Dynamic task results retain the map-based execution contract.
pub type NodeResult = TypedNodeResult<Outputs>;

/// Output fields and execution control metadata returned by a typed task.
#[derive(Clone, Debug, Default)]
pub struct TypedNodeResult<O> {
    pub outputs: O,
    pub skipped: BTreeSet<String>,
    pub loop_summary: Option<LoopSummary>,
}

/// Dynamic publication or validated presence evidence for a generated internal result.
pub enum GeneratedNodeResult {
    Dynamic(NodeResult),
    Typed {
        produced: Vec<&'static str>,
        skipped: BTreeSet<String>,
        loop_summary: Option<LoopSummary>,
    },
}

impl GeneratedNodeResult {
    pub fn typed<O: crate::TypedNodeValue>(
        result: &TypedNodeResult<O>,
    ) -> Result<Self, NodeExecutionError> {
        let produced = result
            .outputs
            .validate_typed()
            .context(crate::node::OutputEncodeSnafu)?;
        Ok(Self::Typed {
            produced,
            skipped: result.skipped.clone(),
            loop_summary: result.loop_summary.clone(),
        })
    }

    pub fn encoded<O: crate::NodeOutputs>(
        result: TypedNodeResult<O>,
    ) -> Result<Self, NodeExecutionError> {
        Ok(Self::Dynamic(crate::encode_typed_result(result)?))
    }

    fn dynamic(&self) -> Option<&NodeResult> {
        match self {
            Self::Dynamic(result) => Some(result),
            Self::Typed { .. } => None,
        }
    }

    fn produced_names(&self) -> Vec<String> {
        let mut names: Vec<_> = match self {
            Self::Dynamic(result) => result.outputs.keys().cloned().collect(),
            Self::Typed { produced, .. } => {
                produced.iter().map(|name| (*name).to_owned()).collect()
            }
        };
        names.sort();
        names
    }

    fn skipped(&self) -> &BTreeSet<String> {
        match self {
            Self::Dynamic(result) => &result.skipped,
            Self::Typed { skipped, .. } => skipped,
        }
    }

    fn loop_summary(&self) -> Option<&LoopSummary> {
        match self {
            Self::Dynamic(result) => result.loop_summary.as_ref(),
            Self::Typed { loop_summary, .. } => loop_summary.as_ref(),
        }
    }
}

impl<O> From<O> for TypedNodeResult<O> {
    fn from(outputs: O) -> Self {
        Self {
            outputs,
            skipped: BTreeSet::new(),
            loop_summary: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextValue<'a> {
    Value(&'a Value),
    Skipped,
}

/// Completed output values for one run. Nodes receive an immutable reference.
#[derive(Debug)]
pub struct ExecutionContext {
    // A present None is an explicit skip; an absent key is a missing output.
    outputs: BTreeMap<String, Option<Value>>,
    typed_outputs: BTreeSet<String>,
    observation: Option<RunObservation>,
    body_observation: Option<BodyObservation>,
    scopes: Vec<ExecutionScope>,
    pending_loop_write: Option<(String, Value)>,
    current_position: Option<usize>,
    current_node: Option<crate::DefinitionId>,
    scope_exit_cutoff: Arc<AtomicUsize>,
    remaining_steps: StepBudget,
    snapshots: Option<crate::SnapshotRecorder>,
    scope_prefix: Vec<LoopPathEntry>,
    inputs: crate::ExecutionInputs,
    stdin: Option<std::sync::Arc<std::sync::Mutex<crate::TextInput>>>,
    cancellation: crate::StreamCancellation,
    worker_handle: Option<RuntimeWorkerHandle>,
    worker_limit: NonZeroUsize,
    #[cfg(feature = "async")]
    future_executor: Arc<crate::future_execution::FutureExecutor>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            outputs: BTreeMap::new(),
            typed_outputs: BTreeSet::new(),
            observation: None,
            body_observation: None,
            scopes: Vec::new(),
            pending_loop_write: None,
            current_position: None,
            current_node: None,
            scope_exit_cutoff: Arc::new(AtomicUsize::new(usize::MAX)),
            remaining_steps: StepBudget::new(crate::MAX_SCHEDULED_STEPS),
            snapshots: None,
            scope_prefix: Vec::new(),
            inputs: crate::ExecutionInputs::default(),
            stdin: None,
            cancellation: crate::StreamCancellation::default(),
            worker_handle: None,
            worker_limit: crate::RuntimeOptions::default().max_parallel_domains,
            #[cfg(feature = "async")]
            future_executor: Arc::default(),
        }
    }
}

impl ExecutionContext {
    /// Identifies the executing node independently of optional observation.
    pub fn current_node(&self) -> Option<&str> {
        self.current_node.as_ref().map(crate::DefinitionId::as_str)
    }

    /// Checks an operation's limits between synchronous setup or conversion phases.
    #[cfg(feature = "async")]
    pub fn check_future_limits(
        &self,
        deadline: Option<std::time::Instant>,
    ) -> Result<(), crate::FutureExecutionError> {
        crate::future_execution::check_limits(&self.cancellation, deadline)
    }

    /// Drives an owned operation on the calling synchronous thread.
    /// Async hosts must invoke synchronous workflow APIs on a blocking thread.
    #[cfg(feature = "async")]
    pub fn execute_future<F: std::future::Future>(
        &self,
        future: F,
        deadline: Option<std::time::Instant>,
    ) -> Result<F::Output, crate::FutureExecutionError> {
        self.future_executor.execute(
            future,
            &self.cancellation,
            deadline,
            self.worker_limit.get(),
        )
    }

    pub fn set_stdin(&mut self, input: crate::TextInput) {
        self.stdin = Some(std::sync::Arc::new(std::sync::Mutex::new(input)));
    }

    pub fn set_cancellation(&mut self, cancellation: crate::StreamCancellation) {
        self.cancellation = cancellation;
    }
    pub fn cancellation(&self) -> crate::StreamCancellation {
        self.cancellation.clone()
    }

    pub fn stdin_line(&self) -> Result<Option<String>, crate::StreamError> {
        let input = self.stdin.as_ref().ok_or_else(|| {
            crate::stream_instance::ExecutionSnafu {
                message: "stdin is unavailable".to_owned(),
            }
            .build()
        })?;
        input.lock().unwrap().next_line(&self.cancellation)
    }

    pub fn fork_stream(&self, observation: Option<RunObservation>) -> Self {
        Self {
            outputs: self.outputs.clone(),
            typed_outputs: self.typed_outputs.clone(),
            remaining_steps: self.remaining_steps.clone(),
            stdin: self.stdin.clone(),
            cancellation: self.cancellation.clone(),
            observation,
            body_observation: None,
            scopes: Vec::new(),
            pending_loop_write: None,
            current_position: None,
            current_node: None,
            scope_exit_cutoff: self.scope_exit_cutoff.clone(),
            snapshots: self.snapshots.clone(),
            scope_prefix: self.scope_prefix.clone(),
            inputs: crate::ExecutionInputs::default(),
            worker_handle: self.worker_handle.clone(),
            worker_limit: self.worker_limit,
            #[cfg(feature = "async")]
            future_executor: self.future_executor.clone(),
        }
    }

    pub fn set_workflow_arguments(&mut self, arguments: crate::WorkflowArguments) {
        self.inputs.set_arguments(arguments);
    }

    pub(super) fn initialize_target(
        &mut self,
        target: &crate::NodeTarget,
        inputs: crate::WorkflowArguments,
        context: crate::WorkflowArguments,
    ) -> Result<(), WorkflowRunError> {
        snafu::ensure!(
            self.scopes.is_empty(),
            ContextSnafu {
                definition_id: target.id.as_str(),
                message: "isolated execution requires a top-level context",
            }
        );
        target.validate_context(&context)?;
        self.set_workflow_arguments(inputs);
        self.bind_workflow_inputs(&target.inputs)?;
        if self.inputs.stdin_owner().is_none() {
            self.stdin = None;
        }
        self.outputs = context
            .0
            .into_iter()
            .map(|(id, value)| (id, Some(value)))
            .collect();
        self.typed_outputs.clear();
        self.remaining_steps = StepBudget::new(crate::MAX_SCHEDULED_STEPS);
        self.current_position = None;
        self.pending_loop_write = None;
        self.scope_exit_cutoff = Arc::new(AtomicUsize::new(usize::MAX));
        Ok(())
    }

    pub(super) fn target_outputs<N>(
        &mut self,
        node: &FlowNode<N>,
    ) -> Result<Outputs, WorkflowRunError> {
        let mut outputs = Outputs::new();
        for port in &node.metadata.ports.outputs {
            let id = node.definition_id.as_str();
            if !port.required && !self.outputs.contains_key(&output_id(id, &port.name)) {
                continue;
            }
            if let Some(value) = self.select_output(&port.name, id, &port.name, !port.required)? {
                outputs.insert(port.name.to_string(), value);
            }
        }
        Ok(outputs)
    }

    pub fn bind_workflow_inputs(
        &mut self,
        schema: &crate::WorkflowInputSchema,
    ) -> Result<(), WorkflowRunError> {
        self.inputs
            .bind(schema, |name| std::env::var_os(name), self.stdin.is_some())?;
        Ok(())
    }

    pub(super) fn set_frame_observation(&mut self, observation: RunObservation) {
        self.observation = Some(observation);
    }

    pub fn configure_worker_limit(&mut self, limit: NonZeroUsize) {
        self.worker_limit = limit;
    }

    pub fn worker_limit(&self) -> NonZeroUsize {
        self.worker_limit
    }

    pub fn set_worker_handle(&mut self, handle: RuntimeWorkerHandle) {
        self.worker_handle = Some(handle);
    }

    pub fn worker_handle(&self) -> Option<RuntimeWorkerHandle> {
        self.worker_handle
            .as_ref()
            .filter(|handle| handle.is_active())
            .cloned()
    }

    /// Runs independent synchronous jobs through the active runtime worker pool.
    ///
    /// The builder receives the context after a temporary pool has been attached
    /// for direct node invocations. `max_jobs` bounds the number of jobs prepared
    /// by the builder and limits any temporary pool to the same runtime setting.
    pub fn run_parallel<T, F>(
        &mut self,
        max_jobs: usize,
        build_jobs: impl FnOnce(&Self) -> Vec<F>,
    ) -> Result<Vec<T>, crate::WorkerPoolError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        if max_jobs == 0 {
            return Ok(Vec::new());
        }
        if let Some(handle) = self.worker_handle() {
            return Ok(handle.run_parallel(build_jobs(self)));
        }

        let worker_count = max_jobs.min(self.worker_limit.get());
        let workers = WorkerPool::new(worker_count, WorkerJob::run)?;
        let handle = workers.handle();
        self.worker_handle = Some(handle.clone());
        let jobs = build_jobs(self);
        let result = handle.run_parallel(jobs);
        self.worker_handle = None;
        Ok(result)
    }

    pub fn replace_execution_position(&mut self, position: Option<usize>) -> Option<usize> {
        std::mem::replace(&mut self.current_position, position)
    }

    pub fn scope_exit_cutoff(&self) -> usize {
        self.scope_exit_cutoff.load(Ordering::Acquire)
    }

    pub fn for_message<N>(
        source: &FlowNode<N>,
        result: NodeResult,
    ) -> Result<Self, WorkflowRunError> {
        let mut context = Self::default();
        context.publish(source, Some(result))?;
        Ok(context)
    }

    pub fn fork_domain_with_visible_outputs(&self, visible_outputs: &BTreeSet<String>) -> Self {
        self.fork_domain_with_observation_and_visible_outputs(
            self.observation.clone(),
            visible_outputs,
        )
    }

    pub fn fork_domain_with_observation_and_visible_outputs(
        &self,
        observation: Option<RunObservation>,
        visible_outputs: &BTreeSet<String>,
    ) -> Self {
        let mut context = self.fork_domain_with_observation(observation);
        context
            .outputs
            .retain(|output, _| visible_outputs.contains(output));
        context
    }

    pub fn fork_domain_with_observation(&self, observation: Option<RunObservation>) -> Self {
        Self {
            outputs: self.outputs.clone(),
            typed_outputs: self.typed_outputs.clone(),
            observation,
            body_observation: self.body_observation.clone(),
            scopes: self.scopes.clone(),
            pending_loop_write: None,
            current_position: None,
            current_node: None,
            scope_exit_cutoff: self.scope_exit_cutoff.clone(),
            remaining_steps: self.remaining_steps.clone(),
            snapshots: self.snapshots.clone(),
            scope_prefix: self.scope_prefix.clone(),
            inputs: self.inputs.clone(),
            stdin: self.stdin.clone(),
            cancellation: self.cancellation.clone(),
            worker_handle: self.worker_handle.clone(),
            worker_limit: self.worker_limit,
            #[cfg(feature = "async")]
            future_executor: self.future_executor.clone(),
        }
    }

    pub fn merge_domain_outputs<'a, N: 'a>(
        &mut self,
        source: &Self,
        nodes: impl IntoIterator<Item = &'a FlowNode<N>>,
    ) {
        for node in nodes {
            for port in &node.metadata.ports.outputs {
                let id = output_id(node.definition_id.as_str(), &port.name);
                if source.typed_outputs.contains(&id) {
                    self.outputs.remove(&id);
                    self.typed_outputs.insert(id);
                } else if let Some(value) = source.outputs.get(&id) {
                    self.typed_outputs.remove(&id);
                    self.outputs.insert(id, value.clone());
                }
            }
        }
        for (scope_index, scope) in source.scopes.iter().enumerate() {
            if let Some(target) = self.scopes.get_mut(scope_index) {
                for (name, value) in &scope.variables {
                    // JSON equality ignores secret metadata; preserve metadata-only writes.
                    if !target
                        .variables
                        .get(name)
                        .is_some_and(|previous| previous.ptr_eq(value))
                    {
                        target.variables.insert(name.clone(), value.clone());
                        self.outputs
                            .insert(output_id(&target.source_id, name), Some(value.clone()));
                    }
                }
                target.exit_requested |= scope.exit_requested;
            }
        }
    }

    pub fn event_inputs<N>(
        &mut self,
        node: &FlowNode<N>,
        dependencies: &[crate::FlowDependency],
    ) -> Result<Option<Inputs>, WorkflowRunError> {
        let (inputs, resolution) = self.event_input_resolution(node, dependencies)?;
        Ok(resolution.causes.is_empty().then_some(inputs))
    }

    pub fn event_input_resolution<N>(
        &mut self,
        node: &FlowNode<N>,
        dependencies: &[crate::FlowDependency],
    ) -> Result<(Inputs, crate::InputResolution), WorkflowRunError> {
        self.reserve_step(node.definition_id.as_str())?;
        let mut inputs = Inputs::new();
        let resolution = self.inputs.resolve_node_inputs(
            node,
            !self.scopes.is_empty(),
            dependencies.iter().map(crate::FlowDependency::borrowed),
            |dependency| {
                self.output(&output_id(dependency.source_node, dependency.source_output))
                    .map(Some)
            },
            &mut inputs,
        )?;
        Ok((inputs, resolution))
    }

    pub fn set_snapshot_recorder(&mut self, recorder: crate::SnapshotRecorder) {
        self.snapshots = Some(recorder);
    }

    pub fn snapshot_recorder(&self) -> Option<&crate::SnapshotRecorder> {
        self.snapshots.as_ref()
    }

    pub fn fork_body(&self, observation: Option<BodyObservation>) -> Self {
        let mut child = Self::for_body(observation);
        child.cancellation = self.cancellation.clone();
        child.worker_handle = self.worker_handle.clone();
        child.worker_limit = self.worker_limit;
        #[cfg(feature = "async")]
        {
            child.future_executor = self.future_executor.clone();
        }
        child.snapshots = self.snapshots.clone();
        child.scope_prefix = self.invocation_path();
        child
    }

    /// Includes enclosing body scopes even when observation and snapshots are disabled.
    pub fn invocation_path(&self) -> Vec<LoopPathEntry> {
        let mut path = self.scope_prefix.clone();
        path.extend(self.scope_path());
        path
    }

    fn capture_node<N>(
        &self,
        node: &FlowNode<N>,
        inputs: &Inputs,
        result: Option<&NodeResult>,
        outcome: crate::SnapshotOutcome,
        error: Option<&dyn std::fmt::Display>,
    ) {
        if let Some(recorder) = &self.snapshots {
            recorder.record(crate::SnapshotEvent {
                scope: self.invocation_path(),
                node: node.definition_id.as_str().into(),
                snapshot: crate::NodeSnapshot {
                    inputs: crate::snapshot::ports_snapshot(inputs),
                    outputs: result.map_or_else(
                        || crate::ValueRef::object([]),
                        |result| crate::snapshot::ports_snapshot(&result.outputs),
                    ),
                    skipped: if outcome == crate::SnapshotOutcome::Skipped {
                        node.metadata
                            .ports
                            .outputs
                            .iter()
                            .map(|port| port.name.to_string())
                            .collect::<Vec<_>>()
                            .into()
                    } else {
                        result
                            .map_or_else(Vec::new, |result| {
                                result.skipped.iter().cloned().collect()
                            })
                            .into()
                    },
                    outcome,
                    error: error.map(|error| error.to_string().into()),
                },
            });
        }
    }

    pub fn for_body(body_observation: Option<BodyObservation>) -> Self {
        Self {
            body_observation,
            ..Self::default()
        }
    }

    pub fn observation(&self) -> Option<&RunObservation> {
        self.observation.as_ref()
    }

    pub fn observation_mut(&mut self) -> Option<&mut RunObservation> {
        self.observation.as_mut()
    }

    pub fn scope_path(&self) -> Vec<LoopPathEntry> {
        self.scopes
            .iter()
            .map(|scope| LoopPathEntry {
                loop_id: scope.node_id.clone(),
                index: mf_telemetry::Count::try_from(scope.index as i64)
                    .expect("scope index is bounded"),
            })
            .collect()
    }

    /// Runs a synchronous execution scope without changing its result or installing providers.
    pub fn run<T, E: std::fmt::Display>(
        observation: Option<RunObservation>,
        execute: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut state = Self {
            observation,
            ..Self::default()
        };
        let _context = state.observation.as_ref().map(RunObservation::enter);
        let result = execute(&mut state);
        if let Some(run) = state.observation.as_mut() {
            run.finish(
                result
                    .as_ref()
                    .err()
                    .map(|error| error as &dyn std::fmt::Display),
            );
        }
        result
    }

    pub fn preparation_failed_in_loop(
        &mut self,
        scope: &[&str],
        id: &str,
        error: &dyn std::fmt::Display,
    ) {
        if let Some(run) = self.observation.as_mut() {
            run.preparation_failed_unattributed(format!(
                "Loop scope {} node `{id}`: {error}",
                serde_json::to_string(scope).expect("scope IDs serialize")
            ));
        }
    }

    pub fn preparation_failed(&mut self, id: &str, error: &dyn std::fmt::Display) {
        if let Some(run) = self.observation.as_mut() {
            run.preparation_failed(id, error.to_string());
        }
    }

    pub fn select_observation_failure(&mut self, error: &WorkflowRunError) {
        let Some(observation) = self.observation.as_mut() else {
            return;
        };
        let (node, phase) = match error {
            WorkflowRunError::Dependency { definition_id, .. }
            | WorkflowRunError::SuccessDependency { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Dependency)
            }
            WorkflowRunError::InputType { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Dependency)
            }
            WorkflowRunError::OutputType { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Publication)
            }
            WorkflowRunError::OutputSelection { .. } => (None, FailurePhase::OutputSelection),
            WorkflowRunError::NodeExecution { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Execution)
            }
            WorkflowRunError::Context { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Execution)
            }
            WorkflowRunError::WorkflowInputs { .. } => (None, FailurePhase::Dependency),
            WorkflowRunError::NodeTarget { .. } => (None, FailurePhase::Dependency),
            WorkflowRunError::WorkerPool { .. } => (None, FailurePhase::Execution),
        };
        observation.select_failure(node, phase, error.to_string());
    }

    pub fn select_output(
        &mut self,
        name: &str,
        node: &str,
        port: &str,
        optional: bool,
    ) -> Result<Option<Value>, WorkflowRunError> {
        let result = select_context_output(self, name, node, port, optional);
        if let (Err(error), Some(run)) = (&result, self.observation.as_mut()) {
            run.output_selection_failed(error.to_string());
        }
        result
    }

    /// Reads availability without decoding a transferred typed value.
    pub fn output_available(&self, node: &str, port: &str) -> Result<bool, NodeExecutionError> {
        let key = output_id(node, port);
        if self.typed_outputs.contains(&key) {
            return Ok(true);
        }
        self.output(&key)
            .map(|value| matches!(value, ContextValue::Value(_)))
    }

    pub fn resolve_node_inputs<'a, N>(
        &self,
        node: &FlowNode<N>,
        dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
        transferred_inputs: &[&str],
        inputs: &mut Inputs,
        causes: BTreeSet<SkipCause>,
    ) -> Result<crate::InputResolution, WorkflowRunError> {
        self.inputs.resolve_node_inputs_with_causes(
            node,
            !self.scopes.is_empty(),
            dependencies,
            |dependency| {
                let key = output_id(dependency.source_node, dependency.source_output);
                if dependency
                    .input
                    .is_some_and(|input| transferred_inputs.contains(&input))
                    && self.typed_outputs.contains(&key)
                {
                    return Ok(None);
                }
                self.output(&key).map(Some)
            },
            inputs,
            causes,
        )
    }

    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        match self.outputs.get(id) {
            Some(Some(value)) => Ok(ContextValue::Value(value)),
            Some(None) => Ok(ContextValue::Skipped),
            None => Err(NodeExecutionError::ExecutionFailed {
                message: format!("missing context output `{id}`"),
            }),
        }
    }

    pub(crate) fn scope_values(&self) -> Result<Outputs, NodeExecutionError> {
        let frame = self
            .scopes
            .last()
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "scope source is outside an execution scope".into(),
            })?;
        let mut values = frame.variables.clone();
        values.insert("index".into(), Value::from(frame.index as i64));
        Ok(values)
    }

    pub(crate) fn request_scope_exit(&mut self) -> Result<(), NodeExecutionError> {
        let frame = self
            .scopes
            .last_mut()
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "scope exit is outside an execution scope".into(),
            })?;
        frame.exit_requested = true;
        if let Some(position) = self.current_position {
            self.scope_exit_cutoff.fetch_min(position, Ordering::AcqRel);
        }
        Ok(())
    }

    pub fn scope_exit_requested(&self) -> bool {
        self.scopes.last().is_some_and(|frame| frame.exit_requested)
    }

    pub fn scope_visited_steps(&self) -> usize {
        self.scopes
            .last()
            .map_or(0, |scope| scope.visited_steps.load(Ordering::Acquire))
    }

    pub(crate) fn stage_scope_write(
        &mut self,
        variable: &str,
        value: Value,
    ) -> Result<(), NodeExecutionError> {
        let frame = self
            .scopes
            .last()
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "Loop assignment is outside a Loop frame".into(),
            })?;
        let value_type =
            frame
                .types
                .get(variable)
                .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                    message: format!("unknown Loop variable `{variable}`"),
                })?;
        value_type
            .validate_shared(&value)
            .context(LoopVariableTypeSnafu { variable })?;
        self.pending_loop_write = Some((variable.to_owned(), value));
        Ok(())
    }

    pub fn run_scope<T>(
        &mut self,
        scope: ExecutionScope,
        run: impl FnOnce(&mut Self) -> Result<T, WorkflowRunError>,
    ) -> Result<(T, Outputs, bool), WorkflowRunError> {
        let depth = self.scopes.len();
        let parent_outputs = std::mem::take(&mut self.outputs);
        let parent_typed_outputs = std::mem::take(&mut self.typed_outputs);
        let parent_exit_cutoff = std::mem::replace(
            &mut self.scope_exit_cutoff,
            Arc::new(AtomicUsize::new(usize::MAX)),
        );
        self.scopes.push(scope);
        let guard = ScopeGuard {
            context: self,
            parent_outputs,
            parent_typed_outputs,
            parent_exit_cutoff,
            depth,
        };
        let state = &mut *guard.context;
        let result = run(state);
        let frame = state.scopes.pop().expect("execution scope was just pushed");
        result.map(|output| (output, frame.variables, frame.exit_requested))
    }

    fn reserve_step(&mut self, id: &str) -> Result<(), WorkflowRunError> {
        self.remaining_steps.reserve(id)?;
        if let Some(frame) = self.scopes.last_mut() {
            frame.visited_steps.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    fn commit_pending_loop_write(&mut self) {
        if let Some((variable, value)) = self.pending_loop_write.take() {
            let frame = self
                .scopes
                .last_mut()
                .expect("validated Loop write has a frame");
            frame.variables.insert(variable.clone(), value.clone());
            // Later body steps read the current state through the synthetic source.
            self.outputs
                .insert(output_id(&frame.source_id, &variable), Some(value));
        }
    }

    fn publish_generated<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<GeneratedNodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let (produced, skipped) = match result {
            Some(GeneratedNodeResult::Typed {
                produced, skipped, ..
            }) => (produced, skipped),
            Some(GeneratedNodeResult::Dynamic(result)) => return self.publish(node, Some(result)),
            None => return self.publish(node, None),
        };
        validate_skipped_outputs(node, &skipped, |name| produced.contains(&name))?;
        for name in &produced {
            if !node
                .metadata
                .ports
                .outputs
                .iter()
                .any(|port| port.name == *name)
            {
                return Err(state_error(
                    node.definition_id.as_str(),
                    format!("produced undeclared output `{name}`"),
                ));
            }
        }
        for name in &produced {
            let key = output_id(node.definition_id.as_str(), name);
            self.outputs.remove(&key);
            self.typed_outputs.insert(key);
        }
        for name in skipped {
            let key = output_id(node.definition_id.as_str(), &name);
            self.typed_outputs.remove(&key);
            self.outputs.insert(key, None);
        }
        self.commit_pending_loop_write();
        Ok(())
    }

    fn publish<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if let Some(result) = &result {
            validate_skipped_outputs(node, &result.skipped, |name| {
                result.outputs.contains_key(name)
            })?;
            for (name, value) in &result.outputs {
                let Some(port) = node
                    .metadata
                    .ports
                    .outputs
                    .iter()
                    .find(|port| port.name == *name)
                else {
                    return Err(state_error(
                        id,
                        format!("produced undeclared output `{name}`"),
                    ));
                };
                port.value_type
                    .validate_shared(value)
                    .with_context(|_| OutputTypeSnafu {
                        definition_id: id,
                        output: name,
                    })?;
            }
        }
        // Validate the complete result before making any values visible.
        for port in &node.metadata.ports.outputs {
            self.typed_outputs.remove(&output_id(id, &port.name));
        }
        match result {
            Some(result) => {
                self.outputs.extend(
                    result
                        .outputs
                        .into_iter()
                        .map(|(port, value)| (output_id(id, &port), Some(value))),
                );
                self.outputs.extend(
                    result
                        .skipped
                        .into_iter()
                        .map(|port| (output_id(id, &port), None)),
                );
            }
            None => self.outputs.extend(
                node.metadata
                    .ports
                    .outputs
                    .iter()
                    .map(|port| (output_id(id, &port.name), None)),
            ),
        }
        self.commit_pending_loop_write();
        Ok(())
    }
}

fn validate_skipped_outputs<N>(
    node: &FlowNode<N>,
    skipped: &BTreeSet<String>,
    produced: impl Fn(&str) -> bool,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    for name in skipped {
        if produced(name) {
            return Err(state_error(
                id,
                format!("output `{name}` is both produced and skipped"),
            ));
        }
        if !node
            .metadata
            .ports
            .outputs
            .iter()
            .any(|port| port.name == *name && !port.required)
        {
            return Err(state_error(
                id,
                format!("cannot explicitly skip unknown or required output `{name}`"),
            ));
        }
    }
    Ok(())
}

fn state_error(id: &str, message: impl Into<String>) -> WorkflowRunError {
    WorkflowRunError::Context {
        definition_id: id.into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionDependency<'a> {
    /// Candidates with the same target input form one exclusive group.
    pub exclusive: bool,
    pub input: Option<&'a str>,
    pub source_node: &'a str,
    pub source_output: &'a str,
}

enum StepObservation {
    Root(NodeObservation),
    Body(BodyNodeObservation),
}

impl StepObservation {
    fn set_input_selections(&mut self, selected: Vec<mf_telemetry::event::InputSelection>) {
        match self {
            Self::Root(step) => step.set_input_selections(selected),
            Self::Body(step) => step.set_input_selections(selected),
        }
    }
    fn started(&mut self, ctx: &mut ExecutionContext) {
        match self {
            Self::Root(step) => ctx.observation.as_mut().unwrap().node_started(step),
            Self::Body(step) => step.started(),
        }
    }

    fn failed(self, ctx: &mut ExecutionContext, phase: FailurePhase, message: String) {
        match self {
            Self::Root(step) => ctx
                .observation
                .as_mut()
                .unwrap()
                .node_failed(step, phase, message),
            Self::Body(step) => step.failed(phase, message),
        }
    }

    fn succeeded(
        self,
        ctx: &mut ExecutionContext,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        loop_summary: Option<LoopSummary>,
    ) {
        match self {
            Self::Root(step) => ctx
                .observation
                .as_mut()
                .unwrap()
                .node_succeeded_with_loop_summary(
                    step,
                    produced_ports,
                    skipped_ports,
                    loop_summary,
                ),
            Self::Body(step) => step.succeeded(produced_ports, skipped_ports),
        }
    }

    fn skipped(
        self,
        ctx: &mut ExecutionContext,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    ) {
        match self {
            Self::Root(step) => {
                ctx.observation
                    .as_mut()
                    .unwrap()
                    .node_skipped(step, causes, skipped_ports)
            }
            Self::Body(step) => step.skipped(causes, skipped_ports),
        }
    }
}

/// Executes one step of a validated plan, in its topological order.
pub fn execute_node_in_context(
    node: &crate::TaskFlowNode,
    dependencies: &[ExecutionDependency<'_>],
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    let dependencies = if dependencies.is_sorted() {
        std::borrow::Cow::Borrowed(dependencies)
    } else {
        let mut ordered = dependencies.to_vec();
        ordered.sort();
        std::borrow::Cow::Owned(ordered)
    };
    execute_ordered_task_in_context(node, node.node.as_ref(), dependencies.iter().copied(), ctx)
}

pub(super) fn execute_ordered_task_in_context<'a, N>(
    node: &FlowNode<N>,
    task: &dyn crate::TaskNode,
    dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    execute_generated_node_in_context(node, dependencies, ctx, &[], |inputs, ctx| {
        task.execute(inputs, ctx).map(GeneratedNodeResult::Dynamic)
    })
}

/// Invokes a generated task through the ordinary dependency and observation lifecycle.
pub fn execute_generated_node_in_context<'a, N>(
    node: &FlowNode<N>,
    dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
    ctx: &mut ExecutionContext,
    transferred_inputs: &[&str],
    invoke: impl FnOnce(
        Inputs,
        &mut ExecutionContext,
    ) -> Result<GeneratedNodeResult, NodeExecutionError>,
) -> Result<(), WorkflowRunError> {
    execute_resolved_node_in_context(
        node,
        ctx,
        |ctx, inputs| {
            ctx.resolve_node_inputs(
                node,
                dependencies,
                transferred_inputs,
                inputs,
                BTreeSet::new(),
            )
        },
        invoke,
    )
}

/// Runs prepared input resolution through the shared observation and publication lifecycle.
pub fn execute_resolved_node_in_context<N>(
    node: &FlowNode<N>,
    ctx: &mut ExecutionContext,
    resolve: impl FnOnce(
        &ExecutionContext,
        &mut Inputs,
    ) -> Result<crate::InputResolution, WorkflowRunError>,
    invoke: impl FnOnce(
        Inputs,
        &mut ExecutionContext,
    ) -> Result<GeneratedNodeResult, NodeExecutionError>,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    ctx.reserve_step(id)?;
    ctx.pending_loop_write = None;
    let path = ctx.observation.as_ref().map(|_| ctx.scope_path());
    let mut step = if let Some(run) = ctx.observation.as_mut() {
        let path = path.expect("observation has a scope path");
        if path.is_empty() {
            run.begin_node(id).map(StepObservation::Root)
        } else {
            run.begin_invocation(path, id).map(StepObservation::Root)
        }
    } else {
        ctx.body_observation
            .as_ref()
            .and_then(|body| body.begin_node(id))
            .map(StepObservation::Body)
    };
    let _root_context = match &step {
        Some(StepObservation::Root(step)) => Some(step.enter()),
        _ => None,
    };
    let _body_context = match &step {
        Some(StepObservation::Body(step)) => Some(step.enter()),
        _ => None,
    };
    let mut inputs = Inputs::new();
    let resolution = resolve(ctx, &mut inputs);
    let causes = match resolution {
        Ok(resolution) => {
            if let Some(step) = step.as_mut() {
                step.set_input_selections(resolution.selected);
            }
            resolution.causes
        }
        Err(error) => {
            if let Some(step) = step.take() {
                step.failed(ctx, FailurePhase::Dependency, error.to_string());
            }
            ctx.capture_node(
                node,
                &inputs,
                None,
                crate::SnapshotOutcome::Failed,
                Some(&error),
            );
            return Err(error);
        }
    };
    let skipped = !causes.is_empty();
    let snapshot_inputs = ctx.snapshots.as_ref().map(|_| inputs.clone());
    let result = if skipped {
        None
    } else {
        ctx.capture_node(node, &inputs, None, crate::SnapshotOutcome::Started, None);
        if let Some(step) = step.as_mut() {
            step.started(ctx);
        }
        let result = {
            let previous = ctx.current_node.replace(node.definition_id.clone());
            let guard = NodeInvocationGuard {
                context: ctx,
                previous,
            };
            invoke(inputs, guard.context).with_context(|_| NodeExecutionSnafu {
                definition_id: node.definition_id.clone(),
            })
        };
        match result {
            Ok(result) => Some(result),
            Err(error) => {
                if let Some(step) = step.take() {
                    step.failed(ctx, FailurePhase::Execution, error.to_string());
                }
                if let Some(inputs) = &snapshot_inputs {
                    ctx.capture_node(
                        node,
                        inputs,
                        None,
                        crate::SnapshotOutcome::Failed,
                        Some(&error),
                    );
                }
                return Err(error);
            }
        }
    };
    let mut produced_ports = Vec::new();
    let mut skipped_ports = Vec::new();
    if step.is_some() {
        if let Some(result) = &result {
            produced_ports.extend(result.produced_names());
            skipped_ports.extend(result.skipped().iter().cloned());
        } else {
            skipped_ports.extend(
                node.metadata
                    .ports
                    .outputs
                    .iter()
                    .map(|port| port.name.to_string()),
            );
            skipped_ports.sort();
        }
    }
    let loop_summary = result
        .as_ref()
        .and_then(|result| result.loop_summary().cloned());
    let snapshot_result = ctx.snapshots.as_ref().and_then(|_| {
        result
            .as_ref()
            .and_then(GeneratedNodeResult::dynamic)
            .cloned()
    });
    let result = ctx.publish_generated(node, result);
    if let Some(inputs) = &snapshot_inputs {
        let outcome = if result.is_err() {
            crate::SnapshotOutcome::Failed
        } else if skipped {
            crate::SnapshotOutcome::Skipped
        } else {
            crate::SnapshotOutcome::Succeeded
        };
        ctx.capture_node(
            node,
            inputs,
            snapshot_result.as_ref(),
            outcome,
            result
                .as_ref()
                .err()
                .map(|error| error as &dyn std::fmt::Display),
        );
    }
    if let Some(step) = step.take() {
        match &result {
            Err(error) => step.failed(ctx, FailurePhase::Publication, error.to_string()),
            Ok(()) if skipped => step.skipped(ctx, causes.into_iter().collect(), skipped_ports),
            Ok(()) => step.succeeded(ctx, produced_ports, skipped_ports, loop_summary),
        }
    }
    result
}

/// Reads a selection without instrumentation; observed executors use `ExecutionContext::select_output`.
pub fn select_context_output(
    ctx: &ExecutionContext,
    name: &str,
    node: &str,
    port: &str,
    optional: bool,
) -> Result<Option<Value>, WorkflowRunError> {
    match ctx
        .output(&output_id(node, port))
        .context(OutputSelectionSnafu {
            definition_id: node,
            output: name,
        })? {
        ContextValue::Value(value) => Ok(Some(value.clone())),
        ContextValue::Skipped if optional => Ok(None),
        ContextValue::Skipped => Err(state_error(
            node,
            format!(
                "workflow output `{name}` requires skipped output `{}`",
                output_id(node, port)
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flow, NodePorts, PortSpec, TaskNode, ValueType};
    use serde_json::json;
    use std::error::Error;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct EmitNode(Outputs);

    impl TaskNode for EmitNode {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            Ok((self.0.clone()).into())
        }
    }

    struct CountNode(Arc<AtomicUsize>);

    impl TaskNode for CountNode {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok((Outputs::from([("value".into(), json!(true).into())])).into())
        }
    }

    fn port(name: &str, value_type: ValueType, required: bool) -> PortSpec {
        PortSpec::owned(name, value_type, required)
    }

    #[test]
    fn invocation_paths_preserve_parent_scopes_in_unobserved_bodies() {
        let mut context = ExecutionContext::default();
        let outer =
            ExecutionScope::new("outer", "%loop", 2, Outputs::new(), BTreeMap::new()).unwrap();
        context
            .run_scope(outer, |parent| {
                let mut child = parent.fork_body(None);
                let inner =
                    ExecutionScope::new("inner", "%iteration", 3, Outputs::new(), BTreeMap::new())
                        .unwrap();
                child.run_scope(inner, |child| {
                    let path = child.invocation_path();
                    assert_eq!(
                        path.iter()
                            .map(|entry| (entry.loop_id.as_str(), entry.index.get()))
                            .collect::<Vec<_>>(),
                        [("outer", 2), ("inner", 3)]
                    );
                    assert_eq!(child.scope_path().len(), 1);
                    Ok::<_, WorkflowRunError>(())
                })
            })
            .unwrap();
        assert!(context.invocation_path().is_empty());
    }

    #[test]
    fn executing_node_identity_is_nested_and_restored_after_failure_or_panic() {
        let outer = FlowNode {
            definition_id: "outer".into(),
            node: (),
            metadata: Default::default(),
        };
        let inner = FlowNode {
            definition_id: "inner".into(),
            node: (),
            metadata: Default::default(),
        };
        let mut context = ExecutionContext::default();
        for panic in [false, true] {
            execute_generated_node_in_context(&outer, [], &mut context, &[], |_, context| {
                assert_eq!(context.current_node(), Some("outer"));
                assert!(
                    context
                        .fork_domain_with_observation(None)
                        .current_node()
                        .is_none()
                );
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_generated_node_in_context(&inner, [], context, &[], |_, context| {
                        assert_eq!(context.current_node(), Some("inner"));
                        assert!(!panic, "synthetic panic");
                        crate::NodeExecutionFailedSnafu {
                            message: "synthetic failure",
                        }
                        .fail()
                    })
                }));
                assert!(result.is_err() || result.unwrap().is_err());
                assert_eq!(context.current_node(), Some("outer"));
                Ok(GeneratedNodeResult::Dynamic(Outputs::new().into()))
            })
            .unwrap();
            assert!(context.current_node().is_none());
        }
    }

    #[test]
    fn startup_inputs_follow_context_fork_and_scope_boundaries() {
        let node = FlowNode {
            definition_id: "target".into(),
            node: (),
            metadata: crate::NodeMetadata::new(NodePorts {
                inputs: vec![port("value", ValueType::Int64, true)],
                outputs: vec![],
            }),
        };
        let schema = serde_json::from_value(json!({
            "inputs": {"target": {"value": {"type": "int", "required": true}}},
            "stdin": {}
        }))
        .unwrap();
        let mut context = ExecutionContext::default();
        context.set_workflow_arguments(
            crate::WorkflowArguments::try_from(json!({"target.value": 1})).unwrap(),
        );
        context.bind_workflow_inputs(&schema).unwrap();
        let mut domain = context.fork_domain_with_observation(None);
        for mut child in [context.fork_stream(None), context.fork_body(None)] {
            assert!(child.event_inputs(&node, &[]).unwrap().unwrap().is_empty());
        }
        let scope =
            ExecutionScope::new("loop", "%loop", 0, Outputs::new(), BTreeMap::new()).unwrap();
        context
            .run_scope(scope, |context| {
                assert!(context.event_inputs(&node, &[])?.unwrap().is_empty());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            context.event_inputs(&node, &[]).unwrap().unwrap()["value"],
            json!(1)
        );
        context.set_workflow_arguments(
            crate::WorkflowArguments::try_from(json!({"target.value": 2})).unwrap(),
        );
        assert!(
            context
                .event_inputs(&node, &[])
                .unwrap()
                .unwrap()
                .is_empty()
        );
        context.bind_workflow_inputs(&schema).unwrap();
        assert_eq!(
            context.event_inputs(&node, &[]).unwrap().unwrap()["value"],
            json!(2)
        );
        assert_eq!(
            domain.event_inputs(&node, &[]).unwrap().unwrap()["value"],
            json!(1)
        );
    }

    #[test]
    fn event_and_generated_inputs_share_skip_errors_and_partial_failure_snapshots() {
        let node = FlowNode {
            definition_id: "target".into(),
            node: (),
            metadata: crate::NodeMetadata::new(NodePorts {
                inputs: vec![port("value", ValueType::Int64, true)],
                outputs: vec![],
            }),
        };
        for case in ["ready", "skipped", "missing", "wrong_type"] {
            let value = if case == "ready" {
                Value::from(42)
            } else {
                Value::from("wrong type")
            }
            .into_secret();
            let mut context = ExecutionContext::default();
            context
                .outputs
                .insert("source.value".into(), Some(value.clone()));
            context.outputs.insert(
                "gate.value".into(),
                if matches!(case, "skipped" | "missing") {
                    None
                } else {
                    Some(Value::from(true))
                },
            );
            let mut dependencies = vec![
                crate::FlowDependency {
                    exclusive: false,
                    input: Some("value".into()),
                    source_node: "source".into(),
                    source_output: "value".into(),
                },
                crate::FlowDependency {
                    exclusive: false,
                    input: None,
                    source_node: "gate".into(),
                    source_output: "value".into(),
                },
            ];
            if case == "missing" {
                dependencies.push(crate::FlowDependency {
                    exclusive: false,
                    input: None,
                    source_node: "absent".into(),
                    source_output: "value".into(),
                });
            }
            let event = context.event_inputs(&node, &dependencies);
            let recording = crate::SnapshotRecording::memory();
            context.set_snapshot_recorder(recording.recorder());
            let task = execute_generated_node_in_context(
                &node,
                dependencies.iter().map(crate::FlowDependency::borrowed),
                &mut context,
                &[],
                |inputs, _| {
                    assert_eq!(case, "ready");
                    assert!(inputs["value"].ptr_eq(&value));
                    Ok(GeneratedNodeResult::Dynamic(Outputs::new().into()))
                },
            );
            let snapshot = recording.current();
            let snapshot = snapshot.node(&[], "target").unwrap();
            assert_eq!(snapshot.inputs["value"], json!("[REDACTED]"));
            match (event, task) {
                (Ok(Some(inputs)), Ok(())) => {
                    assert_eq!(case, "ready");
                    assert!(inputs["value"].ptr_eq(&value));
                    assert_eq!(snapshot.outcome, crate::SnapshotOutcome::Succeeded);
                }
                (Ok(None), Ok(())) => {
                    assert_eq!(case, "skipped");
                    assert_eq!(snapshot.outcome, crate::SnapshotOutcome::Skipped);
                }
                (Err(event), Err(task)) => {
                    assert_eq!(event.to_string(), task.to_string());
                    assert_eq!(snapshot.outcome, crate::SnapshotOutcome::Failed);
                    assert!(matches!(
                        (&task, case),
                        (WorkflowRunError::Dependency { .. }, "missing")
                            | (WorkflowRunError::InputType { .. }, "wrong_type")
                    ));
                }
                _ => panic!("event and task resolution diverged for {case}"),
            }
        }
    }

    #[test]
    fn domain_merge_preserves_secret_only_scope_updates() {
        let public = Value::from("credential");
        let scope = ExecutionScope::new(
            "loop",
            "%loop",
            0,
            Outputs::from([("token".into(), public.clone())]),
            BTreeMap::from([("token".into(), ValueType::String)]),
        )
        .unwrap();
        let mut target = ExecutionContext::default();
        target.scopes.push(scope);
        target
            .outputs
            .insert("%loop.token".into(), Some(public.clone()));
        let mut source = target.fork_domain_with_observation(None);
        source.scopes[0]
            .variables
            .insert("token".into(), public.into_secret());
        target.merge_domain_outputs::<()>(&source, std::iter::empty::<&FlowNode<()>>());
        assert!(target.scope_values().unwrap()["token"].is_secret());
        let ContextValue::Value(value) = target.output("%loop.token").unwrap() else {
            panic!("scope output must be available");
        };
        assert!(value.is_secret());
    }

    #[test]
    fn scoped_execution_restores_parent_state_after_failure_and_unwind() {
        for unwinds in [false, true] {
            let mut state = ExecutionContext::default();
            state
                .outputs
                .insert("parent.value".into(), Some(json!(9).into()));
            let parent_outputs = state.outputs.clone();
            let types = BTreeMap::from([("count".into(), ValueType::Int64)]);
            let source = crate::prepared_scope_source("input", &types)
                .into_task()
                .unwrap();
            let scope = ExecutionScope::new(
                "scope",
                "input",
                0,
                Outputs::from([("count".into(), json!(7).into())]),
                types,
            )
            .unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                state.run_scope(scope, |state| {
                    assert!(state.output("parent.value").is_err());
                    execute_node_in_context(&source, &[], state)?;
                    let nested = ExecutionScope::new(
                        "nested",
                        "nested_input",
                        0,
                        Outputs::new(),
                        BTreeMap::new(),
                    )
                    .unwrap();
                    state.run_scope(nested, |_| {
                        if unwinds {
                            panic!("body panicked");
                        }
                        Err::<(), _>(state_error("body", "body failed"))
                    })?;
                    Ok(())
                })
            }));
            if unwinds {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
            assert_eq!(state.outputs, parent_outputs);
            assert!(state.scopes.is_empty());
            assert_eq!(
                state.remaining_steps.0.load(Ordering::Acquire),
                crate::MAX_SCHEDULED_STEPS - 1
            );
        }
    }

    #[test]
    fn rejects_a_bad_output_without_publishing_any_result() {
        let node = FlowNode::new(
            "producer",
            crate::PreparedNode::from_parts(
                crate::NodeExecution::Task(Box::new(EmitNode(Outputs::from([
                    ("good".into(), json!(1).into()),
                    ("bad".into(), json!("wrong").into()),
                    ("later".into(), json!(false).into()),
                ])))),
                NodePorts {
                    inputs: vec![],
                    outputs: vec![
                        port("good", ValueType::Int64, true),
                        port("bad", ValueType::Int64, true),
                    ],
                },
            ),
        )
        .into_task()
        .unwrap();
        let mut context = ExecutionContext::default();
        let error = execute_node_in_context(&node, &[], &mut context).unwrap_err();
        assert!(matches!(
            &error,
            WorkflowRunError::OutputType { definition_id, output, .. }
                if definition_id.as_str() == "producer" && output == "bad"
        ));
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<crate::TypeMismatch>()
            .unwrap();
        assert_eq!(source.path, "");
        assert_eq!(source.expected, ValueType::Int64);
        assert_eq!(source.actual, "string");
        let message = error.to_string();
        assert!(message.contains("producer") && message.contains("output `bad`"));
        assert!(message.contains("expected int64, found string"));
        assert!(context.output("producer.good").is_err());
        assert!(context.output("producer.bad").is_err());
    }

    #[test]
    fn rejects_a_nested_dynamic_input_before_invoking_the_target() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::from_parts(
                crate::NodeExecution::Task(Box::new(CountNode(Arc::clone(&calls)))),
                NodePorts {
                    inputs: vec![port(
                        "payload",
                        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64)))),
                        true,
                    )],
                    outputs: vec![port("value", ValueType::Boolean, true)],
                },
            ),
        )
        .into_task()
        .unwrap();
        let mut context = ExecutionContext::default();
        context.outputs.insert(
            "source.value".into(),
            Some(json!([{"count": 1}, {"count": "two"}]).into()),
        );
        let dependency = ExecutionDependency {
            exclusive: false,
            input: Some("payload"),
            source_node: "source",
            source_output: "value",
        };
        let error = execute_node_in_context(&node, &[dependency], &mut context).unwrap_err();
        assert!(matches!(
            &error,
            WorkflowRunError::InputType { definition_id, input, .. }
                if definition_id.as_str() == "consumer" && input == "payload"
        ));
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<crate::TypeMismatch>()
            .unwrap();
        assert_eq!(source.path, "/1/count");
        assert_eq!(source.expected, ValueType::Int64);
        assert_eq!(source.actual, "string");
        let message = error.to_string();
        assert!(message.contains("consumer") && message.contains("input `payload`"));
        assert!(message.contains("/1/count") && message.contains("expected int64"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());
    }

    #[test]
    fn rejects_an_undeclared_direct_flow_input() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::from_parts(
                crate::NodeExecution::Task(Box::new(CountNode(Arc::clone(&calls)))),
                NodePorts {
                    inputs: vec![],
                    outputs: vec![port("value", ValueType::Boolean, true)],
                },
            ),
        )
        .into_task()
        .unwrap();
        let mut context = ExecutionContext::default();
        context
            .outputs
            .insert("source.value".into(), Some(json!(1).into()));
        let dependency = ExecutionDependency {
            exclusive: false,
            input: Some("unexpected"),
            source_node: "source",
            source_output: "value",
        };
        let error = execute_node_in_context(&node, &[dependency], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("consumer") && error.contains("undeclared input `unexpected`"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());
    }

    #[test]
    fn loop_assignment_preserves_nested_type_mismatch_sources() {
        let value_type = ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))));
        let scope = ExecutionScope::new(
            "repeat",
            crate::LOOP_SOURCE_ID,
            0,
            Outputs::from([("count".into(), json!([{ "count": 1 }]).into())]),
            BTreeMap::from([("count".into(), value_type)]),
        )
        .unwrap();
        let node = crate::prepared_loop_assign("assign", "count", ValueType::Any)
            .into_task()
            .unwrap();
        let mut context = ExecutionContext::default();
        let error = context
            .run_scope(scope, |context| {
                context.outputs.insert(
                    "source.value".into(),
                    Some(json!([{ "count": "wrong" }]).into()),
                );
                let result = execute_node_in_context(
                    &node,
                    &[ExecutionDependency {
                        exclusive: false,
                        input: Some("value"),
                        source_node: "source",
                        source_output: "value",
                    }],
                    context,
                );
                assert!(context.pending_loop_write.is_none());
                result
            })
            .unwrap_err();
        assert!(matches!(
            &error,
            WorkflowRunError::NodeExecution { definition_id, .. }
                if definition_id.as_str() == "assign"
        ));
        let source = error.source().unwrap();
        assert!(matches!(
            source.downcast_ref::<NodeExecutionError>(),
            Some(NodeExecutionError::LoopVariableType { variable, .. }) if variable == "count"
        ));
        let mismatch = source
            .source()
            .unwrap()
            .downcast_ref::<Box<crate::TypeMismatch>>()
            .unwrap();
        assert_eq!(mismatch.path, "/0/count");
        assert_eq!(mismatch.expected, ValueType::Int64);
        assert_eq!(mismatch.actual, "string");
        assert!(error.to_string().contains("Loop variable `count`"));
    }

    #[test]
    fn output_selection_preserves_lookup_sources() {
        let mut context = ExecutionContext::default();
        for optional in [false, true] {
            let error = context
                .select_output("result", "source", "missing", optional)
                .unwrap_err();
            assert!(matches!(
                &error,
                WorkflowRunError::OutputSelection { definition_id, output, .. }
                    if definition_id.as_str() == "source" && output == "result"
            ));
            assert!(matches!(
                error.source().unwrap().downcast_ref::<NodeExecutionError>(),
                Some(NodeExecutionError::ExecutionFailed { message })
                    if message == "missing context output `source.missing`"
            ));
            assert!(error.to_string().contains("workflow output `result`"));
        }
    }

    #[test]
    fn skips_without_type_checks_but_keeps_missing_output_precedence() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::from_parts(
                crate::NodeExecution::Task(Box::new(CountNode(Arc::clone(&calls)))),
                NodePorts {
                    inputs: vec![port("payload", ValueType::Int64, true)],
                    outputs: vec![port("value", ValueType::Boolean, true)],
                },
            ),
        )
        .into_task()
        .unwrap();
        let skipped = ExecutionDependency {
            exclusive: false,
            input: Some("payload"),
            source_node: "branch",
            source_output: "off",
        };
        let mut context = ExecutionContext::default();
        context.outputs.insert("branch.off".into(), None);
        for input in [None, Some("payload")] {
            let missing = ExecutionDependency {
                exclusive: false,
                input,
                source_node: "source",
                source_output: "missing",
            };
            let error =
                execute_node_in_context(&node, &[skipped, missing], &mut context).unwrap_err();
            assert!(matches!(
                &error,
                WorkflowRunError::Dependency { definition_id, input: actual, .. }
                    if definition_id.as_str() == "consumer"
                        && actual == input.unwrap_or("<control>")
            ));
            assert!(matches!(
                error.source().unwrap().downcast_ref::<NodeExecutionError>(),
                Some(NodeExecutionError::ExecutionFailed { message })
                    if message == "missing context output `source.missing`"
            ));
            assert!(error.to_string().contains("source.missing"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());

        execute_node_in_context(&node, &[skipped], &mut context).unwrap();
        assert!(matches!(
            context.output("consumer.value").unwrap(),
            ContextValue::Skipped
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    struct AlternatingNode(AtomicUsize);

    impl TaskNode for AlternatingNode {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            let value = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                json!("wrong")
            } else {
                json!(42)
            };
            Ok((Outputs::from([("value".into(), value.into())])).into())
        }
    }

    #[test]
    fn a_failed_run_does_not_poison_the_next_run() {
        let flow = Flow::from_plan(
            vec![FlowNode::new(
                "source",
                crate::PreparedNode::from_parts(
                    crate::NodeExecution::Task(Box::new(AlternatingNode(AtomicUsize::new(0)))),
                    NodePorts {
                        inputs: vec![],
                        outputs: vec![port("value", ValueType::Int64, true)],
                    },
                ),
            )]
            .into_iter()
            .map(|node| node.into_task().unwrap())
            .collect(),
            crate::FlowPlan {
                connections: vec![].into(),
                dependencies: vec![std::borrow::Cow::Owned(vec![])].into(),
                execution_order: vec![crate::NodeId::new(0)].into(),
                outputs: vec![crate::FlowOutput {
                    name: "result".into(),
                    node_id: crate::NodeId::new(0),
                    port: "value".into(),
                    optional: false,
                }]
                .into(),
                execution_domains: crate::ExecutionDomains::from_parts(
                    vec![crate::ExecutionDomain {
                        id: 0,
                        nodes: vec![crate::NodeId::new(0)].into(),
                        positions: vec![0].into(),
                        predecessors: vec![].into(),
                        successors: vec![].into(),
                        first_position: 0,
                    }]
                    .into(),
                    vec![std::borrow::Cow::Owned(vec![])].into(),
                ),
            },
            Default::default(),
        );
        assert!(
            flow.execute()
                .unwrap_err()
                .to_string()
                .contains("expected int64")
        );
        assert_eq!(flow.execute().unwrap()["result"], json!(42));
    }

    #[test]
    fn checks_any_source_before_a_refined_consumer_runs() {
        for (value, succeeds) in [(json!(21), true), (json!("21"), false)] {
            let calls = Arc::new(AtomicUsize::new(0));
            let flow = Flow::from_plan(
                vec![
                    FlowNode::new(
                        "source",
                        crate::PreparedNode::from_parts(
                            crate::NodeExecution::Task(Box::new(EmitNode(Outputs::from([(
                                "value".into(),
                                value.into(),
                            )])))),
                            NodePorts {
                                inputs: vec![],
                                outputs: vec![port("value", ValueType::Any, true)],
                            },
                        ),
                    ),
                    FlowNode::new(
                        "consumer",
                        crate::PreparedNode::from_parts(
                            crate::NodeExecution::Task(Box::new(CountNode(Arc::clone(&calls)))),
                            NodePorts {
                                inputs: vec![port("payload", ValueType::Int64, true)],
                                outputs: vec![port("value", ValueType::Boolean, true)],
                            },
                        ),
                    ),
                ]
                .into_iter()
                .map(|node| node.into_task().unwrap())
                .collect(),
                crate::FlowPlan {
                    connections: vec![crate::FlowConnection {
                        exclusive: false,
                        from_node: crate::NodeId::new(0),
                        from_output: "value".into(),
                        to_node: crate::NodeId::new(1),
                        to_input: "payload".into(),
                    }]
                    .into(),
                    dependencies: vec![
                        std::borrow::Cow::Owned(vec![]),
                        std::borrow::Cow::Owned(vec![crate::FlowDependency {
                            exclusive: false,
                            input: Some("payload".into()),
                            source_node: "source".into(),
                            source_output: "value".into(),
                        }]),
                    ]
                    .into(),
                    execution_order: vec![crate::NodeId::new(0), crate::NodeId::new(1)].into(),
                    outputs: vec![crate::FlowOutput {
                        name: "result".into(),
                        node_id: crate::NodeId::new(1),
                        port: "value".into(),
                        optional: false,
                    }]
                    .into(),
                    execution_domains: crate::ExecutionDomains::from_parts(
                        vec![crate::ExecutionDomain {
                            id: 0,
                            nodes: vec![crate::NodeId::new(0), crate::NodeId::new(1)].into(),
                            positions: vec![0, 1].into(),
                            predecessors: vec![].into(),
                            successors: vec![].into(),
                            first_position: 0,
                        }]
                        .into(),
                        vec![std::borrow::Cow::Owned(vec![])].into(),
                    ),
                },
                Default::default(),
            );
            if succeeds {
                assert_eq!(flow.execute().unwrap()["result"], json!(true));
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            } else {
                let error = flow.execute().unwrap_err().to_string();
                assert!(error.contains("consumer") && error.contains("input `payload`"));
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }
    }
}
