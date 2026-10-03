use crate::definition::{DefinitionId, EdgeDefinition, WorkflowOutputDefinition};
#[cfg(test)]
use crate::{Inputs, Outputs};
use crate::{NodeMetadata, PreparedNode, TaskNode};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::collections::{BTreeMap, BTreeSet};

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
}

pub type TaskFlowNode = FlowNode<Box<dyn TaskNode>>;

impl FlowNode {
    pub fn new(definition_id: impl Into<DefinitionId>, prepared: PreparedNode) -> Self {
        Self {
            definition_id: definition_id.into(),
            metadata: prepared.metadata,
            node: Some(prepared.execution),
        }
    }

    pub fn into_task(self) -> Result<TaskFlowNode, FlowBuildError> {
        let Some(task) = self.node.and_then(crate::NodeExecution::into_task_node) else {
            return Err(FlowBuildError::NonTaskNode {
                definition_id: self.definition_id,
            });
        };
        Ok(FlowNode {
            definition_id: self.definition_id,
            metadata: self.metadata,
            node: task,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowConnection {
    pub from_node: NodeId,
    pub from_output: String,
    pub to_node: NodeId,
    pub to_input: String,
}

impl FlowConnection {
    fn new(
        from_node: impl Into<NodeId>,
        from_output: impl Into<String>,
        to_node: impl Into<NodeId>,
        to_input: impl Into<String>,
    ) -> Self {
        Self {
            from_node: from_node.into(),
            from_output: from_output.into(),
            to_node: to_node.into(),
            to_input: to_input.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowOutput {
    pub name: String,
    pub node_id: NodeId,
    pub port: String,
    pub optional: bool,
}

impl FlowOutput {
    fn new(
        name: impl Into<String>,
        node_id: impl Into<NodeId>,
        port: impl Into<String>,
        optional: bool,
    ) -> Self {
        Self {
            name: name.into(),
            node_id: node_id.into(),
            port: port.into(),
            optional,
        }
    }
}

pub type FlowOutputs = crate::Outputs;

#[derive(Debug, Snafu)]
pub enum FlowBuildError {
    #[snafu(display("{source}"))]
    WorkflowInputs { source: crate::WorkflowInputError },
    #[snafu(display("node `{definition_id}` cannot execute in a synchronous flow"))]
    NonTaskNode { definition_id: DefinitionId },
    #[snafu(display("execution plan entry {position} references unknown node `{definition_id}`"))]
    UnknownNodeInExecutionOrder {
        definition_id: DefinitionId,
        position: usize,
    },
    #[snafu(display("execution plan executes node `{definition_id}` more than once"))]
    DuplicateExecutionNode { definition_id: DefinitionId },
    #[snafu(display("execution plan omits node `{definition_id}`"))]
    MissingExecutionNode { definition_id: DefinitionId },
    #[snafu(display(
        "connection from `{definition_id}`.`{from_output}` to input `{to_input}` references an unknown source node"
    ))]
    UnknownConnectionSource {
        definition_id: DefinitionId,
        from_output: String,
        to_input: String,
    },
    #[snafu(display(
        "connection from output `{from_output}` to `{definition_id}`.`{to_input}` references an unknown target node"
    ))]
    UnknownConnectionTarget {
        definition_id: DefinitionId,
        from_output: String,
        to_input: String,
    },
    #[snafu(display(
        "node `{target_definition_id}` input `{target_input}` receives multiple connections"
    ))]
    DuplicateInputConnection {
        target_definition_id: DefinitionId,
        target_input: String,
    },
    #[snafu(display(
        "connection from `{source_definition_id}`.`{source_output}` to `{target_definition_id}`.`{target_input}` conflicts with the execution order"
    ))]
    InvalidExecutionOrder {
        source_definition_id: DefinitionId,
        source_output: String,
        target_definition_id: DefinitionId,
        target_input: String,
    },
    #[snafu(display("node definition ID `{definition_id}` is used more than once"))]
    DuplicateDefinitionId { definition_id: DefinitionId },
    #[snafu(display(
        "output ID `{output_id}` collides between `{first_node}`.`{first_port}` and `{second_node}`.`{second_port}`"
    ))]
    OutputIdCollision {
        output_id: String,
        first_node: DefinitionId,
        first_port: Box<str>,
        second_node: DefinitionId,
        second_port: Box<str>,
    },
    #[snafu(display("invalid control connection: {message}"))]
    InvalidControlConnection { message: String },
    #[snafu(display("workflow output name `{name}` is selected more than once"))]
    DuplicateOutputName { name: String },
    #[snafu(display("workflow output `{name}` references unknown node `{definition_id}`"))]
    UnknownOutputNode {
        name: String,
        definition_id: DefinitionId,
    },
}

