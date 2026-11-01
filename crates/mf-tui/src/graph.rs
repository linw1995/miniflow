//! Stable terminal projection of a described workflow graph.

pub mod routing;

use routing::{Crossing, EdgeKey, EdgeRoute};

use crate::{
    duration::format_duration_ns,
    state::{MAX_SESSION_NODES, NodeObservation, NodeStatus, StateSnapshot},
};
use mf_telemetry::{
    ContractError,
    description::{NodeDescription, WorkflowDescription, WorkflowDescriptionVersion},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use rust_sugiyama::{configure::Config, from_vertices_and_edges};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
    sync::Arc,
    time::Instant,
};

const NODE_WIDTH: u32 = 24;
const NODE_HEIGHT: u32 = 4;
const COLUMN_GAP: u32 = 10;
const ROW_GAP: u32 = 2;
const COMPONENT_GAP: u32 = 4;
const MARGIN: u32 = 2;
const LAYOUT_STACK_BYTES: usize = 64 * 1024 * 1024;
const CLUSTER_KIND: &str = "node cluster";
const FLOW_FRAME_NS: u64 = 100_000_000;
const TRANSFER_PULSE_NS: u64 = 600_000_000;
const NODE_TRANSITION_NS: u64 = 1_200_000_000;

#[derive(Debug, Snafu)]
pub enum GraphError {
    #[snafu(display("invalid workflow description: {source}"))]
    Description { source: ContractError },
    #[snafu(display("workflow graph exceeds the {limit}-node display limit"))]
    TooManyNodes { limit: usize },
    #[snafu(display("Loop body is absent from the workflow description"))]
    MissingLoopBody,
    #[snafu(display("could not start the graph layout worker: {source}"))]
    LayoutWorker { source: std::io::Error },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub rect: GraphRect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCluster {
    pub label: String,
    pub annotation: String,
    pub members: Vec<usize>,
    pub rect: GraphRect,
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionView {
    #[default]
    Execution,
    Dependencies,
    Ports,
}
impl ConnectionView {
    pub fn next(self) -> Self {
        match self {
            Self::Execution => Self::Dependencies,
            Self::Dependencies => Self::Ports,
            Self::Ports => Self::Execution,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Execution => "Execution",
            Self::Dependencies => "Dependencies",
            Self::Ports => "Ports",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataMapping {
    pub output: String,
    pub input: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeKind {
    Data,
    Exclusive,
    Control,
    Success,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphEdge {
    pub key: EdgeKey,
    pub from: usize,
    pub to: usize,
    pub from_output: Option<String>,
    pub members: Vec<DataMapping>,
    pub to_input: Option<String>,
    pub kind: EdgeKind,
    pub from_row: u32,
    pub to_row: u32,
}

impl GraphEdge {
    pub fn has_label(&self) -> bool {
        self.kind != EdgeKind::Data || self.members.len() > 1
    }
}

#[derive(Clone)]
struct TransferMapping {
    source: String,
    target: String,
    output: String,
    input: String,
    exclusive: bool,
}

type TransferEvents = BTreeMap<(Vec<String>, String), Vec<TransferEvent>>;

struct TransferEvent {
    age_ns: u64,
    correlation: Option<crate::traces::SpanKey>,
    snapshot: Arc<mf_runtime::NodeSnapshot>,
}

pub struct GraphLayout {
    streaming: bool,
    view: ConnectionView,
    hidden_data_count: usize,
    hidden_transitive_count: usize,
    hidden_cluster_edge_count: usize,
    nodes: Vec<GraphNode>,
    sources: Vec<Vec<usize>>,
    display_of_node: Vec<usize>,
    node_clusters: Vec<Option<usize>>,
    node_annotations: Vec<Vec<String>>,
    all_nodes: Vec<NodeDescription>,
    source_paths: Vec<Vec<String>>,
    cluster_keys: Vec<(Vec<String>, usize)>,
    expanded_nodes: BTreeSet<usize>,
    body_nodes: BTreeSet<usize>,
    clusters: Vec<GraphCluster>,
    edges: Vec<GraphEdge>,
    transfer_members: Vec<Vec<TransferMapping>>,
    routes: Vec<EdgeRoute>,
    crossings: Vec<Crossing>,
    routing_stats: routing::RoutingStats,
    width: u32,
    height: u32,
}

impl GraphLayout {
    pub fn from_loop_body(
        description: &WorkflowDescription,
        path: &[String],
    ) -> Result<Self, GraphError> {
        Self::from_body_with_view(description, path, ConnectionView::Ports)
    }

    pub fn from_body_with_view(
        description: &WorkflowDescription,
        path: &[String],
        view: ConnectionView,
    ) -> Result<Self, GraphError> {
        Self::from_body_with_view_and_collapsed(description, path, view, &BTreeSet::new())
    }

    pub fn from_body_with_view_and_collapsed(
        description: &WorkflowDescription,
        path: &[String],
        view: ConnectionView,
        collapsed: &BTreeSet<usize>,
    ) -> Result<Self, GraphError> {
        let scope = Self::body_description(description, path)?;
        Self::with_view_and_collapsed(&scope, view, collapsed)
    }

    fn body_description(
        description: &WorkflowDescription,
        path: &[String],
    ) -> Result<WorkflowDescription, GraphError> {
        let body = description.body(path).context(MissingLoopBodySnafu)?;
        let scope = WorkflowDescription {
            order_edges: body.order_edges.clone(),
            version: if description.version.supports_iteration_bodies() {
                description.version
            } else {
                WorkflowDescriptionVersion::V2026_09_27
            },
            workflow_id: description.workflow_id.clone(),
            nodes: body.nodes.clone(),
            data_edges: body.data_edges.clone(),
            control_edges: body.control_edges.clone(),
            execution_order: body.execution_order.clone(),
            execution: description.version.supports_iteration_bodies().then_some(
                mf_telemetry::description::ExecutionDescription {
                    mode: mf_telemetry::description::ExecutionMode::Single,
                },
            ),
            loop_bodies: Vec::new(),
            clusters: body.clusters.clone(),
        };
        Ok(scope)
    }

    pub fn new(description: &WorkflowDescription) -> Result<Self, GraphError> {
        Self::with_routing_limits(description, routing::RoutingLimits::default())
    }

    pub fn with_routing_limits(
        description: &WorkflowDescription,
        limits: routing::RoutingLimits,
    ) -> Result<Self, GraphError> {
        Self::with_view_and_limits(description, ConnectionView::Ports, limits)
    }

    pub fn with_view(
        description: &WorkflowDescription,
        view: ConnectionView,
    ) -> Result<Self, GraphError> {
        Self::with_view_and_collapsed(description, view, &BTreeSet::new())
    }

    pub fn with_view_and_collapsed(
        description: &WorkflowDescription,
        view: ConnectionView,
        collapsed: &BTreeSet<usize>,
    ) -> Result<Self, GraphError> {
        Self::with_view_and_limits_collapsed(
            description,
            view,
            routing::RoutingLimits::default(),
            collapsed,
        )
    }

    pub fn with_view_and_limits(
        description: &WorkflowDescription,
        view: ConnectionView,
        limits: routing::RoutingLimits,
    ) -> Result<Self, GraphError> {
        Self::with_view_and_limits_collapsed(description, view, limits, &BTreeSet::new())
    }

    pub fn with_view_and_limits_collapsed(
        description: &WorkflowDescription,
        view: ConnectionView,
        limits: routing::RoutingLimits,
        collapsed: &BTreeSet<usize>,
    ) -> Result<Self, GraphError> {
        Self::with_children(description, view, limits, collapsed, &BTreeMap::new())
    }

    fn with_children(
        description: &WorkflowDescription,
        view: ConnectionView,
        limits: routing::RoutingLimits,
        collapsed: &BTreeSet<usize>,
        children: &BTreeMap<String, Self>,
    ) -> Result<Self, GraphError> {
        description.validate().context(DescriptionSnafu)?;
        ensure!(
            description.nodes.len() <= MAX_SESSION_NODES,
            TooManyNodesSnafu {
                limit: MAX_SESSION_NODES
            }
        );

        let compact = Self::with_attachment_spacing(
            description,
            limits,
            routing::ATTACHMENT_SPACING,
            view,
            collapsed,
            children,
        )?;
        if compact.unresolved_count() == 0 {
            return Ok(compact);
        }
        let spent = compact.routing_stats;
        // A wider layout may recover crowded routes, but both attempts share the scope budget.
        let remaining = routing::RoutingLimits {
            max_geometry_items: limits
                .max_geometry_items
                .saturating_sub(spent.geometry_items),
            max_conflict_checks: limits
                .max_conflict_checks
                .saturating_sub(spent.conflict_checks),
            max_total_expansions: limits
                .max_total_expansions
                .saturating_sub(spent.search_expansions),
            max_crossings: limits.max_crossings.saturating_sub(compact.crossings.len()),
            retry_rounds: limits.retry_rounds.saturating_sub(spent.retry_rounds),
            ..limits
        };
        let estimate = description
            .nodes
            .len()
            .saturating_mul(4)
            .saturating_add(compact.edges.len().saturating_mul(10));
        if remaining.max_geometry_items < estimate || remaining.max_conflict_checks == 0 {
            return Ok(compact);
        }
        let relaxed =
            Self::with_attachment_spacing(description, remaining, 6, view, collapsed, children)?;
        let extra = relaxed.routing_stats;
        let mut selected = if relaxed.unresolved_count() < compact.unresolved_count() {
            relaxed
        } else {
            compact
        };
        selected.routing_stats = routing::RoutingStats {
            geometry_items: spent.geometry_items + extra.geometry_items,
            conflict_checks: spent.conflict_checks + extra.conflict_checks,
            search_expansions: spent.search_expansions + extra.search_expansions,
            retry_rounds: spent.retry_rounds + extra.retry_rounds,
        };
        Ok(selected)
    }

    fn with_attachment_spacing(
        description: &WorkflowDescription,
        limits: routing::RoutingLimits,
        attachment_spacing: u32,
        view: ConnectionView,
        collapsed: &BTreeSet<usize>,
        children: &BTreeMap<String, Self>,
    ) -> Result<Self, GraphError> {
        let node_index: BTreeMap<_, _> = description
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let cluster_members: Vec<Vec<usize>> = description
            .clusters
            .iter()
            .map(|cluster| {
                cluster
                    .nodes
                    .iter()
                    .map(|id| node_index[id.as_str()])
                    .collect()
            })
            .collect();
        let mut node_cluster = vec![None; description.nodes.len()];
        for (index, members) in cluster_members.iter().enumerate() {
            for &member in members {
                node_cluster[member] = Some(index);
            }
        }
        let collapsed: BTreeSet<usize> = collapsed
            .iter()
            .copied()
            .filter(|index| *index < cluster_members.len())
            .collect();
        let mut display_of_node = vec![usize::MAX; description.nodes.len()];
        let mut cluster_display = vec![None; cluster_members.len()];
        let mut nodes: Vec<GraphNode> = Vec::new();
        let mut sources: Vec<Vec<usize>> = Vec::new();
        let mut node_clusters: Vec<Option<usize>> = Vec::new();
        for id in &description.execution_order {
            let node_index = node_index[id.as_str()];
            let node = &description.nodes[node_index];
            let base_width = if node.kind == "builtin.llm" {
                40
            } else {
                NODE_WIDTH
            };
            let width = node_cluster[node_index].map_or(base_width, |cluster| {
                let label_width =
                    ratatui::text::Span::raw(&description.clusters[cluster].label).width() as u32;
                base_width.max(label_width + 4)
            });
            if let Some(cluster) = node_cluster[node_index]
                && collapsed.contains(&cluster)
            {
                let display = match cluster_display[cluster] {
                    Some(display) => display,
                    None => {
                        let display = nodes.len();
                        nodes.push(GraphNode {
                            id: description.clusters[cluster].label.clone(),
                            kind: CLUSTER_KIND.into(),
                            rect: GraphRect {
                                x: 0,
                                y: 0,
                                width,
                                height: NODE_HEIGHT,
                            },
                        });
                        sources.push(cluster_members[cluster].clone());
                        node_clusters.push(Some(cluster));
                        cluster_display[cluster] = Some(display);
                        display
                    }
                };
                display_of_node[node_index] = display;
                continue;
            }
            let display = nodes.len();
            nodes.push(GraphNode {
                id: node.id.clone(),
                kind: node.kind.clone(),
                rect: GraphRect {
                    x: 0,
                    y: 0,
                    width,
                    height: NODE_HEIGHT,
                },
            });
            sources.push(vec![node_index]);
            node_clusters.push(node_cluster[node_index]);
            display_of_node[node_index] = display;
        }
        let mut annotated = BTreeSet::new();
        let mut node_annotations: Vec<_> = node_clusters
            .iter()
            .enumerate()
            .map(|(index, cluster)| {
                cluster
                    .filter(|cluster| annotated.insert(*cluster))
                    .map_or_else(Vec::new, |cluster| {
                        wrap_annotation(
                            &description.clusters[cluster].annotation,
                            nodes[index].rect.width - 2,
                        )
                    })
            })
            .collect();
        let annotation_heights: Vec<_> = node_annotations
            .iter()
            .map(|lines| {
                if lines.is_empty() {
                    0
                } else {
                    lines.len() as u32 + 1
                }
            })
            .collect();
        let mut header_heights: Vec<_> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                if node.kind == CLUSTER_KIND {
                    0
                } else {
                    annotation_heights[index]
                }
            })
            .collect();
        let mut edges: Vec<GraphEdge> = Vec::new();
        let mut bundles: BTreeMap<(usize, usize, EdgeKind), usize> = BTreeMap::new();
        let mut topology = BTreeSet::new();
        let mut hidden_cluster_edge_count = 0;
        let execution_pairs: BTreeSet<_> = description
            .control_edges
            .iter()
            .map(|edge| {
                (
                    display_of_node[node_index[edge.from_node.as_str()]],
                    display_of_node[node_index[edge.to_node.as_str()]],
                )
            })
            .chain(description.order_edges.iter().map(|edge| {
                (
                    display_of_node[node_index[edge.from_node.as_str()]],
                    display_of_node[node_index[edge.to_node.as_str()]],
                )
            }))
            .filter(|(from, to)| from != to)
            .collect();
        let mut hidden_data_count = 0;
        for edge in &description.data_edges {
            let kind = if edge.exclusive {
                EdgeKind::Exclusive
            } else {
                EdgeKind::Data
            };
            let from = display_of_node[node_index[edge.from_node.as_str()]];
            let to = display_of_node[node_index[edge.to_node.as_str()]];
            if from == to {
                hidden_cluster_edge_count += 1;
                continue;
            }
            topology.insert((from as u32, to as u32));
            let member = DataMapping {
                output: edge.from_output.clone(),
                input: edge.to_input.clone(),
            };
            // An explicit execution connection already shows this dependency's direction.
            if view == ConnectionView::Execution && execution_pairs.contains(&(from, to)) {
                hidden_data_count += 1;
                continue;
            }
            if view != ConnectionView::Ports {
                if let Some(&index) = bundles.get(&(from, to, kind)) {
                    edges[index].members.push(member);
                    continue;
                }
                bundles.insert((from, to, kind), edges.len());
            }
            edges.push(GraphEdge {
                key: EdgeKey {
                    source: nodes[from].id.clone(),
                    destination: nodes[to].id.clone(),
                    output: Some(edge.from_output.clone()),
                    input: Some(edge.to_input.clone()),
                    kind,
                },
                from,
                to,
                from_output: Some(edge.from_output.clone()),
                to_input: Some(edge.to_input.clone()),
                kind,
                members: vec![member],
                from_row: 0,
                to_row: 0,
            });
        }
        let mut controls = BTreeSet::new();
        for edge in &description.control_edges {
            let from = display_of_node[node_index[edge.from_node.as_str()]];
            let to = display_of_node[node_index[edge.to_node.as_str()]];
            if from == to {
                hidden_cluster_edge_count += 1;
                continue;
            }
            topology.insert((from as u32, to as u32));
            if !controls.insert((from, to, edge.from_output.clone())) {
                continue;
            }
            edges.push(GraphEdge {
                key: EdgeKey {
                    source: nodes[from].id.clone(),
                    destination: nodes[to].id.clone(),
                    output: Some(edge.from_output.clone()),
                    input: None,
                    kind: EdgeKind::Control,
                },
                from,
                to,
                from_output: Some(edge.from_output.clone()),
                to_input: None,
                kind: EdgeKind::Control,
                members: Vec::new(),
                from_row: 0,
                to_row: 0,
            });
        }
        let mut orders = BTreeSet::new();
        for edge in &description.order_edges {
            let from = display_of_node[node_index[edge.from_node.as_str()]];
            let to = display_of_node[node_index[edge.to_node.as_str()]];
            if from == to {
                hidden_cluster_edge_count += 1;
                continue;
            }
            topology.insert((from as u32, to as u32));
            if !orders.insert((from, to)) {
                continue;
            }
            edges.push(GraphEdge {
                key: EdgeKey {
                    source: nodes[from].id.clone(),
                    destination: nodes[to].id.clone(),
                    output: None,
                    input: None,
                    kind: EdgeKind::Success,
                },
                from,
                to,
                from_output: None,
                to_input: None,
                kind: EdgeKind::Success,
                members: Vec::new(),
                from_row: 0,
                to_row: 0,
            });
        }
        let mut hidden_transitive_count = 0;
        if view == ConnectionView::Execution {
            let redundant = transitive_dependencies(nodes.len(), &topology);
            edges.retain(|edge| !redundant.contains(&(edge.from, edge.to)));
            hidden_transitive_count = redundant.len();
        }
        for edge in &mut edges {
            edge.members.sort();
            if let Some(first) = edge.members.first() {
                edge.from_output = Some(first.output.clone());
                edge.to_input = Some(first.input.clone());
                edge.key.output = edge.from_output.clone();
                edge.key.input = edge.to_input.clone();
            }
        }
        edges.sort_by(|left, right| left.key.cmp(&right.key));
        let mut transfer_members = vec![Vec::new(); edges.len()];
        let mut transfer_routes = BTreeMap::new();
        let mut execution_routes = BTreeMap::new();
        for (index, edge) in edges.iter().enumerate() {
            for member in &edge.members {
                transfer_routes.insert((edge.from, edge.to, edge.kind, member), index);
            }
            if matches!(edge.kind, EdgeKind::Control | EdgeKind::Success) {
                execution_routes
                    .entry((edge.from, edge.to))
                    .or_insert(index);
            }
        }
        for mapping in &description.data_edges {
            let from = display_of_node[node_index[mapping.from_node.as_str()]];
            let to = display_of_node[node_index[mapping.to_node.as_str()]];
            if from == to {
                continue;
            }
            let kind = if mapping.exclusive {
                EdgeKind::Exclusive
            } else {
                EdgeKind::Data
            };
            let member = DataMapping {
                output: mapping.from_output.clone(),
                input: mapping.to_input.clone(),
            };
            let index = transfer_routes
                .get(&(from, to, kind, &member))
                .copied()
                .or_else(|| {
                    (view == ConnectionView::Execution)
                        .then(|| execution_routes.get(&(from, to)).copied())
                        .flatten()
                });
            if let Some(index) = index {
                transfer_members[index].push(TransferMapping {
                    source: mapping.from_node.clone(),
                    target: mapping.to_node.clone(),
                    output: mapping.from_output.clone(),
                    input: mapping.to_input.clone(),
                    exclusive: mapping.exclusive,
                });
            }
        }
        let mut outgoing = vec![Vec::new(); nodes.len()];
        let mut incoming = vec![Vec::new(); nodes.len()];
        for (index, edge) in edges.iter().enumerate() {
            outgoing[edge.from].push(index);
            incoming[edge.to].push(index);
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            let outgoing_height =
                outgoing[index].len().saturating_sub(1) as u32 * attachment_spacing + 3;
            let incoming_height = incoming[index].last().map_or(0, |&edge| {
                let aligned = incoming[index].len() == 1 && outgoing[edges[edge].from].len() == 1;
                (incoming[index].len() - 1) as u32 * attachment_spacing
                    + if aligned {
                        3
                    } else {
                        attachment_spacing / 2 + 3
                    }
            });
            // Annotation rows stay inside the routing obstacle, above the first expanded member
            // or below the collapsed title, so connections cannot overwrite cluster text.
            node.rect.height = (if node.kind == "builtin.llm" {
                6
            } else {
                NODE_HEIGHT
            })
            .max(outgoing_height)
            .max(incoming_height)
                + annotation_heights[index];
            if let Some(child) = children.get(&node.id).filter(|_| node.kind != CLUSTER_KIND) {
                node.rect.width = node.rect.width.max(child.width + 2);
                node.rect.height += child.height + 1;
            }
        }

        let vertices: Vec<_> = (0..nodes.len())
            .map(|index| {
                (
                    index as u32,
                    (
                        nodes[index].rect.height as f64,
                        nodes[index].rect.width as f64,
                    ),
                )
            })
            .collect();
        let topology: Vec<_> = topology.into_iter().collect();
        let config = Config {
            vertex_spacing: 2.0,
            transpose: false,
            dummy_vertices: false,
            ..Config::default()
        };
        let compute = || from_vertices_and_edges(&vertices, &topology, &config);
        // Sugiyama recurses through the DAG; large inputs need a caller-independent stack budget.
        let mut components = if nodes.len() >= 512 {
            std::thread::scope(|scope| {
                let worker = std::thread::Builder::new()
                    .name("graph-layout".into())
                    .stack_size(LAYOUT_STACK_BYTES)
                    .spawn_scoped(scope, compute)
                    .context(LayoutWorkerSnafu)?;
                Ok::<_, GraphError>(
                    worker
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                )
            })?
        } else {
            compute()
        };
        components.sort_by_key(|(vertices, _, _)| {
            vertices
                .iter()
                .map(|(id, _)| *id)
                .min()
                .unwrap_or(usize::MAX)
        });

        let mut placed = vec![false; nodes.len()];
        let mut node_components = vec![0; nodes.len()];
        let mut component_bottoms = Vec::new();
        let mut component_top = MARGIN;
        let column_width = nodes
            .iter()
            .map(|node| node.rect.width)
            .max()
            .unwrap_or(NODE_WIDTH);
        for (vertices, _, _) in components {
            let mut vertices = vertices;
            vertices.sort_by(|left, right| {
                let (left_id, (_, left_rank)) = left;
                let (right_id, (_, right_rank)) = right;
                left_rank
                    .total_cmp(right_rank)
                    .then_with(|| left_id.cmp(right_id))
            });
            let mut layers: Vec<Vec<(usize, f64)>> = Vec::new();
            let mut previous_rank: Option<f64> = None;
            for (id, (sibling, rank)) in vertices {
                if previous_rank.is_none_or(|previous| (previous - rank).abs() > 1e-6) {
                    layers.push(Vec::new());
                    previous_rank = Some(rank);
                }
                layers
                    .last_mut()
                    .expect("layer was added")
                    .push((id, sibling));
            }
            let component_height = layers
                .iter()
                .map(|layer| {
                    layer
                        .iter()
                        .map(|(id, _)| row_stride(&nodes[*id], attachment_spacing))
                        .sum::<u32>()
                })
                .max()
                .unwrap_or(0);
            for (column, layer) in layers.iter_mut().enumerate() {
                layer.sort_by(|left, right| {
                    left.1
                        .total_cmp(&right.1)
                        .then_with(|| left.0.cmp(&right.0))
                });
                let layer_height = layer
                    .iter()
                    .map(|(id, _)| row_stride(&nodes[*id], attachment_spacing))
                    .sum::<u32>();
                let centering = (component_height - layer_height) / (2 * attachment_spacing)
                    * attachment_spacing;
                let mut row_y = component_top + centering;
                for (id, _) in layer.iter() {
                    if let Some(node) = nodes.get_mut(*id) {
                        node.rect.x = MARGIN + column as u32 * (column_width + COLUMN_GAP);
                        node.rect.y = row_y;
                        row_y += row_stride(node, attachment_spacing);
                        node_components[*id] = component_bottoms.len();
                        placed[*id] = true;
                    }
                }
            }
            component_bottoms.push(component_top + component_height);
            component_top += component_height + COMPONENT_GAP;
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            if !placed[index] {
                node.rect.x = MARGIN;
                node.rect.y = component_top;
                node_components[index] = component_bottoms.len();
                component_bottoms.push(component_top + node.rect.height + ROW_GAP);
                component_top += node.rect.height + ROW_GAP;
            }
        }
        if description
            .clusters
            .iter()
            .enumerate()
            .any(|(index, _)| !collapsed.contains(&index))
        {
            let mut anchors = BTreeMap::new();
            for (index, cluster) in node_clusters.iter().enumerate() {
                if let Some(cluster) = cluster {
                    let anchor = anchors.entry(*cluster).or_insert(index);
                    if (nodes[index].rect.x, index) < (nodes[*anchor].rect.x, *anchor) {
                        *anchor = index;
                    }
                }
            }
            for index in 0..nodes.len() {
                if header_heights[index] == 0 {
                    continue;
                }
                let anchor = anchors[&node_clusters[index].expect("annotated cluster member")];
                if anchor != index {
                    let height = std::mem::take(&mut header_heights[index]);
                    header_heights[anchor] = height;
                    node_annotations[anchor] = std::mem::take(&mut node_annotations[index]);
                    nodes[index].rect.height -= height;
                    nodes[anchor].rect.height += height;
                }
            }
            let bottom =
                arrange_cluster_bands(&mut nodes, &node_clusters, &topology, attachment_spacing);
            node_components.fill(0);
            component_bottoms = vec![bottom];
        }
        for attachments in &mut outgoing {
            attachments.sort_by_key(|&edge| (nodes[edges[edge].to].rect.y, &edges[edge].key));
            for (row, &edge) in attachments.iter().enumerate() {
                edges[edge].from_row =
                    header_heights[edges[edge].from] + 1 + row as u32 * attachment_spacing;
            }
        }
        for attachments in &mut incoming {
            attachments.sort_by_key(|&edge| (nodes[edges[edge].from].rect.y, &edges[edge].key));
            for (row, &edge) in attachments.iter().enumerate() {
                let aligned = attachments.len() == 1 && outgoing[edges[edge].from].len() == 1;
                // Stagger fan/join arrivals so channel crossings stay clear of their turns.
                edges[edge].to_row = header_heights[edges[edge].to]
                    + if aligned {
                        1
                    } else {
                        attachment_spacing / 2 + 1
                    }
                    + row as u32 * attachment_spacing;
            }
        }
        let estimate = nodes
            .len()
            .saturating_mul(4)
            .saturating_add(edges.len().saturating_mul(10));
        let mut routes = if estimate <= limits.max_geometry_items {
            routing::channel_routes(&mut nodes, &edges, &node_components, &mut component_bottoms)
        } else {
            vec![EdgeRoute::default(); edges.len()]
        };
        let (crossings, routing_stats) =
            routing::route_all(&nodes, &edges, &mut routes, &node_components, limits);
        let clusters = description
            .clusters
            .iter()
            .enumerate()
            .map(|(index, cluster)| {
                let members = &cluster_members[index];
                let collapsed = collapsed.contains(&index);
                let rect = if collapsed {
                    let display = cluster_display[index].expect("collapsed cluster has a display");
                    nodes[display].rect
                } else {
                    let display: Vec<_> = members
                        .iter()
                        .map(|member| display_of_node[*member])
                        .collect();
                    let left = display
                        .iter()
                        .map(|index| nodes[*index].rect.x)
                        .min()
                        .unwrap_or(MARGIN);
                    let top = display
                        .iter()
                        .map(|index| nodes[*index].rect.y)
                        .min()
                        .unwrap_or(MARGIN);
                    let right = display
                        .iter()
                        .map(|index| nodes[*index].rect.x + nodes[*index].rect.width)
                        .max()
                        .unwrap_or(left);
                    let bottom = display
                        .iter()
                        .map(|index| nodes[*index].rect.y + nodes[*index].rect.height)
                        .max()
                        .unwrap_or(top);
                    GraphRect {
                        x: left.saturating_sub(1),
                        y: top.saturating_sub(1),
                        width: right.saturating_sub(left.saturating_sub(1)) + 1,
                        height: bottom.saturating_sub(top.saturating_sub(1)) + 1,
                    }
                };
                GraphCluster {
                    label: cluster.label.clone(),
                    annotation: cluster.annotation.clone(),
                    members: members.clone(),
                    rect,
                    collapsed,
                }
            })
            .collect::<Vec<_>>();
        let right = nodes
            .iter()
            .map(|node| node.rect.x + node.rect.width)
            .chain(
                routes
                    .iter()
                    .flat_map(|route| route.points.iter().map(|point| point.x)),
            )
            .chain(
                clusters
                    .iter()
                    .map(|cluster| cluster.rect.x + cluster.rect.width),
            )
            .max()
            .unwrap_or(MARGIN);
        let height = nodes
            .iter()
            .map(|node| node.rect.y + node.rect.height + MARGIN)
            .chain(
                routes
                    .iter()
                    .flat_map(|route| route.points.iter().map(|point| point.y + MARGIN)),
            )
            .chain(
                clusters
                    .iter()
                    .map(|cluster| cluster.rect.y + cluster.rect.height + MARGIN),
            )
            .max()
            .unwrap_or(MARGIN * 2);
        let body_nodes = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                description
                    .body(std::slice::from_ref(&node.id))
                    .is_some()
                    .then_some(index)
            })
            .collect();
        Ok(Self {
            streaming: description.is_streaming(),
            view,
            hidden_data_count,
            hidden_transitive_count,
            hidden_cluster_edge_count,
            routes,
            crossings,
            routing_stats,
            nodes,
            sources,
            display_of_node,
            node_clusters,
            node_annotations,
            all_nodes: description.nodes.clone(),
            source_paths: vec![Vec::new(); description.nodes.len()],
            cluster_keys: (0..description.clusters.len())
                .map(|index| (Vec::new(), index))
                .collect(),
            expanded_nodes: BTreeSet::new(),
            body_nodes,
            clusters,
            edges,
            transfer_members,
            width: right + MARGIN,
            height,
        })
    }

    pub fn with_expanded_bodies(
        description: &WorkflowDescription,
        view: ConnectionView,
        expanded: &BTreeSet<Vec<String>>,
        collapsed: &BTreeMap<Vec<String>, BTreeSet<usize>>,
    ) -> Result<Self, GraphError> {
        description.validate().context(DescriptionSnafu)?;
        let mut remaining = MAX_SESSION_NODES;
        Self::inline_scope(description, &[], view, expanded, collapsed, &mut remaining)
    }

    fn inline_scope(
        description: &WorkflowDescription,
        path: &[String],
        view: ConnectionView,
        expanded: &BTreeSet<Vec<String>>,
        collapsed: &BTreeMap<Vec<String>, BTreeSet<usize>>,
        remaining: &mut usize,
    ) -> Result<Self, GraphError> {
        let body;
        let scope = if path.is_empty() {
            description
        } else {
            body = Self::body_description(description, path)?;
            &body
        };
        ensure!(
            scope.nodes.len() <= *remaining,
            TooManyNodesSnafu {
                limit: MAX_SESSION_NODES
            }
        );
        *remaining -= scope.nodes.len();
        let folded = collapsed.get(path).cloned().unwrap_or_default();
        let hidden: BTreeSet<_> = scope
            .clusters
            .iter()
            .enumerate()
            .filter(|(index, _)| folded.contains(index))
            .flat_map(|(_, cluster)| cluster.nodes.iter().map(String::as_str))
            .collect();
        let mut children = BTreeMap::new();
        for node in &scope.nodes {
            let mut child_path = path.to_vec();
            child_path.push(node.id.clone());
            if !hidden.contains(node.id.as_str())
                && expanded.contains(&child_path)
                && description.body(&child_path).is_some()
            {
                children.insert(
                    node.id.clone(),
                    Self::inline_scope(
                        description,
                        &child_path,
                        view,
                        expanded,
                        collapsed,
                        remaining,
                    )?,
                );
            }
        }
        let mut layout = Self::with_children(
            scope,
            view,
            routing::RoutingLimits::default(),
            &folded,
            &children,
        )?;
        layout.source_paths.fill(path.to_vec());
        layout.body_nodes = layout
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                let mut child = path.to_vec();
                child.push(node.id.clone());
                (node.kind != CLUSTER_KIND && description.body(&child).is_some()).then_some(index)
            })
            .collect();
        for (scope, _) in &mut layout.cluster_keys {
            *scope = path.to_vec();
        }
        for (id, child) in children {
            let owner = layout
                .display_index_for_scoped_node(path, &id)
                .expect("expanded body owner is visible");
            layout.append_body(owner, child);
        }
        Ok(layout)
    }

    fn append_body(&mut self, owner: usize, mut child: Self) {
        // Route scopes independently so parent connections treat the entire body as an obstacle.
        let parent = self.nodes[owner].rect;
        let x = parent.x + 1;
        let y = parent.y + parent.height - child.height - 1;
        let nodes = self.nodes.len();
        let sources = self.all_nodes.len();
        let clusters = self.clusters.len();
        let edges = self.edges.len();
        let translate = |rect: &mut GraphRect| {
            rect.x += x;
            rect.y += y;
        };
        for node in &mut child.nodes {
            translate(&mut node.rect);
        }
        for cluster in &mut child.clusters {
            translate(&mut cluster.rect);
            for source in &mut cluster.members {
                *source += sources;
            }
        }
        for members in &mut child.sources {
            for source in members {
                *source += sources;
            }
        }
        for display in &mut child.display_of_node {
            *display += nodes;
        }
        for cluster in child.node_clusters.iter_mut().flatten() {
            *cluster += clusters;
        }
        for edge in &mut child.edges {
            edge.from += nodes;
            edge.to += nodes;
        }
        for route in &mut child.routes {
            for point in &mut route.points {
                point.x += x;
                point.y += y;
            }
            if let Some(label) = &mut route.label {
                translate(&mut label.rect);
            }
        }
        for crossing in &mut child.crossings {
            crossing.point.x += x;
            crossing.point.y += y;
            crossing.horizontal += edges;
            crossing.vertical += edges;
            crossing.upper += edges;
        }
        self.body_nodes
            .extend(child.body_nodes.into_iter().map(|index| index + nodes));
        self.expanded_nodes.insert(owner);
        self.expanded_nodes
            .extend(child.expanded_nodes.into_iter().map(|index| index + nodes));
        self.nodes.extend(child.nodes);
        self.sources.extend(child.sources);
        self.display_of_node.extend(child.display_of_node);
        self.node_clusters.extend(child.node_clusters);
        self.node_annotations.extend(child.node_annotations);
        self.all_nodes.extend(child.all_nodes);
        self.source_paths.extend(child.source_paths);
        self.cluster_keys.extend(child.cluster_keys);
        self.clusters.extend(child.clusters);
        self.edges.extend(child.edges);
        self.transfer_members.extend(child.transfer_members);
        self.routes.extend(child.routes);
        self.crossings.extend(child.crossings);
        self.hidden_data_count += child.hidden_data_count;
        self.hidden_transitive_count += child.hidden_transitive_count;
        self.hidden_cluster_edge_count += child.hidden_cluster_edge_count;
        self.routing_stats.geometry_items += child.routing_stats.geometry_items;
        self.routing_stats.conflict_checks += child.routing_stats.conflict_checks;
        self.routing_stats.search_expansions += child.routing_stats.search_expansions;
        self.routing_stats.retry_rounds += child.routing_stats.retry_rounds;
    }

    pub fn source_path(&self, source: usize) -> &[String] {
        self.source_paths.get(source).map_or(&[], Vec::as_slice)
    }

    pub fn node_path(&self, display: usize) -> &[String] {
        self.sources(display)
            .first()
            .map_or(&[], |source| self.source_path(*source))
    }

    pub fn cluster_key(&self, cluster: usize) -> &(Vec<String>, usize) {
        &self.cluster_keys[cluster]
    }

    pub fn is_expanded_body(&self, display: usize) -> bool {
        self.expanded_nodes.contains(&display)
    }

    pub fn display_index_for_scoped_node(&self, path: &[String], id: &str) -> Option<usize> {
        let source = self
            .all_nodes
            .iter()
            .enumerate()
            .position(|(source, node)| node.id == id && self.source_path(source) == path)?;
        self.display_index_for_source(source)
    }

    pub fn connection_view(&self) -> ConnectionView {
        self.view
    }
    pub fn hidden_data_count(&self) -> usize {
        self.hidden_data_count
    }
    pub fn hidden_transitive_count(&self) -> usize {
        self.hidden_transitive_count
    }

    pub fn hidden_cluster_edge_count(&self) -> usize {
        self.hidden_cluster_edge_count
    }

    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    pub fn clusters(&self) -> &[GraphCluster] {
        &self.clusters
    }

    pub fn all_nodes(&self) -> &[NodeDescription] {
        &self.all_nodes
    }

    pub fn cluster_of(&self, display: usize) -> Option<usize> {
        self.node_clusters.get(display).copied().flatten()
    }

    pub fn is_collapsed_cluster(&self, display: usize) -> bool {
        self.nodes
            .get(display)
            .is_some_and(|node| node.kind == CLUSTER_KIND)
    }

    pub fn sources(&self, display: usize) -> &[usize] {
        self.sources.get(display).map_or(&[], Vec::as_slice)
    }

    pub fn display_index_for_node_id(&self, id: &str) -> Option<usize> {
        let node = self.all_nodes.iter().position(|node| node.id == id)?;
        self.display_of_node
            .get(node)
            .copied()
            .filter(|index| *index != usize::MAX)
    }

    pub fn display_index_for_source(&self, source: usize) -> Option<usize> {
        self.display_of_node
            .get(source)
            .copied()
            .filter(|index| *index != usize::MAX)
    }

    pub fn display_index_for_cluster(&self, cluster: usize) -> Option<usize> {
        if !self.clusters.get(cluster)?.collapsed {
            return None;
        }
        self.node_clusters
            .iter()
            .enumerate()
            .find_map(|(index, owner)| {
                (*owner == Some(cluster) && self.is_collapsed_cluster(index)).then_some(index)
            })
    }

    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    pub fn routes(&self) -> &[EdgeRoute] {
        &self.routes
    }

    pub fn crossings(&self) -> &[Crossing] {
        &self.crossings
    }

    pub fn routing_stats(&self) -> routing::RoutingStats {
        self.routing_stats
    }

    pub fn unresolved_count(&self) -> usize {
        self.routes
            .iter()
            .filter(|route| route.unresolved.is_some())
            .count()
    }

    pub fn routing_details(&self, node: usize) -> String {
        let mut entries: Vec<_> = self
            .edges
            .iter()
            .zip(&self.routes)
            .filter(|(edge, route)| {
                (edge.from == node || edge.to == node) && route.unresolved.is_some()
            })
            .collect();
        entries.sort_by_key(|(edge, _)| &edge.key);
        if entries.is_empty() {
            return String::new();
        }
        let mut text = String::from("Unresolved connections:\n");
        for (edge, route) in entries {
            if edge.members.len() > 1 {
                text.push_str(&format!(
                    "{:?} bundle ({} mappings):\n",
                    edge.kind,
                    edge.members.len()
                ));
                for member in &edge.members {
                    text.push_str(&format!(
                        "{}.{} -> {}.{}\n",
                        display_line(&edge.key.source),
                        display_line(&member.output),
                        display_line(&edge.key.destination),
                        display_line(&member.input)
                    ));
                }
                text.push_str(&format!(
                    "{}\n",
                    route.unresolved.expect("unresolved reason exists")
                ));
                continue;
            }
            let source = edge.from_output.as_deref().map_or_else(
                || format!("success of {}", display_line(&edge.key.source)),
                |port| format!("{}.{}", display_line(&edge.key.source), display_line(port)),
            );
            let target = edge.to_input.as_ref().map_or_else(
                || display_line(&edge.key.destination),
                |input| {
                    format!(
                        "{}.{}",
                        display_line(&edge.key.destination),
                        display_line(input)
                    )
                },
            );
            text.push_str(&format!(
                "{:?}: {source} -> {target}\n{}\n",
                edge.kind,
                route.unresolved.expect("unresolved reason exists")
            ));
        }
        text
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

fn transitive_dependencies(
    node_count: usize,
    topology: &BTreeSet<(u32, u32)>,
) -> BTreeSet<(usize, usize)> {
    let mut outgoing = vec![Vec::new(); node_count];
    for &(from, to) in topology {
        outgoing[from as usize].push(to as usize);
    }
    // A folded group can introduce a display cycle even though execution remains acyclic.
    let Some(order) = topological_order(&outgoing) else {
        return BTreeSet::new();
    };
    let mut ranks = vec![0; node_count];
    for (rank, &node) in order.iter().enumerate() {
        ranks[node] = rank;
    }
    for successors in &mut outgoing {
        successors.sort_by_key(|&node| ranks[node]);
    }
    let words = node_count.div_ceil(64);
    let mut reachable = vec![vec![0_u64; words]; node_count];
    let mut redundant = BTreeSet::new();
    // Aggregate display indices follow their first member, not the projected dependency order.
    for &from in order.iter().rev() {
        for &to in &outgoing[from] {
            let mask = 1_u64 << (to % 64);
            if reachable[from][to / 64] & mask != 0 {
                redundant.insert((from, to));
                continue;
            }
            let (current, inherited) = if from < to {
                let (before, after) = reachable.split_at_mut(to);
                (&mut before[from], &after[0])
            } else {
                let (before, after) = reachable.split_at_mut(from);
                (&mut after[0], &before[to])
            };
            current[to / 64] |= mask;
            for (word, inherited) in current.iter_mut().zip(inherited) {
                *word |= inherited;
            }
        }
    }
    redundant
}

fn topological_order(outgoing: &[Vec<usize>]) -> Option<Vec<usize>> {
    let node_count = outgoing.len();
    let mut incoming = vec![0; node_count];
    for &next in outgoing.iter().flatten() {
        incoming[next] += 1;
    }
    let mut ready: BTreeSet<_> = (0..node_count)
        .filter(|&node| incoming[node] == 0)
        .collect();
    let mut order = Vec::with_capacity(node_count);
    while let Some(node) = ready.pop_first() {
        order.push(node);
        for &next in &outgoing[node] {
            incoming[next] -= 1;
            if incoming[next] == 0 {
                ready.insert(next);
            }
        }
    }
    (order.len() == node_count).then_some(order)
}

fn arrange_cluster_bands(
    nodes: &mut [GraphNode],
    clusters: &[Option<usize>],
    topology: &[(u32, u32)],
    attachment_spacing: u32,
) -> u32 {
    let mut groups = BTreeMap::<Option<usize>, Vec<usize>>::new();
    for (index, cluster) in clusters.iter().enumerate() {
        groups.entry(*cluster).or_default().push(index);
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by_key(|members| members[0]);
    let mut group_of = vec![0; nodes.len()];
    for (group, members) in groups.iter().enumerate() {
        for &node in members {
            group_of[node] = group;
        }
    }
    let mut outgoing = vec![Vec::new(); groups.len()];
    for &(from, to) in topology {
        let (from, to) = (group_of[from as usize], group_of[to as usize]);
        if from != to {
            outgoing[from].push(to);
        }
    }
    let order = topological_order(&outgoing).unwrap_or_else(|| (0..groups.len()).collect());
    let mut top = MARGIN;
    let mut bottom = MARGIN;
    for group in order {
        let mut columns = BTreeMap::<u32, Vec<usize>>::new();
        for &index in &groups[group] {
            columns.entry(nodes[index].rect.x).or_default().push(index);
        }
        for column in columns.values_mut() {
            column.sort_by_key(|&index| (nodes[index].rect.y, index));
            let mut y = top;
            for &index in column.iter() {
                nodes[index].rect.y = y;
                y += row_stride(&nodes[index], attachment_spacing);
            }
            bottom = bottom.max(y);
        }
        top = bottom + COMPONENT_GAP;
    }
    bottom
}

fn row_stride(node: &GraphNode, attachment_spacing: u32) -> u32 {
    // Keep sibling port rows on the same grid so staggered arrivals avoid channel turns.
    (node.rect.height + ROW_GAP).div_ceil(attachment_spacing) * attachment_spacing
}

pub struct GraphView<'a> {
    layout: &'a GraphLayout,
    nodes: Option<&'a [NodeObservation]>,
    selected: Option<usize>,
    offset: (u32, u32),
    elapsed_ns: u64,
    observed_at_unix_nano: Option<u64>,
    data: Option<&'a crate::snapshots::HistorySnapshot>,
    minimap: bool,
    reader: Option<&'a crate::receiver::ObservationReader>,
}

impl<'a> GraphView<'a> {
    pub fn new(layout: &'a GraphLayout) -> Self {
        Self {
            layout,
            nodes: None,
            selected: None,
            offset: (0, 0),
            elapsed_ns: 0,
            observed_at_unix_nano: None,
            data: None,
            minimap: false,
            reader: None,
        }
    }

    pub fn reader(mut self, reader: &'a crate::receiver::ObservationReader) -> Self {
        self.reader = Some(reader);
        self
    }

    pub fn snapshot(mut self, snapshot: &'a StateSnapshot) -> Self {
        self.nodes = Some(&snapshot.nodes);
        self
    }

    pub fn nodes(mut self, nodes: &'a [NodeObservation]) -> Self {
        self.nodes = Some(nodes);
        self
    }

    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn offset(mut self, x: u32, y: u32) -> Self {
        self.offset = (x, y);
        self
    }

    pub fn elapsed_ns(mut self, elapsed_ns: u64) -> Self {
        self.elapsed_ns = elapsed_ns;
        self
    }

    pub fn observed_at_unix_nano(mut self, timestamp: u64) -> Self {
        self.observed_at_unix_nano = Some(timestamp);
        self
    }

    pub fn data_snapshot(mut self, data: &'a crate::snapshots::HistorySnapshot) -> Self {
        self.data = Some(data);
        self
    }

    pub fn minimap(mut self, visible: bool) -> Self {
        self.minimap = visible;
        self
    }

    fn put(&self, area: Rect, buffer: &mut Buffer, x: u32, y: u32, symbol: &str, style: Style) {
        let Some(local_x) = x.checked_sub(self.offset.0) else {
            return;
        };
        let Some(local_y) = y.checked_sub(self.offset.1) else {
            return;
        };
        if local_x >= u32::from(area.width) || local_y >= u32::from(area.height) {
            return;
        }
        let position = Position::new(area.x + local_x as u16, area.y + local_y as u16);
        if let Some(cell) = buffer.cell_mut(position) {
            cell.reset();
            cell.set_symbol(symbol).set_style(style);
        }
    }

    fn draw_edge(&self, area: Rect, buffer: &mut Buffer, index: usize) {
        let edge = &self.layout.edges[index];
        let route = &self.layout.routes[index];
        if route.unresolved.is_some() {
            return;
        }
        let style = self.edge_style(edge);
        let symbols = routing::line_symbols(edge.kind);
        for segment in route.segments() {
            let low = segment.low();
            let high = segment.high();
            match segment.axis() {
                routing::Axis::Horizontal => {
                    self.horizontal(area, buffer, low.x..=high.x, low.y, symbols[0], style)
                }
                routing::Axis::Vertical => {
                    self.vertical(area, buffer, low.x, low.y..=high.y, symbols[1], style)
                }
            }
        }
        for points in route.points.windows(3) {
            let point = points[1];
            let left = points[0].x < point.x || points[2].x < point.x;
            let right = points[0].x > point.x || points[2].x > point.x;
            let up = points[0].y < point.y || points[2].y < point.y;
            let down = points[0].y > point.y || points[2].y > point.y;
            let symbol = match (left, right, up, down) {
                (false, true, false, true) => symbols[2],
                (true, false, false, true) => symbols[3],
                (false, true, true, false) => symbols[4],
                (true, false, true, false) => symbols[5],
                _ => continue,
            };
            self.put(area, buffer, point.x, point.y, symbol, style);
        }
        if let Some(to) = route.points.last() {
            self.put(area, buffer, to.x, to.y, "▶", style);
        }
        if let Some(label) = &route.label {
            self.text(area, buffer, label.rect, &label.text, style);
        }
    }

    fn draw_crossing(&self, area: Rect, buffer: &mut Buffer, crossing: &Crossing) {
        let rect = crossing.footprint();
        if rect.x >= self.offset.0.saturating_add(u32::from(area.width))
            || rect.x + rect.width <= self.offset.0
            || rect.y >= self.offset.1.saturating_add(u32::from(area.height))
            || rect.y + rect.height <= self.offset.1
        {
            return;
        }
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                self.put(area, buffer, x, y, " ", Style::default());
            }
        }
        let horizontal = &self.layout.edges[crossing.horizontal];
        let vertical = &self.layout.edges[crossing.vertical];
        let upper_horizontal = crossing.upper == crossing.horizontal;
        for cell in routing::crossing_cells(horizontal.kind, vertical.kind, upper_horizontal) {
            let edge = if cell.upper == upper_horizontal {
                horizontal
            } else {
                vertical
            };
            self.put(
                area,
                buffer,
                crossing
                    .point
                    .x
                    .checked_add_signed(cell.dx)
                    .expect("reserved marker bounds"),
                crossing
                    .point
                    .y
                    .checked_add_signed(cell.dy)
                    .expect("reserved marker bounds"),
                cell.symbol,
                self.edge_style(edge),
            );
        }
    }

    fn draw_ports(&self, area: Rect, buffer: &mut Buffer, index: usize) {
        if self.layout.routes[index].unresolved.is_some() {
            return;
        }
        let edge = &self.layout.edges[index];
        let source = self.layout.nodes[edge.from].rect;
        let target = self.layout.nodes[edge.to].rect;
        let style = self.edge_style(edge);
        self.put(
            area,
            buffer,
            source.x + source.width - 1,
            source.y + edge.from_row,
            "├",
            style,
        );
        self.put(area, buffer, target.x, target.y + edge.to_row, "┤", style);
    }

    fn text(&self, area: Rect, buffer: &mut Buffer, rect: GraphRect, text: &str, style: Style) {
        if rect.y < self.offset.1
            || rect.y >= self.offset.1.saturating_add(u32::from(area.height))
            || rect.x >= self.offset.0.saturating_add(u32::from(area.width))
            || rect.x.saturating_add(rect.width) <= self.offset.0
        {
            return;
        }
        let width = rect.width.min(u32::from(u16::MAX)) as u16;
        let mut line = Buffer::empty(Rect::new(0, 0, width, 1));
        line.set_stringn(0, 0, text, width as usize, style);
        for x in 0..u32::from(width) {
            let Some(local_x) = (rect.x + x).checked_sub(self.offset.0) else {
                continue;
            };
            if local_x >= u32::from(area.width) {
                continue;
            }
            if let Some(cell) = buffer.cell_mut(Position::new(
                area.x + local_x as u16,
                area.y + (rect.y - self.offset.1) as u16,
            )) {
                let source = line
                    .cell(Position::new(x as u16, 0))
                    .expect("text cell exists");
                *cell = source.clone();
                if ratatui::text::Span::raw(source.symbol()).width() as u32
                    > u32::from(area.width) - local_x
                {
                    cell.set_symbol(" ");
                }
            }
        }
    }

    fn horizontal(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        span: RangeInclusive<u32>,
        y: u32,
        symbol: &str,
        style: Style,
    ) {
        if y < self.offset.1 || y >= self.offset.1.saturating_add(u32::from(area.height)) {
            return;
        }
        let visible_first = (*span.start()).max(self.offset.0);
        let visible_last = (*span.end()).min(
            self.offset
                .0
                .saturating_add(u32::from(area.width))
                .saturating_sub(1),
        );
        for x in visible_first..=visible_last {
            self.put(area, buffer, x, y, symbol, style);
        }
    }

    fn vertical(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        x: u32,
        span: RangeInclusive<u32>,
        symbol: &str,
        style: Style,
    ) {
        if x < self.offset.0 || x >= self.offset.0.saturating_add(u32::from(area.width)) {
            return;
        }
        let visible_first = (*span.start()).min(*span.end()).max(self.offset.1);
        let visible_last = (*span.start()).max(*span.end()).min(
            self.offset
                .1
                .saturating_add(u32::from(area.height))
                .saturating_sub(1),
        );
        for y in visible_first..=visible_last {
            self.put(area, buffer, x, y, symbol, style);
        }
    }

    fn observation(&self, source: usize) -> Option<&'a NodeObservation> {
        let expected = self.layout.all_nodes().get(source)?;
        self.nodes
            .and_then(|nodes| nodes.get(source))
            .filter(|item| item.id == expected.id)
    }

