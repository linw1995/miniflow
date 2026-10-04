use crate::iteration::{body_definition, normalize_config, parse_config};
use crate::{
    CompiledWorkflow, ExecutionDependency, Flow, FlowBuildError, FlowNode, NodeBuildError,
    NodeRegistry, ValueType,
};
use crate::{DefinitionId, WorkflowDefinition};
use mf_runtime::{
    ITERATION_INPUT_KIND, ITERATION_KIND, PreparedSubgraph, iteration_input_flow_node,
};
use mf_runtime::{OutputDerivation, TypeCompatibility, TypeMismatch};
use mf_telemetry::{
    ContractError,
    description::{
        ControlEdge, DataEdge, LoopBodyDescription, NodeDescription, WorkflowDescription,
        WorkflowDescriptionVersion,
    },
    event::NodeIdentity,
    identity::WorkflowId,
};
use serde_json::Value;
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
pub enum WorkflowCompileError {
    #[snafu(display("{source}"))]
    InterfaceDescription { source: DescriptionError },
    #[snafu(transparent)]
    WorkflowInputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("invalid streaming workflow: {message}"), visibility(pub))]
    InvalidStream { message: String },
    #[snafu(
        display("could not prepare streaming workflow: {source}"),
        visibility(pub)
    )]
    StreamConstruction {
        source: mf_runtime::StreamBuildError,
    },
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
        output_type: Box<ValueType>,
        to_node: DefinitionId,
        to_input: String,
        input_type: Box<ValueType>,
    },
    #[snafu(display(
        "cannot connect `{from_node}`.`{from_output}` ({output_type}) to `{to_node}`.`{to_input}` ({input_type}): known value {source}"
    ))]
    KnownValueTypeConflict {
        from_node: DefinitionId,
        from_output: String,
        output_type: Box<ValueType>,
        to_node: DefinitionId,
        to_input: String,
        input_type: Box<ValueType>,
        source: Box<TypeMismatch>,
    },
    #[snafu(display(
        "cannot infer `{to_node}`.`{to_input}` before source `{from_node}`.`{from_output}`"
    ))]
    UnavailableInferredOutput {
        from_node: DefinitionId,
        from_output: String,
        to_node: DefinitionId,
        to_input: String,
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
    #[snafu(
        display("validated workflow could not be constructed: {source}"),
        visibility(pub)
    )]
    FlowConstruction { source: FlowBuildError },
    #[snafu(
        display("compiled workflow execution order does not match its definition"),
        visibility(pub)
    )]
    NonCanonicalPlanOrder,
    #[snafu(display("invalid node metadata for `{definition_id}`: {message}"))]
    InvalidNodeMetadata {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("invalid inferred output `{output}` for node `{definition_id}`: {source}"))]
    InferredOutputDepth {
        definition_id: DefinitionId,
        output: String,
        source: mf_runtime::TypeDepthError,
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
    #[snafu(display("invalid Loop at {path}: {message}"))]
    InvalidLoop { path: String, message: String },
    #[snafu(display("invalid iteration node `{definition_id}`: {message}"))]
    InvalidIteration {
        definition_id: DefinitionId,
        message: String,
    },
}

#[derive(Clone)]
struct TypeFact {
    value_type: ValueType,
    exact: Option<mf_runtime::ValueRef>,
}

#[derive(Default)]
pub struct TypeInferenceState {
    outputs: BTreeMap<(DefinitionId, String), TypeFact>,
}

