use crate::{
    CompiledGraph, DefinitionId, WorkflowInputError, WorkflowInputSchema, WorkflowInterface,
    workflow_inputs::{from_unique_json, pointer},
};
use mf_telemetry::description::{
    ClusterDescription, ControlEdge, DataEdge, ExecutionDescription, ExecutionMode,
    LoopBodyDescription, NodeDescription, OrderEdge, WorkflowDescription,
    WorkflowDescriptionVersion,
};
use mf_telemetry::identity::WorkflowId;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const MANIFEST_MAGIC: &[u8; 8] = b"MFMANIF\0";
pub const MANIFEST_FRAMING_VERSION: u32 = 1;
pub const MANIFEST_HEADER_BYTES: usize = 20;
pub const MAX_MANIFEST_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MANIFEST_PADDING_BYTES: usize = 4096;
pub const MAX_MANIFEST_SECTION_BYTES: usize =
    MANIFEST_HEADER_BYTES + MAX_MANIFEST_PAYLOAD_BYTES + MAX_MANIFEST_PADDING_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowManifestVersion {
    #[serde(rename = "2026-11-06")]
    V2026_11_06,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowManifest {
    pub version: WorkflowManifestVersion,
    pub graph: CompiledGraph,
    pub input_schema: WorkflowInputSchema,
}

/// Failures describing authoritative compiled graph facts without preparing providers.
///
/// Graph integrity and authored-definition failures belong to the runtime. Only
/// projecting those facts into the observation schema or its identity can produce
/// an observation-contract source.
#[derive(Debug, Snafu)]
pub enum GraphDescriptionError {
    #[snafu(display("invalid compiled graph: {message}"))]
    Invalid { message: String },
    #[snafu(display("invalid compiled workflow definition: {source}"))]
    Definition { source: serde_json::Error },
    #[snafu(
        display("invalid graph observation projection: {source}"),
        visibility(pub)
    )]
    Observation { source: mf_telemetry::ContractError },
}

#[derive(Debug, Snafu)]
pub enum WorkflowManifestError {
    #[snafu(display("compiled workflow interface drift at `{path}`: {component} differs"))]
    InterfaceDrift {
        path: String,
        component: &'static str,
    },
    #[snafu(display("invalid workflow manifest framing: {message}"))]
    Framing { message: &'static str },
    #[snafu(display("unsupported workflow manifest framing version {version}"))]
    UnsupportedFraming { version: u32 },
    #[snafu(display("workflow manifest exceeds the {limit}-byte limit"))]
    TooLarge { limit: usize },
    #[snafu(display("invalid workflow manifest JSON: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("invalid workflow manifest graph: {source}"))]
    Graph { source: GraphDescriptionError },
    #[snafu(display("invalid workflow manifest interface: {source}"))]
    Interface { source: WorkflowInputError },
}

impl WorkflowManifest {
    /// Borrows an original root node without preparing or executing its provider.
    pub fn node_definition(&self, id: &str) -> Option<&crate::NodeDefinition> {
        self.graph
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == id)
    }

    pub fn workflow_id(&self) -> Result<WorkflowId, WorkflowManifestError> {
        self.graph.workflow_id().context(GraphSnafu)
    }

    pub fn description(&self) -> Result<WorkflowDescription, WorkflowManifestError> {
        describe_graph(&self.graph).context(GraphSnafu)
    }

    pub fn interface(&self) -> Result<WorkflowInterface, WorkflowManifestError> {
        let description = self.description()?;
        let interface = self.interface_for(&description);
        interface
            .validate_for_description(&description)
            .context(InterfaceSnafu)?;
        Ok(interface)
    }

    fn interface_for(&self, description: &WorkflowDescription) -> WorkflowInterface {
        let mut schema = self.input_schema.clone();
        if !self.graph.definition.version.supports_startup_inputs()
            && description.execution.is_some()
        {
            // A newer child can require a newer observation schema, whose interface
            // describes every initial node. Preserve the old root's startup policy:
            // these declarations expose no input ports, bindings, or stdin ownership.
            // The stored schema remains the actual prepared schema for drift checks.
            let incoming = description
                .data_edges
                .iter()
                .map(|edge| edge.to_node.as_str())
                .chain(
                    description
                        .control_edges
                        .iter()
                        .map(|edge| edge.to_node.as_str()),
                )
                .chain(
                    description
                        .order_edges
                        .iter()
                        .map(|edge| edge.to_node.as_str()),
                )
                .collect::<BTreeSet<_>>();
            for node in &description.nodes {
                if !incoming.contains(node.id.as_str()) {
                    schema.inputs.entry(node.id.clone()).or_default();
                }
            }
        }
        WorkflowInterface {
            version: crate::WorkflowInterfaceVersion::for_schema(&schema),
            workflow_id: description.workflow_id.clone(),
            schema,
        }
    }

