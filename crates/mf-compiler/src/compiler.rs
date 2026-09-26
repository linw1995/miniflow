use crate::definition::{DefinitionId, WorkflowDefinition};
use crate::{
    CompiledWorkflow, Flow, FlowBuildError, FlowNode, NodeBuildError, NodeRegistry, ValueType,
};
use snafu::{ResultExt, Snafu};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CyclePath(Vec<DefinitionId>);

impl CyclePath {
    pub fn nodes(&self) -> &[DefinitionId] {
        &self.0
    }
}

impl fmt::Display for CyclePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, node) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" -> ")?;
            }
            write!(formatter, "{node}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum WorkflowCompileError {
    #[snafu(display("node at position {position} has a blank definition ID"))]
    InvalidNodeId { position: usize },
    #[snafu(display("node definition ID `{definition_id}` is used more than once"))]
    DuplicateNodeId { definition_id: DefinitionId },
    #[snafu(display("node `{definition_id}` references unknown kind `{kind}`"))]
    UnknownNodeKind {
        definition_id: DefinitionId,
        kind: String,
    },
    #[snafu(display("failed to construct node `{definition_id}` of kind `{kind}`: {source}"))]
    NodeConstruction {
        source: NodeBuildError,
        definition_id: DefinitionId,
        kind: String,
    },
    #[snafu(display(
        "connection from `{from_node}`.`{from_output}` to `{to_node}`.`{to_input}` references an unknown source node"
    ))]
    UnknownEdgeSource {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        to_input: String,
    },
    #[snafu(display(
        "connection from `{from_node}`.`{from_output}` to `{to_node}`.`{to_input}` references an unknown target node"
    ))]
    UnknownEdgeTarget {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        to_input: String,
    },
    #[snafu(display(
        "connection from `{from_node}`.`{from_output}` to `{to_node}`.`{to_input}` references a missing output port"
    ))]
    UnknownOutputPort {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        to_input: String,
    },
    #[snafu(display(
        "connection from `{from_node}`.`{from_output}` to `{to_node}`.`{to_input}` references a missing input port"
    ))]
    UnknownInputPort {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        to_input: String,
    },
    #[snafu(display(
        "cannot connect `{from_node}`.`{from_output}` ({output_type}) to `{to_node}`.`{to_input}` ({input_type})"
    ))]
    IncompatiblePortTypes {
        from_node: DefinitionId,
        from_output: String,
        output_type: ValueType,
        to_node: DefinitionId,
        to_input: String,
        input_type: ValueType,
    },
    #[snafu(display("node `{node_id}` input `{port}` receives multiple connections"))]
    DuplicateInputConnection { node_id: DefinitionId, port: String },
    #[snafu(display("node `{node_id}` requires input `{port}`"))]
    MissingRequiredInput { node_id: DefinitionId, port: String },
    #[snafu(display("workflow output `{name}` references unknown node `{node_id}`"))]
    UnknownWorkflowOutputNode { name: String, node_id: DefinitionId },
    #[snafu(display(
        "workflow output `{name}` references missing port `{port}` on node `{node_id}`"
    ))]
    UnknownWorkflowOutputPort {
        name: String,
        node_id: DefinitionId,
        port: String,
    },
    #[snafu(display("workflow output name `{name}` is used more than once"))]
    DuplicateWorkflowOutputName { name: String },
    #[snafu(display("workflow contains a cycle: {path}"))]
    Cycle { path: CyclePath },
    #[snafu(display("validated workflow could not be constructed: {source}"))]
    FlowConstruction { source: FlowBuildError },
    #[snafu(display("compiled workflow execution order does not match its definition"))]
    NonCanonicalPlanOrder,
    #[snafu(display("invalid node metadata for `{definition_id}`: {message}"))]
    InvalidNodeMetadata {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display(
        "invalid control edge `{from_node}`.`{from_output}` -> `{to_node}`: {message}"
    ))]
    InvalidControlEdge {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        message: String,
    },
}

pub fn validate_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<(), WorkflowCompileError> {
    prepare_definition(definition, registry).map(|_| ())
}