    fn display_status(&self, display: usize) -> NodeStatus {
        let sources = self.layout.sources(display);
        sources
            .iter()
            .filter_map(|source| self.observation(*source))
            .map(|observation| observation.status)
            .fold(None, |current, status| {
                Some(current.map_or(status, |current| merge_status(current, status)))
            })
            .unwrap_or(NodeStatus::Pending)
    }

    fn display_uncertain(&self, display: usize) -> bool {
        self.layout.sources(display).iter().any(|source| {
            self.observation(*source)
                .is_some_and(|observation| observation.possibly_missing_events)
        })
    }

    fn edge_style(&self, edge: &GraphEdge) -> Style {
        let default = match edge.kind {
            EdgeKind::Data => Color::Cyan,
            EdgeKind::Exclusive => Color::Magenta,
            EdgeKind::Control => Color::Blue,
            EdgeKind::Success => Color::Yellow,
        };
        let collapsed = self.layout.is_collapsed_cluster(edge.from);
        let source = (!collapsed)
            .then(|| self.layout.sources(edge.from).first().copied())
            .flatten()
            .and_then(|source| self.observation(source));
        let status = source.map_or_else(
            || {
                if collapsed {
                    self.display_status(edge.from)
                } else {
                    NodeStatus::Pending
                }
            },
            |source| source.status,
        );
        if source.is_some_and(|source| source.possibly_missing_events)
            || self.display_uncertain(edge.from)
            || matches!(status, NodeStatus::Unknown | NodeStatus::Interrupted)
        {
            return Style::default().fg(Color::Magenta);
        }
        if matches!(status, NodeStatus::Skipped) {
            return Style::default().fg(Color::DarkGray);
        }
        if edge.kind == EdgeKind::Success {
            return match status {
                NodeStatus::Succeeded => Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                NodeStatus::Failed => Style::default().fg(Color::Red),
                _ => Style::default().fg(default),
            };
        }
        let Some(source) = source else {
            return Style::default().fg(default);
        };
        let port_state = |port: &str| {
            if source.produced_ports.iter().any(|name| name == port) {
                1
            } else if source.skipped_ports.iter().any(|name| name == port) {
                2
            } else {
                0
            }
        };
        let state = edge.from_output.as_deref().map_or(0, port_state);
        if edge.members.len() > 1
            && (state == 0
                || edge
                    .members
                    .iter()
                    .any(|member| port_state(&member.output) != state))
        {
            return Style::default().fg(Color::Magenta);
        }
        if state == 2 {
            return Style::default().fg(Color::DarkGray);
        }
        if state == 1 {
            return Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
        }
        Style::default().fg(default)
    }