pub struct Flow<N = Box<dyn TaskNode>> {
    nodes: Vec<FlowNode<N>>,
    connections: Vec<FlowConnection>,
    dependencies: Vec<Vec<PreparedDependency>>,
    execution_order: Vec<NodeId>,
    outputs: Vec<FlowOutput>,
    controls: Vec<crate::ControlEdgeDefinition>,
    input_schema: crate::WorkflowInputSchema,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PreparedDependency {
    input: Option<String>,
    source_node: String,
    source_output: String,
}

impl PreparedDependency {
    fn borrowed(&self) -> crate::ExecutionDependency<'_> {
        crate::ExecutionDependency {
            input: self.input.as_deref(),
            source_node: &self.source_node,
            source_output: &self.source_output,
        }
    }
}

impl Flow<Option<crate::NodeExecution>> {
    pub fn into_tasks(self) -> Result<Flow, FlowBuildError> {
        Ok(Flow {
            nodes: self
                .nodes
                .into_iter()
                .map(FlowNode::into_task)
                .collect::<Result<_, _>>()?,
            connections: self.connections,
            dependencies: self.dependencies,
            execution_order: self.execution_order,
            outputs: self.outputs,
            controls: self.controls,
            input_schema: self.input_schema,
        })
    }

    pub fn into_stream(
        self,
        execution: crate::StreamExecution,
    ) -> Result<crate::PreparedStream, crate::StreamBuildError> {
        let outputs = self
            .outputs
            .iter()
            .map(|output| WorkflowOutputDefinition {
                name: output.name.clone(),
                node: self.nodes[output.node_id.index()].definition_id.clone(),
                port: output.port.clone(),
                optional: output.optional,
            })
            .collect();
        let mut nodes: Vec<_> = self.nodes.into_iter().map(Some).collect();
        let ordered = self
            .execution_order
            .into_iter()
            .map(|id| {
                nodes[id.index()]
                    .take()
                    .expect("validated unique execution order")
            })
            .collect();
        let dependencies = self
            .dependencies
            .into_iter()
            .map(|dependencies| {
                dependencies
                    .into_iter()
                    .map(|dependency| crate::StreamDependency {
                        input: dependency.input,
                        source_node: dependency.source_node,
                        source_output: dependency.source_output,
                    })
                    .collect()
            })
            .collect();
        crate::PreparedStream::new(execution, ordered, dependencies, outputs)
    }
}

impl Flow {
    pub fn new(
        nodes: Vec<FlowNode>,
        connections: Vec<EdgeDefinition>,
        execution_order: Vec<DefinitionId>,
        outputs: Vec<WorkflowOutputDefinition>,
    ) -> Result<Self, FlowBuildError> {
        Flow::prepare(nodes, connections, execution_order, outputs)?.into_tasks()
    }
}

