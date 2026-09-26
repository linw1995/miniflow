use crate::{ContractError, Count, DESCRIPTION_SCHEMA_VERSION, identity::WorkflowId, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDescription {
    pub schema_version: i64,
    pub workflow_id: WorkflowId,
    pub nodes: Vec<NodeDescription>,
    pub data_edges: Vec<DataEdge>,
    pub control_edges: Vec<ControlEdge>,
    pub execution_order: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescription {
    pub id: String,
    pub kind: String,
    pub inputs: Vec<PortDescription>,
    pub outputs: Vec<PortDescription>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDescription {
    pub name: String,
    pub value_type: PortType,
    pub required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortType {
    Any,
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataEdge {
    pub from_node: String,
    pub from_output: String,
    pub to_node: String,
    pub to_input: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ControlEdge {
    pub from_node: String,
    pub from_output: String,
    pub to_node: String,
}

impl WorkflowDescription {
    pub fn from_json(input: &[u8]) -> Result<Self, ContractError> {
        require(
            input.len() <= MAX_DESCRIPTION_BYTES,
            "description exceeds size limit",
        )?;
        let description: Self = serde_json::from_slice(input)?;
        description.validate()?;
        Ok(description)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        require(
            bytes.len() <= MAX_DESCRIPTION_BYTES,
            "description exceeds size limit",
        )?;
        Ok(bytes)
    }

    pub fn node_count(&self) -> Result<Count, ContractError> {
        let count =
            i64::try_from(self.nodes.len()).map_err(|_| crate::invalid("too many nodes"))?;
        Count::try_from(count)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            self.schema_version == DESCRIPTION_SCHEMA_VERSION,
            format!(
                "unsupported description schema version {}",
                self.schema_version
            ),
        )?;
        crate::maximum_event_count(self.node_count()?)?;
        let mut nodes = BTreeMap::new();
        for node in &self.nodes {
            require(
                !node.id.trim().is_empty() && !node.kind.trim().is_empty(),
                "blank node ID or kind",
            )?;
            require(
                nodes.insert(node.id.as_str(), node).is_none(),
                "duplicate node ID",
            )?;
            for ports in [&node.inputs, &node.outputs] {
                let mut names = BTreeSet::new();
                for port in ports {
                    require(
                        !port.name.is_empty() && names.insert(&port.name),
                        "empty or duplicate port name",
                    )?;
                }
            }
        }
        let mut positions = BTreeMap::new();
        for (index, id) in self.execution_order.iter().enumerate() {
            require(
                nodes.contains_key(id.as_str()) && positions.insert(id.as_str(), index).is_none(),
                "execution order contains an unknown or duplicate node",
            )?;
        }
        require(
            positions.len() == nodes.len(),
            "execution order is incomplete",
        )?;
        let endpoint = |source: &str, port: &str, target: &str| -> Result<(), ContractError> {
            let from = nodes
                .get(source)
                .ok_or_else(|| crate::invalid("unknown source node"))?;
            require(nodes.contains_key(target), "unknown target node")?;
            require(
                positions[source] < positions[target],
                "edge violates execution order",
            )?;
            require(
                from.outputs.iter().any(|p| p.name == port),
                "unknown source output",
            )
        };
        let mut inputs = BTreeSet::new();
        for edge in &self.data_edges {
            endpoint(&edge.from_node, &edge.from_output, &edge.to_node)?;
            require(
                nodes[edge.to_node.as_str()]
                    .inputs
                    .iter()
                    .any(|p| p.name == edge.to_input),
                "unknown target input",
            )?;
            require(
                inputs.insert((&edge.to_node, &edge.to_input)),
                "duplicate target input binding",
            )?;
        }
        let mut controls = BTreeSet::new();
        for edge in &self.control_edges {
            endpoint(&edge.from_node, &edge.from_output, &edge.to_node)?;
            require(controls.insert(edge), "duplicate control edge")?;
        }
        Ok(())
    }
}