    fn matching_events<'b>(
        &self,
        events: &'b TransferEvents,
        source: usize,
    ) -> impl Iterator<Item = &'b TransferEvent> {
        let described = &self.layout.all_nodes[source];
        let path = self.layout.source_path(source);
        let correlation = self
            .observation(source)
            .and_then(|node| node.correlation.clone());
        let scoped = !path.is_empty();
        events
            .get(&(path.to_vec(), described.id.clone()))
            .into_iter()
            .flatten()
            .filter(move |event| {
                // Never borrow current emissions for a historical pass/item or another invocation.
                match (&correlation, &event.correlation) {
                    (Some(expected), Some(actual)) => expected == actual,
                    _ => !scoped && correlation.is_none(),
                }
            })
    }

    fn task_transfer_age(&self, index: usize, events: Option<&TransferEvents>) -> Option<u64> {
        let edge = &self.layout.edges[index];
        self.layout
            .transfer_members
            .get(index)?
            .iter()
            .filter_map(|member| {
                let target = *self
                    .layout
                    .sources(edge.to)
                    .iter()
                    .find(|target| self.layout.all_nodes[**target].id == member.target)?;
                let observation = self.observation(target)?;
                if member.exclusive
                    && !observation.input_selections.iter().any(|selection| {
                        selection.input == member.input
                            && selection.source_output == member.output
                            && selection.source_node == member.source
                    })
                {
                    return None;
                }
                if let Some(events) = events {
                    self.matching_events(events, target)
                        .filter(|event| {
                            event.snapshot.outcome == mf_runtime::SnapshotOutcome::Started
                                && (!self.layout.streaming
                                    || event.snapshot.inputs.get(member.input.as_str()).is_some())
                        })
                        .map(|event| event.age_ns)
                        .min()
                } else if self.layout.streaming {
                    None
                } else {
                    // Ordinary inputs resolve before NodeStarted. Only exclusive bindings need
                    // a selected_inputs witness; Running alone is never a transfer event.
                    self.elapsed_ns
                        .checked_sub(observation.started_elapsed_ns?.get().max(0) as u64)
                }
            })
            .min()
    }

    fn stream_transfer_ages(&self, index: usize, events: &TransferEvents) -> Vec<u64> {
        let edge = &self.layout.edges[index];
        let mut ages = BTreeSet::new();
        for member in &self.layout.transfer_members[index] {
            let Some(source) = self
                .layout
                .sources(edge.from)
                .iter()
                .find(|source| self.layout.all_nodes[**source].id == member.source)
            else {
                continue;
            };
            for event in self.matching_events(events, *source) {
                if event.snapshot.outcome == mf_runtime::SnapshotOutcome::Emitted
                    && event.snapshot.outputs.get(member.output.as_str()).is_some()
                {
                    // Coalesce emissions in the same visual frame, not every emission
                    // from a hot producer into a continually restarted source pulse.
                    ages.insert(event.age_ns / FLOW_FRAME_NS * FLOW_FRAME_NS);
                }
            }
        }
        ages.into_iter().collect()
    }

    fn draw_transfer(&self, area: Rect, buffer: &mut Buffer, index: usize, age_ns: u64) {
        if age_ns > TRANSFER_PULSE_NS {
            return;
        }
        let edge = &self.layout.edges[index];
        let route = &self.layout.routes[index];
        if route.unresolved.is_some() {
            return;
        }
        let total: u64 = route
            .points
            .windows(2)
            .map(|points| {
                u64::from(points[0].x.abs_diff(points[1].x))
                    + u64::from(points[0].y.abs_diff(points[1].y))
            })
            .sum();
        if total == 0 {
            return;
        }
        let head =
            (u128::from(age_ns) * u128::from(total + 3) / u128::from(TRANSFER_PULSE_NS)) as u64;
        let style = self.edge_style(edge).add_modifier(Modifier::BOLD);
        let mut travelled = 0_u64;
        for points in route.points.windows(2) {
            let (from, to) = (points[0], points[1]);
            let length = from.x.abs_diff(to.x) + from.y.abs_diff(to.y);
            if length == 0 {
                continue;
            }
            let (first, last) = if from.y == to.y {
                let low = from.x.min(to.x).max(self.offset.0);
                let high = from.x.max(to.x).min(
                    self.offset
                        .0
                        .saturating_add(u32::from(area.width))
                        .saturating_sub(1),
                );
                (low, high)
            } else {
                let low = from.y.min(to.y).max(self.offset.1);
                let high = from.y.max(to.y).min(
                    self.offset
                        .1
                        .saturating_add(u32::from(area.height))
                        .saturating_sub(1),
                );
                (low, high)
            };
            for coordinate in first..=last {
                let (x, y, step) = if from.y == to.y {
                    (coordinate, from.y, from.x.abs_diff(coordinate))
                } else {
                    (from.x, coordinate, from.y.abs_diff(coordinate))
                };
                let distance = travelled + u64::from(step);
                if distance <= head && head - distance < 4 {
                    let symbol = ["▓", "▒", "░", "·"][(head - distance) as usize];
                    self.put(area, buffer, x, y, symbol, style);
                }
            }
            travelled += u64::from(length);
        }
        if let Some(to) = route.points.last() {
            self.put(area, buffer, to.x, to.y, "▶", style);
        }
        if let Some(label) = &route.label {
            self.text(area, buffer, label.rect, &label.text, style);
        }
    }

    fn draw_cluster(&self, area: Rect, buffer: &mut Buffer, index: usize, cluster: &GraphCluster) {
        if cluster.collapsed {
            return;
        }
        let rect = cluster.rect;
        if rect.width < 3 || rect.height < 2 {
            return;
        }
        let style = cluster_style(self.layout.cluster_key(index).1);
        self.horizontal(
            area,
            buffer,
            rect.x + 1..=rect.x + rect.width - 2,
            rect.y,
            "─",
            style,
        );
        self.horizontal(
            area,
            buffer,
            rect.x + 1..=rect.x + rect.width - 2,
            rect.y + rect.height - 1,
            "─",
            style,
        );
        self.vertical(
            area,
            buffer,
            rect.x,
            rect.y + 1..=rect.y + rect.height - 2,
            "│",
            style,
        );
        self.vertical(
            area,
            buffer,
            rect.x + rect.width - 1,
            rect.y + 1..=rect.y + rect.height - 2,
            "│",
            style,
        );
        for (x, y, symbol) in [
            (rect.x, rect.y, "┌"),
            (rect.x + rect.width - 1, rect.y, "┐"),
            (rect.x, rect.y + rect.height - 1, "└"),
            (rect.x + rect.width - 1, rect.y + rect.height - 1, "┘"),
        ] {
            self.put(area, buffer, x, y, symbol, style);
        }
    }

    fn cluster_label_rect(&self, area: Rect, cluster: &GraphCluster) -> Option<GraphRect> {
        let rect = cluster.rect;
        if cluster.collapsed || rect.width < 3 || rect.height < 2 {
            return None;
        }
        let left = rect.x.max(self.offset.0);
        let top = rect.y.max(self.offset.1);
        let right = (rect.x + rect.width).min(self.offset.0.saturating_add(u32::from(area.width)));
        let bottom =
            (rect.y + rect.height).min(self.offset.1.saturating_add(u32::from(area.height)));
        if left >= right || top >= bottom {
            return None;
        }

        // Preserve the border padding when possible, but use the last visible cell in tiny views.
        let x = (rect.x + 2).max(left).min(right - 1);
        let width = (rect.x + rect.width - 2).min(right).max(x + 1) - x;
        Some(GraphRect {
            x,
            y: top,
            width,
            height: 1,
        })
    }

    fn draw_cluster_label(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        index: usize,
        cluster: &GraphCluster,
    ) {
        let Some(mut label_rect) = self.cluster_label_rect(area, cluster) else {
            return;
        };
        let label = display_line(&cluster.label);
        let label = if label_rect.width < 3 {
            label
        } else {
            format!(" {label} ")
        };
        label_rect.width = (ratatui::text::Span::raw(&label).width() as u32).min(label_rect.width);
        self.text(
            area,
            buffer,
            label_rect,
            &label,
            cluster_style(self.layout.cluster_key(index).1),
        );
    }

    fn draw_node(&self, area: Rect, buffer: &mut Buffer, index: usize, node: &GraphNode) {
        let mut rect = node.rect;
        if rect.x >= self.offset.0.saturating_add(u32::from(area.width))
            || rect.x.saturating_add(rect.width) <= self.offset.0
            || rect.y >= self.offset.1.saturating_add(u32::from(area.height))
            || rect.y.saturating_add(rect.height) <= self.offset.1
        {
            return;
        }
        let annotation = &self.layout.node_annotations[index];
        let collapsed = self.layout.is_collapsed_cluster(index);
        let annotation_y = if collapsed { rect.y + 3 } else { rect.y };
        if !collapsed && !annotation.is_empty() {
            let header = annotation.len() as u32 + 1;
            rect.y += header;
            rect.height -= header;
        }
        let observed = self
            .layout
            .sources(index)
            .first()
            .copied()
            .filter(|_| !self.layout.is_collapsed_cluster(index))
            .and_then(|source| self.observation(source));
        let status = if self.layout.is_collapsed_cluster(index) {
            self.display_status(index)
        } else {
            observed.map_or(NodeStatus::Pending, |item| item.status)
        };
        let style = node_style(status);
        let border_style = self.layout.cluster_of(index).map_or(style, |cluster| {
            cluster_style(self.layout.cluster_key(cluster).1)
        });
        self.horizontal(
            area,
            buffer,
            rect.x + 1..=rect.x + rect.width - 2,
            rect.y,
            "─",
            border_style,
        );
        self.horizontal(
            area,
            buffer,
            rect.x + 1..=rect.x + rect.width - 2,
            rect.y + rect.height - 1,
            "─",
            border_style,
        );
        self.vertical(
            area,
            buffer,
            rect.x,
            rect.y + 1..=rect.y + rect.height - 2,
            "│",
            border_style,
        );
        self.vertical(
            area,
            buffer,
            rect.x + rect.width - 1,
            rect.y + 1..=rect.y + rect.height - 2,
            "│",
            border_style,
        );
        for (x, y, symbol) in [
            (rect.x, rect.y, "┌"),
            (rect.x + rect.width - 1, rect.y, "┐"),
            (rect.x, rect.y + rect.height - 1, "└"),
            (rect.x + rect.width - 1, rect.y + rect.height - 1, "┘"),
        ] {
            self.put(area, buffer, x, y, symbol, border_style);
        }

        let marker = status_marker(status, observed, self.elapsed_ns);
        let fold = if self.layout.is_expanded_body(index) {
            " [-]"
        } else if self.layout.body_nodes.contains(&index) {
            " [+]"
        } else {
            ""
        };
        let title = format!("{marker} {}{fold}", display_line(&node.id));
        let detail = if self.layout.is_collapsed_cluster(index) {
            format!("{} nodes | collapsed", self.layout.sources(index).len())
        } else {
            display_line(&node_detail(node, observed, self.elapsed_ns))
        };
        let title_rect = GraphRect {
            x: rect.x + 1,
            y: rect.y + 1,
            width: rect.width - 2,
            height: 1,
        };
        let title_style = if self.selected == Some(index) {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        };
        self.text(area, buffer, title_rect, &title, title_style);
        if collapsed {
            let label_style = if self.selected == Some(index) {
                border_style.add_modifier(Modifier::REVERSED)
            } else {
                border_style
            };
            self.text(
                area,
                buffer,
                GraphRect {
                    x: title_rect.x + 2,
                    width: title_rect.width.saturating_sub(2),
                    ..title_rect
                },
                &display_line(&node.id),
                label_style,
            );
        }
        self.text(
            area,
            buffer,
            GraphRect {
                y: title_rect.y + 1,
                ..title_rect
            },
            &detail,
            Style::default().fg(Color::Gray),
        );
        if node.kind == "builtin.llm" && !collapsed {
            let calls = self
                .reader
                .zip(observed)
                .map_or_else(Vec::new, |(reader, observed)| {
                    crate::performance::node_calls(reader, observed)
                });
            for (row, line) in crate::performance::card_lines(&calls).iter().enumerate() {
                self.text(
                    area,
                    buffer,
                    GraphRect {
                        y: title_rect.y + 2 + row as u32,
                        ..title_rect
                    },
                    line,
                    Style::default().fg(Color::DarkGray),
                );
            }
        }
        if let Some(cluster) = self.layout.cluster_of(index) {
            let cluster = &self.layout.clusters()[cluster];
            if self
                .cluster_label_rect(area, cluster)
                .is_some_and(|label| label.x != cluster.rect.x + 2 || label.y != cluster.rect.y)
            {
                return;
            }
        }
        for (row, line) in annotation.iter().enumerate() {
            self.text(
                area,
                buffer,
                GraphRect {
                    x: rect.x + 1,
                    y: annotation_y + row as u32,
                    width: rect.width - 2,
                    height: 1,
                },
                line,
                Style::default().fg(Color::Gray),
            );
        }
    }

    fn draw_node_activity(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        index: usize,
        node: &GraphNode,
        events: Option<&TransferEvents>,
    ) {
        let status = self.display_status(index);
        if status == NodeStatus::Pending
            || self.display_uncertain(index)
            || node.rect.x >= self.offset.0.saturating_add(u32::from(area.width))
            || node.rect.x.saturating_add(node.rect.width) <= self.offset.0
            || node.rect.y >= self.offset.1.saturating_add(u32::from(area.height))
            || node.rect.y.saturating_add(node.rect.height) <= self.offset.1
        {
            return;
        }
        let mut rect = node.rect;
        let collapsed = self.layout.is_collapsed_cluster(index);
        let annotation = &self.layout.node_annotations[index];
        if !collapsed && !annotation.is_empty() {
            let header = annotation.len() as u32 + 1;
            rect.y += header;
            rect.height -= header;
        }
        let style = node_style(status);
        let terminal_age = if let Some(events) = events {
            let now = Instant::now();
            self.layout
                .sources(index)
                .iter()
                .filter_map(|source| self.observation(*source))
                .filter_map(|observation| observation.terminal_received_at)
                .map(|received_at| {
                    now.saturating_duration_since(received_at)
                        .as_nanos()
                        .min(u64::MAX as u128) as u64
                })
                .min()
                .or_else(|| {
                    self.layout
                        .sources(index)
                        .iter()
                        .flat_map(|source| self.matching_events(events, *source))
                        .filter(|event| {
                            matches!(
                                event.snapshot.outcome,
                                mf_runtime::SnapshotOutcome::Succeeded
                                    | mf_runtime::SnapshotOutcome::Failed
                                    | mf_runtime::SnapshotOutcome::Skipped
                            )
                        })
                        .map(|event| event.age_ns)
                        .min()
                })
        } else {
            self.layout
                .sources(index)
                .iter()
                .filter_map(|source| self.observation(*source))
                .filter_map(|observation| observation.terminal_elapsed_ns)
                .filter_map(|terminal| self.elapsed_ns.checked_sub(terminal.get().max(0) as u64))
                .min()
        };
        if status == NodeStatus::Running {
            let cells = perimeter_cells(rect);
            if cells != 0 {
                self.draw_perimeter_chase(
                    area,
                    buffer,
                    rect,
                    (self.elapsed_ns / FLOW_FRAME_NS) % cells,
                    style,
                );
            }
        } else if let Some(age) = terminal_age.filter(|age| *age < NODE_TRANSITION_NS) {
            let level = 3 - (u128::from(age) * 3 / u128::from(NODE_TRANSITION_NS)) as usize;
            if let Some(symbol) = ["", "░", "▒", "▓"].get(level) {
                self.draw_perimeter_flash(area, buffer, rect, symbol, style);
            }
        }
    }

    fn draw_perimeter_chase(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        rect: GraphRect,
        head: u64,
        style: Style,
    ) {
        let cells = perimeter_cells(rect);
        if cells == 0 {
            return;
        }
        let style = style.add_modifier(Modifier::BOLD);
        for (offset, symbol) in ["▓", "▒", "░", "·"].into_iter().enumerate() {
            let position = (head + cells - offset as u64 % cells) % cells;
            let Some((x, y)) = perimeter_cell(rect, position) else {
                continue;
            };
            self.put(area, buffer, x, y, symbol, style);
        }
    }

    fn draw_perimeter_flash(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        rect: GraphRect,
        symbol: &str,
        style: Style,
    ) {
        let cells = perimeter_cells(rect);
        if cells == 0 {
            return;
        }
        let style = style.add_modifier(Modifier::BOLD);
        for position in 0..cells {
            let Some((x, y)) = perimeter_cell(rect, position) else {
                continue;
            };
            self.put(area, buffer, x, y, symbol, style);
        }
    }

    fn minimap_area(&self, area: Rect) -> Option<Rect> {
        let (width, height) = self.layout.size();
        if !self.minimap
            || area.width < 24
            || area.height < 8
            || width <= u32::from(area.width) && height <= u32::from(area.height)
        {
            return None;
        }
        let width = (area.width / 3).clamp(14, 32);
        let height = (area.height / 3).clamp(5, 12);
        Some(Rect {
            x: area.x + area.width - width,
            y: area.y + area.height - height,
            width,
            height,
        })
    }

    fn mini_put(buffer: &mut Buffer, area: Rect, x: u16, y: u16, symbol: &str, style: Style) {
        if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
            return;
        }
        if let Some(cell) = buffer.cell_mut(Position::new(x, y)) {
            cell.reset();
            cell.set_symbol(symbol).set_style(style);
        }
    }

    fn draw_minimap(&self, area: Rect, buffer: &mut Buffer, map: Rect) {
        for y in map.y..map.y + map.height {
            for x in map.x..map.x + map.width {
                Self::mini_put(buffer, map, x, y, " ", Style::default());
            }
        }
        let border = Style::default().fg(Color::DarkGray);
        for x in map.x + 1..map.x + map.width - 1 {
            Self::mini_put(buffer, map, x, map.y, "─", border);
            Self::mini_put(buffer, map, x, map.y + map.height - 1, "─", border);
        }
        for y in map.y + 1..map.y + map.height - 1 {
            Self::mini_put(buffer, map, map.x, y, "│", border);
            Self::mini_put(buffer, map, map.x + map.width - 1, y, "│", border);
        }
        for (x, y, symbol) in [
            (map.x, map.y, "┌"),
            (map.x + map.width - 1, map.y, "┐"),
            (map.x, map.y + map.height - 1, "└"),
            (map.x + map.width - 1, map.y + map.height - 1, "┘"),
        ] {
            Self::mini_put(buffer, map, x, y, symbol, border);
        }
        for (offset, symbol) in [" ", "M", "a", "p", " "].into_iter().enumerate() {
            Self::mini_put(
                buffer,
                map,
                map.x + 1 + offset as u16,
                map.y,
                symbol,
                border,
            );
        }
        let inner = Rect {
            x: map.x + 1,
            y: map.y + 1,
            width: map.width - 2,
            height: map.height - 2,
        };
        let (canvas_width, canvas_height) = self.layout.size();
        let scale = |x: u32, y: u32| {
            let x = u64::from(x) * u64::from(inner.width.saturating_sub(1))
                / u64::from(canvas_width.saturating_sub(1).max(1));
            let y = u64::from(y) * u64::from(inner.height.saturating_sub(1))
                / u64::from(canvas_height.saturating_sub(1).max(1));
            (inner.x + x as u16, inner.y + y as u16)
        };
        let line_style = Style::default().fg(Color::DarkGray);
        for route in &self.layout.routes {
            if route.unresolved.is_some() {
                continue;
            }
            for points in route.points.windows(2) {
                let (from, to) = (
                    scale(points[0].x, points[0].y),
                    scale(points[1].x, points[1].y),
                );
                let dx = to.0.abs_diff(from.0);
                let dy = to.1.abs_diff(from.1);
                let steps = dx.max(dy);
                for step in 0..=steps {
                    let x = if steps == 0 {
                        i32::from(from.0)
                    } else {
                        i32::from(from.0)
                            + (i32::from(to.0) - i32::from(from.0)) * i32::from(step)
                                / i32::from(steps)
                    } as u16;
                    let y = if steps == 0 {
                        i32::from(from.1)
                    } else {
                        i32::from(from.1)
                            + (i32::from(to.1) - i32::from(from.1)) * i32::from(step)
                                / i32::from(steps)
                    } as u16;
                    Self::mini_put(buffer, inner, x, y, "·", line_style);
                }
            }
        }
        for (index, node) in self.layout.nodes.iter().enumerate() {
            let (x, y) = scale(
                node.rect.x + node.rect.width / 2,
                node.rect.y + node.rect.height / 2,
            );
            let style = node_style(self.display_status(index));
            Self::mini_put(
                buffer,
                inner,
                x,
                y,
                if self.selected == Some(index) {
                    "◆"
                } else {
                    "■"
                },
                if self.selected == Some(index) {
                    style.add_modifier(Modifier::REVERSED)
                } else {
                    style
                },
            );
        }
        let viewport = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let (left, top) = scale(
            self.offset.0.min(canvas_width.saturating_sub(1)),
            self.offset.1.min(canvas_height.saturating_sub(1)),
        );
        let (mut right, mut bottom) = scale(
            self.offset
                .0
                .saturating_add(u32::from(area.width))
                .min(canvas_width - 1),
            self.offset
                .1
                .saturating_add(u32::from(area.height))
                .min(canvas_height - 1),
        );
        if right == left && right < inner.x + inner.width - 1 {
            right += 1;
        }
        if bottom == top && bottom < inner.y + inner.height - 1 {
            bottom += 1;
        }
        for x in left + 1..right {
            Self::mini_put(buffer, inner, x, top, "─", viewport);
            Self::mini_put(buffer, inner, x, bottom, "─", viewport);
        }
        for y in top + 1..bottom {
            Self::mini_put(buffer, inner, left, y, "│", viewport);
            Self::mini_put(buffer, inner, right, y, "│", viewport);
        }
        for (x, y, symbol) in [
            (left, top, "┌"),
            (right, top, "┐"),
            (left, bottom, "└"),
            (right, bottom, "┘"),
        ] {
            Self::mini_put(buffer, inner, x, y, symbol, viewport);
        }
        // Keep the selection legible when compression lands it on the viewport frame
        // or on another node in the same minimap cell.
        if let Some(selected) = self.selected
            && let Some(node) = self.layout.nodes.get(selected)
        {
            let (x, y) = scale(
                node.rect.x + node.rect.width / 2,
                node.rect.y + node.rect.height / 2,
            );
            Self::mini_put(
                buffer,
                inner,
                x,
                y,
                "◆",
                node_style(self.display_status(selected)).add_modifier(Modifier::REVERSED),
            );
        }
    }
}

