use crate::execution_domains::{ExecutionDomain, ExecutionDomains};
use crate::runner::ContextSnafu;
use crate::{FlowNode, NodeExecution, StreamExecution, TaskNode};
use snafu::OptionExt;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, Weak, atomic::AtomicU64};

type OperatorStates = Vec<Option<NodeExecution>>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDependency {
    /// Candidates with the same target input form one exclusive group.
    pub exclusive: bool,
    pub input: Option<Cow<'static, str>>,
    pub source_node: Cow<'static, str>,
    pub source_output: Cow<'static, str>,
    /// Precomputed `output_id` of the source node port.
    pub source_key: Cow<'static, str>,
}

impl FlowDependency {
    pub fn borrowed(&self) -> crate::ExecutionDependency<'_> {
        crate::ExecutionDependency {
            exclusive: self.exclusive,
            input: self.input.as_deref(),
            source_node: self.source_node.as_ref(),
            source_output: self.source_output.as_ref(),
            source_key: std::borrow::Cow::Borrowed(self.source_key.as_ref()),
        }
    }
}

pub type GeneratedStreamDomainExecutor = dyn Fn(&StreamPlan, &mut crate::ExecutionContext) -> Result<(), crate::WorkflowRunError>
    + Send
    + Sync;

pub type StreamMessageInitializer = dyn Fn(&StreamPlan, crate::NodeResult) -> Result<crate::ExecutionContext, crate::WorkflowRunError>
    + Send
    + Sync;
pub type StreamOutputSelector = dyn Fn(&mut crate::ExecutionContext) -> Result<crate::FlowOutputs, crate::WorkflowRunError>
    + Send
    + Sync;
pub type StreamInputResolver = dyn Fn(
        &StreamPlan,
        &crate::ExecutionContext,
        &mut crate::Inputs,
        std::collections::BTreeSet<crate::SkipCause>,
    ) -> Result<crate::InputResolution, crate::WorkflowRunError>
    + Send
    + Sync;

/// A fixed emission channel bound before an invocation starts.
#[derive(Clone)]
pub struct StreamEmissionBinding {
    pub message: usize,
    pub connected: bool,
    pub terminal: bool,
    pub initialize: Arc<StreamMessageInitializer>,
}

/// Executable entry point and output selector for one message channel.
pub struct StreamMessageProgram {
    /// Initial execution domains that must commit before admitting this message.
    pub startup_dependencies: Box<[usize]>,
    /// Startup evidence imported once into each admitted frame, without skip propagation.
    pub startup_visibility: crate::OutputVisibility,
    pub source: Option<usize>,
    pub schedule: MessageSchedule,
    pub select_outputs: Option<Arc<StreamOutputSelector>>,
}

/// Executable wiring and scheduling layout lowered before business execution.
pub struct StreamProgram {
    /// Union of initial evidence retained for future frames.
    pub startup_visibility: crate::OutputVisibility,
    pub execution_domains: ExecutionDomains,
    pub messages: Vec<StreamMessageProgram>,
    pub emissions: Vec<Option<StreamEmissionBinding>>,
    pub inputs: Vec<Arc<StreamInputResolver>>,
}

/// Precomputed scheduling layout for one message domain.
#[derive(Clone)]
pub struct MessageSchedule {
    /// Execution domain ids in rank order (== execution_domains_for_message).
    pub domains: Box<[usize]>,
    /// Global execution domain -> local rank; usize::MAX when not in this message.
    pub local: Box<[usize]>,
    /// Initial pending predecessor count per local rank (in-message predecessors only).
    pub initial_deps: Box<[usize]>,
    /// Local successor ranks per local rank (in-message successors only).
    pub successors: Box<[Box<[usize]>]>,
}

