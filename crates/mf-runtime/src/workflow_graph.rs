use crate::{
    ControlEdgeDefinition, DefinitionId, EdgeDefinition, FlowConnection, FlowNode, NodePorts,
    ValueSchema,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Read-only port contracts and topology. It contains neither executors nor runtime values.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowGraph {
    #[serde(default)]
    pub inputs: BTreeMap<String, GraphInput>,
    pub nodes: BTreeMap<DefinitionId, GraphNode>,
    pub connections: Vec<EdgeDefinition>,
    pub controls: Vec<ControlEdgeDefinition>,
}

impl std::fmt::Debug for WorkflowGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowGraph")
            .field("nodes", &self.nodes.len())
            .field("connections", &self.connections.len())
            .field("inputs", &self.inputs.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    pub ports: NodePorts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_outputs: Vec<crate::PortSpec>,
    pub input_schemas: BTreeMap<String, ValueSchema>,
    pub secret_inputs: BTreeSet<String>,
    pub exclusive_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph: Option<Arc<WorkflowGraph>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphInput {
    pub node: DefinitionId,
    pub port: String,
    pub value_type: crate::ValueType,
    pub required: bool,
    pub schema: Option<ValueSchema>,
    pub secret: bool,
}

impl WorkflowGraph {
    pub fn describe_inputs(schema: &crate::WorkflowInputSchema) -> BTreeMap<String, GraphInput> {
        schema
            .inputs
            .iter()
            .flat_map(|(node, ports)| {
                ports.iter().map(move |(port, input)| {
                    (
                        crate::output_id(node, port),
                        GraphInput {
                            node: node.as_str().into(),
                            port: port.clone(),
                            value_type: input.value_type.clone(),
                            required: input.required
                                && input.binding.env.is_none()
                                && input.binding.default.is_none(),
                            schema: input.schema.clone(),
                            secret: input.binding.secret,
                        },
                    )
                })
            })
            .collect()
    }

    pub fn input_origin(&self, name: &str) -> Option<&GraphInput> {
        let input = self.inputs.get(name)?;
        if let Some(graph) = self.nodes.get(&input.node)?.subgraph.as_deref()
            && graph.inputs.contains_key(&input.port)
        {
            return graph.input_origin(&input.port);
        }
        Some(input)
    }

    pub fn describe_nodes<N>(nodes: &[FlowNode<N>]) -> BTreeMap<DefinitionId, GraphNode> {
        nodes
            .iter()
            .map(|node| {
                let metadata = &node.metadata;
                (
                    node.definition_id.clone(),
                    GraphNode {
                        ports: metadata.ports.clone(),
                        stream_outputs: metadata.stream_outputs.clone(),
                        input_schemas: metadata.input_schemas.clone(),
                        secret_inputs: metadata
                            .input_bindings
                            .iter()
                            .filter(|(_, binding)| binding.secret)
                            .map(|(name, _)| name.clone())
                            .collect(),
                        exclusive_outputs: metadata.exclusive_outputs.clone(),
                        subgraph: metadata.subgraph.clone(),
                    },
                )
            })
            .collect()
    }

    pub fn from_connections<N>(
        nodes: &[FlowNode<N>],
        connections: &[FlowConnection],
        controls: Vec<ControlEdgeDefinition>,
    ) -> Self {
        Self {
            inputs: BTreeMap::new(),
            nodes: Self::describe_nodes(nodes),
            connections: connections
                .iter()
                .map(|edge| EdgeDefinition {
                    exclusive: edge.exclusive,
                    from_node: nodes[edge.from_node.index()].definition_id.clone(),
                    from_output: edge.from_output.to_string(),
                    to_node: nodes[edge.to_node.index()].definition_id.clone(),
                    to_input: edge.to_input.to_string(),
                })
                .collect(),
            controls,
        }
    }

    /// Resolves a node path through ordinary prepared subgraphs.
    pub fn locate(&self, path: &[DefinitionId]) -> Option<(&WorkflowGraph, &GraphNode)> {
        let (first, rest) = path.split_first()?;
        let node = self.nodes.get(first)?;
        if rest.is_empty() {
            Some((self, node))
        } else {
            node.subgraph.as_deref()?.locate(rest)
        }
    }
}