impl Widget for GraphView<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let reader_data = self.reader.map(|reader| {
            reader.recent_data_snapshot(std::time::Duration::from_nanos(TRANSFER_PULSE_NS))
        });
        let data = self.data.or(reader_data.as_ref());
        let observed_at_unix_nano = self.observed_at_unix_nano.or_else(|| {
            self.reader.and_then(|_| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|elapsed| elapsed.as_nanos().min(u64::MAX as u128) as u64)
            })
        });
        let mut stream_events = BTreeMap::new();
        let animation_now = std::time::Instant::now();
        if let (Some(data), Some(now)) = (data, observed_at_unix_nano) {
            for (entry, metadata) in data.entries.iter().zip(&data.metadata).rev() {
                let age = if let Some(received_at) = metadata.received_at {
                    animation_now
                        .saturating_duration_since(received_at)
                        .as_nanos()
                        .min(u64::MAX as u128) as u64
                } else if let Some(age) = now.checked_sub(metadata.time_unix_nano) {
                    age
                } else {
                    continue;
                };
                if age > TRANSFER_PULSE_NS {
                    // Live receipt times are monotonic even when source records are reordered.
                    if metadata.received_at.is_some() {
                        break;
                    }
                    continue;
                }
                let scope = entry
                    .scope
                    .iter()
                    .map(|entry| entry.node_id().to_owned())
                    .collect::<Vec<_>>();
                let key = (scope, entry.node.to_string());
                if let Some(snapshot) = entry.snapshot.node(&entry.scope, &entry.node) {
                    stream_events
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(TransferEvent {
                            age_ns: age,
                            correlation: metadata.correlation.clone(),
                            snapshot: Arc::clone(snapshot),
                        });
                }
            }
        }
        for (index, cluster) in self.layout.clusters().iter().enumerate() {
            self.draw_cluster(area, buffer, index, cluster);
        }
        for index in 0..self.layout.edges.len() {
            self.draw_edge(area, buffer, index);
            let task = self.task_transfer_age(index, observed_at_unix_nano.map(|_| &stream_events));
            for age in self
                .stream_transfer_ages(index, &stream_events)
                .into_iter()
                .rev()
                .chain(task)
            {
                self.draw_transfer(area, buffer, index, age);
            }
        }
        for crossing in &self.layout.crossings {
            self.draw_crossing(area, buffer, crossing);
        }
        for (index, node) in self.layout.nodes.iter().enumerate() {
            self.draw_node(area, buffer, index, node);
        }
        for index in 0..self.layout.edges.len() {
            self.draw_ports(area, buffer, index);
        }
        // Attachments belong to the border: chase and flash must cover them too.
        // Each frame restores their ordinary glyph before applying node activity.
        for (index, node) in self.layout.nodes.iter().enumerate() {
            self.draw_node_activity(
                area,
                buffer,
                index,
                node,
                observed_at_unix_nano.map(|_| &stream_events),
            );
        }
        // Pinned labels must remain readable over nodes and connections after panning.
        for (index, cluster) in self.layout.clusters().iter().enumerate() {
            self.draw_cluster_label(area, buffer, index, cluster);
        }
        if let Some(map) = self.minimap_area(area) {
            self.draw_minimap(area, buffer, map);
        }
    }
}