fn prepare_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    let order = structural_order(definition)?;
    let nodes = resolve_nodes(definition, registry)?;
    let registrations: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| (node.definition_id.clone(), node.ports.as_ref().unwrap()))
        .collect();
    let invalid = |id: &DefinitionId, message: String| WorkflowCompileError::InvalidNodeMetadata {
        definition_id: id.clone(),
        message,
    };
    let mut output_index = BTreeMap::new();
    for node in &nodes {
        let ports = registrations[&node.definition_id];
        for (direction, specs) in [("input", &ports.inputs), ("output", &ports.outputs)] {
            let mut names = BTreeSet::new();
            for port in specs {
                if port.name.is_empty() || !names.insert(&port.name) {
                    return Err(invalid(
                        &node.definition_id,
                        format!("empty or duplicate {direction} port `{}`", port.name),
                    ));
                }
            }
        }
        for port in &ports.outputs {
            let key = crate::output_id(node.definition_id.as_str(), &port.name);
            if let Some((previous_node, previous_port)) =
                output_index.insert(key.clone(), (&node.definition_id, &port.name))
            {
                return Err(invalid(
                    &node.definition_id,
                    format!(
                        "output ID `{key}` collides between `{previous_node}`.`{previous_port}` and `{}`.`{}`",
                        node.definition_id, port.name
                    ),
                ));
            }
        }
    }
    let pairs = dependency_pairs(definition);
    let mut ancestors: BTreeMap<DefinitionId, BTreeSet<DefinitionId>> = BTreeMap::new();
    for id in &order {
        let mut sources = BTreeSet::new();
        for (from, to) in &pairs {
            if to == id {
                sources.insert(from.clone());
                sources.extend(ancestors[from].iter().cloned());
            }
        }
        ancestors.insert(id.clone(), sources);
    }
    for node in &nodes {
        for reference in &node.references {
            let Some((producer, _)) = output_index.get(&reference.output) else {
                return Err(invalid(
                    &node.definition_id,
                    format!(
                        "reference `{}` ({}) is unknown",
                        reference.output, reference.label
                    ),
                ));
            };
            if !ancestors[&node.definition_id].contains(*producer) {
                return Err(invalid(
                    &node.definition_id,
                    format!(
                        "reference `{}` ({}) requires an explicit dependency on producer `{producer}`",
                        reference.output, reference.label
                    ),
                ));
            }
        }
    }
    for edge in &definition.control_edges {
        if !registrations[&edge.from_node]
            .outputs
            .iter()
            .any(|port| port.name == edge.from_output)
        {
            return Err(control_error(edge, "unknown source output"));
        }
    }

    let mut connected_inputs = BTreeSet::new();
    for edge in &definition.edges {
        let source = registrations[&edge.from_node];
        let target = registrations[&edge.to_node];
        let Some(output_port) = source
            .outputs
            .iter()
            .find(|port| port.name == edge.from_output)
        else {
            return UnknownOutputPortSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
            }
            .fail();
        };
        let Some(input_port) = target.inputs.iter().find(|port| port.name == edge.to_input) else {
            return UnknownInputPortSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
            }
            .fail();
        };
        if !output_port
            .value_type
            .is_assignable_to(input_port.value_type)
        {
            return IncompatiblePortTypesSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                output_type: output_port.value_type,
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
                input_type: input_port.value_type,
            }
            .fail();
        }
        connected_inputs.insert((edge.to_node.clone(), edge.to_input.clone()));
    }

    for node in &definition.nodes {
        let registration = registrations[&node.id];
        for port in registration.inputs.iter().filter(|port| port.required) {
            if !connected_inputs.contains(&(node.id.clone(), port.name.to_owned())) {
                return MissingRequiredInputSnafu {
                    node_id: node.id.clone(),
                    port: port.name.to_owned(),
                }
                .fail();
            }
        }
    }

    for output in &definition.outputs {
        let registration = registrations[&output.node];
        if !registration
            .outputs
            .iter()
            .any(|port| port.name == output.port)
        {
            return UnknownWorkflowOutputPortSnafu {
                name: output.name.clone(),
                node_id: output.node.clone(),
                port: output.port.clone(),
            }
            .fail();
        }
    }

    Ok((nodes, order))
}

