use crate::ValueRef as Value;
use crate::runner::{ContextSnafu, DependencySnafu, InputTypeSnafu, NodeExecutionSnafu};
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
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
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
    parent_exit_cutoff: Arc<AtomicUsize>,
    depth: usize,
}

impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        self.context.scopes.truncate(self.depth);
        self.context.outputs = std::mem::take(&mut self.parent_outputs);
        self.context.scope_exit_cutoff = self.parent_exit_cutoff.clone();
        self.context.pending_loop_write = None;
    }
}

#[derive(Clone, Debug, Default)]
pub struct NodeResult {
    pub outputs: Outputs,
    pub skipped: BTreeSet<String>,
    pub loop_summary: Option<LoopSummary>,
}

impl From<Outputs> for NodeResult {
    fn from(outputs: Outputs) -> Self {
        Self {
            outputs,
            ..Self::default()
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
    observation: Option<RunObservation>,
    body_observation: Option<BodyObservation>,
    scopes: Vec<ExecutionScope>,
    pending_loop_write: Option<(String, Value)>,
    current_position: Option<usize>,
    scope_exit_cutoff: Arc<AtomicUsize>,
    remaining_steps: StepBudget,
    snapshots: Option<crate::SnapshotRecorder>,
    snapshot_prefix: Vec<LoopPathEntry>,
    workflow_arguments: crate::WorkflowArguments,
    startup_inputs_bound: bool,
    stdin: Option<std::sync::Arc<std::sync::Mutex<crate::TextInput>>>,
    cancellation: crate::StreamCancellation,
    worker_handle: Option<RuntimeWorkerHandle>,
    worker_limit: NonZeroUsize,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            outputs: BTreeMap::new(),
            observation: None,
            body_observation: None,
            scopes: Vec::new(),
            pending_loop_write: None,
            current_position: None,
            scope_exit_cutoff: Arc::new(AtomicUsize::new(usize::MAX)),
            remaining_steps: StepBudget::new(crate::MAX_SCHEDULED_STEPS),
            snapshots: None,
            snapshot_prefix: Vec::new(),
            workflow_arguments: crate::WorkflowArguments::default(),
            startup_inputs_bound: false,
            stdin: None,
            cancellation: crate::StreamCancellation::default(),
            worker_handle: None,
            worker_limit: crate::RuntimeOptions::default().max_parallel_domains,
        }
    }
}

