use crate::{
    ExecutionDependency, FlowNode, NodeExecution, STREAM_INPUT_ID, StreamExecution, TaskNode,
    WorkflowOutputDefinition, output_id,
};
use snafu::Snafu;
use std::collections::{BTreeMap, BTreeSet};

type EventStates = Vec<Option<Box<dyn crate::EventNode>>>;

#[derive(Debug, Snafu)]
#[snafu(display("invalid streaming workflow: {message}"))]
pub struct StreamBuildError {
    pub message: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDomain {
    pub source: usize,
    pub steps: Vec<usize>,
}

pub struct StreamPlan {
    execution: StreamExecution,
    nodes: Vec<FlowNode<Option<Box<dyn TaskNode>>>>,
    dependencies: Vec<Vec<StreamDependency>>,
    domains: Vec<StreamDomain>,
    output_domains: Vec<usize>,
    outputs: Vec<WorkflowOutputDefinition>,
    selected_domain: Option<usize>,
}

pub struct PreparedStream {
    plan: StreamPlan,
    event_states: EventStates,
}

impl std::fmt::Debug for PreparedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedStream")
            .field("execution", &self.plan.execution)
            .field("domains", &self.plan.domains)
            .field("selected_domain", &self.plan.selected_domain)
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
        let invalid = |message: String| StreamBuildError { message };
        execution.limits.validate().map_err(invalid)?;
        execution
            .input_type
            .check_depth()
            .map_err(|error| invalid(error.to_string()))?;
        if nodes.len() != dependencies.len()
            || nodes
                .first()
                .is_none_or(|node| node.definition_id.as_str() != STREAM_INPUT_ID)
        {
            return Err(invalid(
                "stream plan requires its typed input source first".into(),
            ));
        }
        let mut indices = BTreeMap::new();
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
        let mut output_index = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if indices.insert(node.definition_id.as_str(), index).is_some() {
                return Err(invalid(format!("duplicate node `{}`", node.definition_id)));
            }
            for port in &node.metadata.ports.outputs {
                let name = output_id(node.definition_id.as_str(), &port.name);
                if output_index.insert(name.clone(), index).is_some() {
                    return Err(invalid(format!("ambiguous output `{name}`")));
                }
            }
        }
        let mut domains = vec![StreamDomain {
            source: 0,
            steps: Vec::new(),
        }];
        let mut output_domains = vec![0; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            if index == 0 {
                if !dependencies[index].is_empty() {
                    return Err(invalid(
                        "stream input cannot have incoming dependencies".into(),
                    ));
                }
                continue;
            }
            let mut incoming_domains = BTreeSet::new();
            for dependency in &dependencies[index] {
                let source = indices
                    .get(dependency.source_node.as_str())
                    .copied()
                    .ok_or_else(|| {
                        invalid(format!(
                            "node `{}` has unknown dependency `{}`",
                            node.definition_id, dependency.source_node
                        ))
                    })?;
                if source >= index {
                    return Err(invalid(format!(
                        "dependency `{}` must precede `{}`",
                        dependency.source_node, node.definition_id
                    )));
                }
                incoming_domains.insert(output_domains[source]);
            }
            if incoming_domains.len() != 1 {
                let edges = dependencies[index]
                    .iter()
                    .map(|dependency| {
                        format!(
                            "{}.{} -> {}.{}",
                            dependency.source_node,
                            dependency.source_output,
                            node.definition_id,
                            dependency.input.as_deref().unwrap_or("<control>"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid(format!(
                    "node `{}` requires one message domain and an explicit path from {STREAM_INPUT_ID}; incoming domains: {incoming_domains:?}; dependencies: {edges}",
                    node.definition_id
                )));
            }
            let domain = *incoming_domains.first().unwrap();
            domains[domain].steps.push(index);
            output_domains[index] = match &node.node {
                Some(NodeExecution::Task(_)) => domain,
                Some(NodeExecution::Event(_)) => {
                    let new_domain = domains.len();
                    domains.push(StreamDomain {
                        source: index,
                        steps: Vec::new(),
                    });
                    new_domain
                }
                None => {
                    return Err(invalid(format!(
                        "node `{}` has no execution implementation",
                        node.definition_id
                    )));
                }
            };
            for reference in &node.metadata.context_references {
                let producer = output_index
                    .get(&reference.output)
                    .copied()
                    .ok_or_else(|| {
                        invalid(format!(
                            "node `{}` references unknown output `{}`",
                            node.definition_id, reference.output
                        ))
                    })?;
                if output_domains[producer] != domain || producer >= index {
                    return Err(invalid(format!(
                        "node `{}` context reference `{}` crosses a message boundary",
                        node.definition_id, reference.output
                    )));
                }
            }
        }
        let mut selected_domain = None;
        for output in &outputs {
            let index = indices.get(output.node.as_str()).copied().ok_or_else(|| {
                invalid(format!("unknown selected output node `{}`", output.node))
            })?;
            let domain = output_domains[index];
            if selected_domain.is_some_and(|selected| selected != domain) {
                return Err(invalid(format!(
                    "selected output `{}` belongs to a different message domain",
                    output.name
                )));
            }
            selected_domain = Some(domain);
        }
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
            output_domains,
            outputs,
            selected_domain,
        };
        crate::stream_limits::StreamResources::new(&plan)
            .map_err(|error| invalid(error.to_string()))?;
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
        &self.domains
    }
    pub fn output_domain(&self, node: usize) -> usize {
        self.output_domains[node]
    }
    pub fn outputs(&self) -> &[WorkflowOutputDefinition] {
        &self.outputs
    }
    pub fn selected_domain(&self) -> Option<usize> {
        self.selected_domain
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
