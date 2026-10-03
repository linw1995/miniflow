use crate::message_domain::MessageDomains;
use crate::runner::ContextSnafu;
use crate::{
    ExecutionDependency, FlowNode, NodeExecution, STREAM_INPUT_ID, StreamDomain, StreamExecution,
    TaskNode, WorkflowOutputDefinition,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};

type OperatorStates = Vec<Option<NodeExecution>>;

#[derive(Debug, Snafu)]
pub enum StreamBuildError {
    #[snafu(display("invalid streaming workflow: {message}"), visibility(pub))]
    InvalidPlan { message: String },
    #[snafu(display("invalid stream input type: {source}"), visibility(pub))]
    InputType { source: crate::TypeDepthError },
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
}

pub struct PreparedStream {
    plan: StreamPlan,
    operator_states: OperatorStates,
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
        execution.input_type.check_depth().context(InputTypeSnafu)?;
        ensure!(
            nodes.len() == dependencies.len()
                && nodes
                    .first()
                    .is_some_and(|node| node.definition_id.as_str() == STREAM_INPUT_ID),
            InvalidPlanSnafu {
                message: "stream plan requires its typed input source first",
            }
        );
        let source = &nodes[0];
        ensure!(
            source.metadata.ports.inputs.is_empty()
                && source.metadata.ports.outputs.len() == 1
                && source.metadata.ports.outputs[0].name == "item"
                && source.metadata.ports.outputs[0].value_type == execution.input_type
                && source.node.is_none(),
            InvalidPlanSnafu {
                message: "stream input source must expose its declared item type",
            }
        );
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
        })
    }

    pub fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    pub(super) fn into_parts(self) -> (StreamPlan, OperatorStates) {
        (self.plan, self.operator_states)
    }
}

impl StreamPlan {
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