pub fn topological_order(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Vec<DefinitionId>, WorkflowCompileError> {
    prepare_definition(definition, registry).map(|(_, order)| order)
}

fn validate_structure(definition: &WorkflowDefinition) -> Result<(), WorkflowCompileError> {
    let mut ids = BTreeSet::new();
    for (index, node) in definition.nodes.iter().enumerate() {
        if node.id.as_str().trim().is_empty() {
            return InvalidNodeIdSnafu {
                position: index + 1,
            }
            .fail();
        }
        if !ids.insert(node.id.clone()) {
            return DuplicateNodeIdSnafu {
                definition_id: node.id.clone(),
            }
            .fail();
        }
    }
    let mut connected = BTreeSet::new();
    for edge in &definition.edges {
        if !ids.contains(&edge.from_node) {
            return UnknownEdgeSourceSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
            }
            .fail();
        }
        if !ids.contains(&edge.to_node) {
            return UnknownEdgeTargetSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
            }
            .fail();
        }
        if !connected.insert((&edge.to_node, &edge.to_input)) {
            return DuplicateInputConnectionSnafu {
                node_id: edge.to_node.clone(),
                port: edge.to_input.clone(),
            }
            .fail();
        }
    }
    let mut controls = BTreeSet::new();
    for edge in &definition.control_edges {
        if !ids.contains(&edge.from_node) || !ids.contains(&edge.to_node) {
            return Err(control_error(edge, "unknown source or target node"));
        }
        if !controls.insert(edge) {
            return Err(control_error(edge, "duplicate control edge"));
        }
    }
    let mut output_names = BTreeSet::new();
    for output in &definition.outputs {
        if !output_names.insert(&output.name) {
            return DuplicateWorkflowOutputNameSnafu {
                name: output.name.clone(),
            }
            .fail();
        }
        if !ids.contains(&output.node) {
            return UnknownWorkflowOutputNodeSnafu {
                name: output.name.clone(),
                node_id: output.node.clone(),
            }
            .fail();
        }
    }
    Ok(())
}

fn control_error(edge: &crate::ControlEdgeDefinition, message: &str) -> WorkflowCompileError {
    WorkflowCompileError::InvalidControlEdge {
        from_node: edge.from_node.clone(),
        from_output: edge.from_output.clone(),
        to_node: edge.to_node.clone(),
        message: message.into(),
    }
}

fn dependency_pairs(definition: &WorkflowDefinition) -> BTreeSet<(DefinitionId, DefinitionId)> {
    definition
        .edges
        .iter()
        .map(|edge| (edge.from_node.clone(), edge.to_node.clone()))
        .chain(
            definition
                .control_edges
                .iter()
                .map(|edge| (edge.from_node.clone(), edge.to_node.clone())),
        )
        .collect()
}

pub fn structural_order(
    definition: &WorkflowDefinition,
) -> Result<Vec<DefinitionId>, WorkflowCompileError> {
    validate_structure(definition)?;
    let mut indegree = BTreeMap::new();
    let mut outgoing: BTreeMap<DefinitionId, Vec<DefinitionId>> = BTreeMap::new();
    let mut incoming: BTreeMap<DefinitionId, Vec<DefinitionId>> = BTreeMap::new();
    for node in &definition.nodes {
        indegree.insert(node.id.clone(), 0usize);
        outgoing.insert(node.id.clone(), Vec::new());
        incoming.insert(node.id.clone(), Vec::new());
    }
    for (from, to) in dependency_pairs(definition) {
        *indegree.get_mut(&to).unwrap() += 1;
        outgoing.get_mut(&from).unwrap().push(to.clone());
        incoming.get_mut(&to).unwrap().push(from);
    }
    for neighbors in outgoing.values_mut() {
        neighbors.sort();
    }
    for predecessors in incoming.values_mut() {
        predecessors.sort();
    }

    let mut ready: BTreeSet<DefinitionId> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = Vec::with_capacity(definition.nodes.len());
    while let Some(node_id) = ready.pop_first() {
        order.push(node_id.clone());
        for target_id in &outgoing[&node_id] {
            let degree = indegree.get_mut(target_id).unwrap();
            *degree -= 1;
            if *degree == 0 {
                ready.insert(target_id.clone());
            }
        }
    }

    if order.len() != definition.nodes.len() {
        let remaining: BTreeSet<DefinitionId> = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect();
        return CycleSnafu {
            path: find_cycle(&incoming, &remaining),
        }
        .fail();
    }

    Ok(order)
}