impl TypeInferenceState {
    pub fn resolve_node<N>(
        &mut self,
        node: &mut FlowNode<N>,
        dependencies: &[ExecutionDependency<'_>],
    ) -> Result<(), WorkflowCompileError> {
        let id = &node.definition_id;
        let derivations = node.metadata.output_derivations.clone();
        node.metadata
            .ports
            .validate_derivations(id.as_str(), &derivations)
            .map_err(|error| WorkflowCompileError::InvalidNodeMetadata {
                definition_id: id.clone(),
                message: error.to_string(),
            })?;

        let mut inputs = BTreeMap::new();
        for dependency in dependencies {
            let Some(input_name) = dependency.input else {
                continue;
            };
            let input = node
                .metadata
                .ports
                .inputs
                .iter()
                .find(|port| port.name == input_name)
                .ok_or_else(|| WorkflowCompileError::InvalidNodeMetadata {
                    definition_id: id.clone(),
                    message: format!("unknown input `{input_name}` in type binding"),
                })?;
            let source_id = DefinitionId::from(dependency.source_node);
            let source = self
                .outputs
                .get(&(source_id.clone(), dependency.source_output.to_owned()))
                .ok_or_else(|| WorkflowCompileError::UnavailableInferredOutput {
                    from_node: source_id.clone(),
                    from_output: dependency.source_output.to_owned(),
                    to_node: id.clone(),
                    to_input: input_name.to_owned(),
                })?;
            if let Some(value) = &source.exact {
                input
                    .value_type
                    .validate_shared(value)
                    .map_err(
                        |source_error| WorkflowCompileError::KnownValueTypeConflict {
                            from_node: source_id.clone(),
                            from_output: dependency.source_output.to_owned(),
                            output_type: Box::new(source.value_type.clone()),
                            to_node: id.clone(),
                            to_input: input_name.to_owned(),
                            input_type: Box::new(input.value_type.clone()),
                            source: Box::new(source_error),
                        },
                    )?;
            } else if source.value_type.compatibility_with(&input.value_type)
                == TypeCompatibility::Incompatible
            {
                return Err(WorkflowCompileError::IncompatiblePortTypes {
                    from_node: source_id,
                    from_output: dependency.source_output.to_owned(),
                    output_type: Box::new(source.value_type.clone()),
                    to_node: id.clone(),
                    to_input: input_name.to_owned(),
                    input_type: Box::new(input.value_type.clone()),
                });
            }
            let value_type = if source.value_type.is_assignable_to(&input.value_type) {
                source.value_type.clone()
            } else {
                input.value_type.clone()
            };
            inputs.insert(
                input_name,
                TypeFact {
                    value_type,
                    exact: source.exact.clone(),
                },
            );
        }

        let derivations: BTreeMap<_, _> = derivations
            .into_iter()
            .map(|derivation| (derivation.output().to_owned(), derivation))
            .collect();
        for output in &mut node.metadata.ports.outputs {
            let declared = output.value_type.clone();
            let mut fact = match derivations.get(output.name.as_ref()) {
                Some(OutputDerivation::Literal { value, .. }) => TypeFact {
                    value_type: ValueType::infer_shared(value),
                    exact: Some(value.clone()),
                },
                Some(OutputDerivation::ForwardInput { input, .. }) => inputs
                    .get(input.as_str())
                    .cloned()
                    .unwrap_or_else(|| TypeFact {
                        value_type: node
                            .metadata
                            .ports
                            .inputs
                            .iter()
                            .find(|port| port.name == input.as_str())
                            .expect("derivation input was validated")
                            .value_type
                            .clone(),
                        exact: None,
                    }),
                Some(OutputDerivation::CollectInput { input, .. }) => TypeFact {
                    value_type: ValueType::List(Box::new(inputs.get(input.as_str()).map_or_else(
                        || {
                            node.metadata
                                .ports
                                .inputs
                                .iter()
                                .find(|port| port.name == input.as_str())
                                .expect("derivation input was validated")
                                .value_type
                                .clone()
                        },
                        |fact| fact.value_type.clone(),
                    ))),
                    exact: None,
                },
                None => TypeFact {
                    value_type: declared.clone(),
                    exact: None,
                },
            };
            fact.value_type
                .check_depth()
                .with_context(|_| InferredOutputDepthSnafu {
                    definition_id: id.clone(),
                    output: output.name.as_ref(),
                })?;
            if let Some(value) = &fact.exact {
                declared.validate_shared(value).map_err(|error| {
                    WorkflowCompileError::InvalidNodeMetadata {
                        definition_id: id.clone(),
                        message: format!("output `{}` known value: {error}", output.name),
                    }
                })?;
            } else if fact.value_type.compatibility_with(&declared)
                == TypeCompatibility::Incompatible
            {
                return Err(WorkflowCompileError::InvalidNodeMetadata {
                    definition_id: id.clone(),
                    message: format!(
                        "output `{}` forwards {} but declares {declared}",
                        output.name, fact.value_type
                    ),
                });
            }
            if !fact.value_type.is_assignable_to(&declared) {
                fact.value_type = declared;
            }
            output.value_type = fact.value_type.clone();
            self.outputs
                .insert((id.clone(), output.name.to_string()), fact);
        }
        Ok(())
    }
}

pub fn validate_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<(), WorkflowCompileError> {
    compile_definition(definition, registry).map(|_| ())
}

pub fn prepare_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    crate::loops::validate_structure(definition)?;
    definition
        .validate_execution()
        .context(StreamConstructionSnafu)?;
    prepare_graph(definition, registry, None, false)
}

