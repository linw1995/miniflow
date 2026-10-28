use crate::definition::DefinitionId;
use crate::execution_domains::{ExecutionDomain, ExecutionDomains};
use crate::{NodeMetadata, PreparedNode, TaskNode};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc;

/// A compact runtime node index into a `Flow`'s node vector.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct NodeId(usize);

impl NodeId {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0
    }
}

impl From<usize> for NodeId {
    fn from(index: usize) -> Self {
        Self::new(index)
    }
}

pub struct FlowNode<N = Option<crate::NodeExecution>> {
    /// The definition-facing node ID, retained for diagnostics.
    pub definition_id: DefinitionId,
    pub node: N,
    pub metadata: NodeMetadata,
    /// Lazily computed `output_id` keys aligned with `metadata.ports.outputs`.
    #[doc(hidden)]
    pub output_keys: std::sync::OnceLock<Box<[String]>>,
}

pub type TaskFlowNode = FlowNode<Box<dyn TaskNode>>;

impl<N> FlowNode<N> {
    /// Cached `output_id` keys aligned by index with `metadata.ports.outputs`.
    ///
    /// Computed on first access, so node metadata must be final before any
    /// execution-path read.
    pub fn output_keys(&self) -> &[String] {
        self.output_keys.get_or_init(|| {
            self.metadata
                .ports
                .outputs
                .iter()
                .map(|port| crate::output_id(self.definition_id.as_str(), &port.name))
                .collect()
        })
    }

    /// The cached `output_id` key of a declared output port.
    pub fn output_key(&self, port: &str) -> Option<&str> {
        let index = self
            .metadata
            .ports
            .outputs
            .iter()
            .position(|candidate| candidate.name == port)?;
        Some(&self.output_keys()[index])
    }
}

impl FlowNode {
    pub fn new(definition_id: impl Into<DefinitionId>, prepared: PreparedNode) -> Self {
        Self {
            definition_id: definition_id.into(),
            metadata: prepared.metadata,
            node: Some(prepared.execution),
            output_keys: std::sync::OnceLock::new(),
        }
    }