fn merge_status(left: NodeStatus, right: NodeStatus) -> NodeStatus {
    use NodeStatus::*;
    let rank = |status| match status {
        Failed => 6,
        Running => 5,
        Unknown | Interrupted => 4,
        Pending => 3,
        Succeeded => 2,
        Skipped => 1,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

fn cluster_style(index: usize) -> Style {
    const COLORS: [Color; 6] = [
        Color::LightCyan,
        Color::LightMagenta,
        Color::LightBlue,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightRed,
    ];
    Style::default()
        .fg(COLORS[index % COLORS.len()])
        .add_modifier(Modifier::BOLD)
}

fn node_style(status: NodeStatus) -> Style {
    match status {
        NodeStatus::Pending => Style::default().fg(Color::DarkGray),
        NodeStatus::Running => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        NodeStatus::Succeeded => Style::default().fg(Color::Green),
        NodeStatus::Failed => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        NodeStatus::Skipped => Style::default().fg(Color::DarkGray),
        NodeStatus::Unknown | NodeStatus::Interrupted => Style::default().fg(Color::Magenta),
    }
}

fn perimeter_cells(rect: GraphRect) -> u64 {
    if rect.width < 2 || rect.height < 2 {
        return 0;
    }
    u64::from(rect.width + rect.height) * 2 - 4
}

fn perimeter_cell(rect: GraphRect, position: u64) -> Option<(u32, u32)> {
    let width = u64::from(rect.width);
    let height = u64::from(rect.height);
    let cells = perimeter_cells(rect);
    if position >= cells {
        return None;
    }
    let right = rect.x + rect.width - 1;
    let bottom = rect.y + rect.height - 1;
    let top_run = width;
    let left_run = height - 1;
    let bottom_run = width - 1;
    Some(if position < top_run {
        // Counter-clockwise from the top-right corner.
        (right - position as u32, rect.y)
    } else if position < top_run + left_run {
        (rect.x, rect.y + 1 + (position - top_run) as u32)
    } else if position < top_run + left_run + bottom_run {
        (rect.x + 1 + (position - top_run - left_run) as u32, bottom)
    } else {
        (
            right,
            bottom - 1 - (position - top_run - left_run - bottom_run) as u32,
        )
    })
}

fn status_marker(status: NodeStatus, observed: Option<&NodeObservation>, elapsed_ns: u64) -> char {
    if observed.is_some_and(|node| node.possibly_missing_events) {
        return '?';
    }
    match status {
        NodeStatus::Pending => '·',
        NodeStatus::Running => ['|', '/', '-', '\\'][(elapsed_ns / 200_000_000 % 4) as usize],
        NodeStatus::Succeeded => '✓',
        NodeStatus::Failed => '×',
        NodeStatus::Skipped => '↷',
        NodeStatus::Unknown | NodeStatus::Interrupted => '?',
    }
}

fn node_detail(node: &GraphNode, observed: Option<&NodeObservation>, elapsed_ns: u64) -> String {
    let Some(observed) = observed else {
        return node.kind.clone();
    };
    if observed.possibly_missing_events {
        if let Some(last_known) = observed.last_known {
            return format!("last {}; missing", status_name(last_known));
        }
        return format!("missing events | {}", node.kind);
    }
    if let Some(last_known) = observed.last_known {
        return format!("last {} | {}", status_name(last_known), node.kind);
    }
    let duration = if observed.status == NodeStatus::Running {
        observed
            .started_elapsed_ns
            .map(|start| elapsed_ns.saturating_sub(start.get().max(0) as u64))
    } else {
        observed
            .duration_ns
            .map(|duration| duration.get().max(0) as u64)
    };
    duration.map_or_else(
        || node.kind.clone(),
        |duration| format!("{} | {}", format_duration_ns(duration), node.kind),
    )
}

fn status_name(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "pending",
        NodeStatus::Running => "running",
        NodeStatus::Succeeded => "succeeded",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Unknown => "unknown",
        NodeStatus::Interrupted => "interrupted",
    }
}

fn display_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn wrap_annotation(text: &str, width: u32) -> Vec<String> {
    let mut lines = Vec::new();
    for source in text.split('\n') {
        let source_start = lines.len();
        let span = ratatui::text::Span::raw(source.replace('\t', "    "));
        let mut line = String::new();
        let mut used = 0;
        let mut word_start = None;
        let mut previous_whitespace = false;
        let mut has_word = false;
        let mut continuation = false;
        for grapheme in span.styled_graphemes(Style::default()) {
            let cells = ratatui::text::Span::raw(grapheme.symbol).width() as u32;
            let whitespace = grapheme.is_whitespace();
            if !whitespace && previous_whitespace && has_word {
                word_start = Some(line.len());
            }
            if used + cells > width && !line.is_empty() {
                // Move the whole pending word before falling back to grapheme-safe hard wrapping.
                let word = if whitespace {
                    None
                } else {
                    word_start.take().map(|start| line.split_off(start))
                };
                line.truncate(
                    line.trim_end_matches(|ch: char| {
                        ch.is_whitespace() && ch != '\u{a0}' || ch == '\u{200b}'
                    })
                    .len(),
                );
                lines.push(std::mem::take(&mut line));
                line = word.unwrap_or_default();
                used = ratatui::text::Span::raw(&line).width() as u32;
                if used + cells > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                word_start = None;
                has_word = !line.is_empty();
                continuation = true;
            }
            previous_whitespace = whitespace;
            if whitespace && line.is_empty() && continuation {
                continue;
            }
            line.push_str(grapheme.symbol);
            used += cells;
            has_word |= !whitespace;
        }
        if !line.is_empty() || lines.len() == source_start {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    use routing::{Point, RoutingStats};

    #[test]
    fn annotations_prefer_word_boundaries_and_preserve_explicit_lines() {
        for (text, width, expected) in [
            (
                "Read the source and forward its value.",
                22,
                vec!["Read the source and", "forward its value."],
            ),
            ("alpha beta gamma", 10, vec!["alpha beta", "gamma"]),
            ("alpha beta gamma", 11, vec!["alpha beta", "gamma"]),
            ("alpha beta gamma", 9, vec!["alpha", "beta", "gamma"]),
            ("one  two three", 8, vec!["one  two", "three"]),
            ("abcdefghijklmno", 6, vec!["abcdef", "ghijkl", "mno"]),
            ("a\u{a0}b", 2, vec!["a\u{a0}", "b"]),
            (
                "a\u{301}b\u{301}c\u{301} 🦀🦀",
                4,
                vec!["a\u{301}b\u{301}c\u{301}", "🦀🦀"],
            ),
            ("alpha\n\n  beta", 8, vec!["alpha", "", "  beta"]),
            (
                "\tKeep a\u{301} and 🦀.",
                22,
                vec!["    Keep a\u{301} and 🦀."],
            ),
            ("alpha ", 5, vec!["alpha"]),
            ("", 22, vec![""]),
        ] {
            let actual = wrap_annotation(text, width);
            assert_eq!(actual, expected, "{text:?}, width {width}");
            assert!(
                actual
                    .iter()
                    .all(|line| ratatui::text::Span::raw(line).width() <= width as usize)
            );
        }
    }

    #[test]
    fn incomplete_node_details_distinguish_last_known_status_from_missing_history() {
        let layout = crossing_layout(EdgeKind::Data, EdgeKind::Control, true);
        let node = &layout.nodes[0];
        let mut observed = NodeObservation::pending(&node.id, &node.kind);
        observed.status = NodeStatus::Interrupted;
        observed.possibly_missing_events = true;
        observed.last_known = Some(NodeStatus::Running);
        assert_eq!(
            node_detail(node, Some(&observed), 0),
            "last running; missing"
        );
        observed.last_known = None;
        assert_eq!(
            node_detail(node, Some(&observed), 0),
            "missing events | fixture.node"
        );
    }

    fn crossing_layout(
        horizontal: EdgeKind,
        vertical: EdgeKind,
        upper_horizontal: bool,
    ) -> GraphLayout {
        let nodes: Vec<_> = ["horizontal", "vertical"]
            .into_iter()
            .enumerate()
            .map(|(index, id)| GraphNode {
                id: id.into(),
                kind: "fixture.node".into(),
                rect: GraphRect {
                    x: 100 + index as u32 * 40,
                    y: 100,
                    width: 24,
                    height: 6,
                },
            })
            .collect();
        let edges = [horizontal, vertical]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| GraphEdge {
                key: EdgeKey {
                    source: nodes[index].id.clone(),
                    destination: nodes[index].id.clone(),
                    output: Some("value".into()),
                    input: Some("input".into()),
                    kind,
                },
                from: index,
                to: index,
                from_output: Some("value".into()),
                to_input: Some("input".into()),
                kind,
                members: Vec::new(),
                from_row: 1,
                to_row: 4,
            })
            .collect();
        GraphLayout {
            streaming: false,
            view: ConnectionView::Ports,
            hidden_data_count: 0,
            hidden_transitive_count: 0,
            hidden_cluster_edge_count: 0,
            sources: vec![vec![0], vec![1]],
            display_of_node: vec![0, 1],
            node_clusters: vec![None, None],
            node_annotations: vec![Vec::new(), Vec::new()],
            source_paths: vec![Vec::new(); nodes.len()],
            cluster_keys: Vec::new(),
            expanded_nodes: BTreeSet::new(),
            body_nodes: BTreeSet::new(),
            all_nodes: nodes
                .iter()
                .map(|node| NodeDescription {
                    id: node.id.clone(),
                    kind: node.kind.clone(),
                })
                .collect(),
            clusters: Vec::new(),
            nodes,
            edges,
            transfer_members: vec![Vec::new(), Vec::new()],
            routes: vec![
                EdgeRoute::from_points([Point::new(2, 10), Point::new(18, 10)]),
                EdgeRoute::from_points([
                    Point::new(2, 2),
                    Point::new(10, 2),
                    Point::new(10, 18),
                    Point::new(18, 18),
                ]),
            ],
            crossings: vec![Crossing {
                point: Point::new(10, 10),
                horizontal: 0,
                vertical: 1,
                upper: usize::from(!upper_horizontal),
            }],
            routing_stats: RoutingStats::default(),
            width: 22,
            height: 22,
        }
    }

    #[test]
    fn continuous_crossings_preserve_each_strokes_color_and_modifier() {
        for horizontal in [EdgeKind::Data, EdgeKind::Control] {
            for vertical in [EdgeKind::Data, EdgeKind::Control] {
                for upper_horizontal in [true, false] {
                    let layout = crossing_layout(horizontal, vertical, upper_horizontal);
                    let upper = layout.crossings[0].upper;
                    let mut observations: Vec<_> = layout
                        .nodes
                        .iter()
                        .map(|node| NodeObservation::pending(&node.id, &node.kind))
                        .collect();
                    observations[upper].produced_ports.push("value".into());
                    observations[1 - upper].possibly_missing_events = true;
                    let mut buffer = Buffer::empty(Rect::new(0, 0, 22, 22));
                    GraphView::new(&layout)
                        .nodes(&observations)
                        .render(buffer.area, &mut buffer);
                    for along in [-2i16, -1, 0, 1, 2] {
                        for horizontal_axis in [true, false] {
                            let is_upper = horizontal_axis == upper_horizontal;
                            if along == 0 && !is_upper {
                                continue;
                            }
                            let rendered = buffer
                                .cell(Position::new(
                                    (10 + if horizontal_axis { along } else { 0 }) as u16,
                                    (10 + if horizontal_axis { 0 } else { along }) as u16,
                                ))
                                .unwrap();
                            let expected = if along == 0 {
                                if upper_horizontal { "╪" } else { "╫" }
                            } else if horizontal_axis {
                                "─"
                            } else {
                                "│"
                            };
                            assert_eq!(rendered.symbol(), expected);
                            assert_eq!(
                                rendered.fg,
                                if is_upper {
                                    Color::Green
                                } else {
                                    Color::Magenta
                                }
                            );
                            assert_eq!(rendered.modifier.contains(Modifier::BOLD), is_upper);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn partially_visible_crossings_match_full_canvas_symbols_and_styles() {
        for horizontal in [EdgeKind::Data, EdgeKind::Control] {
            for vertical in [EdgeKind::Data, EdgeKind::Control] {
                for upper_horizontal in [true, false] {
                    let layout = crossing_layout(horizontal, vertical, upper_horizontal);
                    let mut full = Buffer::empty(Rect::new(0, 0, 22, 22));
                    GraphView::new(&layout).render(full.area, &mut full);
                    for offset in [(9, 9), (10, 10), (11, 9), (8, 11), (0, 0)] {
                        let mut clipped = Buffer::empty(Rect::new(3, 5, 4, 5));
                        GraphView::new(&layout)
                            .offset(offset.0, offset.1)
                            .render(clipped.area, &mut clipped);
                        for y in 0..5 {
                            for x in 0..4 {
                                assert_eq!(
                                    clipped.cell(Position::new(3 + x, 5 + y)).unwrap(),
                                    full.cell(Position::new(
                                        offset.0 as u16 + x,
                                        offset.1 as u16 + y
                                    ))
                                    .unwrap()
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