fn prepare_graph(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    if allow_iteration_input {
        crate::loops::validate_structure(definition)?;
    }
    let order = structural_order_graph(definition)?;
    let mut nodes = resolve_nodes_in_scope(definition, registry, enclosing, allow_iteration_input)?;
    if definition.execution.is_none()
        && let Some(node) = nodes.iter().find(|node| {
            matches!(
                &node.node,
                Some(mf_runtime::NodeExecution::Event(_) | mf_runtime::NodeExecution::Stream(_))
            )
        })
    {
        return InvalidStreamSnafu {
            message: format!(
                "node `{}` requires streaming execution and is not supported in a synchronous scope",
                node.definition_id
            ),
        }.fail();
    }
    let incoming = incoming_dependencies(definition);
    let startup = definition.version.supports_startup_inputs()
        && enclosing.is_none()
        && !allow_iteration_input;
    validate_base_metadata(definition, &nodes, &incoming, startup)?;
    if startup {
        mf_runtime::WorkflowInputSchema::from_nodes(
            nodes
                .iter()
                .map(|node| (node, !incoming.contains_key(node.definition_id.as_str()))),
        )?;
        if let Some(node) = nodes.iter().find(|node| {
            !incoming.contains_key(node.definition_id.as_str())
                && matches!(node.node, Some(mf_runtime::NodeExecution::Event(_)))
        }) {
            return InvalidStreamSnafu {
                message: format!(
                    "initial event node `{}` requires an explicit activation source",
                    node.definition_id
                ),
            }
            .fail();
        }
    } else if let Some(node) = nodes
        .iter()
        .find(|node| !node.metadata.resources.is_empty())
    {
        return InvalidNodeMetadataSnafu {
            definition_id: node.definition_id.clone(),
            message: String::from(
                "input resources require a schema 2026-10-03 top-level initial node",
            ),
        }
        .fail();
    }

    let indices: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.definition_id.clone(), index))
        .collect();
    let mut inference = TypeInferenceState::default();
    for id in &order {
        let dependencies = incoming.get(id.as_str()).map_or(&[][..], Vec::as_slice);
        inference.resolve_node(&mut nodes[indices[id]], dependencies)?;
    }
    Ok((nodes, order))
}

pub fn incoming_dependencies(
    definition: &WorkflowDefinition,
) -> BTreeMap<&str, Vec<ExecutionDependency<'_>>> {
    let mut incoming: BTreeMap<&str, Vec<ExecutionDependency<'_>>> = BTreeMap::new();
    for edge in &definition.edges {
        incoming
            .entry(edge.to_node.as_str())
            .or_default()
            .push(ExecutionDependency {
                input: Some(&edge.to_input),
                source_node: edge.from_node.as_str(),
                source_output: &edge.from_output,
            });
    }
    for edge in &definition.control_edges {
        incoming
            .entry(edge.to_node.as_str())
            .or_default()
            .push(ExecutionDependency {
                input: None,
                source_node: edge.from_node.as_str(),
                source_output: &edge.from_output,
            });
    }
    incoming
}