pub struct StreamPlan {
    execution: StreamExecution,
    nodes: Vec<FlowNode<Option<Box<dyn TaskNode>>>>,
    program: StreamProgram,
    has_startup_dependencies: bool,
    input_schema: crate::WorkflowInputSchema,
    generated_domains: BTreeMap<usize, Arc<GeneratedStreamDomainExecutor>>,
    call_interfaces: crate::CallInterfaces,
    calls: Option<crate::invocation::CallTable>,
    dispatcher: OnceLock<Weak<dyn crate::invocation::CallDispatcher>>,
    call_sequence: Arc<AtomicU64>,
    emission_nodes: Vec<Option<FlowNode<()>>>,
}

pub struct PreparedStream {
    plan: StreamPlan,
    operator_states: OperatorStates,
}

impl std::fmt::Debug for PreparedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedStream")
            .field("execution", &self.plan.execution)
            .field(
                "message_sources",
                &self.plan.message_sources().collect::<Vec<_>>(),
            )
            .field(
                "selected_domains",
                &self.plan.selected_domains().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl PreparedStream {
    pub fn with_prepared_calls(
        mut self,
        calls: Vec<crate::PreparedCall>,
    ) -> Result<Self, crate::CallDispatchError> {
        for call in &calls {
            snafu::ensure!(
                self.task_node(call.binding.caller.index()).is_some(),
                crate::invocation::InvalidPlanSnafu {
                    message: "stream caller must have a task executor"
                }
            );
            snafu::ensure!(
                self.plan
                    .nodes
                    .get(call.binding.target.index())
                    .is_some_and(|node| node.node.is_some()),
                crate::invocation::InvalidPlanSnafu {
                    message: "stream call target must be a terminal task"
                }
            );
        }
        let table = crate::invocation::CallTable::new(
            &self.plan.nodes,
            &self.plan.program.execution_domains,
            calls,
        )?;
        self.plan.call_interfaces = table
            .as_ref()
            .map(|table| table.interfaces().clone())
            .unwrap_or_default();
        self.plan.calls = table;
        Ok(self)
    }

    pub fn task_node(&self, node: usize) -> Option<&dyn TaskNode> {
        self.plan
            .nodes
            .get(node)?
            .node
            .as_deref()
            .or_else(|| self.operator_states.get(node)?.as_ref()?.as_task_node())
    }
    /// Binds executor state to an already validated executable layout.
    pub fn from_plan(
        execution: StreamExecution,
        nodes: Vec<FlowNode>,
        program: StreamProgram,
        input_schema: crate::WorkflowInputSchema,
    ) -> Self {
        let mut operator_states = Vec::with_capacity(nodes.len());
        let emission_nodes = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                if !matches!(
                    node.node,
                    Some(NodeExecution::Event(_) | NodeExecution::Stream(_))
                ) && program.emissions[index].is_none()
                {
                    return None;
                }
                let outputs = if matches!(node.node, Some(NodeExecution::TaskStream(_))) {
                    node.metadata.stream_outputs.clone()
                } else {
                    node.metadata.ports.outputs.clone()
                };
                // Emitted records need only their output contract for validation and publication.
                let metadata = crate::NodeMetadata::new(crate::NodePorts {
                    inputs: Vec::new(),
                    outputs,
                });
                Some(FlowNode {
                    definition_id: node.definition_id.clone(),
                    node: (),
                    metadata,
                    output_keys: Default::default(),
                })
            })
            .collect();
        let nodes: Vec<FlowNode<Option<Box<dyn TaskNode>>>> = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| {
                let (task, state) = match node.node {
                    Some(NodeExecution::Task(task)) => (Some(task), None),
                    Some(NodeExecution::TaskStream(mut task)) => {
                        task.set_stream_contract(&node.metadata.stream_outputs);
                        if program.emissions[index]
                            .as_ref()
                            .is_some_and(|route| route.terminal)
                        {
                            (None, Some(NodeExecution::TaskStream(task)))
                        } else {
                            (Some(task as Box<dyn TaskNode>), None)
                        }
                    }
                    Some(state @ (NodeExecution::Event(_) | NodeExecution::Stream(_))) => {
                        (None, Some(state))
                    }
                    None => (None, None),
                };
                operator_states.push(state);
                FlowNode {
                    definition_id: node.definition_id,
                    metadata: node.metadata,
                    node: task,
                    output_keys: node.output_keys,
                }
            })
            .collect();
        let has_startup_dependencies = program
            .messages
            .iter()
            .any(|message| !message.startup_dependencies.is_empty());
        let plan = StreamPlan {
            execution,
            has_startup_dependencies,
            nodes,
            program,
            input_schema,
            generated_domains: BTreeMap::new(),
            call_interfaces: Default::default(),
            calls: None,
            dispatcher: OnceLock::new(),
            call_sequence: Arc::new(AtomicU64::new(0)),
            emission_nodes,
        };
        Self {
            plan,
            operator_states,
        }
    }

    pub fn with_prepared_domains(
        mut self,
        domains: impl IntoIterator<Item = (usize, Arc<GeneratedStreamDomainExecutor>)>,
    ) -> Self {
        self.plan.generated_domains.extend(domains);
        self
    }

    pub fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    pub(super) fn into_parts(self) -> (StreamPlan, OperatorStates) {
        (self.plan, self.operator_states)
    }
}

