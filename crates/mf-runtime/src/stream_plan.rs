use crate::message_domain::MessageDomains;
use crate::{
    ExecutionDependency, FlowNode, NodeExecution, STREAM_INPUT_ID, StreamDomain, StreamExecution,
    TaskNode, WorkflowOutputDefinition,
};
use snafu::{ResultExt, Snafu};

type EventStates = Vec<Option<Box<dyn crate::EventNode>>>;

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
    event_states: EventStates,
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
        let invalid = |message: String| InvalidPlanSnafu { message }.build();
        execution.limits.validate().map_err(invalid)?;
        execution.input_type.check_depth().context(InputTypeSnafu)?;
        if nodes.len() != dependencies.len()
            || nodes
                .first()
                .is_none_or(|node| node.definition_id.as_str() != STREAM_INPUT_ID)
        {
            return Err(invalid(
                "stream plan requires its typed input source first".into(),
            ));
        }
        let source = &nodes[0];
        if !source.metadata.ports.inputs.is_empty()
            || source.metadata.ports.outputs.len() != 1
            || source.metadata.ports.outputs[0].name != "item"
            || source.metadata.ports.outputs[0].value_type != execution.input_type
            || source.node.is_some()
        {
            return Err(invalid(
                "stream input source must expose its declared item type".into(),
            ));
        }
        let domains = MessageDomains::new(&nodes, &dependencies, &outputs)?;
        let mut event_states = Vec::with_capacity(nodes.len());
        let nodes = nodes
            .into_iter()
            .map(|node| {
                let (task, state) = match node.node {
                    Some(NodeExecution::Task(task)) => (Some(task), None),
                    Some(NodeExecution::Event(state)) => (None, Some(state)),
                    None => (None, None),
                };
                event_states.push(state);
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
        if plan.execution().limits.max_pending_messages < plan.domains().len() {
            return Err(invalid(format!(
                "max_pending_messages must reserve at least {} domain slots",
                plan.domains().len()
            )));
        }
        Ok(Self { plan, event_states })
    }

    pub fn plan(&self) -> &StreamPlan {
        &self.plan
    }

    pub(super) fn into_parts(self) -> (StreamPlan, EventStates) {
        (self.plan, self.event_states)
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
        let node = self
            .nodes
            .get(index)
            .ok_or_else(|| crate::WorkflowRunError::Context {
                definition_id: "<stream>".into(),
                message: "invalid stream dispatch index".into(),
            })?;
        crate::context::execute_ordered_task_in_context(
            node,
            node.node
                .as_deref()
                .ok_or_else(|| crate::WorkflowRunError::Context {
                    definition_id: node.definition_id.clone(),
                    message: "stream boundary is not a task".into(),
                })?,
            self.dependencies(index)
                .iter()
                .map(StreamDependency::borrowed),
            context,
        )
    }
}