impl<N> Flow<N> {
    pub fn with_workflow_inputs(mut self) -> Result<Self, crate::WorkflowInputError> {
        let initial: BTreeSet<_> = self
            .execution_order
            .iter()
            .enumerate()
            .filter(|(position, _)| self.dependencies[*position].is_empty())
            .map(|(_, id)| id.index())
            .collect();
        self.input_schema = crate::WorkflowInputSchema::from_nodes(
            self.nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node, initial.contains(&index))),
        )?;
        Ok(self)
    }

    pub fn input_schema(&self) -> &crate::WorkflowInputSchema {
        &self.input_schema
    }

    /// Validates graph structure while retaining ownership of each prepared execution kind.
    pub fn prepare(
        nodes: Vec<FlowNode<N>>,
        connections: Vec<EdgeDefinition>,
        execution_order: Vec<DefinitionId>,
        outputs: Vec<WorkflowOutputDefinition>,
    ) -> Result<Self, FlowBuildError> {
        let mut definition_indices = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if definition_indices
                .insert(node.definition_id.clone(), NodeId::new(index))
                .is_some()
            {
                return DuplicateDefinitionIdSnafu {
                    definition_id: node.definition_id.clone(),
                }
                .fail();
            }
        }

        let mut output_ids = BTreeMap::new();
        for node in &nodes {
            for port in &node.metadata.ports.outputs {
                let output_id = crate::output_id(node.definition_id.as_str(), &port.name);
                if let Some((first_node, first_port)) =
                    output_ids.insert(output_id.clone(), (&node.definition_id, &port.name))
                {
                    return OutputIdCollisionSnafu {
                        output_id,
                        first_node: first_node.clone(),
                        first_port: first_port.to_string().into_boxed_str(),
                        second_node: node.definition_id.clone(),
                        second_port: port.name.to_string().into_boxed_str(),
                    }
                    .fail();
                }
            }
        }

        let mut positions = vec![None; nodes.len()];
        let mut resolved_order = Vec::with_capacity(execution_order.len());
        for (position, definition_id) in execution_order.into_iter().enumerate() {
            let Some(&node_id) = definition_indices.get(&definition_id) else {
                return UnknownNodeInExecutionOrderSnafu {
                    definition_id,
                    position: position + 1,
                }
                .fail();
            };
            if positions[node_id.index()].replace(position).is_some() {
                return DuplicateExecutionNodeSnafu { definition_id }.fail();
            }
            resolved_order.push(node_id);
        }

        for (index, node) in nodes.iter().enumerate() {
            if positions[index].is_none() {
                return MissingExecutionNodeSnafu {
                    definition_id: node.definition_id.clone(),
                }
                .fail();
            }
        }

        let mut resolved_connections = Vec::with_capacity(connections.len());
        let mut connected_inputs = BTreeSet::new();
        for connection in connections {
            let Some(&from_node) = definition_indices.get(&connection.from_node) else {
                return UnknownConnectionSourceSnafu {
                    definition_id: connection.from_node,
                    from_output: connection.from_output,
                    to_input: connection.to_input,
                }
                .fail();
            };
            let Some(&to_node) = definition_indices.get(&connection.to_node) else {
                return UnknownConnectionTargetSnafu {
                    definition_id: connection.to_node,
                    from_output: connection.from_output,
                    to_input: connection.to_input,
                }
                .fail();
            };

            if positions[from_node.index()] >= positions[to_node.index()] {
                return InvalidExecutionOrderSnafu {
                    source_definition_id: connection.from_node,
                    source_output: connection.from_output,
                    target_definition_id: connection.to_node,
                    target_input: connection.to_input,
                }
                .fail();
            }

            if !connected_inputs.insert((to_node, connection.to_input.clone())) {
                return DuplicateInputConnectionSnafu {
                    target_definition_id: connection.to_node,
                    target_input: connection.to_input,
                }
                .fail();
            }

            resolved_connections.push(FlowConnection::new(
                from_node,
                connection.from_output,
                to_node,
                connection.to_input,
            ));
        }

        let mut resolved_outputs = Vec::with_capacity(outputs.len());
        let mut output_names = BTreeSet::new();
        for output in outputs {
            let Some(&node_id) = definition_indices.get(&output.node) else {
                return UnknownOutputNodeSnafu {
                    name: output.name,
                    definition_id: output.node,
                }
                .fail();
            };
            if !output_names.insert(output.name.clone()) {
                return DuplicateOutputNameSnafu { name: output.name }.fail();
            }
            resolved_outputs.push(FlowOutput::new(
                output.name,
                node_id,
                output.port,
                output.optional,
            ));
        }

        let mut flow = Self {
            nodes,
            connections: resolved_connections,
            dependencies: Vec::new(),
            execution_order: resolved_order,
            outputs: resolved_outputs,
            controls: Vec::new(),
            input_schema: crate::WorkflowInputSchema::default(),
        };
        flow.prepare_execution();
        Ok(flow)
    }

    pub fn with_control_edges(
        mut self,
        controls: Vec<crate::ControlEdgeDefinition>,
    ) -> Result<Self, FlowBuildError> {
        if self.controls == controls {
            return Ok(self);
        }
        let positions: BTreeMap<_, _> = self
            .execution_order
            .iter()
            .enumerate()
            .map(|(position, id)| (self.nodes[id.index()].definition_id.clone(), position))
            .collect();
        let mut unique = BTreeSet::new();
        for edge in &controls {
            let invalid = || FlowBuildError::InvalidControlConnection {
                message: format!(
                    "`{}`.`{}` -> `{}` must have valid endpoints, precede its target, and be unique",
                    edge.from_node, edge.from_output, edge.to_node
                ),
            };
            let Some(from) = positions.get(&edge.from_node) else {
                return Err(invalid());
            };
            let Some(to) = positions.get(&edge.to_node) else {
                return Err(invalid());
            };
            if from >= to || !unique.insert(edge) {
                return Err(invalid());
            }
        }
        self.controls = controls;
        self.prepare_execution();
        if !self.input_schema.inputs.is_empty() {
            return self
                .with_workflow_inputs()
                .map_err(|source| FlowBuildError::WorkflowInputs { source });
        }
        Ok(self)
    }

    fn prepare_execution(&mut self) {
        let positions: BTreeMap<_, _> = self
            .execution_order
            .iter()
            .enumerate()
            .map(|(position, id)| (self.nodes[id.index()].definition_id.as_str(), position))
            .collect();
        let mut incoming = vec![Vec::new(); self.nodes.len()];
        for edge in &self.connections {
            let source = &self.nodes[edge.from_node.index()];
            let target = &self.nodes[edge.to_node.index()];
            incoming[positions[target.definition_id.as_str()]].push(PreparedDependency {
                input: Some(edge.to_input.clone()),
                source_node: source.definition_id.to_string(),
                source_output: edge.from_output.clone(),
            });
        }
        for edge in &self.controls {
            incoming[positions[edge.to_node.as_str()]].push(PreparedDependency {
                input: None,
                source_node: edge.from_node.to_string(),
                source_output: edge.from_output.clone(),
            });
        }
        for dependencies in &mut incoming {
            dependencies.sort();
        }
        self.dependencies = incoming;
    }

    pub fn definition_node_id(&self, id: &NodeId) -> Option<&str> {
        self.nodes
            .get(id.index())
            .map(|node| node.definition_id.as_str())
    }

    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).map(NodeId::new)
    }

    pub fn connections(&self) -> &[FlowConnection] {
        &self.connections
    }

    pub fn execution_order(&self) -> &[NodeId] {
        &self.execution_order
    }
}

