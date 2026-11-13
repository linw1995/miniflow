use crate::{
    CompiledWorkflow, ExecutionDependency, Flow, FlowBuildError, FlowNode, NodeBuildError,
    NodeRegistry, ValueType,
};
use crate::{DefinitionId, WorkflowDefinition};
use mf_runtime::{GraphRelations, PreparedSubgraph, ScopeInstruction};
use mf_runtime::{OutputDerivation, TypeCompatibility, TypeMismatch};
use mf_telemetry::{description::WorkflowDescription, event::NodeIdentity};
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, OnceLock};

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
    #[snafu(transparent)]
    FlowResolution { source: crate::FlowResolutionError },
    #[snafu(
        display("invalid graph for node `{definition_id}`: {message}"),
        visibility(pub)
    )]
    InvalidGraph {
        definition_id: DefinitionId,
        message: String,
    },
    #[snafu(
        display("invalid node configuration for `{definition_id}`: {source}"),
        visibility(pub)
    )]
    NodeConfiguration {
        definition_id: DefinitionId,
        source: serde_json::Error,
    },
    #[snafu(
        display("invalid child graph for node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    ChildGraph {
        definition_id: DefinitionId,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
    #[snafu(
        display("invalid Flow parameters on node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    FlowParameters {
        definition_id: DefinitionId,
        source: mf_runtime::FlowParameterError,
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
    CompiledGraph {
        #[snafu(source(from(crate::WorkflowBuildError, Box::new)))]
        source: Box<crate::WorkflowBuildError>,
    },
    #[snafu(transparent)]
    WorkflowInputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(transparent)]
    Manifest {
        source: mf_runtime::WorkflowManifestError,
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
    #[snafu(display("invalid scope at {path}: {message}"), visibility(pub))]
    InvalidScope { path: String, message: String },
    #[snafu(
        display("invalid scope variables at {path}: {source}"),
        visibility(pub)
    )]
    ScopeVariables {
        path: String,
        source: mf_runtime::LoopVariableError,
    },
    #[snafu(
        display("invalid child scope on node `{definition_id}`: {source}"),
        visibility(pub)
    )]
    ScopeStructure {
        definition_id: DefinitionId,
        source: crate::ScopeStructureError,
    },
    #[snafu(
        display("invalid scope configuration at {path}: {source}"),
        visibility(pub)
    )]
    ScopeConfiguration {
        path: String,
        source: serde_json::Error,
    },
    #[snafu(
        display("invalid workflow variables at {path}: {source}"),
        visibility(pub)
    )]
    WorkflowVariables {
        path: String,
        #[snafu(source(from(mf_runtime::WorkflowVariablesError, Box::new)))]
        source: Box<mf_runtime::WorkflowVariablesError>,
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
                .with_context(|| InvalidNodeMetadataSnafu {
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
                .with_context(|| UnavailableInferredOutputSnafu {
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
            } else if source.value_type.compatibility_with(&input.value_type)
                == TypeCompatibility::Incompatible
            {
                return IncompatiblePortTypesSnafu {
                    from_node: source_id,
                    from_output: dependency
                        .source_output()
                        .expect("input output source")
                        .to_owned(),
                    output_type: Box::new(source.value_type.clone()),
                    to_node: id.clone(),
                    to_input: input_name.to_owned(),
                    input_type: Box::new(input.value_type.clone()),
                }
                .fail();
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
                return InvalidNodeMetadataSnafu {
                    definition_id: id.clone(),
                    message: format!(
                        "output `{}` forwards {} but declares {declared}",
                        output.name, fact.value_type
                    ),
                }
                .fail();
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

/// Prepares an already resolved graph using host-linked providers and returns its canonical order.
pub fn prepare_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    prepare_definition_with_layouts(definition, registry, None)
}

pub fn prepare_definition_with_layouts(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    body_layouts: Option<&mut ScopeLayouts>,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    let plan = plan_resolved_definition(definition, registry)?;
    prepare_compiled_graph_with_layouts(&plan.graph, registry, body_layouts)
}

/// Prepare an artifact using its stored child tree, without source resolution.
pub fn prepare_compiled_graph_with_layouts(
    graph: &mf_runtime::CompiledGraph,
    registry: &NodeRegistry,
    body_layouts: Option<&mut ScopeLayouts>,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    validate_graph_structure(graph)?;
    crate::validate_compiled_graph(registry, graph)?;
    let origin_identity = Arc::new(OnceLock::new());
    let (nodes, order) = prepare_graph(
        graph,
        &mut GraphPreparation {
            registry,
            root_version: graph.definition.version,
            body_layouts,
            path: Vec::new(),
            origin_identity: origin_identity.clone(),
        },
    )?;
    finalize_origin_identity(graph, &nodes, &origin_identity)?;
    Ok((nodes, order))
}

/// Factories may determine that an authored root is finite or streaming. Finalize
/// the shared origin only after that decision, without constructing any node twice.
fn finalize_origin_identity(
    graph: &mf_runtime::CompiledGraph,
    nodes: &[FlowNode],
    identity: &OnceLock<mf_telemetry::identity::WorkflowId>,
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    let mut resolved = graph.clone();
    resolved.definition = resolve_execution(&graph.definition, nodes)?;
    validate_relations(&resolved, &[])?;
    let workflow_id = resolved.workflow_id().context(GraphSnafu)?;
    // This cell belongs exclusively to this root preparation and is set once,
    // after every factory has returned. Prepared descendants only read it.
    ensure!(
        identity.set(workflow_id).is_ok(),
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("<workflow>"),
            message: "Flow origin identity was already finalized",
        }
    );
    Ok(resolved.definition)
}

pub type ScopeLayouts = BTreeMap<
    Vec<DefinitionId>,
    (
        mf_runtime::ExecutionDomains,
        Vec<mf_runtime::SubgraphStreamOutput>,
    ),
>;

struct GraphPreparation<'a> {
    registry: &'a NodeRegistry,
    root_version: mf_runtime::WorkflowDefinitionVersion,
    body_layouts: Option<&'a mut ScopeLayouts>,
    path: Vec<DefinitionId>,
    origin_identity: Arc<OnceLock<mf_telemetry::identity::WorkflowId>>,
}

fn prepare_graph(
    graph: &mf_runtime::CompiledGraph,
    preparation: &mut GraphPreparation<'_>,
) -> Result<(Vec<FlowNode>, Vec<DefinitionId>), WorkflowCompileError> {
    let definition = &graph.definition;
    let order = graph.execution_order.clone();
    let mut nodes = resolve_nodes_in_scope(graph, preparation)?;
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
    let startup = definition.version.supports_startup_inputs() && graph.relations.is_callable();
    validate_base_metadata(definition, &nodes, &incoming, startup)?;
    if startup {
        crate::workflow_input_schema(
            nodes
                .iter()
                .map(|node| (node, !incoming.contains_key(node.definition_id.as_str()))),
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
        if startup && !incoming.contains_key(node.id.as_str()) {
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
    compile_definition(definition, registry).map(|plan| plan.graph.execution_order)
}

fn validate_structure(definition: &WorkflowDefinition) -> Result<(), WorkflowCompileError> {
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
        if cluster.collapsed && !definition.version.supports_cluster_collapsed() {
            return InvalidClusterSnafu {
                message: "default-collapsed clusters require version 2026-11-11",
            }
            .fail();
        }
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
    InvalidControlEdgeSnafu {
        from_node: edge.from_node.clone(),
        from_output: edge.from_output.to_string(),
        to_node: edge.to_node.clone(),
        message,
    }
    .build()
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
        .collect()
}

pub fn structural_order(
    definition: &WorkflowDefinition,
) -> Result<Vec<DefinitionId>, WorkflowCompileError> {
    let mut definition = definition.clone();
    crate::lowering::lower_scope(&mut definition, &mut GraphRelations::default())?;
    crate::validate_execution(&definition).context(StreamConstructionSnafu)?;
    structural_order_graph(&definition)
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

fn resolution_error(error: crate::FlowResolutionError) -> WorkflowCompileError {
    match error {
        crate::FlowResolutionError::Lowering { source } => *source,
        crate::FlowResolutionError::ChildScope {
            definition_id,
            source,
        } => ChildGraphSnafu { definition_id }.into_error(resolution_error(*source)),
        error => error.into(),
    }
}

pub fn compile_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    // Lower only this authoring scope before checking its providers. A missing
    // receiver has no authority to load or interpret any child body.
    let mut root = definition.clone();
    let mut relations = GraphRelations::default();
    crate::lowering::lower_scope(&mut root, &mut relations)?;
    for node in &root.nodes {
        if relations.instruction(&node.id).is_none() {
            registration_for(node, registry)?;
        }
    }
    let resolved =
        crate::resolve_flows_with_registry(definition, None, registry).map_err(resolution_error)?;
    compile_planned_workflow(plan_resolved_workflow(&resolved)?, registry)
}

/// Prepares a frozen definition whose sources and dependency ownership are already resolved.
pub fn compile_resolved_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    compile_planned_workflow(plan_resolved_definition(definition, registry)?, registry)
}

fn compile_planned_workflow(
    plan: CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let (resolved, builder) = prepare_compiled_plan(&plan, registry)?;
    if let Some(execution) = &resolved.graph.definition.execution {
        builder
            .into_stream(execution.clone())
            .context(StreamConstructionSnafu)?;
    } else {
        builder.into_tasks().context(FlowConstructionSnafu)?;
    }
    Ok(resolved)
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
    let resolved = crate::resolve_flows(definition, None).map_err(resolution_error)?;
    plan_resolved_workflow(&resolved)
}

/// Freeze the actual resolved scopes without consulting a provider registry.
pub fn plan_resolved_workflow(
    resolved: &crate::ResolvedWorkflow,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let order = structural_order_graph(&resolved.definition)?;
    let graph = normalize_scope(resolved, &resolved.definition, order, 0)?;
    validate_graph_structure(&graph)?;
    Ok(CompiledWorkflow { graph })
}

/// Authoring convenience for already resolved inline definitions. Saved artifacts
/// must use their stored graph instead of passing through the source resolver.
pub fn plan_resolved_definition(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
) -> Result<CompiledWorkflow, WorkflowCompileError> {
    let resolved = resolve_inline_scopes(
        definition,
        registry,
        crate::lowering::ScopeContext::default(),
        0,
    )?;
    plan_resolved_workflow(&resolved)
}

fn resolve_inline_scopes(
    definition: &WorkflowDefinition,
    registry: &NodeRegistry,
    mut context: crate::lowering::ScopeContext,
    depth: usize,
) -> Result<crate::ResolvedWorkflow, WorkflowCompileError> {
    ensure!(
        depth <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    let mut definition = definition.clone();
    context.lower_scope(&mut definition)?;
    let mut children = BTreeMap::new();
    for node in &definition.nodes {
        if context.relations.instruction(&node.id).is_some() {
            continue;
        }
        let Some(registration) = registry.get(&node.kind) else {
            // No receiving callback means no authority to discover child fields.
            // Preparation still requires this provider unless a host Call owns it.
            continue;
        };
        let fields = match registration
            .validate_flow_parameters(node, context.root_version.unwrap_or(definition.version))
        {
            Err(NodeBuildError::FlowParameters { source }) => {
                return Err(FlowParametersSnafu {
                    definition_id: node.id.clone(),
                }
                .into_error(source));
            }
            result => result.context(NodeConstructionSnafu {
                definition_id: node.id.clone(),
                kind: node.kind.clone(),
            })?,
        };
        let mut flows = BTreeMap::new();
        for (name, body) in fields {
            let child_context = context.child(definition.version, node, &name)?;
            let child = crate::calls::child_result(
                node,
                resolve_inline_scopes(&body, registry, child_context, depth + 1),
            )?;
            flows.insert(name, child);
        }
        if !flows.is_empty() {
            children.insert(node.id.clone(), flows);
        }
    }
    Ok(crate::ResolvedWorkflow {
        definition,
        relations: context.relations,
        sources: BTreeSet::new(),
        children,
    })
}

fn normalize_scope(
    resolved: &crate::ResolvedWorkflow,
    definition: &WorkflowDefinition,
    execution_order: Vec<DefinitionId>,
    depth: usize,
) -> Result<mf_runtime::CompiledGraph, WorkflowCompileError> {
    ensure!(
        depth <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "combined container nesting exceeds twenty scopes",
        }
    );
    let nodes_by_id: BTreeMap<DefinitionId, _> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    validate_child_owners(definition, &resolved.children)?;
    let mut nodes: Vec<crate::NodeDefinition> = execution_order
        .iter()
        .map(|id| (*nodes_by_id[id]).clone())
        .collect();
    let mut children = BTreeMap::new();
    for node in &mut nodes {
        let mut flows = BTreeMap::new();
        for (name, body) in resolved.children.get(&node.id).into_iter().flatten() {
            ensure!(
                !name.trim().is_empty(),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "blank receiving Flow parameter declaration",
                }
            );
            let order = crate::calls::child_result(node, structural_order_graph(&body.definition))?;
            let normalized = crate::calls::child_result(
                node,
                normalize_scope(body, &body.definition, order, depth + 1),
            )?;
            crate::calls::validate_child(node, &normalized)?;
            node.set_node_flow(name, normalized.definition.clone())
                .context(FlowParametersSnafu {
                    definition_id: node.id.clone(),
                })?;
            flows.insert(name.clone(), normalized);
        }
        if !flows.is_empty() {
            children.insert(node.id.clone(), flows);
        }
    }
    let definition = canonical_definition(definition, nodes);
    Ok(mf_runtime::CompiledGraph {
        definition,
        execution_order,
        relations: resolved.relations.clone(),
        children,
    })
}

fn canonical_definition(
    definition: &WorkflowDefinition,
    nodes: Vec<crate::NodeDefinition>,
) -> WorkflowDefinition {
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

    WorkflowDefinition {
        version: definition.version,
        execution: definition.execution.clone(),
        flows: definition.flows.clone(),
        variables: definition.variables.clone(),
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
    }
}

fn validate_child_owners<T>(
    definition: &WorkflowDefinition,
    children: &BTreeMap<DefinitionId, BTreeMap<String, T>>,
) -> Result<(), WorkflowCompileError> {
    for (id, flows) in children {
        ensure!(
            definition.nodes.iter().any(|node| &node.id == id) && !flows.is_empty(),
            InvalidGraphSnafu {
                definition_id: id.clone(),
                message: "child graphs require a known owner and nonempty binding set"
            }
        );
    }
    Ok(())
}

/// Project actual child definitions for canonical host-binding comparison. This
/// never discovers children from config, nor creates an executable graph.
pub fn project_graph_definition(
    graph: &mf_runtime::CompiledGraph,
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    project_graph_definition_at(graph, 0)
}

fn project_graph_definition_at(
    graph: &mf_runtime::CompiledGraph,
    depth: usize,
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    ensure!(
        depth <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    let mut definition = graph.definition.clone();
    for node in &mut definition.nodes {
        let flows = crate::calls::compiled_node_flows(graph, node)?
            .into_iter()
            .map(|(name, child)| Ok((name, project_graph_definition_at(child, depth + 1)?)))
            .collect::<Result<BTreeMap<_, _>, WorkflowCompileError>>()?;
        *node = crate::calls::node_with_actual_flows(
            node,
            flows
                .iter()
                .map(|(name, definition)| (name.as_str(), definition)),
        )?;
    }
    Ok(definition)
}

pub fn validate_graph_structure(
    graph: &mf_runtime::CompiledGraph,
) -> Result<(), WorkflowCompileError> {
    validate_scope_graph(
        graph,
        &ScopeValidation {
            depth: 0,
            call_depth: 0,
            state_depth: 0,
            item_scope_ancestor: false,
            path: &[],
        },
    )
}

struct ScopeValidation<'a> {
    depth: usize,
    call_depth: usize,
    state_depth: usize,
    item_scope_ancestor: bool,
    path: &'a [DefinitionId],
}

fn validate_scope_graph(
    graph: &mf_runtime::CompiledGraph,
    scope: &ScopeValidation<'_>,
) -> Result<(), WorkflowCompileError> {
    let ScopeValidation {
        depth, call_depth, ..
    } = *scope;
    ensure!(
        call_depth <= mf_runtime::MAX_CALL_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "Flow nesting exceeds sixteen calls"
        }
    );
    ensure!(
        depth <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    validate_child_owners(&graph.definition, &graph.children)?;
    ensure!(
        structural_order_graph(&graph.definition)? == graph.execution_order,
        NonCanonicalPlanOrderSnafu
    );
    crate::validate_execution(&graph.definition).context(StreamConstructionSnafu)?;
    validate_relations(graph, scope.path)?;
    let frame = graph.relations.scope.as_ref();
    ensure!(
        !scope.item_scope_ancestor || frame.is_none(),
        InvalidScopeSnafu {
            path: crate::loops::path_label(scope.path),
            message: "child scopes inside an item scope cannot contain container scopes"
        }
    );
    let inherited_state_depth = if graph.relations.is_callable() {
        0
    } else {
        scope.state_depth
    };
    let state_depth =
        inherited_state_depth + usize::from(frame.is_some_and(|frame| frame.is_stateful()));
    ensure!(
        state_depth <= mf_runtime::MAX_LOOP_DEPTH,
        InvalidScopeSnafu {
            path: crate::loops::path_label(scope.path),
            message: "state-scope nesting exceeds four"
        }
    );
    let item_scope_ancestor =
        scope.item_scope_ancestor || frame.is_some_and(|frame| !frame.is_stateful());
    let mut child_keys = BTreeSet::new();
    for node in &graph.definition.nodes {
        let flows = graph.children.get(&node.id);
        for (name, child) in flows.into_iter().flatten() {
            ensure!(
                !name.trim().is_empty(),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "blank receiving Flow parameter declaration"
                }
            );
            ensure!(
                child.relations.streams.is_empty()
                    || graph.definition.version.supports_child_streams(),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "child stream exports require definition version 2026-10-28"
                }
            );
            let key = child.child_scope_key(node, name);
            ensure!(
                key == node.id.as_str()
                    || key == mf_runtime::ScopePathEntry::binding_key(node.id.as_str(), name),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "scope keys must reference their actual receiving owner and binding",
                }
            );
            ensure!(
                !key.trim().is_empty()
                    && child_keys.insert(key.clone())
                    && (key == node.id.as_str()
                        || !graph
                            .definition
                            .nodes
                            .iter()
                            .any(|other| other.id.as_str() == key)),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "Flow parameter body key collides with an authored node ID"
                }
            );
            crate::calls::validate_child(node, child)?;
            let child_call_depth = call_depth + usize::from(child.relations.is_callable());
            let mut path = scope.path.to_vec();
            path.push(key.into());
            crate::calls::child_result(
                node,
                validate_scope_graph(
                    child,
                    &ScopeValidation {
                        depth: depth + 1,
                        call_depth: child_call_depth,
                        state_depth,
                        item_scope_ancestor,
                        path: &path,
                    },
                ),
            )?;
        }
    }
    Ok(())
}

