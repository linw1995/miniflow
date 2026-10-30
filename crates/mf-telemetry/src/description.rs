use crate::{ContractError, Count, identity::WorkflowId, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CLUSTERS_PER_SCOPE: usize = 1024;
pub const MAX_CLUSTER_LABEL_BYTES: usize = 200;
pub const MAX_CLUSTER_ANNOTATION_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDescriptionVersion {
    #[serde(rename = "2026-09-27")]
    V2026_09_27,
    #[serde(rename = "2026-09-29")]
    V2026_09_29,
    #[serde(rename = "2026-10-02")]
    V2026_10_02,
    #[serde(rename = "2026-10-03")]
    V2026_10_03,
    #[serde(rename = "2026-10-19")]
    V2026_10_19,
    #[serde(rename = "2026-10-23")]
    V2026_10_23,
}

impl WorkflowDescriptionVersion {
    /// Default protocol for finite workflow observations.
    pub const CURRENT: Self = Self::V2026_09_29;

    pub fn supports_loops(self) -> bool {
        self != Self::V2026_09_27
    }

    pub fn supports_iteration_bodies(self) -> bool {
        matches!(self, Self::V2026_10_19 | Self::V2026_10_23)
    }

    pub fn supports_clusters(self) -> bool {
        self == Self::V2026_10_23
    }

    pub fn uses_node_success_edges(self) -> bool {
        matches!(self, Self::V2026_10_19 | Self::V2026_10_23)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Single,
    Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionDescription {
    pub mode: ExecutionMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDescription {
    pub version: WorkflowDescriptionVersion,
    pub workflow_id: WorkflowId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionDescription>,
    pub nodes: Vec<NodeDescription>,
    pub data_edges: Vec<DataEdge>,
    pub control_edges: Vec<ControlEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_edges: Vec<OrderEdge>,
    pub execution_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loop_bodies: Vec<LoopBodyDescription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clusters: Vec<ClusterDescription>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopBodyDescription {
    pub path: Vec<String>,
    pub nodes: Vec<NodeDescription>,
    pub data_edges: Vec<DataEdge>,
    pub control_edges: Vec<ControlEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_edges: Vec<OrderEdge>,
    pub execution_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clusters: Vec<ClusterDescription>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDescription {
    pub label: String,
    pub annotation: String,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescription {
    pub id: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataEdge {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclusive: bool,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderEdge {
    pub from_node: String,
    pub to_node: String,
}

impl WorkflowDescription {
    pub fn is_streaming(&self) -> bool {
        self.execution.as_ref().map_or(
            self.version == WorkflowDescriptionVersion::V2026_10_02,
            |execution| execution.mode == ExecutionMode::Stream,
        )
    }

    pub fn event_schema_version(&self) -> i64 {
        if self.is_streaming() {
            crate::STREAM_EVENT_SCHEMA_VERSION
        } else {
            crate::EVENT_SCHEMA_VERSION
        }
    }

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

    pub fn body(&self, path: &[String]) -> Option<&LoopBodyDescription> {
        self.loop_bodies.iter().find(|body| body.path == path)
    }

    pub fn validate_scope_path(
        &self,
        path: &[crate::event::ScopePathEntry],
    ) -> Result<(), ContractError> {
        require(
            path.is_empty() || self.version.supports_loops(),
            "scope paths require a description with subgraph support",
        )?;
        let mut parent = &self.nodes;
        let mut static_path = Vec::new();
        for entry in path {
            let container = parent
                .iter()
                .find(|node| node.id == entry.node_id())
                .ok_or_else(|| crate::invalid("unknown scope container"))?;
            require(
                if entry.is_call() {
                    container.kind == "workflow.call"
                } else {
                    container.kind == "workflow.loop"
                },
                "scope path kind disagrees with its container",
            )?;
            static_path.push(entry.node_id().to_owned());
            parent = &self
                .body(&static_path)
                .ok_or_else(|| crate::invalid("unknown scope body"))?
                .nodes;
        }
        Ok(())
    }

    /// Lifecycle paths contain Loop and Call scopes; Iteration items use separate detail spans.
    pub fn loop_body(&self, path: &[String]) -> Option<&LoopBodyDescription> {
        if path.len() > crate::MAX_SCOPE_DEPTH {
            return None;
        }
        let mut nodes = &self.nodes;
        let mut body = None;
        let mut has_call = false;
        for (index, id) in path.iter().enumerate() {
            let container = nodes.iter().find(|node| node.id == *id)?;
            match container.kind.as_str() {
                "workflow.call" => has_call = true,
                "workflow.loop" => {}
                _ => return None,
            }
            if !has_call && index >= crate::MAX_LOOP_DEPTH {
                return None;
            }
            let next = self.body(&path[..=index])?;
            nodes = &next.nodes;
            body = Some(next);
        }
        body
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            self.version.uses_node_success_edges()
                || (self.order_edges.is_empty()
                    && self
                        .loop_bodies
                        .iter()
                        .all(|body| body.order_edges.is_empty())),
            "node-success edges require a supported description version",
        )?;
        if matches!(
            self.version,
            WorkflowDescriptionVersion::V2026_10_03
                | WorkflowDescriptionVersion::V2026_10_19
                | WorkflowDescriptionVersion::V2026_10_23
        ) {
            require(
                self.execution.is_some(),
                "missing workflow execution description",
            )?;
        } else {
            require(
                self.execution.is_none(),
                "old descriptions cannot declare execution metadata",
            )?;
        }
        self.node_count()?;
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
        let mut inputs = BTreeMap::new();
        let mut candidates: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for edge in &self.data_edges {
            endpoint(&edge.from_node, &edge.from_output, &edge.to_node)?;
            require(!edge.to_input.is_empty(), "empty target input name")?;
            require(
                inputs
                    .insert((&edge.to_node, &edge.to_input), edge.exclusive)
                    .is_none_or(|previous| previous && edge.exclusive),
                "duplicate target input binding",
            )?;
            if edge.exclusive {
                require(
                    candidates
                        .entry((&edge.to_node, &edge.to_input))
                        .or_default()
                        .insert((&edge.from_node, &edge.from_output)),
                    "duplicate exclusive candidate",
                )?;
            }
        }
        for sources in candidates.values() {
            require(
                sources.len() >= 2,
                "exclusive input requires at least two sources",
            )?;
        }
        let mut controls = BTreeSet::new();
        for edge in &self.control_edges {
            endpoint(&edge.from_node, &edge.from_output, &edge.to_node)?;
            require(controls.insert(edge), "duplicate control edge")?;
        }
        validate_clusters(self.version, &self.clusters, &nodes)?;
        let mut orders = BTreeSet::new();
        for edge in &self.order_edges {
            require(
                nodes.contains_key(edge.from_node.as_str())
                    && nodes.contains_key(edge.to_node.as_str()),
                "unknown node-success endpoint",
            )?;
            require(
                positions[edge.from_node.as_str()] < positions[edge.to_node.as_str()],
                "node-success edge violates execution order",
            )?;
            require(orders.insert(edge), "duplicate node-success edge")?;
        }
        let mut scopes: BTreeMap<Vec<String>, (Vec<NodeDescription>, bool)> = BTreeMap::new();
        scopes.insert(Vec::new(), (self.nodes.clone(), false));
        let mut bodies: Vec<_> = self.loop_bodies.iter().collect();
        bodies.sort_by_key(|body| body.path.len());
        for body in bodies {
            require(
                !body.path.is_empty() && body.path.len() <= crate::MAX_SCOPE_DEPTH,
                "invalid Loop body path",
            )?;
            let (loop_id, parent_path) = body.path.split_last().expect("path is nonempty");
            let (parent, parent_has_call) = scopes
                .get(parent_path)
                .ok_or_else(|| crate::invalid("unknown parent Loop scope"))?;
            let container = parent
                .iter()
                .find(|node| node.id == *loop_id)
                .ok_or_else(|| crate::invalid("unknown body container"))?;
            let iteration =
                self.version.supports_iteration_bodies() && container.kind == "builtin.iteration";
            let call = container.kind == "workflow.call";
            let has_call = *parent_has_call || call;
            require(
                has_call || body.path.len() <= crate::MAX_LOOP_DEPTH,
                "invalid Loop body path",
            )?;
            require(
                container.kind == "workflow.loop" || iteration || call,
                "body path does not name a supported container",
            )?;
            require(
                call || body.nodes.iter().any(|node| {
                    if iteration {
                        node.id == "%iteration" && node.kind == "builtin.iteration_input"
                    } else {
                        matches!(node.id.as_str(), "%loop" | "$loop") && node.kind == node.id
                    }
                }),
                "body omits its synthetic source",
            )?;
            let scope = Self {
                version: if self.version.supports_iteration_bodies() {
                    self.version
                } else {
                    WorkflowDescriptionVersion::V2026_09_27
                },
                workflow_id: self.workflow_id.clone(),
                order_edges: body.order_edges.clone(),
                nodes: body.nodes.clone(),
                data_edges: body.data_edges.clone(),
                control_edges: body.control_edges.clone(),
                execution_order: body.execution_order.clone(),
                execution: self.version.supports_iteration_bodies().then_some(
                    ExecutionDescription {
                        mode: ExecutionMode::Single,
                    },
                ),
                loop_bodies: Vec::new(),
                clusters: body.clusters.clone(),
            };
            scope.validate()?;
            require(
                scopes
                    .insert(body.path.clone(), (body.nodes.clone(), has_call))
                    .is_none(),
                "duplicate Loop body path",
            )?;
        }
        Ok(())
    }
}

fn validate_clusters(
    version: WorkflowDescriptionVersion,
    clusters: &[ClusterDescription],
    nodes: &BTreeMap<&str, &NodeDescription>,
) -> Result<(), ContractError> {
    require(
        clusters.is_empty() || version.supports_clusters(),
        "node clusters require a supported description version",
    )?;
    require(
        clusters.len() <= MAX_CLUSTERS_PER_SCOPE,
        "too many node clusters",
    )?;
    let mut members = BTreeSet::new();
    for cluster in clusters {
        require(
            !cluster.label.trim().is_empty()
                && cluster.label.len() <= MAX_CLUSTER_LABEL_BYTES
                && !cluster.label.chars().any(char::is_control),
            "invalid node cluster label",
        )?;
        require(
            !cluster.annotation.trim().is_empty()
                && cluster.annotation.len() <= MAX_CLUSTER_ANNOTATION_BYTES
                && cluster
                    .annotation
                    .chars()
                    .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t')),
            "invalid node cluster annotation",
        )?;
        require(!cluster.nodes.is_empty(), "node cluster must not be empty")?;
        for node in &cluster.nodes {
            require(
                !matches!(node.as_str(), "%loop" | "$loop" | "%iteration"),
                "synthetic nodes cannot belong to a cluster",
            )?;
            require(
                nodes.contains_key(node.as_str()),
                "unknown node cluster member",
            )?;
            require(
                members.insert(node.as_str()),
                "node belongs to multiple clusters",
            )?;
        }
    }
    Ok(())
}