impl Flow {
    pub fn node(&self, id: &NodeId) -> Option<&dyn TaskNode> {
        self.nodes.get(id.index()).map(|node| node.node.as_ref())
    }

    pub fn execute_with_inputs(
        &self,
        arguments: crate::WorkflowArguments,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        self.execute_with_resources(arguments, crate::ExecutionResources::default())
    }

    pub fn execute_with_resources(
        &self,
        arguments: crate::WorkflowArguments,
        resources: crate::ExecutionResources,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        let mut state = crate::ExecutionContext::default();
        state.set_execution_resources(resources, crate::StreamCancellation::default());
        state.set_workflow_arguments(arguments);
        self.execute_in_context(&mut state)
    }

    pub fn execute(&self) -> Result<FlowOutputs, crate::WorkflowRunError> {
        self.execute_with_observation(None)
    }

    pub fn execute_with_observation(
        &self,
        observation: Option<crate::RunObservation>,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        crate::ExecutionContext::run(observation, |state| self.execute_in_context(state))
    }

    /// Executes already constructed nodes inside a caller-owned run scope.
    pub fn execute_in_context(
        &self,
        state: &mut crate::ExecutionContext,
    ) -> Result<FlowOutputs, crate::WorkflowRunError> {
        if state.scope_path().is_empty() {
            state.bind_workflow_inputs(&self.input_schema)?;
        }
        for (position, node_id) in self.execution_order.iter().enumerate() {
            let node = &self.nodes[node_id.index()];
            crate::context::execute_ordered_node_in_context(
                node,
                self.dependencies[position]
                    .iter()
                    .map(PreparedDependency::borrowed),
                state,
            )?;
            if state.scope_exit_requested() {
                break;
            }
        }
        let mut workflow_outputs = FlowOutputs::new();
        for output in &self.outputs {
            let id = self.nodes[output.node_id.index()].definition_id.as_str();
            if let Some(value) =
                state.select_output(&output.name, id, &output.port, output.optional)?
            {
                workflow_outputs.insert(output.name.clone(), value);
            }
        }
        Ok(workflow_outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeExecutionError;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct EmptyNode;

    impl TaskNode for EmptyNode {
        fn execute(
            &self,
            _inputs: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            Ok(Outputs::new().into())
        }
    }

    enum Action {
        Emit {
            output: &'static str,
            value: i64,
        },
        Increment {
            input: &'static str,
            output: &'static str,
        },
        Sum {
            left: &'static str,
            right: &'static str,
            output: &'static str,
        },
        Fail,
    }

    struct TestNode {
        name: &'static str,
        action: Action,
        trace: Arc<Mutex<Vec<&'static str>>>,
    }

    impl TaskNode for TestNode {
        fn execute(
            &self,
            inputs: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            self.trace.lock().unwrap().push(self.name);

            match self.action {
                Action::Emit { output, value } => {
                    Ok((Outputs::from([(output.to_owned(), json!(value).into())])).into())
                }
                Action::Increment { input, output } => {
                    let value = inputs[input].as_i64().unwrap() + 1;
                    Ok((Outputs::from([(output.to_owned(), json!(value).into())])).into())
                }
                Action::Sum {
                    left,
                    right,
                    output,
                } => {
                    let value = inputs[left].as_i64().unwrap() + inputs[right].as_i64().unwrap();
                    Ok((Outputs::from([(output.to_owned(), json!(value).into())])).into())
                }
                Action::Fail => Err(NodeExecutionError::ExecutionFailed {
                    message: "deliberate failure".to_owned(),
                }),
            }
        }
    }

    fn test_node(
        name: &'static str,
        action: Action,
        trace: &Arc<Mutex<Vec<&'static str>>>,
    ) -> FlowNode {
        let mut ports = crate::NodePorts::default();
        match &action {
            Action::Emit { output, .. }
            | Action::Increment { output, .. }
            | Action::Sum { output, .. } => {
                ports
                    .outputs
                    .push(crate::PortSpec::new(output, crate::ValueType::Number, true));
            }
            Action::Fail => {}
        }
        match &action {
            Action::Increment { input, .. } => {
                ports
                    .inputs
                    .push(crate::PortSpec::new(input, crate::ValueType::Number, true));
            }
            Action::Sum { left, right, .. } => {
                ports
                    .inputs
                    .push(crate::PortSpec::new(left, crate::ValueType::Number, true));
                ports
                    .inputs
                    .push(crate::PortSpec::new(right, crate::ValueType::Number, true));
            }
            Action::Fail => {
                ports.inputs.push(crate::PortSpec::new(
                    "value",
                    crate::ValueType::Number,
                    true,
                ));
            }
            Action::Emit { .. } => {}
        }
        FlowNode::new(
            name,
            crate::PreparedNode::new(
                TestNode {
                    name,
                    action,
                    trace: Arc::clone(trace),
                },
                ports,
            ),
        )
    }

    fn edge(from_node: &str, from_output: &str, to_node: &str, to_input: &str) -> EdgeDefinition {
        EdgeDefinition {
            from_node: from_node.into(),
            from_output: from_output.to_owned(),
            to_node: to_node.into(),
            to_input: to_input.to_owned(),
        }
    }

    #[test]
    fn rejects_ambiguous_output_ids_before_direct_execution() {
        for required in [false, true] {
            for reverse in [false, true] {
                let source = |id, port| {
                    FlowNode::new(
                        id,
                        crate::PreparedNode::new(
                            EmptyNode,
                            crate::NodePorts {
                                inputs: Vec::new(),
                                outputs: vec![crate::PortSpec::new(
                                    port,
                                    crate::ValueType::Any,
                                    required,
                                )],
                            },
                        ),
                    )
                };
                let mut nodes = vec![source("a.b", "c"), source("a", "b.c")];
                if reverse {
                    nodes.reverse();
                }
                let error = Flow::new(
                    nodes,
                    Vec::new(),
                    vec!["a".into(), "a.b".into()],
                    Vec::new(),
                )
                .err()
                .unwrap();
                assert!(matches!(
                    &error,
                    FlowBuildError::OutputIdCollision { output_id, .. } if output_id == "a.b.c"
                ));
                let message = error.to_string();
                assert!(message.contains("`a.b`.`c`") && message.contains("`a`.`b.c`"));
            }
        }
    }

    #[test]
    fn prepared_controls_keep_error_order_and_refresh_when_replaced() {
        for reverse in [false, true] {
            let source = |id| {
                FlowNode::new(
                    id,
                    crate::PreparedNode::new(
                        EmptyNode,
                        crate::NodePorts {
                            inputs: Vec::new(),
                            outputs: vec![crate::PortSpec::new(
                                "value",
                                crate::ValueType::Any,
                                false,
                            )],
                        },
                    ),
                )
            };
            let trace = Arc::new(Mutex::new(Vec::new()));
            let mut controls = vec![
                crate::ControlEdgeDefinition {
                    from_node: "b".into(),
                    from_output: "value".into(),
                    to_node: "target".into(),
                },
                crate::ControlEdgeDefinition {
                    from_node: "a".into(),
                    from_output: "value".into(),
                    to_node: "target".into(),
                },
            ];
            if reverse {
                controls.reverse();
            }
            let flow = Flow::new(
                vec![
                    test_node("target", Action::Fail, &trace),
                    source("a"),
                    source("b"),
                ],
                Vec::new(),
                ["b", "a", "target"].into_iter().map(Into::into).collect(),
                Vec::new(),
            )
            .unwrap()
            .with_control_edges(controls)
            .unwrap();
            for _ in 0..2 {
                assert!(flow.execute().unwrap_err().to_string().contains("a.value"));
            }
            let flow = flow.with_control_edges(Vec::new()).unwrap();
            assert!(
                flow.execute()
                    .unwrap_err()
                    .to_string()
                    .contains("deliberate failure")
            );
        }
    }

    fn selected_output(name: &str, node: &str, port: &str) -> WorkflowOutputDefinition {
        WorkflowOutputDefinition {
            name: name.to_owned(),
            node: node.into(),
            port: port.to_owned(),
            optional: false,
        }
    }

    fn order(ids: &[&str]) -> Vec<DefinitionId> {
        ids.iter().copied().map(DefinitionId::from).collect()
    }

    #[test]
    fn routes_named_outputs_to_named_inputs() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let source_a = NodeId::new(0);
        let source_b = NodeId::new(1);
        let join = NodeId::new(2);
        let flow = Flow::new(
            vec![
                test_node(
                    "source-a",
                    Action::Emit {
                        output: "value",
                        value: 3,
                    },
                    &trace,
                ),
                test_node(
                    "source-b",
                    Action::Emit {
                        output: "value",
                        value: 5,
                    },
                    &trace,
                ),
                test_node(
                    "join",
                    Action::Sum {
                        left: "left",
                        right: "right",
                        output: "result",
                    },
                    &trace,
                ),
            ],
            vec![
                edge("source-a", "value", "join", "left"),
                edge("source-b", "value", "join", "right"),
            ],
            order(&["source-a", "source-b", "join"]),
            vec![selected_output("sum", "join", "result")],
        )
        .unwrap();

        assert_eq!(flow.execution_order(), [source_a, source_b, join]);
        assert_eq!(flow.connections().len(), 2);
        assert_eq!(flow.connections()[0].from_node, source_a);
        assert_eq!(flow.connections()[0].to_node, join);
        assert!(flow.node(&join).is_some());
        assert_eq!(flow.definition_node_id(&join), Some("join"));
        assert_eq!(flow.execute().unwrap()["sum"], json!(8));
        assert_eq!(*trace.lock().unwrap(), ["source-a", "source-b", "join"]);
    }

    #[test]
    fn executes_a_linear_flow_in_topological_order_and_collects_outputs() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = Flow::new(
            vec![
                test_node(
                    "source",
                    Action::Emit {
                        output: "value",
                        value: 3,
                    },
                    &trace,
                ),
                test_node(
                    "increment",
                    Action::Increment {
                        input: "value",
                        output: "value",
                    },
                    &trace,
                ),
            ],
            vec![edge("source", "value", "increment", "value")],
            order(&["source", "increment"]),
            vec![selected_output("result", "increment", "value")],
        )
        .unwrap();

        assert_eq!(
            flow.execute().unwrap(),
            FlowOutputs::from([("result".to_owned(), json!(4).into())])
        );
        assert_eq!(*trace.lock().unwrap(), ["source", "increment"]);
    }

