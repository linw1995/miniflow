use crate::execution_domains::{ExecutionDomain, ExecutionDomains};
use crate::message_domain::MessageDomains;
use crate::runner::ContextSnafu;
use crate::{FlowNode, NodeExecution, StreamExecution, TaskNode, WorkflowOutputDefinition};
use snafu::OptionExt;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

type OperatorStates = Vec<Option<NodeExecution>>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

pub struct StreamPlan {
    execution: StreamExecution,
    nodes: Vec<FlowNode<Option<Box<dyn TaskNode>>>>,
    dependencies: Cow<'static, [Cow<'static, [FlowDependency]>]>,
    domains: MessageDomains,
    outputs: Cow<'static, [WorkflowOutputDefinition]>,
    input_schema: crate::WorkflowInputSchema,
    generated_domains: BTreeMap<usize, Arc<GeneratedStreamDomainExecutor>>,
}

pub struct PreparedStream {
    plan: StreamPlan,
    operator_states: OperatorStates,
}

impl std::fmt::Debug for PreparedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedStream")
            .field("execution", &self.plan.execution)
            .field("message_sources", &self.plan.message_sources())
            .field("selected_domain", &self.plan.selected_domain())
            .finish_non_exhaustive()
    }
}

impl PreparedStream {
    /// Binds executor state to an already validated executable layout.
    pub fn from_plan(
        execution: StreamExecution,
        nodes: Vec<FlowNode>,
        dependencies: Cow<'static, [Cow<'static, [FlowDependency]>]>,
        domains: MessageDomains,
        outputs: Cow<'static, [WorkflowOutputDefinition]>,
        input_schema: crate::WorkflowInputSchema,
    ) -> Self {
        let mut operator_states = Vec::with_capacity(nodes.len());
        let nodes = nodes
            .into_iter()
            .map(|node| {
                let (task, state) = match node.node {
                    Some(NodeExecution::Task(task)) => (Some(task), None),
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
        let plan = StreamPlan {
            execution,
            nodes,
            dependencies,
            domains,
            outputs,
            input_schema,
            generated_domains: BTreeMap::new(),
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
    pub fn input_schema(&self) -> &crate::WorkflowInputSchema {
        &self.input_schema
    }
    pub fn execution(&self) -> &StreamExecution {
        &self.execution
    }
    pub fn nodes(&self) -> &[FlowNode<Option<Box<dyn TaskNode>>>] {
        &self.nodes
    }
    pub fn dependencies(&self, node: usize) -> &[FlowDependency] {
        &self.dependencies[node]
    }
    pub fn message_domains(&self) -> &MessageDomains {
        &self.domains
    }
    pub fn message_sources(&self) -> &[Option<usize>] {
        self.domains.sources()
    }
    pub fn execution_domains(&self) -> &ExecutionDomains {
        self.domains.execution_domains()
    }
    pub fn execution_domains_for_message(&self, message_domain: usize) -> &[usize] {
        self.domains.execution_domains_for_message(message_domain)
    }
    pub fn execution_domain(&self, id: usize) -> &ExecutionDomain {
        &self.execution_domains().domains()[id]
    }
    pub fn message_domain_for_execution(&self, id: usize) -> usize {
        self.domains.message_domain_for_execution(id)
    }
    pub fn output_domain(&self, node: usize) -> usize {
        self.domains.output_domain(node)
    }
    pub fn outputs(&self) -> &[WorkflowOutputDefinition] {
        &self.outputs
    }
    pub fn selected_domain(&self) -> Option<usize> {
        self.domains.selected_domain()
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
        crate::execute_resolved_node_in_context(
            node,
            context,
            |ctx, inputs| resolve(node, ctx, inputs),
            |inputs, ctx| {
                task.execute(inputs, ctx)
                    .map(crate::GeneratedNodeResult::Dynamic)
            },
        )
    }

    pub fn execute_domain(
        &self,
        id: usize,
        context: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        if let Some(executor) = self.generated_domains.get(&id) {
            return executor(self, context);
        }
        let domain = self.execution_domain(id);
        for &position in domain.positions.iter() {
            if context.cancellation().failure().is_some() {
                break;
            }
            self.execute_resolved_position(position, context, |node, ctx, inputs| {
                ctx.resolve_node_inputs(
                    node,
                    self.dependencies(position)
                        .iter()
                        .map(FlowDependency::borrowed),
                    &[],
                    inputs,
                    Default::default(),
                )
            })?;
            if context.scope_exit_requested() {
                break;
            }
        }
        Ok(())
    }
}
