use crate::{ContractError, Count, identity::WorkflowId, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDescriptionVersion {
    #[serde(rename = "2026-09-27")]
    V2026_09_27,
    #[serde(rename = "2026-09-29")]
    V2026_09_29,
    #[serde(rename = "2026-10-02")]
    V2026_10_02,
}

impl WorkflowDescriptionVersion {
    /// Default protocol for finite workflow observations.
    pub const CURRENT: Self = Self::V2026_09_29;

    pub fn is_streaming(self) -> bool {
        self == Self::V2026_10_02
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDescription {
    pub version: WorkflowDescriptionVersion,
    pub workflow_id: WorkflowId,
    pub nodes: Vec<NodeDescription>,
    pub data_edges: Vec<DataEdge>,
    pub control_edges: Vec<ControlEdge>,
    pub execution_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loop_bodies: Vec<LoopBodyDescription>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopBodyDescription {
    pub path: Vec<String>,
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

    pub fn static_node_count(&self) -> Result<Count, ContractError> {
        let count = self
            .loop_bodies
            .iter()
            .try_fold(self.nodes.len(), |count, body| {
                count
                    .checked_add(body.nodes.len())
                    .ok_or_else(|| crate::invalid("too many nodes"))
            })?;
        let count = i64::try_from(count).map_err(|_| crate::invalid("too many nodes"))?;
        Count::try_from(count)
    }

    pub fn loop_body(&self, path: &[String]) -> Option<&LoopBodyDescription> {
        self.loop_bodies.iter().find(|body| body.path == path)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if self.version.is_streaming() {
            self.node_count()?;
            require(
                self.execution_order
                    .first()
                    .is_some_and(|id| id == "%input")
                    && self
                        .nodes
                        .iter()
                        .any(|node| node.id == "%input" && node.kind == "%input"),
                "stream description omits its input source",
            )?;
        } else {
            crate::maximum_event_count(self.node_count()?)?;
        }
        if self.version == WorkflowDescriptionVersion::V2026_09_27 {
            require(
                self.loop_bodies.is_empty(),
                "old description cannot contain Loop bodies",
            )?;
        }
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
        let mut scopes: BTreeMap<Vec<String>, Vec<NodeDescription>> = BTreeMap::new();
        scopes.insert(Vec::new(), self.nodes.clone());
        let mut bodies: Vec<_> = self.loop_bodies.iter().collect();
        bodies.sort_by_key(|body| body.path.len());
        for body in bodies {
            require(
                !body.path.is_empty() && body.path.len() <= crate::MAX_LOOP_DEPTH,
                "invalid Loop body path",
            )?;
            let (loop_id, parent_path) = body.path.split_last().expect("path is nonempty");
            let parent = scopes
                .get(parent_path)
                .ok_or_else(|| crate::invalid("unknown parent Loop scope"))?;
            require(
                parent
                    .iter()
                    .any(|node| node.id == *loop_id && node.kind == "workflow.loop"),
                "Loop body path does not name a parent Loop",
            )?;
            require(
                body.nodes
                    .iter()
                    // Existing runners embed the original source name in their descriptions.
                    .any(|node| {
                        matches!(node.id.as_str(), "%loop" | "$loop") && node.kind == node.id
                    }),
                "Loop body omits its synthetic source",
            )?;
            let scope = Self {
                version: WorkflowDescriptionVersion::V2026_09_27,
                workflow_id: self.workflow_id.clone(),
                nodes: body.nodes.clone(),
                data_edges: body.data_edges.clone(),
                control_edges: body.control_edges.clone(),
                execution_order: body.execution_order.clone(),
                loop_bodies: Vec::new(),
            };
            scope.validate()?;
            require(
                scopes
                    .insert(body.path.clone(), body.nodes.clone())
                    .is_none(),
                "duplicate Loop body path",
            )?;
        }
        Ok(())
    }
}
