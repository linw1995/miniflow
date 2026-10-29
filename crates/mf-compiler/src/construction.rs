use crate::{
    DefinitionId, EdgeDefinition, FlowConnection, FlowDependency, FlowNode, FlowOutput,
    LoopVariableDefinition, NodeBuildError, NodeRegistry, ValueType, WorkflowOutputDefinition,
    output_id,
};
use mf_runtime::{
    ExecutionDomain, ExecutionDomains, NodeExecution, NodeId, StdinRequirement, WorkflowInput,
    WorkflowInputError,
};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};
#[derive(Debug, Snafu)]
pub enum FlowBuildError {
    #[snafu(display("invalid node-success dependency `{from_node}` -> `{to_node}`: {message}"))]
    InvalidSuccessDependency {
        from_node: String,
        to_node: String,
        message: &'static str,
    },
    #[snafu(display("node `{definition_id}` has invalid exclusive outputs: {message}"))]
    InvalidExclusiveOutputs {
        definition_id: DefinitionId,
        message: &'static str,
    },
    #[snafu(display(
        "cannot prove exclusive input `{definition_id}`.`{input}`: `{first_node}`.`{first_output}` and `{second_node}`.`{second_output}` are not proven mutually exclusive"
    ))]
    UnprovenExclusiveInput {
        definition_id: DefinitionId,
        input: String,
        first_node: Box<str>,
        first_output: Box<str>,
        second_node: Box<str>,
        second_output: Box<str>,
    },
    #[snafu(transparent)]
    ExclusiveInput {
        source: mf_runtime::ExclusiveInputError,
    },
    #[snafu(transparent)]
    WorkflowInputs { source: crate::WorkflowInputError },
    #[snafu(display("node `{definition_id}` cannot execute in a synchronous flow"))]
    NonTaskNode { definition_id: DefinitionId },
    #[snafu(display(
        "node `{definition_id}` declares separate stream ports without a combined executor"
    ))]
    InvalidStreamPorts { definition_id: DefinitionId },
    #[snafu(display(
        "stream dependency `{from_node}`.`{port}` -> `{to_node}` requires streaming execution"
    ))]
    SynchronousStreamEdge {
        from_node: String,
        port: String,
        to_node: String,
    },
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
    #[snafu(display("node `{definition_id}` has no declared {direction} port `{port}`"))]
    UnknownPort {
        definition_id: DefinitionId,
        direction: &'static str,
        port: String,
    },
    #[snafu(display("workflow output name `{name}` is selected more than once"))]
    DuplicateOutputName { name: String },
    #[snafu(display("workflow output `{name}` references unknown node `{definition_id}`"))]
    UnknownOutputNode {
        name: String,
        definition_id: DefinitionId,
    },
}

/// Proves alternatives using necessary branch facts, never runtime values.
pub fn validate_exclusive_branches<'a, D>(
    ordered: impl IntoIterator<Item = (&'a FlowNode, D)>,
) -> Result<(), FlowBuildError>
where
    D: IntoIterator<Item = crate::ExecutionDependency<'a>>,
{
    type Facts<'a> = BTreeSet<(&'a str, &'a str)>;
    let mut outputs: BTreeMap<(&str, &str), Facts<'_>> = BTreeMap::new();
    let mut successes: BTreeMap<&str, Facts<'_>> = BTreeMap::new();
    for (node, dependencies) in ordered {
        let id = node.definition_id.as_str();
        let exclusive = &node.metadata.exclusive_outputs;
        if !exclusive.is_empty() {
            ensure!(
                exclusive.len() >= 2
                    && node
                        .node
                        .as_ref()
                        .and_then(NodeExecution::as_task_node)
                        .is_some(),
                InvalidExclusiveOutputsSnafu {
                    definition_id: id,
                    message: "at least two outputs on a task node are required",
                }
            );
            let mut seen = BTreeSet::new();
            for port in exclusive {
                ensure!(
                    seen.insert(port)
                        && node
                            .metadata
                            .ports
                            .outputs
                            .iter()
                            .any(|output| output.name == *port),
                    InvalidExclusiveOutputsSnafu {
                        definition_id: id,
                        message: "ports must be distinct declared outputs",
                    }
                );
            }
        }
        let mut activation = Facts::new();
        let mut groups: BTreeMap<&str, Vec<(crate::ExecutionDependency<'_>, Facts<'_>)>> =
            BTreeMap::new();
        for dependency in dependencies {
            let facts = match dependency.source_output() {
                Some(port) => outputs.get(&(dependency.source_node(), port)),
                None => successes.get(dependency.source_node()),
            }
            .cloned()
            .unwrap_or_default();
            if dependency.exclusive() {
                groups
                    .entry(dependency.input().unwrap_or("<control>"))
                    .or_default()
                    .push((dependency, facts));
            } else {
                activation.extend(facts);
            }
        }
        for (input, candidates) in groups {
            for (index, (first, first_facts)) in candidates.iter().enumerate() {
                for (second, second_facts) in &candidates[index + 1..] {
                    ensure!(
                        first_facts.iter().any(|(router, branch)| second_facts
                            .iter()
                            .any(|(other_router, other_branch)| router == other_router
                                && branch != other_branch)),
                        UnprovenExclusiveInputSnafu {
                            definition_id: id,
                            input,
                            first_node: first.source_node(),
                            first_output: first.source_output().expect("exclusive output source"),
                            second_node: second.source_node(),
                            second_output: second.source_output().expect("exclusive output source"),
                        }
                    );
                }
            }
            if let Some((_, first)) = candidates.first() {
                activation.extend(
                    first
                        .iter()
                        .copied()
                        .filter(|fact| candidates.iter().all(|(_, facts)| facts.contains(fact))),
                );
            }
        }
        // Stateful operators can emit buffered values from a different invocation.
        if node
            .node
            .as_ref()
            .and_then(NodeExecution::as_task_node)
            .is_none()
        {
            activation.clear();
        }
        successes.insert(id, activation.clone());
        for port in node.metadata.output_ports() {
            let mut facts = if node
                .metadata
                .stream_outputs
                .iter()
                .any(|stream| stream.name == port.name)
            {
                Facts::new()
            } else {
                activation.clone()
            };
            if exclusive.iter().any(|name| *name == port.name) {
                facts.insert((id, port.name.as_ref()));
            }
            outputs.insert((id, port.name.as_ref()), facts);
        }
    }
    Ok(())
}

pub struct FlowBuilder {
    nodes: Vec<FlowNode>,
    connections: Vec<FlowConnection>,
    dependencies: Vec<Vec<crate::FlowDependency>>,
    execution_order: Vec<NodeId>,
    outputs: Vec<FlowOutput>,
    controls: Vec<crate::ControlEdgeDefinition>,
    orders: Vec<crate::OrderEdgeDefinition>,
    input_schema: crate::WorkflowInputSchema,
}

