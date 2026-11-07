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
    /// Synthetic input endpoint supplied by the compiled scope relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Current-frame state variables exposed as scope outputs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state_outputs: BTreeMap<String, String>,
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

    /// All statically prepared bodies owned by a node, including passed Flow bindings.
    pub fn body_paths_for_node(&self, parent: &[String], id: &str) -> Vec<&Vec<String>> {
        self.loop_bodies
            .iter()
            .filter(|body| {
                body.path.len() == parent.len() + 1
                    && body.path.starts_with(parent)
                    && body.path.last().is_some_and(|key| {
                        crate::event::ScopePathEntry::binding_owner(key)
                            .as_deref()
                            .unwrap_or(key)
                            == id
                    })
            })
            .map(|body| &body.path)
            .collect()
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
            let key = entry.node_id();
            let owner = crate::event::ScopePathEntry::binding_owner(key);
            require(
                parent
                    .iter()
                    .any(|node| node.id == owner.as_deref().unwrap_or(key)),
                "unknown scope container",
            )?;
            static_path.push(key.to_owned());
            let body = self
                .body(&static_path)
                .ok_or_else(|| crate::invalid("unknown scope body"))?;
            require(
                if entry.is_call() {
                    body.source.is_none()
                } else {
                    body.source.is_some() && !body.state_outputs.is_empty()
                },
                "scope path kind disagrees with its container",
            )?;
            parent = &body.nodes;
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
            let owner = crate::event::ScopePathEntry::binding_owner(id);
            nodes
                .iter()
                .find(|node| node.id == owner.as_deref().unwrap_or(id))?;
            let next = self.body(&path[..=index])?;
            if next.source.is_none() {
                has_call = true;
            } else if next.state_outputs.is_empty() {
                return None;
            }
            if !has_call && index >= crate::MAX_LOOP_DEPTH {
                return None;
            }
            nodes = &next.nodes;
            body = Some(next);
        }
        body
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        self.validate_with_source(None)
    }

    fn validate_with_source(&self, source: Option<&str>) -> Result<(), ContractError> {
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
        require(
            source.is_none_or(|source| nodes.contains_key(source)),
            "unknown body source endpoint",
        )?;
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
        validate_clusters(self.version, &self.clusters, &nodes, source)?;
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
            let owner = crate::event::ScopePathEntry::binding_owner(loop_id);
            require(
                parent
                    .iter()
                    .any(|node| node.id == owner.as_deref().unwrap_or(loop_id)),
                "unknown body container",
            )?;
            let has_call = *parent_has_call || body.source.is_none();
            require(
                has_call || body.path.len() <= crate::MAX_LOOP_DEPTH,
                "invalid Loop body path",
            )?;
            require(
                body.state_outputs.is_empty() || body.source.is_some(),
                "body state outputs require a source endpoint",
            )?;
            require(
                body.state_outputs.iter().all(|(variable, output)| {
                    !variable.trim().is_empty() && variable != "index" && variable == output
                }),
                "invalid body state output relation",
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
            scope.validate_with_source(body.source.as_deref())?;
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
    source: Option<&str>,
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
                source != Some(node.as_str()),
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