/// Check explicit program relationships without interpreting provider configuration.
fn validate_relations(
    graph: &mf_runtime::CompiledGraph,
    path: &[DefinitionId],
) -> Result<(), WorkflowCompileError> {
    let invalid = |message: &str| {
        InvalidScopeSnafu {
            path: crate::loops::path_label(path),
            message,
        }
        .build()
    };
    let ids: BTreeSet<_> = graph.definition.nodes.iter().map(|node| &node.id).collect();
    ensure!(
        graph
            .relations
            .host
            .as_ref()
            .is_none_or(|host| { graph.relations.is_callable() && !host.trim().is_empty() }),
        InvalidScopeSnafu {
            path: crate::loops::path_label(path),
            message: "host bindings require a callable graph and a nonblank Flow name"
        }
    );
    for (name, reference) in &graph.relations.streams {
        if name.trim().is_empty()
            || reference.port.trim().is_empty()
            || !ids.contains(&reference.node)
        {
            return Err(invalid(
                "stream relationships require a named export and known source",
            ));
        }
    }
    if let Some(root) = &graph.relations.root {
        ensure!(
            graph.relations.is_callable()
                && graph.definition.execution.is_none()
                && graph.definition.version.supports_workflow_variables(),
            InvalidScopeSnafu {
                path: crate::loops::path_label(path),
                message: "workflow variables require a finite callable Flow at version 2026-11-13",
            }
        );
        root.validate_values().context(WorkflowVariablesSnafu {
            path: crate::loops::path_label(path),
        })?;
        let mut instructions = BTreeSet::new();
        for id in std::iter::once(&root.input).chain(root.writes.keys()) {
            ensure!(
                ids.contains(id) && instructions.insert(id) && !graph.children.contains_key(id),
                InvalidScopeSnafu {
                    path: crate::loops::path_label(path),
                    message: "root state instructions must be disjoint known nodes without children",
                }
            );
        }
        ensure!(
            root.writes
                .values()
                .all(|variable| root.values.contains_key(variable)),
            InvalidScopeSnafu {
                path: crate::loops::path_label(path),
                message: "root write target must name a declared workflow variable",
            }
        );
        ensure!(
            !dependency_pairs(&graph.definition)
                .iter()
                .any(|(_, to)| to == &root.input),
            InvalidScopeSnafu {
                path: crate::loops::path_label(path),
                message: "workflow variables source cannot have incoming dependencies",
            }
        );
    }
    let Some(frame) = &graph.relations.scope else {
        return Ok(());
    };
    ensure!(
        !frame.is_stateful()
            || graph.definition.outputs.is_empty()
            || graph.definition.version.supports_loop_output_updates(),
        InvalidScopeSnafu {
            path: crate::loops::path_label(path),
            message: "Loop body output updates require definition version 2026-11-13",
        }
    );
    ensure!(
        !graph
            .definition
            .clusters
            .iter()
            .any(|cluster| cluster.nodes.contains(&frame.input)),
        InvalidClusterSnafu {
            message: "scope inputs cannot belong to a cluster"
        }
    );
    let mut instructions = BTreeSet::new();
    for id in std::iter::once(&frame.input)
        .chain(frame.writes.keys())
        .chain(&frame.exits)
    {
        if !ids.contains(id) || !instructions.insert(id) {
            return Err(invalid(
                "scope instruction relationships must be disjoint and reference known nodes",
            ));
        }
        if graph.children.contains_key(id) {
            return Err(invalid("scope instructions cannot own child graphs"));
        }
    }
    let mut outputs = BTreeSet::new();
    for (variable, output) in &frame.state_outputs {
        if !frame.values.contains_key(variable) || output != variable || !outputs.insert(output) {
            return Err(invalid(
                "state outputs must preserve current-frame variable names and reference known values",
            ));
        }
    }
    for variable in frame.writes.values() {
        if !frame.state_outputs.contains_key(variable) || !frame.values.contains_key(variable) {
            return Err(invalid(
                "scope write target must name an available state output and value",
            ));
        }
    }
    if !frame.is_stateful() && !frame.exits.is_empty() {
        return Err(invalid("scope exits require a stateful frame"));
    }
    for (name, value_type) in &frame.values {
        if name.trim().is_empty() || name == "index" {
            return Err(invalid(
                "scope value names must be nonblank and cannot replace the intrinsic index",
            ));
        }
        value_type
            .check_depth()
            .with_context(|_| PortTypeDepthSnafu {
                definition_id: frame.input.clone(),
                direction: "scope value",
                port: name.clone(),
            })?;
    }
    Ok(())
}

