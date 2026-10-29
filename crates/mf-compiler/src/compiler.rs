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
use mf_telemetry::{ContractError, description::WorkflowDescription, event::NodeIdentity};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
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
    #[snafu(
        display("directed calls are not supported by this execution backend"),
        visibility(pub)
    )]
    DirectedCallExecution,
    #[snafu(transparent)]
    DirectedCalls {
        #[snafu(source(from(crate::CallBindingError, Box::new)))]
        source: Box<crate::CallBindingError>,
    },
    #[snafu(display("node `{definition_id}` call outputs: {source}"))]
    CallOutputs {
        definition_id: DefinitionId,
        source: mf_runtime::CallContractError,
    },
    #[snafu(transparent)]
    FlowResolution { source: crate::FlowResolutionError },
    #[snafu(
        display("invalid Flow call `{definition_id}`: {message}"),
        visibility(pub)
    )]
    InvalidCall {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(
        display("invalid Flow call `{definition_id}`: {source}"),
        visibility(pub)
    )]
    CallConfiguration {
        definition_id: DefinitionId,
        source: serde_json::Error,
    },
    #[snafu(
        display("invalid Flow call `{definition_id}`: {source}"),
        visibility(pub)
    )]
    CallBody {
        definition_id: DefinitionId,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
    #[snafu(display("invalid node-success dependency: {message}"))]
    InvalidSuccessDependency { message: String },
    #[snafu(transparent)]
    ExclusiveInput {
        source: mf_runtime::ExclusiveInputError,
    },
    #[snafu(transparent)]
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
    StreamConstruction { source: crate::StreamBuildError },
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
        #[snafu(source(from(TypeMismatch, Box::new)))]
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
    #[snafu(display("invalid typed generation for `{definition_id}`: {source}"))]
    TypedGeneration {
        definition_id: DefinitionId,
        source: mf_runtime::TypedGenerationError,
    },
    #[snafu(display("invalid node metadata for `{definition_id}`: {source}"))]
    OutputDerivation {
        definition_id: DefinitionId,
        #[snafu(source(from(mf_runtime::OutputDerivationError, Box::new)))]
        source: Box<mf_runtime::OutputDerivationError>,
    },
    #[snafu(display(
        "invalid node metadata for `{definition_id}`: {direction} port `{port}`: {source}"
    ))]
    PortTypeDepth {
        definition_id: DefinitionId,
        direction: String,
        port: String,
        source: mf_runtime::TypeDepthError,
    },
    #[snafu(display(
        "invalid node metadata for `{definition_id}`: output `{output}` known value: {source}"
    ))]
    KnownOutputValueTypeConflict {
        definition_id: DefinitionId,
        output: String,
        source: TypeMismatch,
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
    #[snafu(display("invalid node cluster: {message}"))]
    InvalidCluster { message: String },
    #[snafu(display("invalid Loop at {path}: {message}"), visibility(pub))]
    InvalidLoop { path: String, message: String },
    #[snafu(display("invalid Loop at {path}: {source}"), visibility(pub))]
    LoopAssignmentConfiguration {
        path: String,
        source: serde_json::Error,
    },
    #[snafu(
        display("invalid iteration node `{definition_id}`: {message}"),
        visibility(pub)
    )]
    InvalidIteration {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(display("invalid Loop at {path}: {source}"), visibility(pub))]
    LoopBody {
        path: String,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
    #[snafu(
        display("invalid iteration node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    IterationBody {
        definition_id: DefinitionId,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
    #[snafu(
        display("invalid iteration node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    IterationConfiguration {
        definition_id: DefinitionId,
        source: serde_json::Error,
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
        mf_runtime::NodePorts {
            inputs: node.metadata.ports.inputs.clone(),
            outputs: node.metadata.output_ports().cloned().collect(),
        }
        .validate_derivations(id.as_str(), &derivations)
        .with_context(|_| OutputDerivationSnafu {
            definition_id: id.clone(),
        })?;

        let mut inputs: BTreeMap<&str, TypeFact> = BTreeMap::new();
        for dependency in dependencies {
            let Some(input_name) = dependency.input() else {
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
            let source_id = DefinitionId::from(dependency.source_node());
            let source = self
                .outputs
                .get(&(
                    source_id.clone(),
                    dependency
                        .source_output()
                        .expect("input output source")
                        .to_owned(),
                ))
                .ok_or_else(|| WorkflowCompileError::UnavailableInferredOutput {
                    from_node: source_id.clone(),
                    from_output: dependency
                        .source_output()
                        .expect("input output source")
                        .to_owned(),
                    to_node: id.clone(),
                    to_input: input_name.to_owned(),
                })?;
            if let Some(value) = &source.exact {
                input.value_type.validate_shared(value).with_context(|_| {
                    KnownValueTypeConflictSnafu {
                        from_node: source_id.clone(),
                        from_output: dependency
                            .source_output()
                            .expect("input output source")
                            .to_owned(),
                        output_type: Box::new(source.value_type.clone()),
                        to_node: id.clone(),
                        to_input: input_name.to_owned(),
                        input_type: Box::new(input.value_type.clone()),
                    }
                })?;
            } else {
                validate_connection_types(
                    &source_id,
                    dependency.source_output().expect("input output source"),
                    &source.value_type,
                    id,
                    input_name,
                    &input.value_type,
                )?;
            }
            let value_type = if source.value_type.is_assignable_to(&input.value_type) {
                source.value_type.clone()
            } else {
                input.value_type.clone()
            };
            if dependency.exclusive()
                && let Some(fact) = inputs.get_mut(input_name)
            {
                if fact.value_type != value_type {
                    fact.value_type = input.value_type.clone();
                }
                if fact.exact != source.exact {
                    fact.exact = None;
                }
            } else {
                inputs.insert(
                    input_name,
                    TypeFact {
                        value_type,
                        exact: source.exact.clone(),
                    },
                );
            }
        }

        let derivations: BTreeMap<_, _> = derivations
            .into_iter()
            .map(|derivation| (derivation.output().to_owned(), derivation))
            .collect();
        for output in node
            .metadata
            .ports
            .outputs
            .iter_mut()
            .chain(&mut node.metadata.stream_outputs)
        {
            let declared = output.value_type.clone();
            let mut fact = match derivations.get(output.name.as_ref()) {
                Some(OutputDerivation::KnownType { value_type, .. }) => TypeFact {
                    value_type: value_type.clone(),
                    exact: None,
                },
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
                declared.validate_shared(value).with_context(|_| {
                    KnownOutputValueTypeConflictSnafu {
                        definition_id: id.clone(),
                        output: output.name.as_ref(),
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

pub fn validate_connection_types(
    from_node: &DefinitionId,
    from_output: &str,
    output_type: &ValueType,
    to_node: &DefinitionId,
    to_input: &str,
    input_type: &ValueType,
) -> Result<(), WorkflowCompileError> {
    ensure!(
        output_type.compatibility_with(input_type) != TypeCompatibility::Incompatible,
        IncompatiblePortTypesSnafu {
            from_node: from_node.clone(),
            from_output,
            output_type: Box::new(output_type.clone()),
            to_node: to_node.clone(),
            to_input,
            input_type: Box::new(input_type.clone()),
        }
    );
    Ok(())
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
    prepare_definition_with_layouts(definition, registry, None)
}

pub fn prepare_definition_with_layouts(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    body_layouts: Option<&mut BTreeMap<Vec<DefinitionId>, mf_runtime::ExecutionDomains>>,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    crate::loops::validate_structure(definition)?;
    crate::validate_execution(definition).context(StreamConstructionSnafu)?;
    prepare_graph(
        definition,
        &mut GraphPreparation {
            registry,
            body_layouts,
            path: Vec::new(),
        },
        None,
        false,
    )
}

struct GraphPreparation<'a> {
    registry: &'a NodeRegistry,
    body_layouts: Option<&'a mut BTreeMap<Vec<DefinitionId>, mf_runtime::ExecutionDomains>>,
    path: Vec<DefinitionId>,
}

fn prepare_graph(
    definition: &WorkflowDefinition,
    preparation: &mut GraphPreparation<'_>,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    if allow_iteration_input {
        crate::loops::validate_structure(definition)?;
    }
    let order = structural_order_graph(definition)?;
    let mut nodes =
        resolve_nodes_in_scope(definition, preparation, enclosing, allow_iteration_input)?;
    if !preparation.path.is_empty()
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
    let indices: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.definition_id.clone(), index))
        .collect();
    for edge in &definition.order_edges {
        crate::validate_success_connection(
            &nodes[indices[&edge.from_node]],
            &nodes[indices[&edge.to_node]],
        )
        .context(FlowConstructionSnafu)?;
    }
    let incoming = incoming_dependencies(definition);
    let startup = definition.version.supports_startup_inputs()
        && enclosing.is_none()
        && !allow_iteration_input;
    let call_targets = definition
        .calls
        .values()
        .flat_map(|aliases| aliases.values().map(|call| call.target.clone()))
        .collect::<BTreeSet<_>>();
    validate_base_metadata(definition, &nodes, &incoming, startup, &call_targets)?;
    if startup {
        crate::workflow_input_schema(
            nodes.iter().map(|node| {
                (
                    node,
                    !incoming.contains_key(node.definition_id.as_str())
                        && !call_targets.contains(&node.definition_id),
                )
            }),
            |node, input| {
                definition
                    .edges
                    .iter()
                    .any(|edge| edge.to_node.as_str() == node && edge.to_input == input)
            },
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
    } else if let Some(node) = nodes.iter().find(|node| node.metadata.stdin.is_some()) {
        return InvalidNodeMetadataSnafu {
            definition_id: node.definition_id.clone(),
            message: String::from("stdin requires a schema 2026-10-03 top-level initial node"),
        }
        .fail();
    }

    let mut inference = TypeInferenceState::default();
    for id in &order {
        let dependencies = incoming.get(id.as_str()).map_or(&[][..], Vec::as_slice);
        inference.resolve_node(&mut nodes[indices[id]], dependencies)?;
    }
    crate::construction::validate_exclusive_branches(order.iter().map(|id| {
        (
            &nodes[indices[id]],
            incoming.get(id.as_str()).into_iter().flatten().copied(),
        )
    }))
    .context(FlowConstructionSnafu)?;
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
            .push(ExecutionDependency::Output {
                exclusive: edge.exclusive,
                input: Some(&edge.to_input),
                source_node: edge.from_node.as_str(),
                source_output: &edge.from_output,
            });
    }
    for edge in &definition.control_edges {
        incoming
            .entry(edge.to_node.as_str())
            .or_default()
            .push(ExecutionDependency::Output {
                exclusive: false,
                input: None,
                source_node: edge.from_node.as_str(),
                source_output: &edge.from_output,
            });
    }
    for edge in &definition.order_edges {
        incoming
            .entry(edge.to_node.as_str())
            .or_default()
            .push(ExecutionDependency::NodeSuccess {
                source_node: edge.from_node.as_str(),
            });
    }
    incoming
}

fn validate_base_metadata(
    definition: &WorkflowDefinition,
    nodes: &[FlowNode],
    incoming: &BTreeMap<&str, Vec<ExecutionDependency<'_>>>,
    startup: bool,
    call_targets: &BTreeSet<DefinitionId>,
) -> Result<(), WorkflowCompileError> {
    let registrations: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| (node.definition_id.clone(), &node.metadata))
        .collect();
    let invalid = |id: &DefinitionId, message: String| {
        InvalidNodeMetadataSnafu {
            definition_id: id.clone(),
            message,
        }
        .build()
    };
    let mut output_index = BTreeMap::new();
    for node in nodes {
        node.metadata
            .validate_call_outputs()
            .context(CallOutputsSnafu {
                definition_id: node.definition_id.clone(),
            })?;
        let ports = registrations[&node.definition_id];
        for (name, binding) in &ports.input_bindings {
            let port = ports
                .ports
                .inputs
                .iter()
                .find(|port| port.name == *name)
                .ok_or_else(|| {
                    invalid(
                        &node.definition_id,
                        "input binding names an unknown port".into(),
                    )
                })?;
            binding.validate(
                &mf_runtime::output_id(node.definition_id.as_str(), name),
                &port.value_type,
            )?;
        }
        ensure!(
            ports.stream_outputs.is_empty()
                || matches!(node.node, Some(mf_runtime::NodeExecution::TaskStream(_))),
            InvalidNodeMetadataSnafu {
                definition_id: node.definition_id.clone(),
                message: "separate stream outputs require a combined executor",
            }
        );
        if node
            .metadata
            .input_bindings
            .values()
            .any(|binding| binding.env.is_some() || binding.default.is_some())
            && (!startup || incoming.contains_key(node.definition_id.as_str()))
        {
            return Err(invalid(
                &node.definition_id,
                "environment and default bindings require a top-level initial node".into(),
            ));
        }
        if let Some(generation) = &node.metadata.typed_generation {
            generation
                .validate(&ports.ports)
                .with_context(|_| TypedGenerationSnafu {
                    definition_id: node.definition_id.clone(),
                })?;
        }
        for (direction, specs) in [
            ("input", &ports.ports.inputs),
            ("output", &ports.ports.outputs),
            ("stream output", &node.metadata.stream_outputs),
        ] {
            let mut names = BTreeSet::new();
            for port in specs {
                if port.name.is_empty() || !names.insert(&port.name) {
                    return Err(invalid(
                        &node.definition_id,
                        format!("empty or duplicate {direction} port `{}`", port.name),
                    ));
                }
                port.value_type
                    .check_depth()
                    .with_context(|_| PortTypeDepthSnafu {
                        definition_id: node.definition_id.clone(),
                        direction,
                        port: port.name.as_ref(),
                    })?;
            }
        }
        for port in &node.metadata.stream_outputs {
            if ports
                .ports
                .outputs
                .iter()
                .any(|terminal| terminal.name == port.name)
            {
                return Err(invalid(
                    &node.definition_id,
                    format!("terminal and stream outputs share port `{}`", port.name),
                ));
            }
        }
        for port in ports.output_ports() {
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
                if ancestors.insert(dependency.source_node()) {
                    pending.push(dependency.source_node());
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
            .output_ports()
            .any(|port| port.name == edge.from_output)
        {
            return Err(control_error(edge, "unknown source output"));
        }
    }

    let mut connected_inputs = BTreeSet::new();
    for edge in definition.edges.iter() {
        let source = registrations[&edge.from_node];
        let target = registrations[&edge.to_node];
        let Some(_) = source
            .output_ports()
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
        let Some(_) = target
            .ports
            .inputs
            .iter()
            .find(|port| port.name == edge.to_input)
        else {
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
        // Call-owned inputs are validated after explicit external bindings have been resolved.
        if call_targets.contains(&node.id) || (startup && !incoming.contains_key(node.id.as_str()))
        {
            continue;
        }
        for port in registration
            .ports
            .inputs
            .iter()
            .filter(|port| port.required)
        {
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
            .output_ports()
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
    crate::call_bindings::declarations(definition)?;
    if definition.has_order_edges() && !definition.version.supports_node_success_edges() {
        return InvalidSuccessDependencySnafu {
            message: "node-success edges require version 2026-10-19",
        }
        .fail();
    }
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
    validate_clusters(definition, &ids)?;
    crate::validate_exclusive_inputs(&definition.edges)?;
    let mut connected = BTreeSet::new();
    for edge in definition.edges.iter() {
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
        if !connected.insert((&edge.to_node, &edge.to_input)) && !edge.exclusive {
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
    let mut orders = BTreeSet::new();
    for edge in &definition.order_edges {
        if edge.from_node == edge.to_node
            || !ids.contains(&edge.from_node)
            || !ids.contains(&edge.to_node)
            || !orders.insert(edge)
        {
            return InvalidSuccessDependencySnafu {
                message: "node-success edges must have distinct known endpoints and be unique",
            }
            .fail();
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

fn validate_clusters(
    definition: &WorkflowDefinition,
    node_ids: &BTreeSet<DefinitionId>,
) -> Result<(), WorkflowCompileError> {
    if !definition.clusters.is_empty() && !definition.version.supports_clusters() {
        return InvalidClusterSnafu {
            message: "node clusters require version 2026-10-23",
        }
        .fail();
    }
    if definition.clusters.len() > mf_telemetry::description::MAX_CLUSTERS_PER_SCOPE {
        return InvalidClusterSnafu {
            message: "too many node clusters",
        }
        .fail();
    }
    let mut members = BTreeSet::new();
    for cluster in &definition.clusters {
        if cluster.label.trim().is_empty()
            || cluster.label.len() > mf_telemetry::description::MAX_CLUSTER_LABEL_BYTES
            || cluster.label.chars().any(char::is_control)
        {
            return InvalidClusterSnafu {
                message: "cluster label must be a nonblank single line within the size limit",
            }
            .fail();
        }
        if cluster.annotation.trim().is_empty()
            || cluster.annotation.len() > mf_telemetry::description::MAX_CLUSTER_ANNOTATION_BYTES
            || !cluster
                .annotation
                .chars()
                .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        {
            return InvalidClusterSnafu {
                message: "cluster annotation must be nonblank, printable, and within the size limit",
            }
            .fail();
        }
        if cluster.nodes.is_empty() {
            return InvalidClusterSnafu {
                message: "cluster must list at least one node",
            }
            .fail();
        }
        for node in &cluster.nodes {
            if matches!(node.as_str(), "%loop" | "$loop" | "%iteration") {
                return InvalidClusterSnafu {
                    message: "synthetic nodes cannot belong to a cluster",
                }
                .fail();
            }
            if !node_ids.contains(node) {
                return InvalidClusterSnafu {
                    message: format!("cluster references unknown node `{node}`"),
                }
                .fail();
            }
            if !members.insert(node.clone()) {
                return InvalidClusterSnafu {
                    message: format!("node `{node}` belongs to more than one cluster"),
                }
                .fail();
            }
        }
    }
    Ok(())
}

fn control_error(edge: &crate::ControlEdgeDefinition, message: &str) -> WorkflowCompileError {
    WorkflowCompileError::InvalidControlEdge {
        from_node: edge.from_node.clone(),
        from_output: edge.from_output.to_string(),
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
        .chain(
            definition
                .order_edges
                .iter()
                .map(|edge| (edge.from_node.clone(), edge.to_node.clone())),
        )
        .chain(crate::call_bindings::readiness_pairs(definition))
        .collect()
}

pub fn structural_order(
    definition: &WorkflowDefinition,
) -> Result<Vec<DefinitionId>, WorkflowCompileError> {
    crate::loops::validate_structure(definition)?;
    crate::validate_execution(definition).context(StreamConstructionSnafu)?;
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
    let resolved = crate::resolve_flows(definition, None)?;
    compile_resolved_definition(&resolved.definition, registry)
}

pub fn require_no_directed_calls(
    definition: &WorkflowDefinition,
) -> Result<(), WorkflowCompileError> {
    ensure!(!definition.has_directed_calls(), DirectedCallExecutionSnafu);
    Ok(())
}

/// Prepares a frozen definition whose sources and dependency ownership are already resolved.
pub fn compile_resolved_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let (nodes, execution_order) = prepare_definition(definition, registry)?;
    let definition = resolve_execution(definition, &nodes)?;
    let flow = crate::FlowBuilder::prepare(
        nodes,
        definition.edges.clone(),
        execution_order.clone(),
        definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(definition.control_edges.clone()))
    .and_then(|flow| flow.with_order_edges(definition.order_edges.clone()))
    .context(FlowConstructionSnafu)?
    .with_declared_calls(&definition)?;

    if let Some(execution) = &definition.execution {
        require_no_directed_calls(&definition)?;
        flow.into_stream(execution.clone())
            .context(StreamConstructionSnafu)?;
    } else {
        flow.into_tasks().context(FlowConstructionSnafu)?;
    }

    normalize_plan(&definition, execution_order)
}

pub fn resolve_execution(
    definition: &WorkflowDefinition,
    nodes: &[FlowNode],
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    let stream_outputs: BTreeSet<_> = nodes
        .iter()
        .flat_map(|node| {
            node.metadata
                .stream_outputs
                .iter()
                .map(move |port| (node.definition_id.as_str(), port.name.as_ref()))
        })
        .collect();
    let streaming =
        nodes.iter().any(|node| {
            matches!(
                node.node,
                Some(mf_runtime::NodeExecution::Event(_) | mf_runtime::NodeExecution::Stream(_))
            )
        }) || definition.edges.iter().any(|edge| {
            stream_outputs.contains(&(edge.from_node.as_str(), edge.from_output.as_ref()))
        }) || definition.control_edges.iter().any(|edge| {
            stream_outputs.contains(&(edge.from_node.as_str(), edge.from_output.as_ref()))
        }) || definition
            .outputs
            .iter()
            .any(|output| stream_outputs.contains(&(output.node.as_str(), output.port.as_ref())));
    let mut resolved = definition.clone();
    resolved.execution = streaming.then(|| definition.execution.clone().unwrap_or_default());
    crate::validate_execution(&resolved).context(StreamConstructionSnafu)?;
    Ok(resolved)
}

/// Plan graph structure without loading nodes. Preparation validates linked node implementations.
pub fn plan_definition(
    definition: &WorkflowDefinition,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let resolved = crate::resolve_flows(definition, None)?;
    let definition = &resolved.definition;
    plan_resolved_definition(definition)
}

pub fn plan_resolved_definition(
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
        normalize_loop_node(node, &definition.dependencies, definition.version)?;
        if node.kind == ITERATION_KIND {
            node.config = normalize_config(definition, node)?;
        }
        if node.kind == mf_runtime::CALL_KIND {
            let body = crate::calls::body_definition(node)?;
            let normalized = plan_resolved_definition(&body)?.definition;
            node.config["definition"] =
                serde_json::to_value(normalized).context(CallConfigurationSnafu {
                    definition_id: node.id.clone(),
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
    let mut calls = definition.calls.clone();
    for aliases in calls.values_mut() {
        for call in aliases.values_mut() {
            call.ports.sort_by(|left, right| {
                (&left.input, &left.output).cmp(&(&right.input, &right.output))
            });
        }
    }

    Ok(CompiledWorkflow {
        definition: WorkflowDefinition {
            version: definition.version,
            execution: definition.execution.clone(),
            flows: definition.flows.clone(),
            calls,
            dependencies: definition.dependencies.clone(),
            nodes,
            edges,
            control_edges,
            order_edges: {
                let mut edges = definition.order_edges.clone();
                edges.sort();
                edges
            },
            outputs,
            clusters: definition.clusters.clone(),
        },
        execution_order,
    })
}

fn normalize_loop_node(
    node: &mut crate::NodeDefinition,
    dependencies: &BTreeMap<String, crate::NodeDependency>,
    version: crate::WorkflowDefinitionVersion,
) -> Result<(), WorkflowCompileError> {
    let Some(loop_definition) = &mut node.loop_definition else {
        return Ok(());
    };
    let body = crate::loops::body_definition(&loop_definition.body, dependencies, version);
    let mut normalized = normalize_plan(&body, structural_order_graph(&body)?)?.definition;
    normalized
        .nodes
        .retain(|node| node.id.as_str() != crate::LOOP_SOURCE_ID);
    loop_definition.body.nodes = normalized.nodes;
    loop_definition.body.calls = normalized.calls;
    loop_definition.body.edges = normalized.edges;
    loop_definition.body.control_edges = normalized.control_edges;
    loop_definition.body.order_edges = normalized.order_edges;
    Ok(())
}

pub fn instantiate_compiled(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<Flow, WorkflowCompileError> {
    let (resolved, builder) = prepare_compiled_plan(plan, registry)?;
    if resolved.definition.execution.is_some() {
        return InvalidStreamSnafu {
            message: "use instantiate_stream for streaming workflows",
        }
        .fail();
    }
    builder.into_tasks().context(FlowConstructionSnafu)
}

pub fn prepare_compiled_plan(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<(CompiledWorkflow, crate::FlowBuilder), WorkflowCompileError> {
    let (nodes, canonical_order) = prepare_definition(&plan.definition, registry)?;
    if plan.execution_order != canonical_order {
        return NonCanonicalPlanOrderSnafu.fail();
    }
    let resolved = CompiledWorkflow {
        definition: resolve_execution(&plan.definition, &nodes)?,
        execution_order: canonical_order,
    };
    let builder = crate::FlowBuilder::prepare(
        nodes,
        plan.definition.edges.clone(),
        plan.execution_order.clone(),
        plan.definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(plan.definition.control_edges.clone()))
    .and_then(|flow| flow.with_order_edges(plan.definition.order_edges.clone()))
    .context(FlowConstructionSnafu)?
    .with_declared_calls(&resolved.definition)?;
    let builder = if plan.definition.version.supports_startup_inputs() {
        builder.with_workflow_inputs()?
    } else {
        builder
    };
    Ok((resolved, builder))
}

/// Prepare a task workflow and report attributed node preparation failures.
pub fn instantiate_compiled_with_observation(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
    observation: Option<&mut mf_runtime::RunObservation>,
) -> Result<Flow, WorkflowCompileError> {
    instantiate_compiled(plan, registry).inspect_err(|error| {
        if let Some(observation) = observation
            && let WorkflowCompileError::NodeConstruction { definition_id, .. }
            | WorkflowCompileError::UnknownNodeKind { definition_id, .. }
            | WorkflowCompileError::InvalidNodeMetadata { definition_id, .. }
            | WorkflowCompileError::CallOutputs { definition_id, .. }
            | WorkflowCompileError::TypedGeneration { definition_id, .. }
            | WorkflowCompileError::OutputDerivation { definition_id, .. }
            | WorkflowCompileError::PortTypeDepth { definition_id, .. }
            | WorkflowCompileError::KnownOutputValueTypeConflict { definition_id, .. }
            | WorkflowCompileError::InvalidIteration { definition_id, .. }
            | WorkflowCompileError::IterationBody { definition_id, .. }
            | WorkflowCompileError::IterationConfiguration { definition_id, .. } = error
        {
            observation.preparation_failed(definition_id.as_str(), error.to_string());
        }
    })
}

pub fn describe_workflow_inputs(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<mf_runtime::WorkflowInputSchema, WorkflowCompileError> {
    prepare_interface(plan, registry).map(|(_, schema)| schema)
}

pub fn prepare_interface(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<(CompiledWorkflow, mf_runtime::WorkflowInputSchema), WorkflowCompileError> {
    let (resolved, builder) = prepare_compiled_plan(plan, registry)?;
    let schema = if let Some(execution) = &resolved.definition.execution {
        builder
            .into_stream(execution.clone())
            .context(StreamConstructionSnafu)?
            .plan()
            .input_schema()
            .clone()
    } else {
        builder
            .into_tasks()
            .context(FlowConstructionSnafu)?
            .input_schema()
            .clone()
    };
    Ok((resolved, schema))
}

pub fn describe_interface(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<mf_runtime::WorkflowInterface, WorkflowCompileError> {
    let (resolved, schema) = prepare_interface(plan, registry)?;
    let graph = describe_compiled(&resolved)?;
    let interface = mf_runtime::WorkflowInterface {
        version: mf_runtime::WorkflowInterfaceVersion::for_schema(&schema),
        workflow_id: graph.workflow_id.clone(),
        schema,
    };
    interface.validate_for_description(&graph)?;
    Ok(interface)
}

#[derive(Debug, Snafu)]
pub enum WorkflowExecutionError {
    #[snafu(transparent)]
    Preparation { source: WorkflowCompileError },
    #[snafu(transparent)]
    Execution {
        source: mf_runtime::WorkflowRunError,
    },
    #[snafu(transparent)]
    StreamExecution { source: mf_runtime::StreamError },
}

#[derive(Debug, Snafu)]
pub enum DescriptionError {
    #[snafu(display("invalid observation setup: {message}"), visibility(pub))]
    InvalidObservation { message: String },
    #[snafu(display("compiled workflow has incomplete or duplicate node definitions"))]
    InvalidPlan,
    #[snafu(display("could not order described Loop body at {path:?}: {source}"))]
    DescriptionOrder {
        path: Vec<String>,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
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
    ensure!(
        definitions.len() == plan.definition.nodes.len()
            && plan.execution_order.len() == plan.definition.nodes.len(),
        InvalidPlanSnafu
    );
    for definition_id in &plan.execution_order {
        definitions
            .get(definition_id.as_str())
            .with_context(|| MissingNodeSnafu {
                definition_id: definition_id.clone(),
            })?;
    }
    let order = plan
        .execution_order
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    mf_runtime::describe_definition(
        &plan.definition,
        &order,
        &loop_execution_orders(&plan.definition)?,
    )
    .context(ContractSnafu)
}

pub fn loop_execution_orders(
    definition: &WorkflowDefinition,
) -> Result<Vec<mf_runtime::LoopExecutionOrder>, DescriptionError> {
    let mut orders = Vec::new();
    collect_loop_execution_orders(
        definition,
        &mut Vec::new(),
        &mut orders,
        definition.has_order_edges() || definition.version.supports_clusters(),
    )?;
    Ok(orders)
}

fn collect_loop_execution_orders(
    definition: &WorkflowDefinition,
    path: &mut Vec<String>,
    orders: &mut Vec<mf_runtime::LoopExecutionOrder>,
    include_iterations: bool,
) -> Result<(), DescriptionError> {
    for node in &definition.nodes {
        let body = if let Some(loop_definition) = &node.loop_definition {
            crate::loops::body_definition(
                &loop_definition.body,
                &definition.dependencies,
                definition.version,
            )
        } else if node.kind == mf_runtime::CALL_KIND {
            crate::calls::body_definition(node)
                .with_context(|_| DescriptionOrderSnafu { path: path.clone() })?
        } else if include_iterations && node.kind == ITERATION_KIND {
            let config = parse_config(node)
                .with_context(|_| DescriptionOrderSnafu { path: path.clone() })?;
            body_definition(definition, &config)
                .map_err(|message| {
                    InvalidIterationSnafu {
                        definition_id: node.id.clone(),
                        message,
                    }
                    .build()
                })
                .with_context(|_| DescriptionOrderSnafu { path: path.clone() })?
        } else {
            continue;
        };
        path.push(node.id.to_string());
        let order = structural_order_graph(&body)
            .with_context(|_| DescriptionOrderSnafu { path: path.clone() })?;
        orders.push(mf_runtime::LoopExecutionOrder {
            path: path.clone(),
            execution_order: order.iter().map(ToString::to_string).collect(),
        });
        collect_loop_execution_orders(&body, path, orders, include_iterations)?;
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
        let flow = instantiate_compiled_with_observation(plan, registry, state.observation_mut())?;
        Ok(flow.execute_in_context(state)?)
    })
}

pub fn resolve_nodes(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    let resolved = crate::resolve_flows(definition, None)?;
    let definition = &resolved.definition;
    crate::loops::validate_structure(definition)?;
    crate::validate_execution(definition).context(StreamConstructionSnafu)?;
    resolve_nodes_in_scope(
        definition,
        &mut GraphPreparation {
            registry,
            body_layouts: None,
            path: Vec::new(),
        },
        None,
        false,
    )
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
    preparation: &mut GraphPreparation<'_>,
    body: &WorkflowDefinition,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
    options: Value,
) -> Result<FlowNode, WorkflowCompileError> {
    ensure!(
        preparation.path.len() < mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidCallSnafu {
            definition_id: node.id.clone(),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    let registration = registration_for(node, preparation.registry)?;
    preparation.path.push(node.id.clone());
    let prepared = prepare_graph(body, preparation, enclosing, allow_iteration_input);
    let path = preparation
        .body_layouts
        .as_ref()
        .map(|_| preparation.path.clone());
    preparation.path.pop();
    let (nodes, order) = if node.kind == crate::LOOP_KIND {
        prepared.context(LoopBodySnafu {
            path: format!("{:?}", node.id),
        })?
    } else if node.kind == mf_runtime::CALL_KIND {
        prepared.context(CallBodySnafu {
            definition_id: node.id.clone(),
        })?
    } else {
        prepared.context(IterationBodySnafu {
            definition_id: node.id.clone(),
        })?
    };
    if node.kind == mf_runtime::CALL_KIND {
        ensure!(
            resolve_execution(body, &nodes)?.execution.is_none(),
            InvalidCallSnafu {
                definition_id: node.id.clone(),
                message: "Flow calls require terminal child ports"
            }
        );
    }
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
            crate::PortSpec::owned(
                output.name.as_ref(),
                port.value_type.clone(),
                !output.optional,
            )
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
    let builder =
        crate::FlowBuilder::prepare(nodes, body.edges.clone(), order, body.outputs.clone())
            .and_then(|flow| flow.with_control_edges(body.control_edges.clone()))
            .and_then(|flow| flow.with_order_edges(body.order_edges.clone()))
            .context(FlowConstructionSnafu)?
            .with_declared_calls(body)?;
    let builder = if node.kind == mf_runtime::CALL_KIND {
        builder.with_workflow_inputs()?
    } else {
        builder
    };
    let flow = builder.into_tasks().context(FlowConstructionSnafu)?;
    let input_schema = flow.input_schema().clone();
    if let Some(layouts) = preparation.body_layouts.as_mut() {
        layouts.insert(
            path.expect("layout collection has a body path"),
            flow.execution_domains().clone(),
        );
    }
    let body = PreparedSubgraph::new(identities, outputs, move |state| {
        flow.execute_in_context(state)
    })
    .with_input_schema(input_schema);
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
    preparation: &mut GraphPreparation<'_>,
    enclosing: Option<&BTreeMap<String, ValueType>>,
    allow_iteration_input: bool,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    definition
        .nodes
        .iter()
        .map(|node| {
            if node.kind == mf_runtime::CALL_KIND {
                let body = crate::calls::body_definition(node)?;
                ensure!(!allow_iteration_input || !crate::calls::contains_iteration_containers(&body)?, InvalidCallSnafu {
                    definition_id: node.id.clone(), message: "calls inside Iteration cannot contain Loop or Iteration constructs"
                });
                crate::loops::validate_structure(&body)?;
                return bind_subgraph_node(node, preparation, &body, None, false, Value::Null);
            }
            match node.kind.as_str() {
                crate::LOOP_KIND => {
                    let loop_definition = node.loop_definition.as_deref().expect("validated Loop");
                    let types = mf_runtime::loop_variable_types(&loop_definition.variables)
                        .expect("validated Loop variables");
                    let body = crate::loops::body_definition(
                        &loop_definition.body,
                        &definition.dependencies,
                        definition.version,
                    );
                    let options = serde_json::json!({
                        "max_iterations": loop_definition.max_iterations,
                        "variables": loop_definition.variables,
                        "until": loop_definition.until,
                    });
                    return bind_subgraph_node(
                        node,
                        preparation,
                        &body,
                        Some(&types),
                        false,
                        options,
                    );
                }
                crate::LOOP_ASSIGN_KIND => {
                    let target = crate::loops::assignment_target(
                        &node.config,
                        std::slice::from_ref(&node.id),
                    )
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
                let config = parse_config(node)?;
                let body = body_definition(definition, &config).map_err(|message| {
                    InvalidIterationSnafu {
                        definition_id: node.id.clone(),
                        message,
                    }
                    .build()
                })?;
                return bind_subgraph_node(node, preparation, &body, None, true, Value::Null);
            }
            let instance = registration_for(node, preparation.registry)?
                .instantiate(node.config.clone())
                .context(NodeConstructionSnafu {
                    definition_id: node.id.clone(),
                    kind: node.kind.clone(),
                })?;
            Ok(FlowNode::new(node.id.clone(), instance))
        })
        .collect()
}