    pub fn into_task(self) -> Option<TaskFlowNode> {
        let mut execution = self.node?;
        if let crate::NodeExecution::TaskStream(task) = &mut execution {
            task.set_stream_contract(&self.metadata.stream_outputs);
        }
        let task = execution.into_task_node()?;
        Some(FlowNode {
            definition_id: self.definition_id,
            metadata: self.metadata,
            node: task,
            output_keys: self.output_keys,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowConnection {
    /// Candidates with the same target input form one exclusive group.
    pub exclusive: bool,
    pub from_node: NodeId,
    pub from_output: Cow<'static, str>,
    pub to_node: NodeId,
    pub to_input: Cow<'static, str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowOutput {
    pub name: Cow<'static, str>,
    pub node_id: NodeId,
    pub port: Cow<'static, str>,
    pub optional: bool,
    /// Precomputed `output_id` of the referenced node port.
    pub key: Cow<'static, str>,
}

pub type FlowOutputs = crate::Outputs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Runtime scheduling limits shared by oneshot and stream workflows.
pub struct RuntimeOptions {
    /// Maximum number of ready execution domains that can run concurrently.
    pub max_parallel_domains: NonZeroUsize,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            max_parallel_domains: NonZeroUsize::new(4).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// Executes prepared oneshot Flows and starts stream instances with one limit policy.
pub struct FlowRuntime {
    options: RuntimeOptions,
}

impl FlowRuntime {
    /// Creates a runtime with explicit domain-concurrency options.
    pub const fn new(options: RuntimeOptions) -> Self {
        Self { options }
    }

    pub const fn options(&self) -> RuntimeOptions {
        self.options
    }

    pub fn execute(&self, flow: &Flow) -> Result<FlowOutputs, crate::WorkflowRunError> {
        self.execute_with_observation(flow, None)
    }

    pub fn execute_with_inputs(
        &self,
        flow: &Flow,
        arguments: crate::WorkflowArguments,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        let mut state = crate::ExecutionContext::default();
        state.set_workflow_arguments(arguments);
        self.execute_in_context(flow, &mut state)
    }

    pub fn execute_with_observation(
        &self,
        flow: &Flow,
        observation: Option<crate::RunObservation>,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        crate::ExecutionContext::run(observation, |state| self.execute_in_context(flow, state))
    }

    pub fn execute_in_context(
        &self,
        flow: &Flow,
        state: &mut crate::ExecutionContext,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        flow.execute_in_context_with_options(state, self.options)
    }

    pub fn start_stream(
        &self,
        prepared: crate::PreparedStream,
        options: crate::StreamOptions,
    ) -> Result<crate::StreamInstance, crate::StreamError> {
        prepared.start_with_runtime_options(options, self.options)
    }
}

pub type GeneratedDomainExecutor = dyn Fn(&Flow, &mut crate::ExecutionContext) -> Result<(), crate::WorkflowRunError>
    + Send
    + Sync;

pub struct Flow {
    inner: Arc<FlowData>,
}

struct FlowData {
    nodes: Vec<TaskFlowNode>,
    plan: FlowPlan,
    input_schema: crate::WorkflowInputSchema,
    generated_domains: BTreeMap<usize, Arc<GeneratedDomainExecutor>>,
    call_interfaces: crate::CallInterfaces,
    calls: Option<crate::invocation::CallTable>,
}

impl Flow {
    fn data_mut(&mut self) -> &mut FlowData {
        Arc::get_mut(&mut self.inner)
            .expect("Flow executors are uniquely owned while binding inputs")
    }

    fn shared_clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// An immutable compiler-validated task graph without executor state.
#[derive(Clone)]
pub struct FlowPlan {
    pub connections: Cow<'static, [FlowConnection]>,
    pub dependencies: Cow<'static, [Cow<'static, [crate::FlowDependency]>]>,
    pub execution_order: Cow<'static, [NodeId]>,
    pub outputs: Cow<'static, [FlowOutput]>,
    pub execution_domains: ExecutionDomains,
}

impl Flow {
    pub fn definition_node_id(&self, id: &NodeId) -> Option<&str> {
        self.inner
            .nodes
            .get(id.index())
            .map(|node| node.definition_id.as_str())
    }

    pub fn node_metadata(&self, id: &NodeId) -> Option<&NodeMetadata> {
        self.inner.nodes.get(id.index()).map(|node| &node.metadata)
    }

    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.inner.nodes.len()).map(NodeId::new)
    }

    pub fn connections(&self) -> &[FlowConnection] {
        &self.inner.plan.connections
    }

    pub fn execution_order(&self) -> &[NodeId] {
        &self.inner.plan.execution_order
    }
}

impl Flow {
    /// Binds task instances to a compiler-validated plan without constructing a graph.
    pub fn from_plan(
        nodes: Vec<TaskFlowNode>,
        plan: FlowPlan,
        input_schema: crate::WorkflowInputSchema,
    ) -> Self {
        Self {
            inner: Arc::new(FlowData {
                nodes,
                plan,
                input_schema,
                generated_domains: BTreeMap::new(),
                call_interfaces: Default::default(),
                calls: None,
            }),
        }
    }
    /// Installs read-only discovery metadata without changing node activation or scheduling.
    pub fn with_call_interfaces(
        mut self,
        interfaces: crate::CallInterfaces,
    ) -> Result<Self, crate::CallInterfaceError> {
        interfaces.validate_callers(self.inner.nodes.iter().map(|node| &node.definition_id))?;
        self.data_mut().call_interfaces = interfaces;
        self.data_mut().calls = None;
        Ok(self)
    }

    /// Host-side compiler contracts; node discovery remains limited to `call_targets`.
    pub fn prepared_calls(&self) -> impl Iterator<Item = &crate::PreparedCall> {
        self.inner
            .calls
            .iter()
            .flat_map(|calls| calls.prepared_calls())
    }

    pub fn call_interfaces(&self) -> &crate::CallInterfaces {
        &self.inner.call_interfaces
    }

    /// Binds compiler-validated typed bodies without changing the domain graph.
    pub fn with_generated_domains(
        mut self,
        domains: impl IntoIterator<Item = (usize, Arc<GeneratedDomainExecutor>)>,
    ) -> Self {
        for (id, executor) in domains {
            let fallback = self.inner.generated_domains.get(&id).cloned();
            self.data_mut().generated_domains.insert(
                id,
                Arc::new(move |flow, state| {
                    if state.snapshot_recorder().is_none() && state.scope_path().is_empty() {
                        executor(flow, state)
                    } else if let Some(fallback) = &fallback {
                        fallback(flow, state)
                    } else {
                        flow.execute_domain_positions(
                            &flow.execution_domains().domains()[id],
                            state,
                        )
                    }
                }),
            );
        }
        self
    }

    /// Binds semantic execution bodies that also run with snapshots and scoped frames.
    pub fn with_prepared_domains(
        mut self,
        domains: impl IntoIterator<Item = (usize, Arc<GeneratedDomainExecutor>)>,
    ) -> Self {
        self.data_mut().generated_domains.extend(domains);
        self
    }

    pub fn execute_position(
        &self,
        position: usize,
        state: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        let node_id = self.inner.plan.execution_order[position];
        let node = &self.inner.nodes[node_id.index()];
        self.execute_generated_position(position, &[], state, |inputs, ctx| {
            node.node
                .execute(inputs, ctx)
                .map(crate::GeneratedNodeResult::Dynamic)
        })
    }

    pub fn execute_generated_position(
        &self,
        position: usize,
        transferred_inputs: &[&str],
        state: &mut crate::ExecutionContext,
        invoke: impl FnOnce(
            crate::Inputs,
            &mut crate::ExecutionContext,
        ) -> Result<crate::GeneratedNodeResult, crate::NodeExecutionError>,
    ) -> Result<(), crate::WorkflowRunError> {
        self.execute_resolved_position(
            position,
            state,
            |node, ctx, inputs| {
                ctx.resolve_node_inputs(
                    node,
                    self.inner.plan.dependencies[position]
                        .iter()
                        .map(crate::FlowDependency::borrowed),
                    transferred_inputs,
                    inputs,
                    BTreeSet::new(),
                )
            },
            invoke,
        )
    }

    pub fn execute_resolved_position(
        &self,
        position: usize,
        state: &mut crate::ExecutionContext,
        resolve: impl FnOnce(
            &TaskFlowNode,
            &crate::ExecutionContext,
            &mut crate::Inputs,
        ) -> Result<crate::InputResolution, crate::WorkflowRunError>,
        invoke: impl FnOnce(
            crate::Inputs,
            &mut crate::ExecutionContext,
        ) -> Result<crate::GeneratedNodeResult, crate::NodeExecutionError>,
    ) -> Result<(), crate::WorkflowRunError> {
        let node = &self.inner.nodes[self.inner.plan.execution_order[position].index()];
        let previous = state.replace_execution_position(Some(position));
        let result = self.with_call_scope(state, |state| {
            crate::execute_resolved_node_in_context(
                node,
                state,
                |ctx, inputs| resolve(node, ctx, inputs),
                invoke,
            )
        });
        state.replace_execution_position(previous);
        result
    }

    pub fn plan(&self) -> &FlowPlan {
        &self.inner.plan
    }
    pub fn nodes(&self) -> &[TaskFlowNode] {
        &self.inner.nodes
    }
    pub fn input_schema(&self) -> &crate::WorkflowInputSchema {
        &self.inner.input_schema
    }
    pub fn with_input_schema(mut self, schema: crate::WorkflowInputSchema) -> Self {
        self.data_mut().input_schema = schema;
        self
    }

    pub fn node(&self, id: &NodeId) -> Option<&dyn TaskNode> {
        self.inner
            .nodes
            .get(id.index())
            .map(|node| node.node.as_ref())
    }

    pub fn execution_domains(&self) -> &ExecutionDomains {
        &self.inner.plan.execution_domains
    }

    pub fn execute_with_inputs(
        &self,
        arguments: crate::WorkflowArguments,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        FlowRuntime::default().execute_with_inputs(self, arguments)
    }

    pub fn execute_with_inputs_and_options(
        &self,
        arguments: crate::WorkflowArguments,
        options: RuntimeOptions,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        FlowRuntime::new(options).execute_with_inputs(self, arguments)
    }

    pub fn execute(&self) -> Result<FlowOutputs, crate::WorkflowRunError> {
        self.execute_with_observation(None)
    }

    pub fn execute_with_options(
        &self,
        options: RuntimeOptions,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        FlowRuntime::new(options).execute(self)
    }

    pub fn execute_with_observation(
        &self,
        observation: Option<crate::RunObservation>,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        FlowRuntime::default().execute_with_observation(self, observation)
    }

    /// Executes already constructed nodes inside a caller-owned run scope.
    pub fn execute_in_context(
        &self,
        state: &mut crate::ExecutionContext,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        FlowRuntime::default().execute_in_context(self, state)
    }

    pub fn execute_in_context_with_options(
        &self,
        state: &mut crate::ExecutionContext,
        options: RuntimeOptions,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        if state.invocation_path().is_empty() {
            state.configure_worker_limit(options.max_parallel_domains);
        }
        if state.scope_path().is_empty() {
            state.bind_workflow_inputs(&self.inner.input_schema)?;
        }
        let domains = &self.inner.plan.execution_domains;
        // Scoped domains retain their ordering for Loop writes and exit cutoffs;
        // parallel work inside those domains still uses the full shared pool.
        let worker_limit = if state.scope_path().is_empty() {
            state.worker_limit().get()
        } else {
            1
        };
        // Nested parallel work initializes this scope's pool on demand and shares
        // it across domains, Loop passes, and Iteration items.
        let _owned_workers = state.worker_scope();
        self.execute_domains(domains, state, worker_limit)?;
        let mut workflow_outputs = FlowOutputs::new();
        for output in self.inner.plan.outputs.iter() {
            let id = self.inner.nodes[output.node_id.index()]
                .definition_id
                .as_str();
            if let Some(value) =
                state.select_output_keyed(&output.name, id, &output.key, output.optional)?
            {
                workflow_outputs.insert(output.name.clone().into_owned(), value);
            }
        }
        Ok(workflow_outputs)
    }

    fn execute_domains(
        &self,
        plan: &ExecutionDomains,
        state: &mut crate::ExecutionContext,
        worker_limit: usize,
    ) -> Result<(), crate::WorkflowRunError> {
        if plan.is_empty() {
            return Ok(());
        }
        if plan.len() == 1 {
            let domain = &plan.domains()[0];
            let mut context = state.fork_domain_with_observation(state.observation().cloned());
            self.execute_domain(domain, &mut context)?;
            state.merge_domain_outputs(
                &context,
                domain
                    .nodes
                    .iter()
                    .map(|node_id| &self.inner.nodes[node_id.index()]),
            );
            return Ok(());
        }
        if worker_limit <= 1 {
            return self.execute_serial_domains(plan, state);
        }
        let worker_limit = worker_limit.min(plan.len());
        let handle = state.initialize_workers()?;
        let worker_limit = worker_limit.min(handle.worker_count());
        let mut failures = Vec::new();
        let mut panics: Vec<(usize, Box<dyn Any + Send>)> = Vec::new();
        let (sender, receiver) = mpsc::channel();
        let mut remaining: Vec<_> = plan
            .domains()
            .iter()
            .map(|domain| domain.predecessors.len())
            .collect();
        let mut ready: BTreeSet<_> = plan
            .domains()
            .iter()
            .filter(|domain| remaining[domain.id] == 0)
            .map(|domain| (domain.first_position, domain.id))
            .collect();
        let mut running = 0;

        loop {
            while failures.is_empty() && panics.is_empty() && running < worker_limit {
                let Some((_, domain_id)) = ready.pop_first() else {
                    break;
                };
                let domain = &plan.domains()[domain_id];
                if domain.first_position > state.scope_exit_cutoff() {
                    release_domain_successors(plan, &mut remaining, &mut ready, domain_id);
                    continue;
                }
                let mut context = state.fork_domain_snapshot(
                    state.observation().cloned(),
                    domain.snapshot_visibility.clone(),
                );
                let sender = sender.clone();
                let parent_context = opentelemetry::Context::current();
                let flow = self.shared_clone();
                let domain = domain.clone();
                let job = crate::worker::WorkerJob::new(move || {
                    let _context = parent_context.attach();
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        flow.execute_domain(&domain, &mut context)
                    }));
                    let _ = sender.send((domain_id, context, result));
                });
                match handle.submit(job) {
                    Ok(()) => running += 1,
                    Err(mpsc::TrySendError::Disconnected(job)) => {
                        job.run();
                        running += 1;
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        unreachable!("blocking worker submission returned a full queue")
                    }
                }
            }

            if running == 0 {
                break;
            }

            let (domain_id, context, result) = loop {
                match receiver.try_recv() {
                    Ok(completion) => break completion,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        unreachable!("an active execution domain must report completion")
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if handle.help_one() {
                            continue;
                        }
                        match receiver.recv_timeout(std::time::Duration::from_millis(10)) {
                            Ok(completion) => break completion,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                unreachable!("an active execution domain must report completion")
                            }
                        }
                    }
                }
            };
            running -= 1;
            match result {
                Ok(Ok(())) if failures.is_empty() && panics.is_empty() => {
                    let domain = &plan.domains()[domain_id];
                    state.merge_domain_outputs(
                        &context,
                        domain
                            .nodes
                            .iter()
                            .map(|node_id| &self.inner.nodes[node_id.index()]),
                    );
                    release_domain_successors(plan, &mut remaining, &mut ready, domain_id);
                }
                Ok(Err(error)) => {
                    failures.push((plan.domains()[domain_id].first_position, error));
                }
                Err(payload) => {
                    panics.push((plan.domains()[domain_id].first_position, payload));
                }
                Ok(Ok(())) => {}
            }
        }
        drop(sender);
        debug_assert!((failures.is_empty() && panics.is_empty()) || running == 0);

        failures.sort_by_key(|(position, _)| *position);
        panics.sort_by_key(|(position, _)| *position);
        if let Some((panic_position, _)) = panics.first()
            && failures
                .first()
                .is_none_or(|(failure_position, _)| panic_position < failure_position)
        {
            let (_, payload) = panics.remove(0);
            std::panic::resume_unwind(payload);
        }
        if let Some((_, error)) = failures.into_iter().next() {
            state.select_observation_failure(&error);
            return Err(error);
        }
        if let Some((_, payload)) = panics.into_iter().next() {
            std::panic::resume_unwind(payload);
        }
        Ok(())
    }

    fn execute_serial_domains(
        &self,
        plan: &ExecutionDomains,
        state: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        for domain in plan.domains() {
            if domain.first_position > state.scope_exit_cutoff() {
                continue;
            }
            let mut context = state.fork_domain_snapshot(
                state.observation().cloned(),
                domain.snapshot_visibility.clone(),
            );
            if let Err(error) = self.execute_domain(domain, &mut context) {
                state.select_observation_failure(&error);
                return Err(error);
            }
            state.merge_domain_outputs(
                &context,
                domain
                    .nodes
                    .iter()
                    .map(|node_id| &self.inner.nodes[node_id.index()]),
            );
        }
        Ok(())
    }

    fn execute_domain(
        &self,
        domain: &ExecutionDomain,
        state: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        self.with_call_scope(state, |state| {
            if let Some(executor) = self.inner.generated_domains.get(&domain.id) {
                return executor(self, state);
            }
            self.execute_domain_positions(domain, state)
        })
    }

    fn execute_domain_positions(
        &self,
        domain: &ExecutionDomain,
        state: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        for &position in domain.positions.iter() {
            if position > state.scope_exit_cutoff() {
                break;
            }
            self.execute_position(position, state)?;
            if state.scope_exit_requested() {
                break;
            }
        }
        Ok(())
    }
}