    #[test]
    fn executes_branching_flows_and_joins_values_by_input_port() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = Flow::new(
            vec![
                test_node(
                    "source",
                    Action::Emit {
                        output: "value",
                        value: 10,
                    },
                    &trace,
                ),
                test_node(
                    "left",
                    Action::Increment {
                        input: "value",
                        output: "value",
                    },
                    &trace,
                ),
                test_node(
                    "right",
                    Action::Increment {
                        input: "value",
                        output: "value",
                    },
                    &trace,
                ),
                test_node(
                    "sum",
                    Action::Sum {
                        left: "left",
                        right: "right",
                        output: "total",
                    },
                    &trace,
                ),
            ],
            vec![
                edge("source", "value", "left", "value"),
                edge("source", "value", "right", "value"),
                edge("left", "value", "sum", "left"),
                edge("right", "value", "sum", "right"),
            ],
            order(&["source", "left", "right", "sum"]),
            vec![selected_output("total", "sum", "total")],
        )
        .unwrap();

        assert_eq!(
            flow.execute().unwrap(),
            FlowOutputs::from([("total".to_owned(), json!(22).into())])
        );
        assert_eq!(*trace.lock().unwrap(), ["source", "left", "right", "sum"]);
    }

    #[test]
    fn reports_node_failures_with_the_definition_id() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = Flow::new(
            vec![
                test_node(
                    "source",
                    Action::Emit {
                        output: "value",
                        value: 1,
                    },
                    &trace,
                ),
                test_node("broken-step", Action::Fail, &trace),
            ],
            vec![edge("source", "value", "broken-step", "value")],
            order(&["source", "broken-step"]),
            Vec::new(),
        )
        .unwrap();

        let error = flow.execute().unwrap_err();
        assert!(matches!(
            &error,
            crate::WorkflowRunError::NodeExecution { definition_id, .. }
                if definition_id.as_str() == "broken-step"
        ));
        assert_eq!(
            error.to_string(),
            "node `broken-step` failed: node execution failed: deliberate failure"
        );
    }

    #[test]
    fn rejects_unknown_definition_ids_during_construction() {
        let error = Flow::new(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::new(EmptyNode, crate::NodePorts::default()),
            )],
            Vec::new(),
            order(&["missing"]),
            Vec::new(),
        )
        .err()
        .unwrap();

        assert!(matches!(
            &error,
            FlowBuildError::UnknownNodeInExecutionOrder { definition_id, position }
                if definition_id.as_str() == "missing" && *position == 1
        ));
        assert_eq!(
            error.to_string(),
            "execution plan entry 1 references unknown node `missing`"
        );
    }

    #[test]
    fn rejects_non_topological_execution_order_during_construction() {
        let error = Flow::new(
            vec![
                FlowNode::new(
                    "source",
                    crate::PreparedNode::new(EmptyNode, crate::NodePorts::default()),
                ),
                FlowNode::new(
                    "sink",
                    crate::PreparedNode::new(EmptyNode, crate::NodePorts::default()),
                ),
            ],
            vec![edge("source", "value", "sink", "input")],
            order(&["sink", "source"]),
            Vec::new(),
        )
        .err()
        .unwrap();

        assert!(matches!(
            error,
            FlowBuildError::InvalidExecutionOrder { .. }
        ));
    }

    #[test]
    fn rejects_missing_connection_and_output_nodes_during_construction() {
        let connection_error = Flow::new(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::new(EmptyNode, crate::NodePorts::default()),
            )],
            vec![edge("missing", "value", "known", "input")],
            order(&["known"]),
            Vec::new(),
        )
        .err()
        .unwrap();
        assert!(matches!(
            &connection_error,
            FlowBuildError::UnknownConnectionSource { .. }
        ));
        assert!(connection_error.to_string().contains("`missing`"));

        let output_error = Flow::new(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::new(EmptyNode, crate::NodePorts::default()),
            )],
            Vec::new(),
            order(&["known"]),
            vec![selected_output("result", "missing", "value")],
        )
        .err()
        .unwrap();
        assert!(matches!(
            &output_error,
            FlowBuildError::UnknownOutputNode { .. }
        ));
        assert!(output_error.to_string().contains("`missing`"));
    }
}