impl StreamPlan {
    pub fn call_interfaces(&self) -> &crate::CallInterfaces {
        &self.call_interfaces
    }

    pub fn emission_node(&self, node: usize) -> &FlowNode<()> {
        self.emission_nodes[node]
            .as_ref()
            .expect("message source has an emission contract")
    }

    pub fn emission_binding(&self, node: usize) -> &StreamEmissionBinding {
        self.program.emissions[node]
            .as_ref()
            .expect("emitting node has a bound channel")
    }

    pub fn has_emission_consumers(&self, node: usize) -> bool {
        self.program.emissions[node]
            .as_ref()
            .is_some_and(|route| route.connected)
    }

    pub(super) fn has_startup_dependencies(&self) -> bool {
        self.has_startup_dependencies
    }

    pub fn program(&self) -> &StreamProgram {
        &self.program
    }

    pub fn resolve_inputs(
        &self,
        node: usize,
        context: &crate::ExecutionContext,
        inputs: &mut crate::Inputs,
        causes: std::collections::BTreeSet<crate::SkipCause>,
    ) -> Result<crate::InputResolution, crate::WorkflowRunError> {
        (self.program.inputs[node])(self, context, inputs, causes)
    }

    pub fn resolve_event_inputs(
        &self,
        node: usize,
        context: &mut crate::ExecutionContext,
    ) -> Result<(crate::Inputs, crate::InputResolution), crate::WorkflowRunError> {
        context.resolve_event_inputs(&self.nodes[node], |context, inputs| {
            self.resolve_inputs(node, context, inputs, Default::default())
        })
    }

    pub fn selects_outputs(&self, message: usize) -> bool {
        self.program.messages[message].select_outputs.is_some()
    }

    pub fn select_outputs(
        &self,
        message: usize,
        context: &mut crate::ExecutionContext,
    ) -> Result<crate::FlowOutputs, crate::WorkflowRunError> {
        (self.program.messages[message]
            .select_outputs
            .as_ref()
            .expect("selected message has an output function"))(context)
    }

