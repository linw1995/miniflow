use crate::{ContractError, Count, identity::WorkflowId, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDescriptionVersion {
    #[serde(rename = "2026-09-27")]
    V2026_09_27,
}

impl WorkflowDescriptionVersion {
    pub const CURRENT: Self = Self::V2026_09_27;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDescription {
    pub version: WorkflowDescriptionVersion,
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
            require(nodes.contains_key(source), "unknown source node")?;
            require(nodes.contains_key(target), "unknown target node")?;
            require(!port.is_empty(), "empty source output name")?;
            require(
                positions[source] < positions[target],
                "edge violates execution order",
            )
        };
        let mut inputs = BTreeSet::new();
        for edge in &self.data_edges {
            endpoint(&edge.from_node, &edge.from_output, &edge.to_node)?;
            require(!edge.to_input.is_empty(), "empty target input name")?;
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