fn release_domain_successors(
    plan: &ExecutionDomains,
    remaining: &mut [usize],
    ready: &mut BTreeSet<(usize, usize)>,
    domain_id: usize,
) {
    for successor in plan.domains()[domain_id].successors.iter() {
        let successor = &plan.domains()[*successor];
        remaining[successor.id] -= 1;
        if remaining[successor.id] == 0 {
            ready.insert((successor.first_position, successor.id));
        }
    }
}

impl Flow {
    /// Binds executable calls to a plan whose ordinary domains already exclude all targets.
    pub fn with_prepared_calls(
        mut self,
        calls: Vec<crate::PreparedCall>,
    ) -> Result<Self, crate::CallDispatchError> {
        let table =
            crate::invocation::CallTable::new(&self.inner.nodes, self.execution_domains(), calls)?;
        let data = self.data_mut();
        data.call_interfaces = table
            .as_ref()
            .map(|table| table.interfaces().clone())
            .unwrap_or_default();
        data.calls = table;
        Ok(self)
    }

    fn with_call_scope<T, E>(
        &self,
        state: &mut crate::ExecutionContext,
        run: impl FnOnce(&mut crate::ExecutionContext) -> Result<T, E>,
    ) -> Result<T, E> {
        let dispatcher = self.inner.calls.as_ref().map(|_| {
            let dispatcher: Arc<dyn crate::invocation::CallDispatcher> = self.inner.clone();
            Arc::downgrade(&dispatcher)
        });
        state.with_call_dispatcher(&self.inner.call_interfaces, dispatcher, run)
    }
}

impl crate::invocation::CallDispatcher for FlowData {
    fn dispatch(
        &self,
        caller: &crate::DefinitionId,
        id: crate::CallId,
        arguments: crate::CallArguments,
        context: &mut crate::ExecutionContext,
        deadline: Option<std::time::Instant>,
    ) -> Result<crate::CallResponse, crate::CallDispatchError> {
        use snafu::OptionExt;
        self.calls
            .as_ref()
            .context(crate::invocation::UnavailableSnafu)?
            .dispatch(
                caller,
                crate::CallRequest { id, arguments },
                context,
                deadline,
                &self.nodes,
                |node, inputs, context| node.node.execute(inputs, context),
            )
    }
}