/// Normalize only for comparing a host executor's registered body to the actual
/// frozen graph. The registered executable remains authoritative.
fn canonical_host_definition(
    definition: &WorkflowDefinition,
    shape: &mf_runtime::CompiledGraph,
    root_version: mf_runtime::WorkflowDefinitionVersion,
    mut relations: GraphRelations,
    depth: usize,
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    ensure!(
        depth <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    let mut definition = definition.clone();
    // The registered definition is authoring input, not a compiled artifact.
    // Recompute instruction relationships rather than trusting the stored shape.
    crate::lowering::lower_scope(&mut definition, &mut relations)?;
    ensure!(
        relations == shape.relations,
        InvalidGraphSnafu {
            definition_id: DefinitionId::from("workflow"),
            message: "host Flow relationships differ from the frozen call graph"
        }
    );
    let order = structural_order_graph(&definition)?;
    let nodes_by_id: BTreeMap<_, _> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut nodes: Vec<_> = order.iter().map(|id| (*nodes_by_id[id]).clone()).collect();
    for node in &mut nodes {
        for (name, child) in shape.children.get(&node.id).into_iter().flatten() {
            let value = node
                .config
                .get(name)
                .filter(|value| !value.is_null())
                .with_context(|| InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "host Flow body does not match compiled bindings",
                })?;
            let raw = WorkflowDefinition::from_value_with_version(value.clone(), root_version)
                .context(NodeConfigurationSnafu {
                    definition_id: node.id.clone(),
                })?;
            let child_relations = crate::lowering::child_relations(definition.version, node, name)?;
            node.set_node_flow(
                name,
                canonical_host_definition(&raw, child, root_version, child_relations, depth + 1)?,
            )
            .context(FlowParametersSnafu {
                definition_id: node.id.clone(),
            })?;
        }
    }
    Ok(canonical_definition(&definition, nodes))
}