impl FlowBuilder {
    pub fn into_tasks(self) -> Result<mf_runtime::Flow, FlowBuildError> {
        for target in &self.nodes {
            for reference in &target.metadata.context_references {
                for source in &self.nodes {
                    for port in &source.metadata.stream_outputs {
                        ensure!(
                            output_id(source.definition_id.as_str(), &port.name)
                                != reference.output,
                            SynchronousStreamEdgeSnafu {
                                from_node: source.definition_id.as_str(),
                                port: port.name.as_ref(),
                                to_node: target.definition_id.as_str(),
                            }
                        );
                    }
                }
            }
        }
        for output in &self.outputs {
            let source = &self.nodes[output.node_id.index()];
            ensure!(
                !source
                    .metadata
                    .stream_outputs
                    .iter()
                    .any(|port| port.name == output.port),
                SynchronousStreamEdgeSnafu {
                    from_node: source.definition_id.as_str(),
                    port: output.port.as_ref(),
                    to_node: "<workflow output>",
                }
            );
        }
        for (target, dependencies) in self.dependencies.iter().enumerate() {
            for dependency in dependencies {
                let edge = dependency.borrowed();
                let Some(port) = edge.source_output() else {
                    continue;
                };
                let source = self
                    .nodes
                    .iter()
                    .find(|node| node.definition_id.as_str() == edge.source_node())
                    .expect("validated source");
                ensure!(
                    !source
                        .metadata
                        .stream_outputs
                        .iter()
                        .any(|output| output.name == port),
                    SynchronousStreamEdgeSnafu {
                        from_node: edge.source_node(),
                        port,
                        to_node: self.nodes[self.execution_order[target].index()]
                            .definition_id
                            .as_str(),
                    }
                );
            }
        }
        validate_exclusive_branches(self.execution_order.iter().zip(&self.dependencies).map(
            |(id, dependencies)| {
                (
                    &self.nodes[id.index()],
                    dependencies.iter().map(crate::FlowDependency::borrowed),
                )
            },
        ))?;
        let guards = crate::dependencies::success_conditions(
            self.execution_order
                .iter()
                .zip(&self.dependencies)
                .map(|(id, deps)| {
                    (
                        self.nodes[id.index()].definition_id.as_str(),
                        deps.iter().map(crate::FlowDependency::borrowed).collect(),
                    )
                }),
        );
        let execution_domains = self.build_execution_domains();
        let data = self;
        let nodes = data
            .nodes
            .into_iter()
            .map(into_task)
            .collect::<Result<_, _>>()?;
        let flow = mf_runtime::Flow::from_plan(
            nodes,
            mf_runtime::FlowPlan {
                connections: data.connections.into(),
                dependencies: data
                    .dependencies
                    .into_iter()
                    .map(|deps| {
                        Cow::Owned(
                            deps.iter()
                                .filter_map(crate::FlowDependency::output_dependency)
                                .collect(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
                execution_order: data.execution_order.into(),
                outputs: data.outputs.into(),
                execution_domains,
            },
            data.input_schema,
        );
        Ok(crate::dependencies::bind_task_guards(flow, guards))
    }
    pub fn into_stream(
        self,
        execution: crate::StreamExecution,
    ) -> Result<crate::PreparedStream, StreamBuildError> {
        let data = self;
        let outputs = data
            .outputs
            .iter()
            .map(|output| WorkflowOutputDefinition {
                name: output.name.clone(),
                node: data.nodes[output.node_id.index()].definition_id.clone(),
                port: output.port.clone(),
                optional: output.optional,
            })
            .collect();
        let mut nodes: Vec<_> = data.nodes.into_iter().map(Some).collect();
        let ordered = data
            .execution_order
            .iter()
            .copied()
            .map(|id| {
                nodes[id.index()]
                    .take()
                    .expect("validated unique execution order")
            })
            .collect();
        let dependencies = data.dependencies;
        prepare_stream(execution, ordered, dependencies, outputs)
    }
}

pub fn build_flow(
    nodes: Vec<FlowNode>,
    connections: Vec<EdgeDefinition>,
    order: Vec<DefinitionId>,
    outputs: Vec<WorkflowOutputDefinition>,
) -> Result<mf_runtime::Flow, FlowBuildError> {
    FlowBuilder::prepare(nodes, connections, order, outputs)?.into_tasks()
}

pub fn into_task(node: FlowNode) -> Result<mf_runtime::TaskFlowNode, FlowBuildError> {
    let definition_id = node.definition_id.clone();
    node.into_task().context(NonTaskNodeSnafu { definition_id })
}
impl FlowBuilder {
    pub fn with_workflow_inputs(mut self) -> Result<Self, crate::WorkflowInputError> {
        let initial: BTreeSet<_> = self
            .execution_order
            .iter()
            .enumerate()
            .filter(|(position, _)| self.dependencies[*position].is_empty())
            .map(|(_, id)| id.index())
            .collect();
        let input_schema = workflow_input_schema(
            self.nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node, initial.contains(&index))),
            |node, input| {
                self.execution_order
                    .iter()
                    .position(|id| self.nodes[id.index()].definition_id.as_str() == node)
                    .is_some_and(|position| {
                        self.dependencies[position]
                            .iter()
                            .any(|dependency| dependency.borrowed().input() == Some(input))
                    })
            },
        )?;
        self.input_schema = input_schema;
        Ok(self)
    }

    pub fn input_schema(&self) -> &crate::WorkflowInputSchema {
        &self.input_schema
    }

    /// Validates graph structure while retaining ownership of each prepared execution kind.
    pub fn prepare(
        nodes: Vec<FlowNode>,
        connections: Vec<EdgeDefinition>,
        execution_order: Vec<DefinitionId>,
        outputs: Vec<WorkflowOutputDefinition>,
    ) -> Result<Self, FlowBuildError> {
        crate::validate_exclusive_inputs(&connections)?;
        let mut definition_indices = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            ensure!(
                node.metadata.stream_outputs.is_empty()
                    || matches!(node.node, Some(NodeExecution::TaskStream(_))),
                InvalidStreamPortsSnafu {
                    definition_id: node.definition_id.clone()
                }
            );
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
            for port in node.metadata.output_ports() {
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

            let exclusive = connection.exclusive;
            if !connected_inputs.insert((to_node, connection.to_input.clone())) && !exclusive {
                return DuplicateInputConnectionSnafu {
                    target_definition_id: connection.to_node,
                    target_input: connection.to_input,
                }
                .fail();
            }

            ensure!(
                nodes[from_node.index()]
                    .metadata
                    .output_ports()
                    .any(|port| port.name == connection.from_output),
                UnknownPortSnafu {
                    definition_id: connection.from_node.clone(),
                    direction: "output",
                    port: connection.from_output.clone(),
                }
            );
            ensure!(
                nodes[to_node.index()]
                    .metadata
                    .ports
                    .inputs
                    .iter()
                    .any(|port| port.name == connection.to_input),
                UnknownPortSnafu {
                    definition_id: connection.to_node.clone(),
                    direction: "input",
                    port: connection.to_input.clone(),
                }
            );

            resolved_connections.push(FlowConnection {
                exclusive,
                from_node,
                from_output: connection.from_output.into(),
                to_node,
                to_input: connection.to_input.into(),
            });
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
            ensure!(
                nodes[node_id.index()]
                    .metadata
                    .output_ports()
                    .any(|port| port.name == output.port),
                UnknownPortSnafu {
                    definition_id: output.node.clone(),
                    direction: "output",
                    port: output.port.clone(),
                }
            );
            let key = mf_runtime::output_id(output.node.as_str(), &output.port);
            resolved_outputs.push(FlowOutput {
                name: output.name,
                node_id,
                port: output.port,
                optional: output.optional,
                key: key.into(),
            });
        }

        let mut flow = Self {
            nodes,
            connections: resolved_connections,
            dependencies: Vec::new(),
            execution_order: resolved_order,
            outputs: resolved_outputs,
            controls: Vec::new(),
            orders: Vec::new(),
            input_schema: crate::WorkflowInputSchema::default(),
        };
        flow.prepare_execution();
        Ok(flow)
    }

    pub fn with_order_edges(
        mut self,
        orders: Vec<crate::OrderEdgeDefinition>,
    ) -> Result<Self, FlowBuildError> {
        let positions: BTreeMap<_, _> = self
            .execution_order
            .iter()
            .enumerate()
            .map(|(position, id)| (self.nodes[id.index()].definition_id.as_str(), position))
            .collect();
        let mut unique = BTreeSet::new();
        for edge in &orders {
            let invalid = |message| {
                InvalidSuccessDependencySnafu {
                    from_node: edge.from_node.as_str(),
                    to_node: edge.to_node.as_str(),
                    message,
                }
                .build()
            };
            let (Some(&from), Some(&to)) = (
                positions.get(edge.from_node.as_str()),
                positions.get(edge.to_node.as_str()),
            ) else {
                return Err(invalid("unknown endpoint"));
            };
            if from >= to || !unique.insert(edge) {
                return Err(invalid(
                    "source must precede target and edges must be unique",
                ));
            }
            validate_success_connection(
                &self.nodes[self.execution_order[from].index()],
                &self.nodes[self.execution_order[to].index()],
            )?;
        }
        self.orders = orders;
        self.prepare_execution();
        if !self.input_schema.inputs.is_empty() {
            self = self.with_workflow_inputs()?;
        }
        Ok(self)
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
            let invalid = || {
                InvalidControlConnectionSnafu {
                message: format!(
                    "`{}`.`{}` -> `{}` must have valid endpoints, precede its target, and be unique",
                    edge.from_node, edge.from_output, edge.to_node
                ),
            }.build()
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
            let source = &self.nodes[self.execution_order[*from].index()];
            ensure!(
                source
                    .metadata
                    .output_ports()
                    .any(|port| port.name == edge.from_output),
                UnknownPortSnafu {
                    definition_id: edge.from_node.clone(),
                    direction: "output",
                    port: edge.from_output.clone(),
                }
            );
        }
        self.controls = controls;
        self.prepare_execution();
        if !self.input_schema.inputs.is_empty() {
            self = self.with_workflow_inputs()?;
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
        for edge in self.connections.iter() {
            let source = &self.nodes[edge.from_node.index()];
            let target = &self.nodes[edge.to_node.index()];
            incoming[positions[target.definition_id.as_str()]].push(
                crate::FlowDependency::Output {
                    exclusive: edge.exclusive,
                    input: Some(edge.to_input.clone()),
                    source_node: source.definition_id.to_string().into(),
                    source_output: edge.from_output.clone(),
                },
            );
        }
        for edge in self.controls.iter() {
            incoming[positions[edge.to_node.as_str()]].push(crate::FlowDependency::Output {
                exclusive: false,
                input: None,
                source_node: edge.from_node.to_string().into(),
                source_output: edge.from_output.clone(),
            });
        }
        for edge in &self.orders {
            incoming[positions[edge.to_node.as_str()]].push(crate::FlowDependency::NodeSuccess {
                source_node: edge.from_node.to_string().into(),
            });
        }
        for dependencies in &mut incoming {
            dependencies.sort();
        }
        self.dependencies = incoming;
    }

    fn build_execution_domains(&self) -> ExecutionDomains {
        let positions: BTreeMap<_, _> = self
            .execution_order
            .iter()
            .enumerate()
            .map(|(position, node_id)| {
                (self.nodes[node_id.index()].definition_id.as_str(), position)
            })
            .collect();
        let mut edges = Vec::new();
        for (target, dependencies) in self.dependencies.iter().enumerate() {
            for dependency in dependencies.iter() {
                if let Some(&source) = positions.get(dependency.borrowed().source_node()) {
                    edges.push((source, target));
                }
            }
        }
        let domains = partition_execution_domains(
            &self.execution_order,
            &edges,
            &vec![false; self.execution_order.len()],
        );
        bind_domain_snapshots(domains, &self.nodes, |_| None)
    }
}
#[derive(Debug, Snafu)]
pub enum WorkflowBuildError {
    #[snafu(transparent)]
    WorkflowInputs { source: crate::WorkflowInputError },
    #[snafu(transparent)]
    FlowBuild { source: FlowBuildError },
    #[snafu(transparent)]
    StreamBuild { source: StreamBuildError },
    #[snafu(
        display("node `{definition_id}` references unavailable kind `{kind}`"),
        visibility(pub)
    )]
    UnknownKind {
        definition_id: DefinitionId,
        kind: String,
    },
    #[snafu(
        display("could not read embedded config for node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    InvalidEmbeddedConfig {
        source: serde_json::Error,
        definition_id: DefinitionId,
    },
    #[snafu(
        display("could not construct node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    NodeConstruction {
        source: NodeBuildError,
        definition_id: DefinitionId,
    },
    #[snafu(
        display("invalid definition for node `{definition_id}`: {message}"),
        visibility(pub)
    )]
    InvalidDefinition {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(
        display("could not resolve metadata for node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    Metadata {
        definition_id: DefinitionId,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[snafu(
        display("could not prepare body of node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    Subgraph {
        definition_id: DefinitionId,
        #[snafu(source(from(WorkflowBuildError, Box::new)))]
        source: Box<WorkflowBuildError>,
    },
}

pub fn instantiate_node_with_metadata(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
) -> Result<crate::FlowNode, WorkflowBuildError> {
    let Some(registration) = registry.get(kind) else {
        return UnknownKindSnafu {
            definition_id: DefinitionId::from(definition_id),
            kind: kind.to_owned(),
        }
        .fail();
    };
    let config: Value = serde_json::from_str(config_json).context(InvalidEmbeddedConfigSnafu {
        definition_id: DefinitionId::from(definition_id),
    })?;
    let node = registration
        .instantiate(config)
        .context(NodeConstructionSnafu {
            definition_id: DefinitionId::from(definition_id),
        })?;
    Ok(crate::FlowNode::new(definition_id, node))
}

pub fn instantiate_subgraph_with_metadata(
    registry: &NodeRegistry,
    definition_id: &str,
    kind: &str,
    config_json: &str,
    options_json: &str,
    body: mf_runtime::PreparedSubgraph,
) -> Result<crate::FlowNode, WorkflowBuildError> {
    let Some(registration) = registry.get(kind) else {
        return UnknownKindSnafu {
            definition_id: DefinitionId::from(definition_id),
            kind: kind.to_owned(),
        }
        .fail();
    };
    let parse = |value| {
        serde_json::from_str(value).context(InvalidEmbeddedConfigSnafu {
            definition_id: DefinitionId::from(definition_id),
        })
    };
    let prepared = registration
        .instantiate_subgraph(
            definition_id,
            parse(config_json)?,
            parse(options_json)?,
            body,
        )
        .context(NodeConstructionSnafu {
            definition_id: DefinitionId::from(definition_id),
        })?;
    Ok(crate::FlowNode::new(definition_id, prepared))
}

pub fn prepared_loop_source_from_json(
    variables_json: &str,
) -> Result<FlowNode, WorkflowBuildError> {
    let variables: Vec<LoopVariableDefinition> =
        serde_json::from_str(variables_json).context(InvalidEmbeddedConfigSnafu {
            definition_id: crate::LOOP_SOURCE_ID,
        })?;
    let types = mf_runtime::loop_variable_types(&variables).or_else(|message| {
        InvalidDefinitionSnafu {
            definition_id: crate::LOOP_SOURCE_ID,
            message,
        }
        .fail()
    })?;
    Ok(mf_runtime::prepared_loop_source_types(&types))
}

pub fn prepared_loop_assign_from_json(
    id: &str,
    variable: &str,
    type_json: &str,
) -> Result<FlowNode, WorkflowBuildError> {
    let descriptor: Value = serde_json::from_str(type_json)
        .context(InvalidEmbeddedConfigSnafu { definition_id: id })?;
    let value_type = ValueType::parse_descriptor(&descriptor).or_else(|message| {
        InvalidDefinitionSnafu {
            definition_id: id,
            message,
        }
        .fail()
    })?;
    Ok(mf_runtime::prepared_loop_assign(id, variable, value_type))
}
#[derive(Debug, Snafu)]
pub enum StreamBuildError {
    #[snafu(transparent)]
    FlowConstruction { source: FlowBuildError },
    #[snafu(display("invalid streaming workflow: {message}"), visibility(pub))]
    InvalidPlan { message: String },
    #[snafu(display("invalid streaming workflow: {source}"), visibility(pub))]
    WorkflowInputs { source: crate::WorkflowInputError },
}

pub fn validate_success_connection(
    source: &FlowNode,
    target: &FlowNode,
) -> Result<(), FlowBuildError> {
    ensure!(
        ordinary_task(source) && ordinary_task(target),
        InvalidSuccessDependencySnafu {
            from_node: source.definition_id.as_str(),
            to_node: target.definition_id.as_str(),
            message: "endpoints must be ordinary Tasks",
        }
    );
    Ok(())
}

fn ordinary_task(node: &FlowNode) -> bool {
    node.node
        .as_ref()
        .and_then(NodeExecution::as_task_node)
        .is_some_and(|task| task.supports_success_dependencies())
}

/// Partitions a validated topological order; boundary nodes form separate domains.
pub fn partition_execution_domains(
    order: &[NodeId],
    edges: &[(usize, usize)],
    boundaries: &[bool],
) -> ExecutionDomains {
    assert_eq!(order.len(), boundaries.len());
    let mut predecessors = vec![BTreeSet::new(); order.len()];
    let mut successors = vec![BTreeSet::new(); order.len()];
    for &(source, target) in edges {
        predecessors[target].insert(source);
        successors[source].insert(target);
    }

    let mut domains: Vec<ExecutionDomain> = Vec::new();
    let mut node_domains = vec![0; order.len()];
    for position in 0..order.len() {
        let append_to = if !boundaries[position] && predecessors[position].len() == 1 {
            let predecessor = *predecessors[position].first().unwrap();
            (!boundaries[predecessor] && successors[predecessor].len() == 1)
                .then_some(node_domains[predecessor])
        } else {
            None
        };

        let domain_id = append_to.unwrap_or_else(|| {
            let domain_id = domains.len();
            domains.push(ExecutionDomain {
                id: domain_id,
                nodes: Vec::new().into(),
                positions: Vec::new().into(),
                predecessors: Vec::new().into(),
                successors: Vec::new().into(),
                first_position: position,
                snapshot_visibility: mf_runtime::OutputVisibility::Borrowed(&[]),
            });
            domain_id
        });
        node_domains[position] = domain_id;
        domains[domain_id].nodes.to_mut().push(order[position]);
        domains[domain_id].positions.to_mut().push(position);
    }

    let mut domain_predecessors = vec![BTreeSet::new(); domains.len()];
    let mut domain_successors = vec![BTreeSet::new(); domains.len()];
    for (target, node_predecessors) in predecessors.iter().enumerate() {
        let target_domain = node_domains[target];
        for &source in node_predecessors {
            let source_domain = node_domains[source];
            if source_domain != target_domain {
                domain_predecessors[target_domain].insert(source_domain);
                domain_successors[source_domain].insert(target_domain);
            }
        }
    }
    for (index, domain) in domains.iter_mut().enumerate() {
        domain.predecessors = domain_predecessors[index]
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .into();
        domain.successors = domain_successors[index]
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .into();
    }
    let mut ancestors = vec![BTreeSet::new(); domains.len()];
    for id in 0..domains.len() {
        for &predecessor in domains[id].predecessors.iter() {
            ancestors[id].insert(predecessor);
            let inherited = ancestors[predecessor].iter().copied().collect::<Vec<_>>();
            ancestors[id].extend(inherited);
        }
    }

    ExecutionDomains::from_parts(
        domains.into(),
        ancestors
            .into_iter()
            .map(|values| values.into_iter().collect::<Vec<_>>().into())
            .collect::<Vec<_>>()
            .into(),
    )
}
/// Freezes inherited output visibility after ports and message ownership are resolved.
pub fn bind_domain_snapshots<N>(
    domains: ExecutionDomains,
    nodes: &[FlowNode<N>],
    message_source: impl Fn(&ExecutionDomain) -> Option<usize>,
) -> ExecutionDomains {
    let visibility = domains
        .domains()
        .iter()
        .map(|domain| {
            let ancestors = domains
                .ancestor_domains(domain.id)
                .iter()
                .flat_map(|&ancestor| domains.domains()[ancestor].nodes.iter().copied());
            let keys: BTreeSet<_> = ancestors
                .chain(message_source(domain).map(NodeId::new))
                .flat_map(|id| {
                    let node = &nodes[id.index()];
                    node.metadata.output_ports().map(move |port| {
                        mf_runtime::output_id(node.definition_id.as_str(), &port.name)
                    })
                })
                .collect();
            mf_runtime::OutputVisibility::from_sorted(keys.into_iter().collect())
        })
        .collect();
    domains.with_snapshot_visibility(visibility)
}

/// Validates message ownership and partitions execution domains for a stream graph.
pub fn build_stream_program(
    nodes: &[FlowNode],
    dependencies: &[Vec<FlowDependency>],
    outputs: &[WorkflowOutputDefinition],
) -> Result<mf_runtime::StreamProgram, StreamBuildError> {
    let mut indices = BTreeMap::new();
    let mut output_index = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        ensure!(
            indices.insert(node.definition_id.as_str(), index).is_none(),
            InvalidPlanSnafu {
                message: format!("duplicate node `{}`", node.definition_id),
            }
        );
        for port in node.metadata.output_ports() {
            let name = output_id(node.definition_id.as_str(), &port.name);
            let stream = node
                .metadata
                .stream_outputs
                .iter()
                .any(|output| output.name == port.name);
            ensure!(
                output_index.insert(name.clone(), (index, stream)).is_none(),
                InvalidPlanSnafu {
                    message: format!("ambiguous output `{name}`"),
                }
            );
        }
    }
    let mut sources = vec![None];
    let mut output_domains = vec![0; nodes.len()];
    let mut stream_output_domains = vec![None; nodes.len()];
    let mut node_message_domains = vec![0; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let mut incoming_domains = BTreeSet::new();
        for dependency in &dependencies[index] {
            let source = indices
                .get(dependency.borrowed().source_node())
                .copied()
                .with_context(|| InvalidPlanSnafu {
                    message: format!(
                        "node `{}` has unknown dependency `{}`",
                        node.definition_id,
                        dependency.borrowed().source_node()
                    ),
                })?;
            ensure!(
                source < index,
                InvalidPlanSnafu {
                    message: format!(
                        "dependency `{}` must precede `{}`",
                        dependency.borrowed().source_node(),
                        node.definition_id
                    ),
                }
            );
            if let Some(port_name) = dependency.borrowed().source_output() {
                ensure!(
                    nodes[source]
                        .metadata
                        .output_ports()
                        .any(|port| port.name == port_name),
                    InvalidPlanSnafu {
                        message: format!(
                            "node `{}` has no output `{}`",
                            dependency.borrowed().source_node(),
                            port_name
                        ),
                    }
                );
            } else {
                ensure!(
                    ordinary_task(&nodes[source]) && ordinary_task(node),
                    InvalidPlanSnafu {
                        message: "node-success endpoints must be ordinary Tasks"
                    }
                );
            }
            if let Some(input) = dependency.borrowed().input() {
                ensure!(
                    node.metadata
                        .ports
                        .inputs
                        .iter()
                        .any(|port| port.name == input),
                    InvalidPlanSnafu {
                        message: format!("node `{}` has no input `{input}`", node.definition_id),
                    }
                );
            }
            let stream = dependency.borrowed().source_output().is_some_and(|port| {
                nodes[source]
                    .metadata
                    .stream_outputs
                    .iter()
                    .any(|output| output.name == port)
            });
            incoming_domains.insert(if stream {
                stream_output_domains[source].expect("connected stream source was planned")
            } else {
                output_domains[source]
            });
        }
        ensure!(incoming_domains.len() <= 1, {
            let edges = dependencies[index]
                .iter()
                .map(|dependency| {
                    format!(
                        "{}.{} -> {}.{}",
                        dependency.borrowed().source_node(),
                        dependency
                            .borrowed()
                            .source_output()
                            .unwrap_or("[node success]"),
                        node.definition_id,
                        dependency.borrowed().input().unwrap_or("<control>"),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            InvalidPlanSnafu {
                message: format!(
                    "node `{}` requires one message domain; incoming domains: {incoming_domains:?}; dependencies: {edges}",
                    node.definition_id
                ),
            }
        });
        let domain = incoming_domains.first().copied().unwrap_or(0);
        ensure!(
            !incoming_domains.is_empty() || !matches!(node.node, Some(NodeExecution::Event(_))),
            InvalidPlanSnafu {
                message: format!(
                    "initial event node `{}` requires an explicit activation source",
                    node.definition_id
                ),
            }
        );
        node_message_domains[index] = domain;
        output_domains[index] = match node.node.as_ref().with_context(|| InvalidPlanSnafu {
            message: format!(
                "node `{}` has no execution implementation",
                node.definition_id
            ),
        })? {
            NodeExecution::Task(_) | NodeExecution::TaskStream(_) => domain,
            NodeExecution::Event(_) | NodeExecution::Stream(_) => {
                let new_domain = sources.len();
                sources.push(Some(index));
                new_domain
            }
        };
        let stream_connected = node.metadata.stream_outputs.iter().any(|port| {
            dependencies.iter().flatten().any(|dependency| {
                let edge = dependency.borrowed();
                edge.source_node() == node.definition_id.as_str()
                    && edge.source_output() == Some(port.name.as_ref())
            }) || outputs
                .iter()
                .any(|output| output.node == node.definition_id && output.port == port.name)
        });
        if stream_connected {
            stream_output_domains[index] = Some(sources.len());
            sources.push(Some(index));
        }
        for reference in &node.metadata.context_references {
            let (producer, stream) =
                output_index
                    .get(&reference.output)
                    .copied()
                    .with_context(|| InvalidPlanSnafu {
                        message: format!(
                            "node `{}` references unknown output `{}`",
                            node.definition_id, reference.output
                        ),
                    })?;
            ensure!(
                (if stream {
                    stream_output_domains[producer]
                } else {
                    Some(output_domains[producer])
                }) == Some(domain)
                    && producer < index,
                InvalidPlanSnafu {
                    message: format!(
                        "node `{}` context reference `{}` crosses a message boundary",
                        node.definition_id, reference.output
                    ),
                }
            );
        }
    }
    let mut selected_outputs = vec![Vec::new(); sources.len()];
    for output in outputs {
        let index = indices
            .get(output.node.as_str())
            .copied()
            .with_context(|| InvalidPlanSnafu {
                message: format!("unknown selected output node `{}`", output.node),
            })?;
        ensure!(
            nodes[index]
                .metadata
                .output_ports()
                .any(|port| port.name == output.port),
            InvalidPlanSnafu {
                message: format!(
                    "selected output `{}` references unknown port `{}.{}`",
                    output.name, output.node, output.port
                ),
            }
        );
        let domain = if nodes[index]
            .metadata
            .stream_outputs
            .iter()
            .any(|port| port.name == output.port)
        {
            stream_output_domains[index].expect("selected stream source was planned")
        } else {
            output_domains[index]
        };
        selected_outputs[domain].push((
            output.name.to_string(),
            output.node.to_string(),
            output_id(output.node.as_str(), &output.port),
            output.optional,
        ));
    }

    let mut execution_edges = Vec::new();
    for (target, node_dependencies) in dependencies.iter().enumerate() {
        for dependency in node_dependencies {
            let source = indices[dependency.borrowed().source_node()];
            if node_message_domains[source] == node_message_domains[target] {
                execution_edges.push((source, target));
            }
        }
    }
    let order: Vec<_> = (0..nodes.len()).map(NodeId::new).collect();
    let boundaries: Vec<_> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            matches!(
                node.node,
                Some(NodeExecution::Event(_) | NodeExecution::Stream(_))
            ) || stream_output_domains[index].is_some()
        })
        .collect();
    let execution_domains = partition_execution_domains(&order, &execution_edges, &boundaries);
    let execution_domains = bind_domain_snapshots(execution_domains, nodes, |execution| {
        sources[node_message_domains[execution.nodes[0].index()]]
    });
    let messages: Vec<_> = selected_outputs
        .into_iter()
        .enumerate()
        .map(|(message, selected)| {
            let select_outputs = (!selected.is_empty()).then(|| {
                std::sync::Arc::new(move |context: &mut mf_runtime::ExecutionContext| {
                    let mut values = mf_runtime::FlowOutputs::new();
                    for (name, node, key, optional) in &selected {
                        if let Some(value) =
                            context.select_output_keyed(name, node, key, *optional)?
                        {
                            values.insert(name.clone(), value);
                        }
                    }
                    Ok(values)
                }) as std::sync::Arc<mf_runtime::StreamOutputSelector>
            });
            mf_runtime::StreamMessageProgram {
                source: sources[message],
                schedule: build_message_schedule(
                    &execution_domains,
                    &node_message_domains,
                    message,
                ),
                select_outputs,
            }
        })
        .collect();
    let emissions = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let terminal = matches!(node.node, Some(NodeExecution::TaskStream(_)));
            let message = if terminal {
                stream_output_domains[index]
            } else {
                sources.iter().position(|source| *source == Some(index))
            }?;
            Some(mf_runtime::StreamEmissionBinding {
                message,
                connected: messages[message].select_outputs.is_some()
                    || !messages[message].schedule.domains.is_empty(),
                terminal,
                initialize: std::sync::Arc::new(move |plan, result| {
                    mf_runtime::ExecutionContext::for_message(plan.emission_node(index), result)
                }),
            })
        })
        .collect();
    let inputs = dependencies
        .iter()
        .enumerate()
        .map(|(index, dependencies)| {
            let dependencies: Vec<_> = dependencies
                .iter()
                .filter_map(crate::FlowDependency::output_dependency)
                .collect();
            std::sync::Arc::new(
                move |plan: &mf_runtime::StreamPlan,
                      context: &mf_runtime::ExecutionContext,
                      inputs: &mut mf_runtime::Inputs,
                      causes| {
                    context.resolve_node_inputs(
                        &plan.nodes()[index],
                        dependencies
                            .iter()
                            .map(mf_runtime::FlowDependency::borrowed),
                        &[],
                        inputs,
                        causes,
                    )
                },
            ) as std::sync::Arc<mf_runtime::StreamInputResolver>
        })
        .collect();
    Ok(mf_runtime::StreamProgram {
        execution_domains,
        messages,
        emissions,
        inputs,
    })
}

fn build_message_schedule(
    execution_domains: &ExecutionDomains,
    node_messages: &[usize],
    message: usize,
) -> mf_runtime::MessageSchedule {
    let domains: Vec<_> = execution_domains
        .domains()
        .iter()
        .filter(|domain| node_messages[domain.nodes[0].index()] == message)
        .map(|domain| domain.id)
        .collect();
    let mut local = vec![usize::MAX; execution_domains.len()];
    for (rank, &domain) in domains.iter().enumerate() {
        local[domain] = rank;
    }
    let initial_deps = domains
        .iter()
        .map(|&domain| {
            execution_domains.domains()[domain]
                .predecessors
                .iter()
                .filter(|&&previous| local[previous] != usize::MAX)
                .count()
        })
        .collect();
    let successors = domains
        .iter()
        .map(|&domain| {
            execution_domains.domains()[domain]
                .successors
                .iter()
                .filter_map(|&next| (local[next] != usize::MAX).then_some(local[next]))
                .collect()
        })
        .collect();
    mf_runtime::MessageSchedule {
        domains: domains.into_boxed_slice(),
        local: local.into_boxed_slice(),
        initial_deps,
        successors,
    }
}

pub fn workflow_input_schema<'a, N: 'a>(
    nodes: impl IntoIterator<Item = (&'a FlowNode<N>, bool)>,
    mut input_bound: impl FnMut(&str, &str) -> bool,
) -> Result<crate::WorkflowInputSchema, WorkflowInputError> {
    let mut schema = crate::WorkflowInputSchema::default();
    let mut stdin_owner = None;
    let mut names = BTreeSet::new();
    for (node, initial) in nodes {
        let id = node.definition_id.as_str();
        if !initial
            && node
                .metadata
                .input_bindings
                .values()
                .any(|binding| binding.env.is_some() || binding.default.is_some())
        {
            return Err(input_schema_error(
                input_pointer("", id),
                "environment and default bindings require an initial node",
            ));
        }
        for (name, binding) in &node.metadata.input_bindings {
            let port = node
                .metadata
                .ports
                .inputs
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| {
                    input_schema_error(input_pointer("", id), "input binding names an unknown port")
                })?;
            binding.validate(&output_id(id, name), &port.value_type)?;
        }
        if initial {
            let mut ports = BTreeMap::new();
            for port in &node.metadata.ports.inputs {
                let name = output_id(id, &port.name);
                let path = input_pointer("", &name);
                if !names.insert(name) {
                    return Err(input_schema_error(path, "ambiguous workflow input name"));
                }
                port.value_type
                    .check_depth()
                    .context(mf_runtime::WorkflowInputTypeDepthSnafu { path: path.clone() })?;
                if port.name.is_empty()
                    || ports
                        .insert(
                            port.name.to_string(),
                            WorkflowInput {
                                value_type: port.value_type.clone(),
                                required: port.required,
                                binding: node
                                    .metadata
                                    .input_bindings
                                    .get(port.name.as_ref())
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        )
                        .is_some()
                {
                    return Err(input_schema_error(path, "empty or duplicate input port"));
                }
            }
            if schema.inputs.insert(id.into(), ports).is_some() {
                return Err(input_schema_error(
                    input_pointer("", id),
                    "duplicate initial node",
                ));
            }
        }
        if let Some(requirement) = &node.metadata.stdin {
            if let StdinRequirement::UnlessInput(input) = requirement {
                if !node
                    .metadata
                    .ports
                    .inputs
                    .iter()
                    .any(|port| port.name == *input)
                {
                    return Err(input_schema_error(
                        input_pointer("", id),
                        "stdin condition names an unknown input",
                    ));
                }
                if !initial && input_bound(id, input) {
                    continue;
                }
            }
            if !initial {
                return Err(input_schema_error(
                    input_pointer("", id),
                    "stdin requires an initial node",
                ));
            }
            if *requirement == StdinRequirement::Always
                && let Some(previous) = stdin_owner.replace(id.to_owned())
            {
                return Err(input_schema_error(
                    input_pointer("", id),
                    format!("stdin is already required by node `{previous}`"),
                ));
            }
            schema.stdin.insert(id.into(), requirement.clone());
        }
    }
    Ok(schema)
}

fn input_pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
fn input_schema_error(path: String, message: impl Into<String>) -> WorkflowInputError {
    mf_runtime::WorkflowInputInvalidSnafu { path, message }.build()
}

pub fn stream_workflow_inputs(
    nodes: &[FlowNode],
    dependencies: &[Cow<'static, [mf_runtime::FlowDependency]>],
    initial: &[bool],
) -> Result<crate::WorkflowInputSchema, WorkflowInputError> {
    workflow_input_schema(
        nodes
            .iter()
            .zip(initial)
            .map(|(node, initial)| (node, *initial)),
        |node, input| {
            nodes
                .iter()
                .position(|candidate| candidate.definition_id.as_str() == node)
                .is_some_and(|index| {
                    dependencies[index]
                        .iter()
                        .any(|dependency| dependency.input.as_deref() == Some(input))
                })
        },
    )
}

pub fn bind_workflow_inputs(
    flow: mf_runtime::Flow,
    initial: &[bool],
) -> Result<mf_runtime::Flow, WorkflowInputError> {
    let order = flow.execution_order();
    let dependencies = &flow.plan().dependencies;
    let schema = workflow_input_schema(
        flow.nodes().iter().enumerate().map(|(index, node)| {
            (
                node,
                initial[order
                    .iter()
                    .position(|id| id.index() == index)
                    .expect("ordered node")],
            )
        }),
        |node, input| {
            order
                .iter()
                .position(|id| flow.nodes()[id.index()].definition_id.as_str() == node)
                .is_some_and(|position| {
                    dependencies[position]
                        .iter()
                        .any(|dependency| dependency.input.as_deref() == Some(input))
                })
        },
    )?;
    Ok(flow.with_input_schema(schema))
}

pub fn validate_stream_limits(limits: &crate::StreamLimits) -> Result<(), StreamBuildError> {
    ensure!(
        ![limits.max_pending_messages, limits.workers].contains(&0),
        InvalidPlanSnafu {
            message: "stream limits must be positive"
        }
    );
    Ok(())
}

pub fn validate_execution(definition: &crate::WorkflowDefinition) -> Result<(), StreamBuildError> {
    if let Some(execution) = &definition.execution {
        ensure!(
            definition.version.supports_startup_inputs(),
            InvalidPlanSnafu {
                message: "stream execution requires workflow schema 2026-10-03"
            }
        );
        validate_stream_limits(&execution.limits)?;
    }
    Ok(())
}

pub fn prepare_stream(
    execution: crate::StreamExecution,
    nodes: Vec<FlowNode>,
    dependencies: Vec<Vec<crate::FlowDependency>>,
    outputs: Vec<WorkflowOutputDefinition>,
) -> Result<mf_runtime::PreparedStream, StreamBuildError> {
    validate_stream_limits(&execution.limits)?;
    ensure!(
        nodes.len() == dependencies.len(),
        InvalidPlanSnafu {
            message: "stream nodes and dependencies must have equal lengths"
        }
    );
    let program = build_stream_program(&nodes, &dependencies, &outputs)?;
    validate_exclusive_branches(nodes.iter().zip(&dependencies).map(|(node, dependencies)| {
        (
            node,
            dependencies.iter().map(crate::FlowDependency::borrowed),
        )
    }))?;
    let reserved_domains = 1 + program
        .messages
        .iter()
        .skip(1)
        .filter(|message| message.select_outputs.is_some() || !message.schedule.domains.is_empty())
        .count();
    ensure!(
        execution.limits.max_pending_messages >= reserved_domains,
        InvalidPlanSnafu {
            message: format!(
                "max_pending_messages must reserve at least {} domain slots",
                reserved_domains
            )
        }
    );
    let guards = crate::dependencies::success_conditions(nodes.iter().zip(&dependencies).map(
        |(node, deps)| {
            (
                node.definition_id.as_str(),
                deps.iter().map(crate::FlowDependency::borrowed).collect(),
            )
        },
    ));
    let initial: Vec<_> = dependencies.iter().map(Vec::is_empty).collect();
    let dependencies: Vec<_> = dependencies
        .into_iter()
        .map(|deps| {
            Cow::Owned(
                deps.iter()
                    .filter_map(crate::FlowDependency::output_dependency)
                    .collect(),
            )
        })
        .collect();
    let schema =
        stream_workflow_inputs(&nodes, &dependencies, &initial).context(WorkflowInputsSnafu)?;
    let prepared = mf_runtime::PreparedStream::from_plan(execution, nodes, program, schema);
    Ok(crate::dependencies::bind_stream_guards(prepared, guards))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeExecutionError;
    use mf_runtime::{FlowOutputs, Inputs, Outputs, RuntimeOptions, TaskNode};
    use serde_json::json;
    use std::num::NonZeroUsize;
    use std::{
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

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

    struct RendezvousNode {
        value: i64,
        gate: Arc<(Mutex<usize>, Condvar)>,
    }

    struct NestedWorkNode {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl TaskNode for NestedWorkNode {
        fn execute(
            &self,
            _inputs: Inputs,
            context: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            let jobs = (0..2)
                .map(|_| {
                    let active = Arc::clone(&self.active);
                    let peak = Arc::clone(&self.peak);
                    move || {
                        let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(running, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        active.fetch_sub(1, Ordering::SeqCst);
                    }
                })
                .collect();
            context.run_parallel(2, |_| jobs)?;
            Ok(Outputs::new().into())
        }
    }

    impl TaskNode for RendezvousNode {
        fn execute(
            &self,
            _inputs: Inputs,
            _ctx: &mut crate::ExecutionContext,
        ) -> Result<crate::NodeResult, NodeExecutionError> {
            let (arrived, changed) = &*self.gate;
            let mut arrived = arrived.lock().unwrap();
            *arrived += 1;
            changed.notify_all();
            let deadline = Instant::now() + Duration::from_secs(2);
            while *arrived < 2 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return mf_runtime::NodeExecutionFailedSnafu {
                        message: "independent domains did not overlap".to_owned(),
                    }
                    .fail();
                }
                let (next, timeout) = changed.wait_timeout(arrived, remaining).unwrap();
                arrived = next;
                if timeout.timed_out() && *arrived < 2 {
                    return mf_runtime::NodeExecutionFailedSnafu {
                        message: "independent domains did not overlap".to_owned(),
                    }
                    .fail();
                }
            }
            Ok((Outputs::from([("value".into(), json!(self.value).into())])).into())
        }
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
                Action::Fail => mf_runtime::NodeExecutionFailedSnafu {
                    message: "deliberate failure".to_owned(),
                }
                .fail(),
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
            crate::PreparedNode::from_parts(
                mf_runtime::NodeExecution::Task(Box::new(TestNode {
                    name,
                    action,
                    trace: Arc::clone(trace),
                })),
                ports,
            ),
        )
    }

    fn edge(from_node: &str, from_output: &str, to_node: &str, to_input: &str) -> EdgeDefinition {
        EdgeDefinition {
            exclusive: false,
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
                        crate::PreparedNode::from_parts(
                            mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
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
                let error = build_flow(
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
        for (reverse, replace) in [(false, false), (true, false), (false, true), (true, true)] {
            let source = |id| {
                FlowNode::new(
                    id,
                    crate::PreparedNode::from_parts(
                        mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
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
            let builder = FlowBuilder::prepare(
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
            let builder = if replace {
                builder.with_control_edges(Vec::new()).unwrap()
            } else {
                builder
            };
            let flow = builder.into_tasks().unwrap();
            for _ in 0..2 {
                let expected = if replace {
                    "deliberate failure"
                } else {
                    "a.value"
                };
                assert!(flow.execute().unwrap_err().to_string().contains(expected));
            }
        }
    }

    fn selected_output(name: &str, node: &str, port: &str) -> WorkflowOutputDefinition {
        WorkflowOutputDefinition {
            name: name.to_owned().into(),
            node: node.into(),
            port: port.to_owned().into(),
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
        let flow = build_flow(
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
        let trace = trace.lock().unwrap();
        assert!(
            trace.as_slice() == ["source-a", "source-b", "join"]
                || trace.as_slice() == ["source-b", "source-a", "join"]
        );
    }

    #[test]
    fn executes_a_linear_flow_in_topological_order_and_collects_outputs() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = build_flow(
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
            flow.execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(1).unwrap(),
            })
            .unwrap(),
            FlowOutputs::from([("result".to_owned(), json!(4).into())])
        );
        assert_eq!(*trace.lock().unwrap(), ["source", "increment"]);
    }

    #[test]
    fn executes_branching_flows_and_joins_values_by_input_port() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = build_flow(
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
            flow.execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(1).unwrap(),
            })
            .unwrap(),
            FlowOutputs::from([("total".to_owned(), json!(22).into())])
        );
        let trace = trace.lock().unwrap();
        assert_eq!(trace.as_slice(), ["source", "left", "right", "sum"]);
    }

    #[test]
    fn runs_independent_domains_concurrently() {
        let gate = Arc::new((Mutex::new(0), Condvar::new()));
        let node = |id, value| {
            FlowNode::new(
                id,
                crate::PreparedNode::from_parts(
                    mf_runtime::NodeExecution::Task(Box::new(RendezvousNode {
                        value,
                        gate: Arc::clone(&gate),
                    })),
                    crate::NodePorts {
                        inputs: Vec::new(),
                        outputs: vec![crate::PortSpec::new(
                            "value",
                            crate::ValueType::Number,
                            true,
                        )],
                    },
                ),
            )
        };
        let flow = build_flow(
            vec![node("left", 3), node("right", 5)],
            Vec::new(),
            order(&["left", "right"]),
            vec![
                selected_output("left", "left", "value"),
                selected_output("right", "right", "value"),
            ],
        )
        .unwrap();

        assert_eq!(
            flow.execute().unwrap(),
            FlowOutputs::from([
                ("left".to_owned(), json!(3).into()),
                ("right".to_owned(), json!(5).into()),
            ])
        );
    }

    #[test]
    fn nested_parallel_work_shares_the_flow_worker_limit() {
        for worker_limit in [1, 2] {
            let active = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let node = |id: &str| {
                FlowNode::new(
                    id,
                    crate::PreparedNode::from_parts(
                        mf_runtime::NodeExecution::Task(Box::new(NestedWorkNode {
                            active: Arc::clone(&active),
                            peak: Arc::clone(&peak),
                        })),
                        crate::NodePorts::default(),
                    ),
                )
            };
            let flow = build_flow(
                vec![node("left"), node("right")],
                Vec::new(),
                order(&["left", "right"]),
                Vec::new(),
            )
            .unwrap();
            flow.execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(worker_limit).unwrap(),
            })
            .unwrap();

            assert_eq!(peak.load(Ordering::SeqCst), worker_limit);
            assert_eq!(active.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn reports_node_failures_with_the_definition_id() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let flow = build_flow(
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
        let error = build_flow(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::from_parts(
                    mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
                    crate::NodePorts::default(),
                ),
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
        let error = build_flow(
            vec![
                FlowNode::new(
                    "source",
                    crate::PreparedNode::from_parts(
                        mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
                        crate::NodePorts::default(),
                    ),
                ),
                FlowNode::new(
                    "sink",
                    crate::PreparedNode::from_parts(
                        mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
                        crate::NodePorts::default(),
                    ),
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
        let connection_error = build_flow(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::from_parts(
                    mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
                    crate::NodePorts::default(),
                ),
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

        let output_error = build_flow(
            vec![FlowNode::new(
                "known",
                crate::PreparedNode::from_parts(
                    mf_runtime::NodeExecution::Task(Box::new(EmptyNode)),
                    crate::NodePorts::default(),
                ),
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
