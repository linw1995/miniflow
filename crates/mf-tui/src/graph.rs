//! Stable terminal projection of a described workflow graph.

pub mod routing;

use routing::{Crossing, EdgeKey, EdgeRoute};

use crate::{
    duration::format_duration_ns,
    state::{MAX_SESSION_NODES, NodeObservation, NodeStatus, StateSnapshot},
};
use mf_telemetry::{
    ContractError,
    description::{WorkflowDescription, WorkflowDescriptionVersion},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use rust_sugiyama::{configure::Config, from_vertices_and_edges};
use snafu::{ResultExt, Snafu};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

const NODE_WIDTH: u32 = 24;
const NODE_HEIGHT: u32 = 6;
const COLUMN_GAP: u32 = 10;
const ROW_GAP: u32 = routing::ATTACHMENT_SPACING;
const COMPONENT_GAP: u32 = 12;
const MARGIN: u32 = 2;
const LAYOUT_STACK_BYTES: usize = 64 * 1024 * 1024;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeKind {
    Data,
    Control,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphEdge {
    pub key: EdgeKey,
    pub from: usize,
    pub to: usize,
    pub from_output: String,
    pub to_input: Option<String>,
    pub kind: EdgeKind,
    pub from_row: u32,
    pub to_row: u32,
}

pub struct GraphLayout {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
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
        let body = description
            .loop_body(path)
            .ok_or(GraphError::MissingLoopBody)?;
        let scope = WorkflowDescription {
            version: WorkflowDescriptionVersion::V2026_09_27,
            workflow_id: description.workflow_id.clone(),
            nodes: body.nodes.clone(),
            data_edges: body.data_edges.clone(),
            control_edges: body.control_edges.clone(),
            execution_order: body.execution_order.clone(),
            execution: None,
            loop_bodies: Vec::new(),
        };
        Self::new(&scope)
    }

    pub fn new(description: &WorkflowDescription) -> Result<Self, GraphError> {
        Self::with_routing_limits(description, routing::RoutingLimits::default())
    }

    pub fn with_routing_limits(
        description: &WorkflowDescription,
        limits: routing::RoutingLimits,
    ) -> Result<Self, GraphError> {
        description.validate().context(DescriptionSnafu)?;
        if description.nodes.len() > MAX_SESSION_NODES {
            return Err(GraphError::TooManyNodes {
                limit: MAX_SESSION_NODES,
            });
        }

        let described: BTreeMap<_, _> = description
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let positions: BTreeMap<_, _> = description
            .execution_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), index))
            .collect();
        let mut nodes: Vec<_> = description
            .execution_order
            .iter()
            .map(|id| GraphNode {
                id: id.clone(),
                kind: described[id.as_str()].kind.clone(),
                rect: GraphRect {
                    x: 0,
                    y: 0,
                    width: NODE_WIDTH,
                    height: NODE_HEIGHT,
                },
            })
            .collect();
        let mut edges =
            Vec::with_capacity(description.data_edges.len() + description.control_edges.len());
        let mut topology = BTreeSet::new();
        for edge in &description.data_edges {
            let from = positions[edge.from_node.as_str()];
            let to = positions[edge.to_node.as_str()];
            topology.insert((from as u32, to as u32));
            edges.push(GraphEdge {
                key: EdgeKey {
                    source: nodes[from].id.clone(),
                    destination: nodes[to].id.clone(),
                    output: edge.from_output.clone(),
                    input: Some(edge.to_input.clone()),
                    kind: EdgeKind::Data,
                },
                from,
                to,
                from_output: edge.from_output.clone(),
                to_input: Some(edge.to_input.clone()),
                kind: EdgeKind::Data,
                from_row: 0,
                to_row: 0,
            });
        }
        for edge in &description.control_edges {
            let from = positions[edge.from_node.as_str()];
            let to = positions[edge.to_node.as_str()];
            topology.insert((from as u32, to as u32));
            edges.push(GraphEdge {
                key: EdgeKey {
                    source: nodes[from].id.clone(),
                    destination: nodes[to].id.clone(),
                    output: edge.from_output.clone(),
                    input: None,
                    kind: EdgeKind::Control,
                },
                from,
                to,
                from_output: edge.from_output.clone(),
                to_input: None,
                kind: EdgeKind::Control,
                from_row: 0,
                to_row: 0,
            });
        }
        let mut outgoing = vec![Vec::new(); nodes.len()];
        let mut incoming = vec![Vec::new(); nodes.len()];
        for (index, edge) in edges.iter().enumerate() {
            outgoing[edge.from].push(index);
            incoming[edge.to].push(index);
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            let count = outgoing[index].len().max(incoming[index].len()) as u32;
            node.rect.height = NODE_HEIGHT.max(count * routing::ATTACHMENT_SPACING);
        }

        let vertices: Vec<_> = (0..nodes.len())
            .map(|index| {
                (
                    index as u32,
                    (nodes[index].rect.height as f64, NODE_WIDTH as f64),
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
                        .map(|(id, _)| nodes[*id].rect.height + ROW_GAP)
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
                    .map(|(id, _)| nodes[*id].rect.height + ROW_GAP)
                    .sum::<u32>();
                let centering = (component_height - layer_height)
                    / (2 * routing::ATTACHMENT_SPACING)
                    * routing::ATTACHMENT_SPACING;
                let mut row_y = component_top + centering;
                for (id, _) in layer.iter() {
                    if let Some(node) = nodes.get_mut(*id) {
                        node.rect.x = MARGIN + column as u32 * (NODE_WIDTH + COLUMN_GAP);
                        node.rect.y = row_y;
                        row_y += node.rect.height + ROW_GAP;
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
        for (index, attachments) in outgoing.iter_mut().enumerate() {
            attachments.sort_by_key(|&edge| (nodes[edges[edge].to].rect.y, &edges[edge].key));
            for (row, &edge) in attachments.iter().enumerate() {
                edges[edge].from_row = 1 + row as u32 * routing::ATTACHMENT_SPACING;
            }
            incoming[index].sort_by_key(|&edge| (nodes[edges[edge].from].rect.y, &edges[edge].key));
            for (row, &edge) in incoming[index].iter().enumerate() {
                edges[edge].to_row = 4 + row as u32 * routing::ATTACHMENT_SPACING;
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
        let right = nodes
            .iter()
            .map(|node| node.rect.x + node.rect.width)
            .chain(
                routes
                    .iter()
                    .flat_map(|route| route.points.iter().map(|point| point.x)),
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
            .max()
            .unwrap_or(MARGIN * 2);
        Ok(Self {
            routes,
            crossings,
            routing_stats,
            nodes,
            edges,
            width: right + MARGIN,
            height,
        })
    }

    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
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
            let source = format!(
                "{}.{}",
                display_line(&edge.key.source),
                display_line(&edge.from_output)
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

pub struct GraphView<'a> {
    layout: &'a GraphLayout,
    nodes: Option<&'a [NodeObservation]>,
    offset: (u32, u32),
    elapsed_ns: u64,
}

impl<'a> GraphView<'a> {
    pub fn new(layout: &'a GraphLayout) -> Self {
        Self {
            layout,
            nodes: None,
            offset: (0, 0),
            elapsed_ns: 0,
        }
    }

    pub fn snapshot(mut self, snapshot: &'a StateSnapshot) -> Self {
        self.nodes = Some(&snapshot.nodes);
        self
    }

    pub fn nodes(mut self, nodes: &'a [NodeObservation]) -> Self {
        self.nodes = Some(nodes);
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

    fn edge_style(&self, edge: &GraphEdge) -> Style {
        let default = match edge.kind {
            EdgeKind::Data => Color::Cyan,
            EdgeKind::Control => Color::Blue,
        };
        let Some(source) = self
            .nodes
            .and_then(|nodes| nodes.get(edge.from))
            .filter(|source| source.id == self.layout.nodes[edge.from].id)
        else {
            return Style::default().fg(default);
        };
        if source.possibly_missing_events
            || matches!(source.status, NodeStatus::Unknown | NodeStatus::Interrupted)
        {
            return Style::default().fg(Color::Magenta);
        }
        if source
            .skipped_ports
            .iter()
            .any(|port| port == &edge.from_output)
            || matches!(source.status, NodeStatus::Skipped | NodeStatus::NotRun)
        {
            return Style::default().fg(Color::DarkGray);
        }
        if source
            .produced_ports
            .iter()
            .any(|port| port == &edge.from_output)
        {
            return Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD);
        }
        Style::default().fg(default)
    }

    fn draw_node(&self, area: Rect, buffer: &mut Buffer, index: usize, node: &GraphNode) {
        let rect = node.rect;
        if rect.x >= self.offset.0.saturating_add(u32::from(area.width))
            || rect.x.saturating_add(rect.width) <= self.offset.0
            || rect.y >= self.offset.1.saturating_add(u32::from(area.height))
            || rect.y.saturating_add(rect.height) <= self.offset.1
        {
            return;
        }
        let observed = self
            .nodes
            .and_then(|nodes| nodes.get(index).filter(|item| item.id == node.id));
        let status = observed.map_or(NodeStatus::Pending, |item| item.status);
        let style = node_style(status);
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

        let marker = status_marker(status, observed, self.elapsed_ns);
        let title = format!("{marker} {}", display_line(&node.id));
        let detail = display_line(&node_detail(node, observed, self.elapsed_ns));
        let title_rect = GraphRect {
            x: rect.x + 1,
            y: rect.y + 1,
            width: rect.width - 2,
            height: 1,
        };
        self.text(area, buffer, title_rect, &title, style);
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
    }
}

impl Widget for GraphView<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for index in 0..self.layout.edges.len() {
            self.draw_edge(area, buffer, index);
        }
        for crossing in &self.layout.crossings {
            self.draw_crossing(area, buffer, crossing);
        }
        for (index, node) in self.layout.nodes.iter().enumerate() {
            self.draw_node(area, buffer, index, node);
        }
    }
}

fn node_style(status: NodeStatus) -> Style {
    match status {
        NodeStatus::Pending => Style::default().fg(Color::DarkGray),
        NodeStatus::Running => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        NodeStatus::Succeeded => Style::default().fg(Color::Green),
        NodeStatus::Failed => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        NodeStatus::Skipped | NodeStatus::NotRun => Style::default().fg(Color::DarkGray),
        NodeStatus::Unknown | NodeStatus::Interrupted => Style::default().fg(Color::Magenta),
    }
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
        NodeStatus::NotRun => '–',
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
        NodeStatus::NotRun => "not run",
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

#[cfg(test)]
mod rendering_tests {
    use super::*;
    use routing::{Point, RoutingStats, crossing_cells};

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
                    output: "value".into(),
                    input: Some("input".into()),
                    kind,
                },
                from: index,
                to: index,
                from_output: "value".into(),
                to_input: Some("input".into()),
                kind,
                from_row: 1,
                to_row: 4,
            })
            .collect();
        GraphLayout {
            nodes,
            edges,
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
    fn crossings_preserve_each_strokes_kind_color_and_modifier() {
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
                    for cell in crossing_cells(horizontal, vertical, upper_horizontal) {
                        let rendered = buffer
                            .cell(Position::new((10 + cell.dx) as u16, (10 + cell.dy) as u16))
                            .unwrap();
                        assert_eq!(rendered.symbol(), cell.symbol);
                        assert_eq!(
                            rendered.fg,
                            if cell.upper {
                                Color::Green
                            } else {
                                Color::Magenta
                            }
                        );
                        assert_eq!(rendered.modifier.contains(Modifier::BOLD), cell.upper);
                    }
                }
            }
        }
    }

    #[test]
    fn partially_visible_crossings_match_full_canvas_symbols_and_styles() {
        let layout = crossing_layout(EdgeKind::Data, EdgeKind::Control, false);
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
                        full.cell(Position::new(offset.0 as u16 + x, offset.1 as u16 + y))
                            .unwrap()
                    );
                }
            }
        }
    }
}