pub fn validate_host_flow_binding(
    node: &crate::NodeDefinition,
    graph: &mf_runtime::CompiledGraph,
    registered: &mf_runtime::RegisteredFlow,
    root_version: mf_runtime::WorkflowDefinitionVersion,
) -> Result<(), WorkflowCompileError> {
    // The root binding's host selector and observation key identify this actual
    // call site. Its executable relationships come independently from the
    // registered root definition; nested registered owners supply their own
    // child relationships during canonicalization.
    let relations = GraphRelations {
        host: graph.relations.host.clone(),
        key: graph.relations.key.clone(),
        ..GraphRelations::default()
    };
    ensure!(
        project_graph_definition(graph)?
            == canonical_host_definition(
                &registered.definition,
                graph,
                root_version,
                relations,
                0
            )?,
        InvalidGraphSnafu {
            definition_id: node.id.clone(),
            message: "host Flow binding differs from the frozen call definition",
        }
    );
    Ok(())
}

pub fn instantiate_compiled(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<Flow, WorkflowCompileError> {
    let (resolved, builder) = prepare_compiled_plan(plan, registry)?;
    if resolved.graph.definition.execution.is_some() {
        return InvalidStreamSnafu {
            message: "use instantiate_stream for streaming workflows",
        }
        .fail();
    }
    builder.into_tasks().context(FlowConstructionSnafu)
}

/// Validate stored orders, actual child bindings and construction-reachable
/// providers before any receiving factory is invoked.
pub fn validate_compiled_scope_orders(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<(), WorkflowCompileError> {
    validate_graph_structure(&plan.graph)?;
    match crate::validate_compiled_graph(registry, &plan.graph) {
        Err(crate::WorkflowBuildError::UnknownKind {
            definition_id,
            kind,
        }) => UnknownNodeKindSnafu {
            definition_id,
            kind,
        }
        .fail(),
        result => {
            result?;
            Ok(())
        }
    }
}

pub fn prepare_compiled_plan(
    plan: &CompiledWorkflow,
    registry: &NodeRegistry,
) -> Result<(CompiledWorkflow, crate::FlowBuilder), WorkflowCompileError> {
    validate_compiled_scope_orders(plan, registry)?;
    let origin_identity = Arc::new(OnceLock::new());
    let (nodes, _) = prepare_graph(
        &plan.graph,
        &mut GraphPreparation {
            registry,
            root_version: plan.graph.definition.version,
            body_layouts: None,
            path: Vec::new(),
            origin_identity: origin_identity.clone(),
        },
    )?;
    let mut resolved = plan.clone();
    resolved.graph.definition = finalize_origin_identity(&plan.graph, &nodes, &origin_identity)?;
    let builder = crate::FlowBuilder::prepare(
        nodes,
        plan.graph.definition.edges.clone(),
        plan.graph.execution_order.clone(),
        plan.graph.definition.outputs.clone(),
    )
    .and_then(|flow| flow.with_control_edges(plan.graph.definition.control_edges.clone()))
    .and_then(|flow| flow.with_order_edges(plan.graph.definition.order_edges.clone()))
    .context(FlowConstructionSnafu)?;
    let builder = builder.with_root_scope(plan.graph.relations.root.clone());
    let builder = if plan.graph.definition.version.supports_startup_inputs()
        && plan.graph.relations.is_callable()
    {
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
            | WorkflowCompileError::TypedGeneration { definition_id, .. }
            | WorkflowCompileError::OutputDerivation { definition_id, .. }
            | WorkflowCompileError::PortTypeDepth { definition_id, .. }
            | WorkflowCompileError::KnownOutputValueTypeConflict { definition_id, .. }
            | WorkflowCompileError::InvalidGraph { definition_id, .. }
            | WorkflowCompileError::ScopeStructure { definition_id, .. }
            | WorkflowCompileError::ChildGraph { definition_id, .. }
            | WorkflowCompileError::NodeConfiguration { definition_id, .. } = error
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
    let schema = if let Some(execution) = &resolved.graph.definition.execution {
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
    let (resolved, input_schema) = prepare_interface(plan, registry)?;
    let manifest = mf_runtime::WorkflowManifest {
        version: mf_runtime::WorkflowManifestVersion::V2026_11_06,
        graph: resolved.graph,
        input_schema,
    };
    Ok(manifest.interface()?)
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
    #[snafu(display("could not order described child scope at {path:?}: {source}"))]
    DescriptionOrder {
        path: Vec<String>,
        #[snafu(source(from(WorkflowCompileError, Box::new)))]
        source: Box<WorkflowCompileError>,
    },
    #[snafu(display("could not describe node `{definition_id}`"))]
    MissingNode { definition_id: DefinitionId },
    #[snafu(display("invalid workflow description: {source}"), visibility(pub))]
    Graph {
        source: mf_runtime::GraphDescriptionError,
    },
}

pub fn describe_compiled(plan: &CompiledWorkflow) -> Result<WorkflowDescription, DescriptionError> {
    let definitions: BTreeMap<_, _> = plan
        .graph
        .definition
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    ensure!(
        definitions.len() == plan.graph.definition.nodes.len()
            && plan.graph.execution_order.len() == plan.graph.definition.nodes.len(),
        InvalidPlanSnafu
    );
    for definition_id in &plan.graph.execution_order {
        definitions
            .get(definition_id.as_str())
            .with_context(|| MissingNodeSnafu {
                definition_id: definition_id.clone(),
            })?;
    }
    validate_description_orders(&plan.graph, &mut Vec::new(), None)?;
    validate_graph_structure(&plan.graph).context(DescriptionOrderSnafu {
        path: Vec::<String>::new(),
    })?;
    mf_runtime::describe_graph(&plan.graph).context(GraphSnafu)
}

fn validate_description_orders(
    graph: &mf_runtime::CompiledGraph,
    path: &mut Vec<String>,
    owner: Option<&crate::NodeDefinition>,
) -> Result<(), DescriptionError> {
    let validate = || -> Result<(), WorkflowCompileError> {
        ensure!(
            path.len() <= mf_telemetry::MAX_SCOPE_DEPTH,
            InvalidGraphSnafu {
                definition_id: DefinitionId::from("workflow"),
                message: "combined container nesting exceeds twenty scopes"
            }
        );
        ensure!(
            structural_order_graph(&graph.definition)? == graph.execution_order,
            NonCanonicalPlanOrderSnafu
        );
        Ok(())
    };
    let result = validate();
    let result = match owner {
        Some(owner) => crate::calls::child_result(owner, result),
        None => result,
    };
    result.with_context(|_| DescriptionOrderSnafu { path: path.clone() })?;
    for node in &graph.definition.nodes {
        for (name, child) in crate::calls::compiled_node_flows(graph, node)
            .with_context(|_| DescriptionOrderSnafu { path: path.clone() })?
        {
            path.push(child.child_scope_key(node, &name));
            validate_description_orders(child, path, Some(node))?;
            path.pop();
        }
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
    let resolved =
        crate::resolve_flows_with_registry(definition, None, registry).map_err(resolution_error)?;
    let normalized = plan_resolved_workflow(&resolved)?;
    validate_compiled_scope_orders(&normalized, registry)?;
    let origin_identity = Arc::new(OnceLock::new());
    let nodes = resolve_nodes_in_scope(
        &normalized.graph,
        &mut GraphPreparation {
            registry,
            root_version: definition.version,
            body_layouts: None,
            path: Vec::new(),
            origin_identity: origin_identity.clone(),
        },
    )?;
    finalize_origin_identity(&normalized.graph, &nodes, &origin_identity)?;
    Ok(nodes)
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

fn prepare_stream_outputs(
    references: &BTreeMap<String, mf_runtime::OutputReferenceDefinition>,
    nodes: &[FlowNode],
    node: &crate::NodeDefinition,
) -> Result<Vec<mf_runtime::SubgraphStreamOutput>, WorkflowCompileError> {
    references
        .iter()
        .map(|(name, reference)| {
            let index = nodes
                .iter()
                .position(|child| child.definition_id == reference.node)
                .with_context(|| mf_runtime::NodeInvalidSubgraphSnafu {
                    message: format!("unknown stream child `{}`", reference.node),
                })?;
            let port = nodes[index]
                .metadata
                .stream_outputs
                .iter()
                .find(|port| port.name == reference.port)
                .cloned()
                .with_context(|| mf_runtime::NodeInvalidSubgraphSnafu {
                    message: format!(
                        "child `{}` has no TaskStream output `{}`",
                        reference.node, reference.port
                    ),
                })?;
            Ok(mf_runtime::SubgraphStreamOutput {
                node: mf_runtime::NodeId::new(index),
                optional: !port.required,
                port,
                name: name.clone(),
            })
        })
        .collect::<Result<Vec<_>, NodeBuildError>>()
        .context(NodeConstructionSnafu {
            definition_id: node.id.clone(),
            kind: node.kind.clone(),
        })
}

fn prepare_subgraph_body(
    node: &crate::NodeDefinition,
    name: &str,
    preparation: &mut GraphPreparation<'_>,
    graph: &mf_runtime::CompiledGraph,
) -> Result<PreparedSubgraph, WorkflowCompileError> {
    let body = &graph.definition;
    ensure!(
        preparation.path.len() < mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidGraphSnafu {
            definition_id: node.id.clone(),
            message: "combined container nesting exceeds twenty scopes"
        }
    );
    let call_like = graph.relations.is_callable();
    preparation
        .path
        .push(graph.child_scope_key(node, name).into());
    let prepared = prepare_graph(graph, preparation);
    let path = preparation.path.clone();
    preparation.path.pop();
    let (nodes, order) = crate::calls::child_result(node, prepared)?;
    let mut stream_outputs = prepare_stream_outputs(&graph.relations.streams, &nodes, node)?;
    let mut terminal_outputs = Vec::new();
    for output in &body.outputs {
        let (index, source) = nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.definition_id == output.node)
            .expect("validated body output source");
        if call_like
            && let Some(port) = source
                .metadata
                .stream_outputs
                .iter()
                .find(|port| port.name == output.port)
        {
            stream_outputs.push(mf_runtime::SubgraphStreamOutput {
                node: mf_runtime::NodeId::new(index),
                port: port.clone(),
                name: output.name.to_string(),
                optional: output.optional,
            });
        } else {
            terminal_outputs.push(output.clone());
        }
    }
    let identities = body
        .nodes
        .iter()
        .filter(|node| {
            graph
                .relations
                .scope
                .as_ref()
                .is_none_or(|frame| node.id != frame.input)
        })
        .map(|node| NodeIdentity {
            id: node.id.to_string(),
            kind: node.kind.clone(),
            path: Vec::new(),
        })
        .collect();
    let builder = crate::FlowBuilder::prepare(nodes, body.edges.clone(), order, terminal_outputs)
        .and_then(|flow| flow.with_control_edges(body.control_edges.clone()))
        .and_then(|flow| flow.with_order_edges(body.order_edges.clone()))
        .context(FlowConstructionSnafu)?;
    let builder = if call_like {
        builder.with_workflow_inputs()?
    } else {
        builder
    };
    let flow = builder
        .with_root_scope(graph.relations.root.clone())
        .into_tasks()
        .context(FlowConstructionSnafu)?;
    if let Some(layouts) = preparation.body_layouts.as_mut() {
        let streams = stream_outputs
            .iter()
            .cloned()
            .map(|mut output| {
                output.node = mf_runtime::NodeId::new(
                    flow.execution_order()
                        .iter()
                        .position(|id| *id == output.node)
                        .expect("prepared child exists"),
                );
                output
            })
            .collect();
        layouts.insert(path.clone(), (flow.execution_domains().clone(), streams));
    }
    let flow = flow
        .with_stream_outputs(stream_outputs)
        .context(NodeConstructionSnafu {
            definition_id: node.id.clone(),
            kind: node.kind.clone(),
        })?;
    Ok(PreparedSubgraph::from_flow(flow, identities)
        .with_relations(graph.relations.clone())
        .with_origin_source(
            preparation.origin_identity.clone(),
            path.iter().map(ToString::to_string).collect(),
        ))
}

/// Prepare every selected child before the single registry factory dispatch.
fn prepare_node_subgraphs(
    definition: &WorkflowDefinition,
    node: &crate::NodeDefinition,
    flows: &BTreeMap<String, mf_runtime::CompiledGraph>,
    preparation: &mut GraphPreparation<'_>,
) -> Result<FlowNode, WorkflowCompileError> {
    let registration = registration_for(node, preparation.registry)?;
    ensure!(
        registration.accepts_subgraphs(),
        InvalidGraphSnafu {
            definition_id: node.id.clone(),
            message: "registered node factory does not accept prepared subgraphs"
        }
    );
    // Validate the actual frozen configuration before constructing any child.
    let declared_flows = registration.flow_parameters(&node.config);
    ensure!(
        declared_flows.keys().eq(flows.keys()),
        InvalidGraphSnafu {
            definition_id: node.id.clone(),
            message: "receiving Flow parameter declarations differ from the prepared binding set",
        }
    );
    let mut subgraphs = BTreeMap::new();
    for (name, body) in flows {
        crate::calls::validate_child(node, body)?;
        let key = body.child_scope_key(node, name);
        ensure!(
            key == node.id.as_str()
                || !definition
                    .nodes
                    .iter()
                    .any(|child| child.id.as_str() == key),
            InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "Flow parameter body key collides with an authored node ID"
            }
        );
        let host = body
            .relations
            .host
            .as_deref()
            .and_then(|name| preparation.registry.flow(name));
        let prepared = if let Some(registered) = host {
            validate_host_flow_binding(node, body, registered, preparation.root_version)?;
            ensure!(
                preparation.body_layouts.is_none(),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "host Flow executors cannot be emitted into generated execution plans"
                }
            );
            let path = preparation
                .path
                .iter()
                .map(ToString::to_string)
                .chain(std::iter::once(key))
                .collect();
            registered
                .body
                .clone()
                .with_relations(body.relations.clone())
                .with_origin_source(preparation.origin_identity.clone(), path)
        } else {
            prepare_subgraph_body(node, name, preparation, body)?
        };
        ensure!(
            prepared.stream_outputs.is_empty() || definition.version.supports_child_streams(),
            InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "child stream exports require definition version 2026-10-28",
            }
        );
        subgraphs.insert(name.clone(), prepared);
    }
    let instance = registration
        .instantiate_subgraphs(node.id.as_str(), node.config.clone(), subgraphs)
        .context(NodeConstructionSnafu {
            definition_id: node.id.clone(),
            kind: node.kind.clone(),
        })?;
    crate::validate_scope_bindings(
        &node.id,
        &instance.metadata,
        flows.values().map(|body| &body.relations),
    )?;
    Ok(FlowNode::new(node.id.clone(), instance))
}

fn resolve_nodes_in_scope(
    graph: &mf_runtime::CompiledGraph,
    preparation: &mut GraphPreparation<'_>,
) -> Result<Vec<FlowNode>, WorkflowCompileError> {
    let definition = &graph.definition;
    definition
        .nodes
        .iter()
        .map(|node| {
            match graph.relations.instruction(&node.id) {
                Some(ScopeInstruction::RootInput) => {
                    let root = graph.relations.root.as_ref().expect("validated root source");
                    return Ok(mf_runtime::prepared_root_source(root.input.as_str(), &root.values));
                }
                Some(ScopeInstruction::RootWrite(target)) => {
                    let root = graph.relations.root.as_ref().expect("validated root write");
                    return Ok(mf_runtime::prepared_root_assign(node.id.as_str(), target, root.values[target].clone()));
                }
                Some(ScopeInstruction::Input) => {
                    let frame = graph.relations.scope.as_ref().expect("validated scope input");
                    return Ok(mf_runtime::prepared_scope_source(frame.input.as_str(), &frame.values));
                }
                Some(ScopeInstruction::Write(target)) => {
                    let frame = graph.relations.scope.as_ref().expect("validated scope write");
                    let value_type = frame.values.get(target).cloned().with_context(|| InvalidScopeSnafu {
                        path: crate::loops::path_label(&preparation.path),
                        message: format!("assignment references unavailable variable `{target}`"),
                    })?;
                    return Ok(mf_runtime::prepared_loop_assign(node.id.as_str(), target, value_type));
                }
                Some(ScopeInstruction::Exit) => {
                    return Ok(mf_runtime::prepared_loop_exit(node.id.as_str()));
                }
                None => {}
            }
            let registration = registration_for(node, preparation.registry)?;
            let flows = graph.children.get(&node.id);
            let declared_flows = registration.flow_parameters(&node.config);
            ensure!(
                declared_flows.keys().eq(flows.into_iter().flat_map(BTreeMap::keys)),
                InvalidGraphSnafu {
                    definition_id: node.id.clone(),
                    message: "receiving Flow parameter declarations differ from the prepared binding set",
                }
            );
            if let Some(flows) = flows.filter(|flows| !flows.is_empty()) {
                return prepare_node_subgraphs(
                    definition,
                    node,
                    flows,
                    preparation,
                );
            }
            let instance =
                registration
                    .instantiate(node.config.clone())
                    .context(NodeConstructionSnafu {
                        definition_id: node.id.clone(),
                        kind: node.kind.clone(),
                    })?;
            Ok(FlowNode::new(node.id.clone(), instance))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    extern crate mfn_core as _;

    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ORIGIN_EVENT_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

    struct NeverRun;

    impl mf_runtime::EventNode for NeverRun {
        fn on_event(
            &mut self,
            _: mf_runtime::NodeEvent,
            _: &mf_runtime::EventContext<'_>,
        ) -> Result<mf_runtime::EventEffects, mf_runtime::NodeExecutionError> {
            panic!("identity finalization must not dispatch an Event")
        }
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind: "compiler-test.origin-event",
            factory: mf_runtime::NodeFactory::Plain(|_| {
                ORIGIN_EVENT_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
                Ok(mf_runtime::PreparedNode::from_parts(
                    mf_runtime::NodeExecution::Event(Box::new(NeverRun)),
                    mf_runtime::NodePorts {
                        inputs: vec![mf_runtime::PortSpec::new("flow", ValueType::Flow, true)],
                        outputs: Vec::new(),
                    },
                ))
            }),
        }
    }

    #[derive(mf_runtime::NodeConfig)]
    #[serde(deny_unknown_fields)]
    struct NativeFlowConfig {
        body: PreparedSubgraph,
    }

    #[derive(mf_runtime::NodeValue)]
    struct NativeEmpty {}

    #[derive(mf_runtime::NodeValue)]
    struct NativeFlowPorts {
        flow: mf_runtime::FlowValue,
    }

    struct NativeFlowProducer(mf_runtime::FlowValue);

    impl mf_runtime::TypedTaskNode for NativeFlowProducer {
        type Input = NativeEmpty;
        type Output = NativeFlowPorts;

        fn execute(
            &self,
            _: Self::Input,
            _: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError>
        {
            Ok(NativeFlowPorts {
                flow: self.0.clone(),
            }
            .into())
        }
    }

    #[derive(mf_runtime::NodeValue)]
    struct NativeAnswer {
        answer: i64,
    }

    struct NativeFlowInvoker;

    impl mf_runtime::TypedTaskNode for NativeFlowInvoker {
        type Input = NativeFlowPorts;
        type Output = NativeAnswer;

        fn execute(
            &self,
            input: Self::Input,
            context: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError>
        {
            let id = context
                .current_node()
                .expect("runtime supplies the native invoker ID")
                .to_owned();
            let outputs = input.flow.invoke(&id, Default::default(), context)?;
            Ok(NativeAnswer {
                answer: outputs["answer"].as_i64().unwrap(),
            }
            .into())
        }
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind: "compiler-test.native-flow-producer",
            factory: mf_runtime::NodeFactory::Subgraph {
                flow_parameters: <NativeFlowConfig as mf_runtime::NodeConfig>::flow_parameters,
                plain: |_| mf_runtime::NodeInvalidSubgraphSnafu {
                    message: "native producer requires a prepared callable child",
                }.fail(),
                prepared: |_, config, children| {
                    let config = <NativeFlowConfig as mf_runtime::NodeConfig>::from_config(config, children)?;
                    mf_runtime::PreparedNode::typed_task(
                        NativeFlowProducer(mf_runtime::FlowValue::new(config.body)?),
                        mf_runtime::NodeMetadata::default(),
                    )
                },
            },
        }
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind: "compiler-test.native-flow-invoker",
            factory: mf_runtime::NodeFactory::Plain(|_| {
                mf_runtime::PreparedNode::typed_task(NativeFlowInvoker, mf_runtime::NodeMetadata::default())
            }),
        }
    }

    #[test]
    fn native_flow_providers_observe_known_callable_children_without_builtin_node_names() {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-23", "dependencies":{},
            "nodes":[
                {"id":"make", "kind":"compiler-test.native-flow-producer", "config":{
                    "body":{"version":"2026-10-23", "dependencies":{},
                        "nodes":[{"id":"answer", "kind":"builtin.constant", "config":{"value":42}}],
                        "outputs":[{"name":"answer", "node":"answer", "port":"value"}]
                    }
                }},
                {"id":"apply", "kind":"compiler-test.native-flow-invoker", "inputs":{
                    "flow":{"from":{"node":"make", "port":"flow"}}
                }}
            ],
            "outputs":[{"name":"answer", "node":"apply", "port":"answer"}]
        }))
        .unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let description = describe_compiled(&plan).unwrap();
        assert_eq!(
            description.version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_11_09,
        );
        let definition_path = vec![mf_runtime::ScopePathEntry::binding_key("make", "body")];
        assert!(description.body(&definition_path).is_some());
        description
            .validate_scope_path(&[mf_runtime::ScopePathEntry::Flow {
                call_id: "apply".into(),
                definition: mf_runtime::FlowOrigin {
                    workflow_id: description.workflow_id.clone(),
                    path: definition_path,
                },
            }])
            .unwrap();
        let observation = plan
            .start_observation(
                &mf_telemetry::observation::Observer::stderr(0),
                mf_telemetry::identity::RunId::new(),
            )
            .unwrap();
        let outputs =
            execute_compiled_with_inputs(&plan, &registry, Default::default(), Some(observation))
                .unwrap();
        assert_eq!(outputs["answer"].as_i64(), Some(42));
    }

    #[test]
    fn input_calls_accept_native_flow_parameters_without_static_children() {
        let registry = NodeRegistry::from_inventory().unwrap();
        for version in ["2026-10-28", "2026-10-23"] {
            let definition: WorkflowDefinition = serde_json::from_value(json!({
                "version":version, "dependencies":{},
                "nodes":[
                    {"id":"formal", "kind":"builtin.parameters", "config":{
                        "parameters":{"callback":{"type":"flow", "secret":false}}
                    }},
                    {"id":"apply", "kind":"workflow.call", "config":{
                        "inputs":[],
                        "outputs":[{"name":"answer", "value_type":"int", "required":true}]
                    }, "inputs":{"flow":{"from":{"node":"formal", "port":"callback"}}}}
                ],
                "outputs":[{"name":"answer", "node":"apply", "port":"answer"}]
            }))
            .unwrap();
            let plan = compile_definition(&definition, &registry).unwrap();
            assert!(plan.graph.children.is_empty());
            assert_eq!(plan.graph.relations, GraphRelations::default());
            assert_eq!(
                plan.graph.definition.nodes[1].config,
                definition.nodes[1].config
            );
            let replay = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
            assert_eq!(replay, plan);
            for plan in [&plan, &replay] {
                let callable = mf_runtime::FlowValue::new(PreparedSubgraph::new(
                    Vec::new(),
                    vec![mf_runtime::PortSpec::new("answer", ValueType::Int64, true)],
                    |_| Ok(mf_runtime::Outputs::from([("answer".into(), 42.into())])),
                ))
                .unwrap();
                let outputs = execute_compiled_with_inputs(
                    plan,
                    &registry,
                    mf_runtime::WorkflowArguments(mf_runtime::Inputs::from([(
                        "formal.callback".into(),
                        callable.into(),
                    )])),
                    None,
                )
                .unwrap();
                assert_eq!(outputs["answer"].as_i64(), Some(42));
            }
        }
    }

    #[test]
    fn callable_child_protocol_promotion_retains_origins_but_preserves_pure_scope_history() {
        let describe = |value| {
            let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
            describe_compiled(&plan_definition(&definition).unwrap()).unwrap()
        };
        let root = describe(json!({
            "version":"2026-10-23", "dependencies":{}, "nodes":[]
        }));
        assert_eq!(
            root.version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_23
        );
        let unbound_invoker = describe(json!({
            "version":"2026-10-23", "dependencies":{},
            "nodes":[{"id":"apply", "kind":"workflow.call", "config":{"inputs":[], "outputs":[]}}]
        }));
        assert_eq!(unbound_invoker.version, root.version);
        let pure_loop = describe(json!({
            "version":"2026-09-29", "dependencies":{},
            "nodes":[{"id":"repeat", "kind":"loop.run", "config":{
                "max_iterations":1, "variables":[{"name":"value", "type":"int"}],
                "body":{"version":"2026-09-29", "dependencies":{}, "nodes":[]}
            }}]
        }));
        assert_eq!(
            pure_loop.version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_09_29
        );
        let mut iteration = json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"each", "kind":"builtin.iteration", "config":{
                "body":{"version":"2026-10-03", "dependencies":{}, "nodes":[]}
            }}]
        });
        let pure_iteration = describe(iteration.clone());
        assert_eq!(
            pure_iteration.version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_03
        );
        assert!(pure_iteration.loop_bodies.is_empty());
        iteration["nodes"][0]["config"]["body"]["nodes"] = json!([{
            "id":"make", "kind":"compiler-test.native-flow-producer", "config":{
                "body":{"version":"2026-10-03", "dependencies":{}, "nodes":[]}
            }
        }]);
        let nested_callable = describe(iteration);
        assert_eq!(
            nested_callable.version,
            mf_telemetry::description::WorkflowDescriptionVersion::V2026_11_09
        );
        // The old root's historically omitted iteration cannot hide the actual
        // definition origin of a capability exported inside that iteration.
        assert!(nested_callable.body(&["each".into()]).is_some());
        assert!(
            nested_callable
                .body(&[
                    "each".into(),
                    mf_runtime::ScopePathEntry::binding_key("make", "body"),
                ])
                .is_some()
        );
    }

    #[test]
    fn prepared_flow_values_resolve_the_final_root_identity_without_rebuilding_factories() {
        ORIGIN_EVENT_CONSTRUCTIONS.store(0, Ordering::SeqCst);
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        for streaming in [false, true] {
            let mut authored = json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[{"id":"make", "kind":"builtin.constant", "config":{
                    "type":"flow", "value":{"version":"2026-10-28", "dependencies":{}, "nodes":[]}
                }}],
                "outputs":[{"name":"flow", "node":"make", "port":"value"}]
            });
            if streaming {
                authored["nodes"].as_array_mut().unwrap().push(json!({
                    "id":"event", "kind":"compiler-test.origin-event",
                    "inputs":{"flow":{"from":{"node":"make", "port":"value"}}}
                }));
            } else {
                authored["execution"] = json!({});
            }
            let definition: WorkflowDefinition = serde_json::from_value(authored).unwrap();
            let plan = plan_definition(&definition).unwrap();
            let original = plan.graph.workflow_id().unwrap();
            let (nodes, _) =
                prepare_compiled_graph_with_layouts(&plan.graph, &registry, None).unwrap();
            let mut resolved = plan.graph.clone();
            resolved.definition = resolve_execution(&resolved.definition, &nodes).unwrap();
            let expected = resolved.workflow_id().unwrap();
            assert_ne!(original, expected);
            let make = nodes
                .iter()
                .find(|node| node.definition_id.as_str() == "make")
                .unwrap();
            assert_eq!(
                make.metadata.output_derivations,
                [OutputDerivation::known_type("value", ValueType::Flow)]
            );
            assert_eq!(make.metadata.ports.outputs[0].value_type, ValueType::Flow);
            let producer = make.node.as_ref().unwrap().as_task_node().unwrap();
            let result = producer
                .execute(
                    Default::default(),
                    &mut mf_runtime::ExecutionContext::default(),
                )
                .unwrap();
            let flow = result.outputs["value"].as_flow().unwrap();
            let origin = flow
                .body()
                .origin()
                .expect("completed root preparation has an origin");
            assert_eq!(origin.workflow_id, expected);
            assert_eq!(
                origin.path,
                [plan.graph.children[&"make".into()]["value"]
                    .child_scope_key(&plan.graph.definition.nodes[0], "value",)],
            );
            assert_eq!(
                ORIGIN_EVENT_CONSTRUCTIONS.load(Ordering::SeqCst),
                usize::from(streaming),
            );
        }
    }
}
