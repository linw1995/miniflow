use crate::message_domain::MessageDomains;
use crate::runner::ContextSnafu;
use crate::{
    ExecutionDependency, FlowNode, NodeExecution, StreamDomain, StreamExecution, TaskNode,
    WorkflowOutputDefinition,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};

type OperatorStates = Vec<Option<NodeExecution>>;

#[derive(Debug, Snafu)]
pub enum StreamBuildError {
    #[snafu(display("invalid streaming workflow: {message}"), visibility(pub))]
    InvalidPlan { message: String },
    #[snafu(display("invalid streaming workflow: {source}"), visibility(pub))]
    WorkflowInputs { source: crate::WorkflowInputError },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDependency {
    pub input: Option<String>,
    pub source_node: String,
    pub source_output: String,
}

impl StreamDependency {
    pub fn borrowed(&self) -> ExecutionDependency<'_> {
        ExecutionDependency {
            input: self.input.as_deref(),
            source_node: &self.source_node,
            source_output: &self.source_output,
        }
    }
}

pub struct StreamPlan {
    execution: StreamExecution,
    nodes: Vec<FlowNode<Option<Box<dyn TaskNode>>>>,
    dependencies: Vec<Vec<StreamDependency>>,
    domains: MessageDomains,
    outputs: Vec<WorkflowOutputDefinition>,
    input_schema: crate::WorkflowInputSchema,
}

pub struct PreparedStream {
    plan: StreamPlan,
    operator_states: OperatorStates,
    resources: crate::ExecutionResources,
}

impl std::fmt::Debug for PreparedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedStream")
            .field("execution", &self.plan.execution)
            .field("domains", &self.plan.domains())
            .field("selected_domain", &self.plan.selected_domain())
            .finish_non_exhaustive()
    }
}

impl PreparedStream {
    /// Consumes nodes with resolved ports in their validated topological order.
    pub fn new(
        execution: StreamExecution,
        nodes: Vec<FlowNode>,
        dependencies: Vec<Vec<StreamDependency>>,
        outputs: Vec<WorkflowOutputDefinition>,
    ) -> Result<Self, StreamBuildError> {
        execution.limits.validate()?;
        ensure!(
            nodes.len() == dependencies.len(),
            InvalidPlanSnafu {
                message: "stream nodes and dependencies must have equal lengths"
            }
        );
        let input_schema = crate::WorkflowInputSchema::from_nodes(
            nodes
                .iter()
                .zip(&dependencies)
                .map(|(node, dependencies)| (node, dependencies.is_empty())),
        )
        .context(WorkflowInputsSnafu)?;
        let domains = MessageDomains::new(&nodes, &dependencies, &outputs)?;
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
        };
        ensure!(
            plan.execution().limits.max_pending_messages >= plan.domains().len(),
            InvalidPlanSnafu {
                message: format!(
                    "max_pending_messages must reserve at least {} domain slots",
                    plan.domains().len()
                ),
            }
        );
        Ok(Self {
            plan,
            operator_states,
            resources: crate::ExecutionResources::default(),
        })
    }

    pub fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    pub fn channel(&mut self, node: &str) -> Result<crate::ChannelSender, crate::StreamError> {
        let source = self
            .plan
            .nodes
            .iter()
            .find(|source| source.definition_id.as_str() == node)
            .filter(|source| {
                source
                    .metadata
                    .resources
                    .contains(&crate::InputResource::Channel)
            })
            .ok_or_else(|| {
                crate::StreamPreparationSnafu {
                    message: format!("node `{node}` does not declare a channel source"),
                }
                .build()
            })?;
        let port = source
            .metadata
            .ports
            .outputs
            .iter()
            .find(|port| port.name == "item")
            .ok_or_else(|| {
                crate::StreamPreparationSnafu {
                    message: format!("channel source `{node}` must declare an item output"),
                }
                .build()
            })?;
        self.resources.channel(
            node,
            port.value_type.clone(),
            self.plan.execution.limits.max_pending_messages,
        )
    }

    pub(super) fn into_parts(self) -> (StreamPlan, OperatorStates, crate::ExecutionResources) {
        (self.plan, self.operator_states, self.resources)
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
    pub fn dependencies(&self, node: usize) -> &[StreamDependency] {
        &self.dependencies[node]
    }
    pub fn domains(&self) -> &[StreamDomain] {
        self.domains.domains()
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

    pub fn execute_step(
        &self,
        index: usize,
        context: &mut crate::ExecutionContext,
    ) -> Result<(), crate::WorkflowRunError> {
        let node = self.nodes.get(index).context(ContextSnafu {
            definition_id: "<stream>",
            message: "invalid stream dispatch index",
        })?;
        crate::context::execute_ordered_task_in_context(
            node,
            node.node.as_deref().with_context(|| ContextSnafu {
                definition_id: node.definition_id.clone(),
                message: "stream boundary is not a task",
            })?,
            self.dependencies(index)
                .iter()
                .map(StreamDependency::borrowed),
            context,
        )
    }
}
