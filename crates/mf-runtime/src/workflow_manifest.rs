use crate::{
    WorkflowInputError, WorkflowInputSchema, WorkflowInterface,
    workflow_inputs::{from_unique_json, pointer},
};
use mf_telemetry::description::{
    ClusterDescription, ControlEdge, DataEdge, ExecutionDescription, ExecutionMode,
    LoopBodyDescription, NodeDescription, OrderEdge, WorkflowDescriptionVersion,
};
use mf_telemetry::identity::WorkflowId;
use mf_telemetry::{ContractError, description::WorkflowDescription};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopExecutionOrder {
    pub path: Vec<String>,
    pub execution_order: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowManifestVersion {
    #[serde(rename = "2026-10-16")]
    V2026_10_16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowManifest {
    pub version: WorkflowManifestVersion,
    pub definition: crate::WorkflowDefinition,
    pub execution_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loop_orders: Vec<LoopExecutionOrder>,
    pub input_schema: WorkflowInputSchema,
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
    Graph { source: ContractError },
    #[snafu(display("invalid workflow manifest interface: {source}"))]
    Interface { source: WorkflowInputError },
}

impl WorkflowManifest {
    /// Borrows an original root node without preparing or executing its provider.
    pub fn node_definition(&self, id: &str) -> Option<&crate::NodeDefinition> {
        self.definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == id)
    }

    pub fn workflow_id(&self) -> Result<WorkflowId, WorkflowManifestError> {
        WorkflowId::from_definition(&self.definition, &self.execution_order).context(GraphSnafu)
    }

    pub fn description(&self) -> Result<WorkflowDescription, WorkflowManifestError> {
        describe_definition(&self.definition, &self.execution_order, &self.loop_orders)
            .context(GraphSnafu)
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
        if !self.definition.version.supports_startup_inputs() && description.execution.is_some() {
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

/// Projects the observation view without planning or providers.
pub fn describe_definition(
    definition: &crate::WorkflowDefinition,
    execution_order: &[String],
    loop_orders: &[LoopExecutionOrder],
) -> Result<WorkflowDescription, ContractError> {
    let mut orders: BTreeMap<_, _> = loop_orders
        .iter()
        .map(|order| (order.path.clone(), order.execution_order.as_slice()))
        .collect();
    ensure!(
        orders.len() == loop_orders.len(),
        mf_telemetry::InvalidSnafu {
            message: "duplicate compiled Loop execution order"
        }
    );
    let mut loop_bodies = Vec::new();
    let mut omitted_iterations = Vec::new();
    describe_loops(
        definition,
        &mut Vec::new(),
        &mut orders,
        &mut loop_bodies,
        definition.has_order_edges() || definition.version.supports_clusters(),
        &mut omitted_iterations,
    )?;
    ensure!(
        orders.is_empty(),
        mf_telemetry::InvalidSnafu {
            message: "compiled execution order names an unknown Loop body"
        }
    );
    // Validate every supplied scope, including scopes omitted by the historical
    // observation projection, using an existing schema that supports all features.
    let mut description = WorkflowDescription {
        order_edges: order_edges(&definition.order_edges),
        version: WorkflowDescriptionVersion::V2026_10_23,
        execution: Some(ExecutionDescription {
            mode: if definition.execution.is_some() {
                ExecutionMode::Stream
            } else {
                ExecutionMode::Single
            },
        }),
        workflow_id: WorkflowId::from_definition(definition, execution_order)?,
        nodes: ordered_nodes(
            definition
                .nodes
                .iter()
                .map(|node| (node.id.as_str(), node.kind.as_str())),
            execution_order,
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
    if !omitted_iterations.is_empty() {
        description.validate()?;
        description.loop_bodies.retain(|body| {
            !omitted_iterations
                .iter()
                .any(|path| body.path.starts_with(path))
        });
    }
    // Receiving fields can carry newer child features even when the authored root
    // is old. Select a compatible existing observation schema from the projection;
    // this never changes the authored workflow version or scans opaque JSON.
    description.version = if definition.version.supports_clusters()
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
    description.validate()?;
    Ok(description)
}

fn ordered_nodes<'a>(
    nodes: impl IntoIterator<Item = (&'a str, &'a str)>,
    order: &[String],
) -> Result<Vec<NodeDescription>, ContractError> {
    let mut definitions = BTreeMap::new();
    for (id, kind) in nodes {
        ensure!(
            definitions.insert(id, kind).is_none(),
            mf_telemetry::InvalidSnafu {
                message: "duplicate node definition"
            }
        );
    }
    ensure!(
        definitions.len() == order.len(),
        mf_telemetry::InvalidSnafu {
            message: "compiled execution order does not match the node count"
        }
    );
    order
        .iter()
        .map(|id| {
            let kind =
                definitions
                    .get(id.as_str())
                    .with_context(|| mf_telemetry::InvalidSnafu {
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

fn describe_loops(
    parent: &crate::WorkflowDefinition,
    path: &mut Vec<String>,
    orders: &mut BTreeMap<Vec<String>, &[String]>,
    bodies: &mut Vec<LoopBodyDescription>,
    include_iterations: bool,
    omitted_iterations: &mut Vec<Vec<String>>,
) -> Result<(), ContractError> {
    for node in &parent.nodes {
        let omit_iteration = !include_iterations && node.kind == crate::ITERATION_KIND;
        if omit_iteration {
            let mut body_path = path.clone();
            body_path.push(node.flow_scope_key("body"));
            // Historical manifests can omit these orders entirely. New compiler
            // facts include them: consume and validate all supplied paths before
            // omitting the subtree from the observation view.
            if !orders.keys().any(|order| order.starts_with(&body_path)) {
                continue;
            }
            omitted_iterations.push(body_path);
        }
        // Scope paths are the compiler's authority for ordinary configuration.
        // Without receiving metadata, an omitted outer typed scope is intentionally
        // indistinguishable from opaque JSON; never infer it from object contents.
        let parameters = node.compiled_flow_fields(path, orders.keys().map(Vec::as_slice));
        // Translate definition errors only at the observation projection boundary.
        // Keep the original JSON source rather than stringifying it.
        let flows = node
            .flow_parameters(&parameters)
            .or_else(|error| match error {
                crate::FlowParameterError::Invalid { message } => {
                    mf_telemetry::InvalidSnafu { message }.fail()
                }
                crate::FlowParameterError::Json { source, .. } => Err(ContractError::from(source)),
            })?;
        for (name, body) in flows {
            path.push(node.flow_scope_key(&name));
            let order = orders
                .remove(path)
                .with_context(|| mf_telemetry::InvalidSnafu {
                    message: format!(
                        "missing compiled {} at {path:?}",
                        if node.flow_scope_key(&name) == node.id.as_str() {
                            "body execution order"
                        } else {
                            "Flow parameter order"
                        }
                    ),
                })?;
            bodies.push(LoopBodyDescription {
                path: path.clone(),
                nodes: ordered_nodes(
                    body.nodes
                        .iter()
                        .map(|node| (node.id.as_str(), node.kind.as_str())),
                    order,
                )?,
                data_edges: data_edges(&body.edges),
                control_edges: control_edges(&body.control_edges),
                order_edges: order_edges(&body.order_edges),
                execution_order: order.to_vec(),
                clusters: clusters(&body.clusters),
            });
            describe_loops(
                &body,
                path,
                orders,
                bodies,
                include_iterations,
                omitted_iterations,
            )?;
            path.pop();
        }
    }
    Ok(())
}