    pub fn input_schema(&self) -> &crate::WorkflowInputSchema {
        &self.input_schema
    }
    pub fn execution(&self) -> &StreamExecution {
        &self.execution
    }
    pub fn nodes(&self) -> &[FlowNode<Option<Box<dyn TaskNode>>>] {
        &self.nodes
    }
    pub fn message_sources(
        &self,
    ) -> impl ExactSizeIterator<Item = Option<usize>> + DoubleEndedIterator + '_ {
        self.program.messages.iter().map(|message| message.source)
    }
    pub fn message_schedule(&self, message: usize) -> &MessageSchedule {
        &self.program.messages[message].schedule
    }
    pub fn execution_domains(&self) -> &ExecutionDomains {
        &self.program.execution_domains
    }
    pub fn execution_domains_for_message(&self, message: usize) -> &[usize] {
        &self.message_schedule(message).domains
    }
    pub fn execution_domain(&self, id: usize) -> &ExecutionDomain {
        &self.execution_domains().domains()[id]
    }
    pub fn selected_domains(&self) -> impl Iterator<Item = usize> + '_ {
        self.program
            .messages
            .iter()
            .enumerate()
            .filter_map(|(message, entry)| entry.select_outputs.is_some().then_some(message))
    }

    pub fn execute_resolved_position(
        &self,
        position: usize,
        context: &mut crate::ExecutionContext,
        resolve: impl FnOnce(
            &FlowNode<Option<Box<dyn TaskNode>>>,
            &crate::ExecutionContext,
            &mut crate::Inputs,
        ) -> Result<crate::InputResolution, crate::WorkflowRunError>,
    ) -> Result<(), crate::WorkflowRunError> {
        let node = self.nodes.get(position).context(ContextSnafu {
            definition_id: "<stream>",
            message: "invalid stream domain dispatch index",
        })?;
        let task = node.node.as_deref().with_context(|| ContextSnafu {
            definition_id: node.definition_id.clone(),
            message: "stream execution domain contains a non-task node",
        })?;
        self.with_call_scope(context, |context| {
            crate::execute_resolved_node_in_context(
                node,
                context,
                |ctx, inputs| resolve(node, ctx, inputs),
                |inputs, ctx| {
                    task.execute(inputs, ctx)
                        .map(crate::GeneratedNodeResult::Dynamic)
                },
            )
        })
    }

    pub fn execute_domain(
        &self,
        id: usize,
        context: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        self.with_call_scope(context, |context| {
            if let Some(executor) = self.generated_domains.get(&id) {
                return executor(self, context);
            }
            let domain = self.execution_domain(id);
            for &position in domain.positions.iter() {
                if context.cancellation().failure().is_some() {
                    break;
                }
                self.execute_resolved_position(position, context, |node, ctx, inputs| {
                    let _ = node;
                    self.resolve_inputs(position, ctx, inputs, Default::default())
                })?;
                if context.scope_exit_requested() {
                    break;
                }
            }
            Ok(())
        })
    }
}

impl StreamPlan {
    pub fn prepared_calls(&self) -> impl Iterator<Item = &crate::PreparedCall> {
        self.calls.iter().flat_map(|calls| calls.prepared_calls())
    }

    pub(super) fn initialize_calls(self: &Arc<Self>) {
        if self.calls.is_some() {
            let dispatcher: Arc<dyn crate::invocation::CallDispatcher> = self.clone();
            let _ = self.dispatcher.set(Arc::downgrade(&dispatcher));
        }
    }

    pub(super) fn install_call_scope(&self, context: &mut crate::ExecutionContext) {
        context.set_call_scope(
            self.call_interfaces.clone(),
            self.dispatcher.get().cloned(),
            self.call_sequence.clone(),
        );
    }

    fn with_call_scope<T, E>(
        &self,
        context: &mut crate::ExecutionContext,
        run: impl FnOnce(&mut crate::ExecutionContext) -> Result<T, E>,
    ) -> Result<T, E> {
        context.with_call_dispatcher(&self.call_interfaces, self.dispatcher.get().cloned(), run)
    }
}

impl crate::invocation::CallDispatcher for StreamPlan {
    fn dispatch(
        &self,
        caller: &crate::DefinitionId,
        id: crate::CallId,
        arguments: crate::CallArguments,
        context: &mut crate::ExecutionContext,
        deadline: Option<std::time::Instant>,
    ) -> Result<crate::CallResponse, crate::CallDispatchError> {
        self.calls
            .as_ref()
            .context(crate::invocation::UnavailableSnafu)?
            .dispatch(
                caller,
                crate::CallRequest { id, arguments },
                context,
                deadline,
                &self.nodes,
                |node, inputs, context| {
                    node.node
                        .as_deref()
                        .expect("validated call target")
                        .execute(inputs, context)
                },
            )
    }
}