impl ExecutionContext {
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
            crate::StreamPreparationSnafu {
                message: "stdin is unavailable".to_owned(),
            }
            .build()
        })?;
        input.lock().unwrap().next_line(&self.cancellation)
    }

    pub fn fork_stream(&self, observation: Option<RunObservation>) -> Self {
        Self {
            outputs: self.outputs.clone(),
            remaining_steps: self.remaining_steps.clone(),
            stdin: self.stdin.clone(),
            cancellation: self.cancellation.clone(),
            observation,
            body_observation: None,
            scopes: Vec::new(),
            pending_loop_write: None,
            current_position: None,
            scope_exit_cutoff: self.scope_exit_cutoff.clone(),
            snapshots: None,
            snapshot_prefix: Vec::new(),
            workflow_arguments: crate::WorkflowArguments::default(),
            startup_inputs_bound: false,
            worker_handle: self.worker_handle.clone(),
            worker_limit: self.worker_limit,
        }
    }

    pub fn set_workflow_arguments(&mut self, arguments: crate::WorkflowArguments) {
        self.workflow_arguments = arguments;
        self.startup_inputs_bound = false;
    }

    pub fn bind_workflow_inputs(
        &mut self,
        schema: &crate::WorkflowInputSchema,
    ) -> Result<(), WorkflowRunError> {
        self.startup_inputs_bound = false;
        schema.validate(&self.workflow_arguments)?;
        schema.validate_stdin(&self.workflow_arguments, self.stdin.is_some())?;
        self.startup_inputs_bound = true;
        Ok(())
    }

    fn initial_inputs(&self, node: &str) -> Inputs {
        if !self.startup_inputs_bound || !self.scopes.is_empty() {
            return Inputs::new();
        }
        self.workflow_arguments
            .0
            .get(node)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn set_frame_observation(&mut self, observation: RunObservation) {
        self.observation = Some(observation);
    }

    pub(crate) fn configure_worker_limit(&mut self, limit: NonZeroUsize) {
        self.worker_limit = limit;
    }

    pub(crate) fn set_worker_handle(&mut self, handle: RuntimeWorkerHandle) {
        self.worker_handle = Some(handle);
    }

    pub(crate) fn worker_handle(&self) -> Option<RuntimeWorkerHandle> {
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

    pub(crate) fn replace_execution_position(&mut self, position: Option<usize>) -> Option<usize> {
        std::mem::replace(&mut self.current_position, position)
    }

    pub(crate) fn scope_exit_cutoff(&self) -> usize {
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

    pub(crate) fn fork_domain_with_visible_outputs(
        &self,
        visible_outputs: &BTreeSet<String>,
    ) -> Self {
        self.fork_domain_with_observation_and_visible_outputs(
            self.observation.clone(),
            visible_outputs,
        )
    }

    pub(crate) fn fork_domain_with_observation_and_visible_outputs(
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

    pub(crate) fn fork_domain_with_observation(&self, observation: Option<RunObservation>) -> Self {
        Self {
            outputs: self.outputs.clone(),
            observation,
            body_observation: self.body_observation.clone(),
            scopes: self.scopes.clone(),
            pending_loop_write: None,
            current_position: None,
            scope_exit_cutoff: self.scope_exit_cutoff.clone(),
            remaining_steps: self.remaining_steps.clone(),
            snapshots: self.snapshots.clone(),
            snapshot_prefix: self.snapshot_prefix.clone(),
            workflow_arguments: self.workflow_arguments.clone(),
            startup_inputs_bound: self.startup_inputs_bound,
            stdin: self.stdin.clone(),
            cancellation: self.cancellation.clone(),
            worker_handle: self.worker_handle.clone(),
            worker_limit: self.worker_limit,
        }
    }

    pub(crate) fn merge_domain_outputs<'a, N: 'a>(
        &mut self,
        source: &Self,
        nodes: impl IntoIterator<Item = &'a FlowNode<N>>,
    ) {
        for node in nodes {
            for port in &node.metadata.ports.outputs {
                let id = output_id(node.definition_id.as_str(), &port.name);
                if let Some(value) = source.outputs.get(&id) {
                    self.outputs.insert(id, value.clone());
                }
            }
        }
        for (scope_index, scope) in source.scopes.iter().enumerate() {
            if let Some(target) = self.scopes.get_mut(scope_index) {
                for (name, value) in &scope.variables {
                    if target.variables.get(name) != Some(value) {
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
        dependencies: &[crate::StreamDependency],
    ) -> Result<Option<Inputs>, WorkflowRunError> {
        let id = node.definition_id.as_str();
        self.reserve_step(id)?;
        let mut inputs = self.initial_inputs(id);
        let mut skipped = false;
        for dependency in dependencies {
            let value = self
                .output(&output_id(
                    &dependency.source_node,
                    &dependency.source_output,
                ))
                .with_context(|_| DependencySnafu {
                    definition_id: id,
                    input: dependency.input.as_deref().unwrap_or("<control>"),
                })?;
            match value {
                ContextValue::Value(value) => {
                    if let Some(input) = &dependency.input {
                        inputs.insert(input.clone(), value.clone());
                    }
                }
                ContextValue::Skipped => skipped = true,
            }
        }
        if skipped {
            return Ok(None);
        }
        for (name, value) in &inputs {
            let port = node
                .metadata
                .ports
                .inputs
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| state_error(id, format!("received undeclared input `{name}`")))?;
            port.value_type
                .validate_shared(value)
                .with_context(|_| InputTypeSnafu {
                    definition_id: id,
                    input: name,
                })?;
        }
        Ok(Some(inputs))
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
        if self.snapshots.is_some() {
            child.snapshots = self.snapshots.clone();
            child.snapshot_prefix = self.snapshot_path();
        }
        child
    }

    fn snapshot_path(&self) -> Vec<LoopPathEntry> {
        let mut path = self.snapshot_prefix.clone();
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
            recorder.record(
                self.snapshot_path(),
                node.definition_id.as_str(),
                crate::NodeSnapshot {
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
            );
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

    pub fn prepare_node(
        &mut self,
        registry: &crate::NodeRegistry,
        id: &str,
        kind: &str,
        config: &str,
    ) -> Result<FlowNode, WorkflowRunError> {
        let result = crate::instantiate_node_with_metadata(registry, id, kind, config);
        if let Err(error) = &result {
            self.preparation_failed(id, error);
        }
        result
    }

    pub fn prepare_node_in_loop(
        &mut self,
        registry: &crate::NodeRegistry,
        id: &str,
        kind: &str,
        config: &str,
        scope: &[&str],
    ) -> Result<FlowNode, WorkflowRunError> {
        let result = crate::instantiate_node_with_metadata(registry, id, kind, config);
        if let Err(error) = &result {
            self.preparation_failed_in_loop(scope, id, error);
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

    pub(crate) fn select_observation_failure(&mut self, error: &WorkflowRunError) {
        let Some(observation) = self.observation.as_mut() else {
            return;
        };
        let (node, phase) = match error {
            WorkflowRunError::Dependency { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Dependency)
            }
            WorkflowRunError::InputType { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Dependency)
            }
            WorkflowRunError::NodeExecution { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Execution)
            }
            WorkflowRunError::Context { definition_id, .. } => {
                (Some(definition_id.to_string()), FailurePhase::Execution)
            }
            WorkflowRunError::WorkflowInputs { .. }
            | WorkflowRunError::FlowBuild { .. }
            | WorkflowRunError::UnknownKind { .. }
            | WorkflowRunError::InvalidEmbeddedConfig { .. }
            | WorkflowRunError::NodeConstruction { .. } => (None, FailurePhase::Preparation),
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
        value_type.validate_shared(&value).map_err(|error| {
            NodeExecutionError::ExecutionFailed {
                message: format!("Loop variable `{variable}`: {error}"),
            }
        })?;
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
        let parent_exit_cutoff = std::mem::replace(
            &mut self.scope_exit_cutoff,
            Arc::new(AtomicUsize::new(usize::MAX)),
        );
        self.scopes.push(scope);
        let guard = ScopeGuard {
            context: self,
            parent_outputs,
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

    fn publish<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if let Some(result) = &result {
            for name in &result.skipped {
                if result.outputs.contains_key(name) {
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
                    .map_err(|error| state_error(id, format!("output `{name}`: {error}")))?;
            }
        }
        // Validate the complete result before making any values visible.
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
        Ok(())
    }
}

fn state_error(id: &str, message: impl Into<String>) -> WorkflowRunError {
    WorkflowRunError::Context {
        definition_id: id.into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionDependency<'a> {
    pub input: Option<&'a str>,
    pub source_node: &'a str,
    pub source_output: &'a str,
}

enum StepObservation {
    Root(NodeObservation),
    Body(BodyNodeObservation),
}

impl StepObservation {
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
    execute_ordered_node_in_context(node, dependencies.iter().copied(), ctx)
}

pub fn execute_ordered_node_in_context<'a>(
    node: &crate::TaskFlowNode,
    dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    execute_ordered_task_in_context(node, node.node.as_ref(), dependencies, ctx)
}

pub(super) fn execute_ordered_task_in_context<'a, N>(
    node: &FlowNode<N>,
    task: &dyn crate::TaskNode,
    dependencies: impl IntoIterator<Item = ExecutionDependency<'a>>,
    ctx: &mut ExecutionContext,
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
    let mut inputs = ctx.initial_inputs(id);
    let mut skipped = false;
    let mut causes = BTreeSet::new();
    for dependency in dependencies {
        let value = ctx
            .output(&output_id(dependency.source_node, dependency.source_output))
            .map_err(|error| {
                state_error(
                    id,
                    format!(
                        "dependency {}: {error}",
                        dependency.input.unwrap_or("<control>")
                    ),
                )
            });
        let value = match value {
            Ok(value) => value,
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
        match value {
            ContextValue::Value(value) => {
                if let Some(input) = dependency.input {
                    inputs.insert(input.to_owned(), value.clone());
                }
            }
            ContextValue::Skipped => {
                skipped = true;
                if step.is_some() {
                    causes.insert(SkipCause {
                        source_node: dependency.source_node.into(),
                        source_output: dependency.source_output.into(),
                    });
                }
            }
        }
    }
    let snapshot_inputs = ctx.snapshots.as_ref().map(|_| inputs.clone());
    let result = if skipped {
        None
    } else {
        for (name, value) in &inputs {
            let validation = node
                .metadata
                .ports
                .inputs
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| state_error(id, format!("received undeclared input `{name}`")))
                .and_then(|port| {
                    port.value_type
                        .validate_shared(value)
                        .map_err(|error| state_error(id, format!("input `{name}`: {error}")))
                });
            if let Err(error) = validation {
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
        }
        ctx.capture_node(node, &inputs, None, crate::SnapshotOutcome::Started, None);
        if let Some(step) = step.as_mut() {
            step.started(ctx);
        }
        let result = task
            .execute(inputs, ctx)
            .with_context(|_| NodeExecutionSnafu {
                definition_id: node.definition_id.clone(),
            });
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
            produced_ports.extend(result.outputs.keys().cloned());
            skipped_ports.extend(result.skipped.iter().cloned());
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
        .and_then(|result| result.loop_summary.clone());
    let snapshot_result = ctx.snapshots.as_ref().and_then(|_| result.clone());
    let result = ctx.publish(node, result);
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
        .map_err(|error| state_error(node, format!("workflow output `{name}`: {error}")))?
    {
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
    use crate::{Flow, NodePorts, PortSpec, TaskNode, ValueType, WorkflowOutputDefinition};
    use serde_json::json;
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
            crate::PreparedNode::new(
                EmitNode(Outputs::from([
                    ("good".into(), json!(1).into()),
                    ("bad".into(), json!("wrong").into()),
                ])),
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
        let error = execute_node_in_context(&node, &[], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("producer") && error.contains("output `bad`"));
        assert!(error.contains("expected int64, found string"));
        assert!(context.output("producer.good").is_err());
        assert!(context.output("producer.bad").is_err());
    }

    #[test]
    fn rejects_a_nested_dynamic_input_before_invoking_the_target() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::new(
                CountNode(Arc::clone(&calls)),
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
            input: Some("payload"),
            source_node: "source",
            source_output: "value",
        };
        let error = execute_node_in_context(&node, &[dependency], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("consumer") && error.contains("input `payload`"));
        assert!(error.contains("/1/count") && error.contains("expected int64"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(context.output("consumer.value").is_err());
    }

    #[test]
    fn rejects_an_undeclared_direct_flow_input() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::new(
                CountNode(Arc::clone(&calls)),
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
    fn skips_without_type_checks_but_keeps_missing_output_precedence() {
        let calls = Arc::new(AtomicUsize::new(0));
        let node = FlowNode::new(
            "consumer",
            crate::PreparedNode::new(
                CountNode(Arc::clone(&calls)),
                NodePorts {
                    inputs: vec![port("payload", ValueType::Int64, true)],
                    outputs: vec![port("value", ValueType::Boolean, true)],
                },
            ),
        )
        .into_task()
        .unwrap();
        let skipped = ExecutionDependency {
            input: Some("payload"),
            source_node: "branch",
            source_output: "off",
        };
        let missing = ExecutionDependency {
            input: None,
            source_node: "source",
            source_output: "missing",
        };
        let mut context = ExecutionContext::default();
        context.outputs.insert("branch.off".into(), None);
        let error = execute_node_in_context(&node, &[skipped, missing], &mut context)
            .unwrap_err()
            .to_string();
        assert!(error.contains("source.missing"));
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
        let flow = Flow::new(
            vec![FlowNode::new(
                "source",
                crate::PreparedNode::new(
                    AlternatingNode(AtomicUsize::new(0)),
                    NodePorts {
                        inputs: vec![],
                        outputs: vec![port("value", ValueType::Int64, true)],
                    },
                ),
            )],
            vec![],
            vec!["source".into()],
            vec![WorkflowOutputDefinition {
                name: "result".into(),
                node: "source".into(),
                port: "value".into(),
                optional: false,
            }],
        )
        .unwrap();
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
            let flow = Flow::new(
                vec![
                    FlowNode::new(
                        "source",
                        crate::PreparedNode::new(
                            EmitNode(Outputs::from([("value".into(), value.into())])),
                            NodePorts {
                                inputs: vec![],
                                outputs: vec![port("value", ValueType::Any, true)],
                            },
                        ),
                    ),
                    FlowNode::new(
                        "consumer",
                        crate::PreparedNode::new(
                            CountNode(Arc::clone(&calls)),
                            NodePorts {
                                inputs: vec![port("payload", ValueType::Int64, true)],
                                outputs: vec![port("value", ValueType::Boolean, true)],
                            },
                        ),
                    ),
                ],
                vec![crate::EdgeDefinition {
                    from_node: "source".into(),
                    from_output: "value".into(),
                    to_node: "consumer".into(),
                    to_input: "payload".into(),
                }],
                vec!["source".into(), "consumer".into()],
                vec![WorkflowOutputDefinition {
                    name: "result".into(),
                    node: "consumer".into(),
                    port: "value".into(),
                    optional: false,
                }],
            )
            .unwrap();
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
