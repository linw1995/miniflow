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
use std::collections::BTreeMap;

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
    #[serde(rename = "2026-10-23")]
    V2026_10_23,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_outputs: Vec<crate::StreamOutputContract>,
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
    pub fn supports_live_output(&self) -> bool {
        self.version == WorkflowManifestVersion::V2026_10_23 && self.definition.execution.is_none()
    }

    pub fn validate_prepared_stream_outputs(
        &self,
        actual: &[crate::StreamOutputContract],
    ) -> Result<(), WorkflowManifestError> {
        let mut expected = self.stream_outputs.clone();
        let mut actual = actual.to_vec();
        expected.sort_by(|a, b| a.name.cmp(&b.name));
        actual.sort_by(|a, b| a.name.cmp(&b.name));
        ensure!(
            actual == expected,
            InterfaceDriftSnafu {
                path: "/stream_outputs",
                component: "live output contract"
            }
        );
        Ok(())
    }
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
        Ok(self.interface_for(self.workflow_id()?))
    }

    fn interface_for(&self, workflow_id: WorkflowId) -> WorkflowInterface {
        WorkflowInterface {
            version: if self.supports_live_output() || !self.stream_outputs.is_empty() {
                crate::WorkflowInterfaceVersion::V2026_10_23
            } else {
                crate::WorkflowInterfaceVersion::for_schema(&self.input_schema)
            },
            workflow_id,
            schema: self.input_schema.clone(),
            live_output: self.supports_live_output(),
            stream_outputs: self.stream_outputs.clone(),
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
        crate::workflow_inputs::validate_stream_outputs(&self.stream_outputs)
            .context(InterfaceSnafu)?;
        ensure!(
            self.version == WorkflowManifestVersion::V2026_10_23
                || (self.stream_outputs.is_empty() && self.definition.stream_outputs.is_empty()),
            FramingSnafu {
                message: "live output contracts require manifest version 2026-10-23",
            }
        );
        let declared = self
            .definition
            .stream_outputs
            .iter()
            .map(|output| (output.name.as_str(), output))
            .collect::<BTreeMap<_, _>>();
        ensure!(
            declared.len() == self.definition.stream_outputs.len()
                && declared.len() == self.stream_outputs.len(),
            FramingSnafu {
                message: "live output declarations and prepared contracts disagree",
            }
        );
        for output in &self.stream_outputs {
            ensure!(
                declared
                    .get(output.name.as_str())
                    .is_some_and(
                        |selection| selection.path == output.path && selection.port == output.port
                    ),
                FramingSnafu {
                    message: "live output source route differs from its declaration",
                }
            );
        }
        let description = self.description()?;
        self.interface_for(description.workflow_id.clone())
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
    describe_loops(
        &definition.nodes,
        &mut Vec::new(),
        &mut orders,
        &mut loop_bodies,
        definition.has_order_edges()
            || definition.version == crate::WorkflowDefinitionVersion::V2026_10_23,
    )?;
    ensure!(
        orders.is_empty(),
        mf_telemetry::InvalidSnafu {
            message: "compiled execution order names an unknown Loop body"
        }
    );
    let description = WorkflowDescription {
        order_edges: order_edges(&definition.order_edges),
        version: if definition.version == crate::WorkflowDefinitionVersion::V2026_10_23 {
            WorkflowDescriptionVersion::V2026_10_23
        } else if definition.has_order_edges() {
            WorkflowDescriptionVersion::V2026_10_19
        } else {
            match definition.version {
                crate::WorkflowDefinitionVersion::V2026_09_26 => {
                    WorkflowDescriptionVersion::V2026_09_27
                }
                crate::WorkflowDefinitionVersion::V2026_09_29
                | crate::WorkflowDefinitionVersion::V2026_10_02 => {
                    WorkflowDescriptionVersion::V2026_09_29
                }
                crate::WorkflowDefinitionVersion::V2026_10_03
                | crate::WorkflowDefinitionVersion::V2026_10_19
                | crate::WorkflowDefinitionVersion::V2026_10_23 => {
                    WorkflowDescriptionVersion::V2026_10_03
                }
            }
        },
        execution: definition
            .version
            .supports_startup_inputs()
            .then(|| ExecutionDescription {
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
    nodes: &[crate::NodeDefinition],
    path: &mut Vec<String>,
    orders: &mut BTreeMap<Vec<String>, &[String]>,
    bodies: &mut Vec<LoopBodyDescription>,
    include_iterations: bool,
) -> Result<(), ContractError> {
    for node in nodes {
        let iteration;
        let (nodes, data, controls, successes, body_clusters, source_id, source_kind) =
            if let Some(definition) = &node.loop_definition {
                let body = &definition.body;
                (
                    &body.nodes,
                    &body.edges,
                    &body.control_edges,
                    &body.order_edges,
                    &body.clusters,
                    crate::LOOP_SOURCE_ID,
                    crate::LOOP_SOURCE_ID,
                )
            } else if include_iterations && node.kind == crate::ITERATION_KIND {
                iteration = serde_json::from_value::<crate::IterationConfig>(node.config.clone())?;
                let body = &iteration.body;
                (
                    &body.nodes,
                    &body.edges,
                    &body.control_edges,
                    &body.order_edges,
                    &body.clusters,
                    crate::ITERATION_INPUT_ID,
                    crate::ITERATION_INPUT_KIND,
                )
            } else {
                continue;
            };
        path.push(node.id.to_string());
        let order = orders
            .remove(path)
            .with_context(|| mf_telemetry::InvalidSnafu {
                message: format!("missing compiled body execution order at {path:?}"),
            })?;
        bodies.push(LoopBodyDescription {
            path: path.clone(),
            nodes: ordered_nodes(
                std::iter::once((source_id, source_kind)).chain(
                    nodes
                        .iter()
                        .map(|node| (node.id.as_str(), node.kind.as_str())),
                ),
                order,
            )?,
            data_edges: data_edges(data),
            control_edges: control_edges(controls),
            order_edges: order_edges(successes),
            execution_order: order.to_vec(),
            clusters: clusters(body_clusters),
        });
        describe_loops(nodes, path, orders, bodies, include_iterations)?;
        path.pop();
    }
    Ok(())
}