    pub fn validate_prepared_schema(
        &self,
        actual: &WorkflowInputSchema,
    ) -> Result<(), WorkflowManifestError> {
        let expected = &self.input_schema;
        for node in expected.inputs.keys().chain(actual.inputs.keys()) {
            let path = pointer("", node);
            let (Some(expected), Some(actual)) =
                (expected.inputs.get(node), actual.inputs.get(node))
            else {
                return InterfaceDriftSnafu {
                    path,
                    component: "initial node declaration",
                }
                .fail();
            };
            for port in expected.keys().chain(actual.keys()) {
                let path = pointer(&path, port);
                let (Some(expected), Some(actual)) = (expected.get(port), actual.get(port)) else {
                    return InterfaceDriftSnafu {
                        path,
                        component: "input port declaration",
                    }
                    .fail();
                };
                ensure!(
                    expected.value_type == actual.value_type,
                    InterfaceDriftSnafu {
                        path: path.clone(),
                        component: "input type"
                    }
                );
                ensure!(
                    expected.required == actual.required,
                    InterfaceDriftSnafu {
                        path: path.clone(),
                        component: "required flag"
                    }
                );
                ensure!(
                    expected.binding == actual.binding,
                    InterfaceDriftSnafu {
                        path,
                        component: "input binding"
                    }
                );
            }
        }
        for node in expected.stdin.keys().chain(actual.stdin.keys()) {
            ensure!(
                expected.stdin.get(node) == actual.stdin.get(node),
                InterfaceDriftSnafu {
                    path: pointer("", node),
                    component: "stdin ownership condition"
                }
            );
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), WorkflowManifestError> {
        let description = self.description()?;
        self.interface_for(&description)
            .validate_for_description(&description)
            .context(InterfaceSnafu)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WorkflowManifestError> {
        ensure!(
            bytes.len() <= MAX_MANIFEST_SECTION_BYTES,
            TooLargeSnafu {
                limit: MAX_MANIFEST_SECTION_BYTES
            }
        );
        ensure!(
            bytes.len() >= MANIFEST_HEADER_BYTES,
            FramingSnafu {
                message: "incomplete header"
            }
        );
        ensure!(
            &bytes[..8] == MANIFEST_MAGIC,
            FramingSnafu {
                message: "invalid magic"
            }
        );
        let version = u32::from_le_bytes(bytes[8..12].try_into().expect("checked header"));
        ensure!(
            version == MANIFEST_FRAMING_VERSION,
            UnsupportedFramingSnafu { version }
        );
        let length = u64::from_le_bytes(bytes[12..20].try_into().expect("checked header"));
        ensure!(
            length <= MAX_MANIFEST_PAYLOAD_BYTES as u64,
            TooLargeSnafu {
                limit: MAX_MANIFEST_PAYLOAD_BYTES
            }
        );
        let end = MANIFEST_HEADER_BYTES + length as usize;
        ensure!(
            end <= bytes.len(),
            FramingSnafu {
                message: "incomplete payload"
            }
        );
        let padding = &bytes[end..];
        ensure!(
            padding.len() <= MAX_MANIFEST_PADDING_BYTES && padding.iter().all(|byte| *byte == 0),
            FramingSnafu {
                message: "invalid alignment padding"
            }
        );
        let manifest: Self =
            from_unique_json(&bytes[MANIFEST_HEADER_BYTES..end]).context(JsonSnafu)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, WorkflowManifestError> {
        self.validate()?;
        let payload = serde_json::to_vec(self).context(JsonSnafu)?;
        ensure!(
            payload.len() <= MAX_MANIFEST_PAYLOAD_BYTES,
            TooLargeSnafu {
                limit: MAX_MANIFEST_PAYLOAD_BYTES
            }
        );
        let mut bytes = Vec::with_capacity(MANIFEST_HEADER_BYTES + payload.len());
        bytes.extend_from_slice(MANIFEST_MAGIC);
        bytes.extend_from_slice(&MANIFEST_FRAMING_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }
}

/// Projects the observation view from actual compiled scopes, without planning,
/// providers, or discovery from configuration JSON.
pub fn describe_graph(graph: &CompiledGraph) -> Result<WorkflowDescription, GraphDescriptionError> {
    validate_relations(graph)?;
    let definition = &graph.definition;
    serde_json::to_writer(std::io::sink(), definition).context(DefinitionSnafu)?;
    let execution_order = string_order(&graph.execution_order);
    let mut loop_bodies = Vec::new();
    let mut omitted_iterations = Vec::new();
    let callable_children = has_callable_children(graph);
    describe_children(
        graph,
        &mut Vec::new(),
        &mut loop_bodies,
        definition.has_order_edges() || definition.version.supports_clusters() || callable_children,
        &mut omitted_iterations,
    )?;
    // Validate every supplied scope, including scopes omitted by the historical
    // observation projection, using an existing schema that supports all features.
    let mut description = WorkflowDescription {
        order_edges: order_edges(&definition.order_edges),
        version: if callable_children {
            WorkflowDescriptionVersion::V2026_11_09
        } else {
            WorkflowDescriptionVersion::V2026_10_23
        },
        execution: Some(ExecutionDescription {
            mode: if definition.execution.is_some() {
                ExecutionMode::Stream
            } else {
                ExecutionMode::Single
            },
        }),
        workflow_id: graph.workflow_id()?,
        nodes: ordered_nodes(
            definition
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node.kind.as_str())),
            &execution_order,
        )?,
        data_edges: data_edges(&definition.edges),
        control_edges: control_edges(&definition.control_edges),
        execution_order: execution_order.to_vec(),
        loop_bodies,
        clusters: clusters(&definition.clusters),
    };
    // An old root's historical projection cannot hide features that require a
    // newer observation schema in an independently versioned child scope.
    omitted_iterations.retain(|path| {
        !description.loop_bodies.iter().any(|body| {
            body.path.starts_with(path)
                && (!body.order_edges.is_empty() || !body.clusters.is_empty())
        })
    });
    description.validate().context(ObservationSnafu)?;
    if !omitted_iterations.is_empty() {
        description.loop_bodies.retain(|body| {
            !omitted_iterations
                .iter()
                .any(|path| body.path.starts_with(path))
        });
    }
    // Receiving fields can carry newer child features even when the authored root
    // is old. Select a compatible existing observation schema from the projection;
    // this never changes the authored workflow version or scans opaque JSON.
    description.version = if callable_children {
        WorkflowDescriptionVersion::V2026_11_09
    } else if definition.version.supports_clusters()
        || !description.clusters.is_empty()
        || description
            .loop_bodies
            .iter()
            .any(|body| !body.clusters.is_empty())
    {
        WorkflowDescriptionVersion::V2026_10_23
    } else if definition.has_order_edges()
        || description
            .loop_bodies
            .iter()
            .any(|body| !body.order_edges.is_empty())
    {
        WorkflowDescriptionVersion::V2026_10_19
    } else if definition.version.supports_startup_inputs() {
        WorkflowDescriptionVersion::V2026_10_03
    } else if definition.version != crate::WorkflowDefinitionVersion::V2026_09_26
        || !description.loop_bodies.is_empty()
    {
        WorkflowDescriptionVersion::V2026_09_29
    } else {
        WorkflowDescriptionVersion::V2026_09_27
    };
    if matches!(
        description.version,
        WorkflowDescriptionVersion::V2026_09_27 | WorkflowDescriptionVersion::V2026_09_29
    ) {
        description.execution = None;
    }
    description.validate().context(ObservationSnafu)?;
    Ok(description)
}

/// Any actual callable child can be exported as a native Flow capability by its
/// receiving provider. Keep its origin observable without knowing provider kinds
/// or interpreting opaque configuration. Pure state/iteration scopes alone do
/// not require the newer protocol.
fn has_callable_children(graph: &CompiledGraph) -> bool {
    graph
        .children
        .values()
        .flat_map(BTreeMap::values)
        .any(|child| child.relations.is_callable() || has_callable_children(child))
}

/// Validate provider-free endpoints before any observation-only projection.
fn validate_relations(graph: &CompiledGraph) -> Result<(), GraphDescriptionError> {
    ensure!(
        graph
            .relations
            .host
            .as_ref()
            .is_none_or(|name| graph.relations.is_callable() && !name.trim().is_empty()),
        InvalidSnafu {
            message: "compiled host references require a callable scope and nonblank name"
        }
    );
    let nodes = graph
        .definition
        .nodes
        .iter()
        .map(|node| &node.id)
        .collect::<BTreeSet<_>>();
    for (name, output) in &graph.relations.streams {
        ensure!(
            !name.trim().is_empty()
                && !output.port.trim().is_empty()
                && nodes.contains(&output.node),
            InvalidSnafu {
                message: "compiled stream relation has an invalid output endpoint"
            }
        );
    }
    let Some(scope) = &graph.relations.scope else {
        return Ok(());
    };
    let mut instructions = BTreeSet::new();
    for id in std::iter::once(&scope.input)
        .chain(scope.writes.keys())
        .chain(scope.exits.iter())
    {
        ensure!(
            nodes.contains(id),
            InvalidSnafu {
                message: format!("compiled scope instruction names unknown node `{id}`")
            }
        );
        ensure!(
            instructions.insert(id),
            InvalidSnafu {
                message: "compiled scope instruction roles collide"
            }
        );
    }
    ensure!(
        scope
            .values
            .keys()
            .all(|name| !name.trim().is_empty() && name != "index"),
        InvalidSnafu {
            message: "compiled scope variable names must be nonblank"
        }
    );
    for variable in scope.writes.values().chain(scope.state_outputs.keys()) {
        ensure!(
            scope.values.contains_key(variable),
            InvalidSnafu {
                message: format!("compiled scope relation names unknown variable `{variable}`")
            }
        );
    }
    let mut outputs = BTreeSet::new();
    for (variable, output) in &scope.state_outputs {
        ensure!(
            output == variable && outputs.insert(output),
            InvalidSnafu {
                message: "compiled state outputs must preserve current-frame variable names"
            }
        );
    }
    ensure!(
        scope
            .writes
            .values()
            .all(|variable| scope.state_outputs.contains_key(variable)),
        InvalidSnafu {
            message: "compiled scope writes require declared state outputs"
        }
    );
    ensure!(
        scope.is_stateful() || scope.exits.is_empty(),
        InvalidSnafu {
            message: "compiled scope exits require a stateful frame"
        }
    );
    ensure!(
        instructions
            .iter()
            .all(|id| !graph.children.contains_key(*id)),
        InvalidSnafu {
            message: "compiled scope instructions cannot own child graphs"
        }
    );
    Ok(())
}

fn ordered_nodes<'a>(
    nodes: impl IntoIterator<Item = (&'a str, &'a str)>,
    order: &[String],
) -> Result<Vec<NodeDescription>, GraphDescriptionError> {
    let mut definitions = BTreeMap::new();
    for (id, kind) in nodes {
        ensure!(
            definitions.insert(id, kind).is_none(),
            InvalidSnafu {
                message: "duplicate node definition"
            }
        );
    }
    ensure!(
        definitions.len() == order.len(),
        InvalidSnafu {
            message: "compiled execution order does not match the node count"
        }
    );
    let mut seen = BTreeSet::new();
    order
        .iter()
        .map(|id| {
            ensure!(
                seen.insert(id),
                InvalidSnafu {
                    message: format!("compiled execution order repeats node `{id}`")
                }
            );
            let kind = definitions.get(id.as_str()).with_context(|| InvalidSnafu {
                message: format!("compiled execution order names unknown node `{id}`"),
            })?;
            Ok(NodeDescription {
                id: id.clone(),
                kind: (*kind).into(),
            })
        })
        .collect()
}

fn data_edges(edges: &[crate::EdgeDefinition]) -> Vec<DataEdge> {
    edges
        .iter()
        .map(|edge| DataEdge {
            exclusive: edge.exclusive,
            from_node: edge.from_node.to_string(),
            from_output: edge.from_output.clone(),
            to_node: edge.to_node.to_string(),
            to_input: edge.to_input.clone(),
        })
        .collect()
}

fn control_edges(edges: &[crate::ControlEdgeDefinition]) -> Vec<ControlEdge> {
    edges
        .iter()
        .map(|edge| ControlEdge {
            from_node: edge.from_node.to_string(),
            from_output: edge.from_output.to_string(),
            to_node: edge.to_node.to_string(),
        })
        .collect()
}

fn clusters(clusters: &[crate::ClusterDefinition]) -> Vec<ClusterDescription> {
    clusters
        .iter()
        .map(|cluster| ClusterDescription {
            label: cluster.label.clone(),
            annotation: cluster.annotation.clone(),
            nodes: cluster.nodes.iter().map(ToString::to_string).collect(),
        })
        .collect()
}

fn order_edges(edges: &[crate::OrderEdgeDefinition]) -> Vec<OrderEdge> {
    edges
        .iter()
        .map(|edge| OrderEdge {
            from_node: edge.from_node.to_string(),
            to_node: edge.to_node.to_string(),
        })
        .collect()
}

fn string_order(order: &[DefinitionId]) -> Vec<String> {
    order.iter().map(ToString::to_string).collect()
}

fn describe_children(
    parent: &CompiledGraph,
    path: &mut Vec<String>,
    bodies: &mut Vec<LoopBodyDescription>,
    include_iterations: bool,
    omitted_iterations: &mut Vec<Vec<String>>,
) -> Result<(), GraphDescriptionError> {
    ensure!(
        path.len() <= mf_telemetry::MAX_SCOPE_DEPTH,
        InvalidSnafu {
            message: "invalid compiled child scope depth"
        }
    );
    for owner in parent.children.keys() {
        ensure!(
            parent.definition.nodes.iter().any(|node| node.id == *owner),
            InvalidSnafu {
                message: format!("compiled child names unknown owner `{owner}` at {path:?}")
            }
        );
    }
    // Walk authored node order and sorted binding names, preserving the stable
    // observation order without using kinds or config shapes to discover children.
    let mut scope_keys = BTreeSet::new();
    for node in &parent.definition.nodes {
        let Some(children) = parent.children.get(&node.id) else {
            continue;
        };
        for (name, child) in children {
            ensure!(
                !name.trim().is_empty(),
                InvalidSnafu {
                    message: "compiled child binding names must be nonblank",
                }
            );
            let key = child.child_scope_key(node, name);
            ensure!(
                key == node.id.as_str()
                    || key == crate::ScopePathEntry::binding_key(node.id.as_str(), name),
                InvalidSnafu {
                    message: "compiled scope keys must reference their actual receiving owner and binding",
                }
            );
            ensure!(
                scope_keys.insert(key.clone()),
                InvalidSnafu {
                    message: "compiled child scope keys collide"
                }
            );
            ensure!(
                key == node.id.as_str()
                    || !parent
                        .definition
                        .nodes
                        .iter()
                        .any(|other| other.id.as_str() == key),
                InvalidSnafu {
                    message: "compiled child scope key collides with another node ID"
                }
            );
            path.push(key);
            // Historical projection omits stateless execution frames, independently
            // of provider names. Keep every scope until full graph validation passes.
            if !include_iterations
                && child
                    .relations
                    .scope
                    .as_ref()
                    .is_some_and(|scope| !scope.is_stateful())
            {
                omitted_iterations.push(path.clone());
            }
            validate_relations(child)?;
            let body = &child.definition;
            // Each actual child has its own authored version and wire invariants.
            // Validate those without decoding any configuration field as a graph,
            // retaining serde's typed source for malformed in-memory definitions.
            serde_json::to_writer(std::io::sink(), body).context(DefinitionSnafu)?;
            let order = string_order(&child.execution_order);
            bodies.push(LoopBodyDescription {
                path: path.clone(),
                source: child
                    .relations
                    .scope
                    .as_ref()
                    .map(|frame| frame.input.to_string()),
                state_outputs: child
                    .relations
                    .scope
                    .as_ref()
                    .map(|frame| frame.state_outputs.clone())
                    .unwrap_or_default(),
                nodes: ordered_nodes(
                    body.nodes
                        .iter()
                        .map(|node| (node.id.as_str(), node.kind.as_str())),
                    &order,
                )?,
                data_edges: data_edges(&body.edges),
                control_edges: control_edges(&body.control_edges),
                order_edges: order_edges(&body.order_edges),
                execution_order: order,
                clusters: clusters(&body.clusters),
            });
            describe_children(child, path, bodies, include_iterations, omitted_iterations)?;
            path.pop();
        }
    }
    Ok(())
}