fn find_cycle(
    incoming: &BTreeMap<DefinitionId, Vec<DefinitionId>>,
    remaining: &BTreeSet<DefinitionId>,
) -> CyclePath {
    let mut seen = BTreeMap::new();
    let mut backward_path: Vec<DefinitionId> = Vec::new();
    let mut current = remaining.iter().next().unwrap().clone();

    loop {
        if let Some(&start) = seen.get(&current) {
            let mut cycle = backward_path[start..].to_vec();
            cycle.reverse();
            let first = cycle
                .iter()
                .enumerate()
                .min_by_key(|(_, node)| *node)
                .unwrap()
                .0;
            cycle.rotate_left(first);
            cycle.push(cycle[0].clone());
            return CyclePath(cycle);
        }
        seen.insert(current.clone(), backward_path.len());
        backward_path.push(current.clone());
        current = incoming[&current]
            .iter()
            .find(|predecessor| remaining.contains(*predecessor))
            .unwrap()
            .clone();
    }
}

pub fn compile_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let (nodes, execution_order) = prepare_definition(definition, registry)?;
    Flow::new(
        nodes,
        definition.edges.clone(),
        execution_order.clone(),
        definition.outputs.clone(),
    )
    .context(FlowConstructionSnafu)?;

    normalize_plan(definition, execution_order)
}

/// Plans graph structure without loading plugins. Validate the generated runner before installation.
pub fn plan_definition(
    definition: &WorkflowDefinition,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    normalize_plan(definition, structural_order(definition)?)
}

fn normalize_plan(
    definition: &WorkflowDefinition,
    execution_order: Vec<DefinitionId>,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let nodes_by_id: BTreeMap<DefinitionId, _> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let nodes = execution_order
        .iter()
        .map(|id| (*nodes_by_id[id]).clone())
        .collect();
    let mut edges = definition.edges.clone();
    edges.sort_by(|left, right| {
        (
            &left.from_node,
            &left.from_output,
            &left.to_node,
            &left.to_input,
        )
            .cmp(&(
                &right.from_node,
                &right.from_output,
                &right.to_node,
                &right.to_input,
            ))
    });
    let mut control_edges = definition.control_edges.clone();
    control_edges.sort();
    let mut outputs = definition.outputs.clone();
    outputs.sort_by(|left, right| {
        (&left.name, &left.node, &left.port).cmp(&(&right.name, &right.node, &right.port))
    });

    Ok(CompiledWorkflow {
        definition: WorkflowDefinition {
            version: definition.version,
            dependencies: definition.dependencies.clone(),
            nodes,
            edges,
            control_edges,
            outputs,
        },
        execution_order,
    })
}

pub fn instantiate_compiled(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<Flow, WorkflowCompileError> {
    let (nodes, canonical_order) = prepare_definition(&plan.definition, registry)?;
    if plan.execution_order != canonical_order {
        return NonCanonicalPlanOrderSnafu.fail();
    }
    Flow::new(
        nodes,
        plan.definition.edges.clone(),
        plan.execution_order.clone(),
        plan.definition.outputs.clone(),
    )
    .context(FlowConstructionSnafu)
}

pub fn resolve_nodes(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    definition
        .nodes
        .iter()
        .map(|node| {
            let Some(registration) = registry.get(&node.kind) else {
                return UnknownNodeKindSnafu {
                    definition_id: node.id.clone(),
                    kind: node.kind.clone(),
                }
                .fail();
            };
            let instance =
                registration
                    .instantiate(node.config.clone())
                    .context(NodeConstructionSnafu {
                        definition_id: node.id.clone(),
                        kind: node.kind.clone(),
                    })?;
            let ports = registration.effective_ports(instance.as_ref());
            Ok(FlowNode::new(node.id.clone(), instance).with_ports(ports))
        })
        .collect()
}