fn validate_base_metadata(
    definition: &WorkflowDefinition,
    nodes: &[FlowNode],
    incoming: &BTreeMap<&str, Vec<ExecutionDependency<'_>>>,
    startup: bool,
) -> Result<(), WorkflowCompileError> {
    let registrations: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| (node.definition_id.clone(), &node.metadata.ports))
        .collect();
    let invalid = |id: &DefinitionId, message: String| WorkflowCompileError::InvalidNodeMetadata {
        definition_id: id.clone(),
        message,
    };
    let mut output_index = BTreeMap::new();
    for node in nodes {
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
                port.value_type.check_depth().map_err(|error| {
                    invalid(
                        &node.definition_id,
                        format!("{direction} port `{}`: {error}", port.name),
                    )
                })?;
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
    for node in nodes {
        if node.metadata.context_references.is_empty() {
            continue;
        }
        // Only reference consumers need reachability, and their sets need not outlive validation.
        let mut ancestors = BTreeSet::new();
        let mut pending = vec![node.definition_id.as_str()];
        while let Some(id) = pending.pop() {
            for dependency in incoming.get(id).into_iter().flatten() {
                if ancestors.insert(dependency.source_node) {
                    pending.push(dependency.source_node);
                }
            }
        }
        for reference in &node.metadata.context_references {
            let Some((producer, _)) = output_index.get(&reference.output) else {
                return Err(invalid(
                    &node.definition_id,
                    format!(
                        "reference `{}` ({}) is unknown",
                        reference.output, reference.label
                    ),
                ));
            };
            if !ancestors.contains(producer.as_str()) {
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
        let Some(_) = source
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
        let Some(_) = target.inputs.iter().find(|port| port.name == edge.to_input) else {
            return UnknownInputPortSnafu {
                from_node: edge.from_node.clone(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.clone(),
                to_input: edge.to_input.clone(),
            }
            .fail();
        };
        connected_inputs.insert((edge.to_node.clone(), edge.to_input.clone()));
    }

    for node in &definition.nodes {
        let registration = registrations[&node.id];
        if startup && !incoming.contains_key(node.id.as_str()) {
            continue;
        }
        for port in registration.inputs.iter().filter(|port| port.required) {
            if !connected_inputs.contains(&(node.id.clone(), port.name.to_string())) {
                return MissingRequiredInputSnafu {
                    node_id: node.id.clone(),
                    port: port.name.to_string(),
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

    Ok(())
}

pub fn topological_order(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Vec<DefinitionId>, WorkflowCompileError> {
    compile_definition(definition, registry).map(|plan| plan.execution_order)
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
    crate::loops::validate_structure(definition)?;
    definition
        .validate_execution()
        .context(StreamConstructionSnafu)?;
    structural_order_graph(definition)
}

pub fn structural_order_graph(
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
    let flow = Flow::prepare(
        nodes,
        definition.edges.clone(),
        execution_order.clone(),
        definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(definition.control_edges.clone()))
    .context(FlowConstructionSnafu)?;

    if let Some(execution) = &definition.execution {
        flow.into_stream(execution.clone())
            .context(StreamConstructionSnafu)?;
    } else {
        flow.into_tasks().context(FlowConstructionSnafu)?;
    }

    normalize_plan(definition, execution_order)
}

/// Plans graph structure without loading plugins. Validate the generated runner before installation.
pub fn plan_definition(
    definition: &WorkflowDefinition,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    for node in &definition.nodes {
        if node.kind == ITERATION_INPUT_KIND {
            return InvalidIterationSnafu {
                definition_id: node.id.clone(),
                message: "internal iteration input is not a workflow node".to_owned(),
            }
            .fail();
        }
    }
    normalize_plan(definition, structural_order(definition)?)
}

pub fn normalize_plan(
    definition: &WorkflowDefinition,
    execution_order: Vec<DefinitionId>,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let nodes_by_id: BTreeMap<DefinitionId, _> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut nodes: Vec<crate::NodeDefinition> = execution_order
        .iter()
        .map(|id| (*nodes_by_id[id]).clone())
        .collect();
    for node in &mut nodes {
        normalize_loop_node(node, &definition.dependencies)?;
        if node.kind == ITERATION_KIND {
            node.config = normalize_config(definition, node).map_err(|message| {
                WorkflowCompileError::InvalidIteration {
                    definition_id: node.id.clone(),
                    message,
                }
            })?;
        }
    }
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
            execution: definition.execution.clone(),
            dependencies: definition.dependencies.clone(),
            nodes,
            edges,
            control_edges,
            outputs,
        },
        execution_order,
    })
}

fn normalize_loop_node(
    node: &mut crate::NodeDefinition,
    dependencies: &BTreeMap<String, crate::NodeDependency>,
) -> Result<(), WorkflowCompileError> {
    let Some(loop_definition) = &mut node.loop_definition else {
        return Ok(());
    };
    let body = crate::loops::body_definition(&loop_definition.body, dependencies);
    let mut normalized = normalize_plan(&body, structural_order_graph(&body)?)?.definition;
    normalized
        .nodes
        .retain(|node| node.id.as_str() != crate::LOOP_SOURCE_ID);
    loop_definition.body.nodes = normalized.nodes;
    loop_definition.body.edges = normalized.edges;
    loop_definition.body.control_edges = normalized.control_edges;
    Ok(())
}

pub fn instantiate_compiled(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<Flow, WorkflowCompileError> {
    if plan.definition.execution.is_some() {
        return InvalidStreamSnafu {
            message: "use instantiate_stream for streaming workflows",
        }
        .fail();
    }
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
    .and_then(|flow| flow.with_control_edges(plan.definition.control_edges.clone()))
    .context(FlowConstructionSnafu)
    .and_then(|flow| {
        if plan.definition.version.supports_startup_inputs() {
            Ok(flow.with_workflow_inputs()?)
        } else {
            Ok(flow)
        }
    })
}

pub fn describe_workflow_inputs(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<mf_runtime::WorkflowInputSchema, WorkflowCompileError> {
    if plan.definition.execution.is_some() {
        Ok(crate::instantiate_stream(plan, registry)?
            .plan()
            .input_schema()
            .clone())
    } else {
        Ok(instantiate_compiled(plan, registry)?.input_schema().clone())
    }
}

pub fn describe_interface(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<mf_runtime::WorkflowInterface, WorkflowCompileError> {
    let graph = describe_compiled(plan).context(InterfaceDescriptionSnafu)?;
    let interface = mf_runtime::WorkflowInterface {
        version: mf_runtime::WorkflowInterfaceVersion::V2026_10_03,
        workflow_id: graph.workflow_id.clone(),
        schema: describe_workflow_inputs(plan, registry)?,
    };
    interface.validate_for_description(&graph)?;
    Ok(interface)
}

#[derive(Debug, Snafu)]
pub enum WorkflowExecutionError {
    #[snafu(display("{source}"))]
    Preparation { source: WorkflowCompileError },
    #[snafu(display("{source}"))]
    Execution {
        source: mf_runtime::WorkflowRunError,
    },
}

#[derive(Debug, Snafu)]
pub enum DescriptionError {
    #[snafu(display("compiled workflow has incomplete or duplicate node definitions"))]
    InvalidPlan,
    #[snafu(display("could not describe node `{definition_id}`"))]
    MissingNode { definition_id: DefinitionId },
    #[snafu(display("invalid workflow description: {source}"), visibility(pub))]
    Contract { source: ContractError },
}

pub fn describe_compiled(plan: &CompiledWorkflow) -> Result<WorkflowDescription, DescriptionError> {
    let definitions: BTreeMap<_, _> = plan
        .definition
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    if definitions.len() != plan.definition.nodes.len()
        || plan.execution_order.len() != plan.definition.nodes.len()
    {
        return Err(DescriptionError::InvalidPlan);
    }
    let mut nodes = Vec::with_capacity(plan.execution_order.len());
    for definition_id in &plan.execution_order {
        let id = definition_id.as_str();
        let definition = definitions
            .get(id)
            .ok_or_else(|| DescriptionError::MissingNode {
                definition_id: definition_id.clone(),
            })?;
        nodes.push(NodeDescription {
            id: id.into(),
            kind: definition.kind.clone(),
        });
    }
    let order: Vec<String> = plan
        .execution_order
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut loop_bodies = Vec::new();
    describe_loop_bodies(&plan.definition, &mut Vec::new(), &mut loop_bodies)?;
    let description = WorkflowDescription {
        version: if plan.definition.version.supports_startup_inputs() {
            WorkflowDescriptionVersion::V2026_10_03
        } else {
            match plan.definition.version {
                mf_runtime::WorkflowDefinitionVersion::V2026_09_26 => {
                    WorkflowDescriptionVersion::V2026_09_27
                }
                mf_runtime::WorkflowDefinitionVersion::V2026_09_29
                | mf_runtime::WorkflowDefinitionVersion::V2026_10_02
                | mf_runtime::WorkflowDefinitionVersion::V2026_10_03 => {
                    WorkflowDescriptionVersion::V2026_09_29
                }
            }
        },
        execution: plan.definition.version.supports_startup_inputs().then(|| {
            mf_telemetry::description::ExecutionDescription {
                mode: if plan.definition.execution.is_some() {
                    mf_telemetry::description::ExecutionMode::Stream
                } else {
                    mf_telemetry::description::ExecutionMode::Single
                },
                event_schema_version: if plan.definition.execution.is_some() {
                    mf_telemetry::STREAM_EVENT_SCHEMA_VERSION
                } else {
                    mf_telemetry::LOOP_EVENT_SCHEMA_VERSION
                },
            }
        }),
        workflow_id: WorkflowId::from_definition(&plan.definition, &order)
            .context(ContractSnafu)?,
        nodes,
        data_edges: plan
            .definition
            .edges
            .iter()
            .map(|edge| DataEdge {
                from_node: edge.from_node.to_string(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.to_string(),
                to_input: edge.to_input.clone(),
            })
            .collect(),
        control_edges: plan
            .definition
            .control_edges
            .iter()
            .map(|edge| ControlEdge {
                from_node: edge.from_node.to_string(),
                from_output: edge.from_output.clone(),
                to_node: edge.to_node.to_string(),
            })
            .collect(),
        execution_order: order,
        loop_bodies,
    };
    description.validate().context(ContractSnafu)?;
    Ok(description)
}

fn describe_loop_bodies(
    definition: &WorkflowDefinition,
    path: &mut Vec<String>,
    bodies: &mut Vec<LoopBodyDescription>,
) -> Result<(), DescriptionError> {
    for node in &definition.nodes {
        let Some(loop_definition) = &node.loop_definition else {
            continue;
        };
        path.push(node.id.to_string());
        let body = crate::loops::body_definition(&loop_definition.body, &definition.dependencies);
        let order = structural_order_graph(&body).map_err(|_| DescriptionError::InvalidPlan)?;
        let nodes_by_id: BTreeMap<_, _> = body.nodes.iter().map(|node| (&node.id, node)).collect();
        bodies.push(LoopBodyDescription {
            path: path.clone(),
            nodes: order
                .iter()
                .map(|id| {
                    let node = nodes_by_id[id];
                    NodeDescription {
                        id: id.to_string(),
                        kind: node.kind.clone(),
                    }
                })
                .collect(),
            data_edges: body
                .edges
                .iter()
                .map(|edge| DataEdge {
                    from_node: edge.from_node.to_string(),
                    from_output: edge.from_output.clone(),
                    to_node: edge.to_node.to_string(),
                    to_input: edge.to_input.clone(),
                })
                .collect(),
            control_edges: body
                .control_edges
                .iter()
                .map(|edge| ControlEdge {
                    from_node: edge.from_node.to_string(),
                    from_output: edge.from_output.clone(),
                    to_node: edge.to_node.to_string(),
                })
                .collect(),
            execution_order: order.iter().map(ToString::to_string).collect(),
        });
        describe_loop_bodies(&body, path, bodies)?;
        path.pop();
    }
    Ok(())
}

/// Observes construction and execution together; validation-only callers keep using instantiate_compiled.
pub fn execute_compiled(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    observation: Option<mf_runtime::RunObservation>,
) -> Result<mf_runtime::FlowOutputs, WorkflowExecutionError> {
    execute_compiled_with_inputs(
        plan,
        registry,
        mf_runtime::WorkflowArguments::default(),
        observation,
    )
}

pub fn execute_compiled_with_inputs(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    arguments: mf_runtime::WorkflowArguments,
    observation: Option<mf_runtime::RunObservation>,
) -> Result<mf_runtime::FlowOutputs, WorkflowExecutionError> {
    mf_runtime::ExecutionContext::run(observation, |state| {
        state.set_workflow_arguments(arguments);
        let flow = instantiate_compiled(plan, registry)
            .inspect_err(|error| {
                if let WorkflowCompileError::NodeConstruction { definition_id, .. }
                | WorkflowCompileError::UnknownNodeKind { definition_id, .. }
                | WorkflowCompileError::InvalidNodeMetadata { definition_id, .. }
                | WorkflowCompileError::InvalidIteration { definition_id, .. } = error
                {
                    state.preparation_failed(definition_id.as_str(), error);
                }
            })
            .context(PreparationSnafu)?;
        flow.execute_in_context(state).context(ExecutionSnafu)
    })
}

pub fn resolve_nodes(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    crate::loops::validate_structure(definition)?;
    definition
        .validate_execution()
        .context(StreamConstructionSnafu)?;
    resolve_nodes_in_scope(definition, registry, None, false)
}

fn registration_for(
    node: &crate::NodeDefinition,
    registry: &NodeRegistry,
) -> Result<&'static crate::NodeRegistration, WorkflowCompileError> {
    let Some(registration) = registry.get(&node.kind) else {
        return UnknownNodeKindSnafu {
            definition_id: node.id.clone(),
            kind: node.kind.clone(),
        }
        .fail();
    };
    Ok(registration)
}

fn bind_subgraph_node(
    node: &crate::NodeDefinition,
    registry: &NodeRegistry,
    body: &WorkflowDefinition,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
    options: Value,
) -> Result<FlowNode, WorkflowCompileError> {
    let registration = registration_for(node, registry)?;
    let (nodes, order) =
        prepare_graph(body, registry, enclosing, allow_iteration_input).map_err(|error| {
            if node.kind == crate::LOOP_KIND {
                WorkflowCompileError::InvalidLoop {
                    path: format!("{:?}", node.id),
                    message: error.to_string(),
                }
            } else {
                WorkflowCompileError::InvalidIteration {
                    definition_id: node.id.clone(),
                    message: error.to_string(),
                }
            }
        })?;
    let outputs = body
        .outputs
        .iter()
        .map(|output| {
            let port = nodes
                .iter()
                .find(|node| node.definition_id == output.node)
                .and_then(|node| {
                    node.metadata
                        .ports
                        .outputs
                        .iter()
                        .find(|port| port.name == output.port)
                })
                .expect("validated body output");
            crate::PortSpec::owned(&output.name, port.value_type.clone(), !output.optional)
        })
        .collect();
    let identities = body
        .nodes
        .iter()
        .filter(|node| node.kind != ITERATION_INPUT_KIND && node.kind != crate::LOOP_SOURCE_ID)
        .map(|node| NodeIdentity {
            id: node.id.to_string(),
            kind: node.kind.clone(),
            path: Vec::new(),
        })
        .collect();
    let flow = Flow::new(nodes, body.edges.clone(), order, body.outputs.clone())
        .and_then(|flow| flow.with_control_edges(body.control_edges.clone()))
        .context(FlowConstructionSnafu)?;
    let body = PreparedSubgraph::new(identities, outputs, move |state| {
        flow.execute_in_context(state)
    });
    let instance = registration
        .instantiate_subgraph(node.id.as_str(), node.config.clone(), options, body)
        .context(NodeConstructionSnafu {
            definition_id: node.id.clone(),
            kind: node.kind.clone(),
        })?;
    Ok(FlowNode::new(node.id.clone(), instance))
}

fn resolve_nodes_in_scope(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    definition
        .nodes
        .iter()
        .map(|node| {
            match node.kind.as_str() {
                crate::LOOP_KIND => {
                    let loop_definition = node.loop_definition.as_deref().expect("validated Loop");
                    let types = mf_runtime::loop_variable_types(&loop_definition.variables)
                        .expect("validated Loop variables");
                    let body = crate::loops::body_definition(
                        &loop_definition.body,
                        &definition.dependencies,
                    );
                    let options = serde_json::json!({
                        "max_iterations": loop_definition.max_iterations,
                        "variables": loop_definition.variables,
                        "until": loop_definition.until,
                    });
                    return bind_subgraph_node(node, registry, &body, Some(&types), false, options);
                }
                crate::LOOP_ASSIGN_KIND => {
                    let target = crate::loops::assignment_target(&node.config)
                        .expect("validated assignment");
                    let value_type = enclosing.expect("assignment has a Loop")[&target].clone();
                    return Ok(mf_runtime::prepared_loop_assign(
                        node.id.as_str(),
                        &target,
                        value_type,
                    ));
                }
                crate::EXIT_LOOP_KIND => {
                    return Ok(mf_runtime::prepared_loop_exit(node.id.as_str()));
                }
                crate::LOOP_SOURCE_ID => {
                    return Ok(mf_runtime::prepared_loop_source_types(
                        enclosing.expect("Loop source has variables"),
                    ));
                }
                _ => {}
            }
            if node.kind == ITERATION_INPUT_KIND {
                if allow_iteration_input && node.id.as_str() == mf_runtime::ITERATION_INPUT_ID {
                    return Ok(iteration_input_flow_node());
                }
                return InvalidIterationSnafu {
                    definition_id: node.id.clone(),
                    message: "internal iteration input is reserved for iteration bodies".to_owned(),
                }
                .fail();
            }
            if node.kind == ITERATION_KIND {
                if allow_iteration_input {
                    return InvalidIterationSnafu {
                        definition_id: node.id.clone(),
                        message: "nested iterations are not supported".to_owned(),
                    }
                    .fail();
                }
                let config = parse_config(node).map_err(|message| {
                    WorkflowCompileError::InvalidIteration {
                        definition_id: node.id.clone(),
                        message,
                    }
                })?;
                let body = body_definition(definition, &config).map_err(|message| {
                    WorkflowCompileError::InvalidIteration {
                        definition_id: node.id.clone(),
                        message,
                    }
                })?;
                return bind_subgraph_node(node, registry, &body, None, true, Value::Null);
            }
            let instance = registration_for(node, registry)?
                .instantiate(node.config.clone())
                .context(NodeConstructionSnafu {
                    definition_id: node.id.clone(),
                    kind: node.kind.clone(),
                })?;
            Ok(FlowNode::new(node.id.clone(), instance))
        })
        .collect()
}
