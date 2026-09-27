//! Fixed terminal projection of a described workflow graph.

use crate::state::{MAX_SESSION_NODES, NodeObservation, NodeStatus, StateSnapshot};
use mf_telemetry::{ContractError, description::WorkflowDescription};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use rust_sugiyama::{configure::Config, from_vertices_and_edges};
use snafu::Snafu;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

const NODE_WIDTH: u32 = 24;
const NODE_HEIGHT: u32 = 6;
const COLUMN_GAP: u32 = 10;
const ROW_GAP: u32 = 2;
const COMPONENT_GAP: u32 = 3;
const MARGIN: u32 = 2;

#[derive(Debug, Snafu)]
pub enum GraphError {
    #[snafu(display("invalid workflow description: {source}"))]
    Description { source: ContractError },
    #[snafu(display("workflow graph exceeds the {limit}-node display limit"))]
    TooManyNodes { limit: usize },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Data,
    Control,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphEdge {
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
    width: u32,
    height: u32,
}

impl GraphLayout {
    pub fn new(description: &WorkflowDescription) -> Result<Self, GraphError> {
        description
            .validate()
            .map_err(|source| GraphError::Description { source })?;
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
                from,
                to,
                from_output: edge.from_output.clone(),
                to_input: None,
                kind: EdgeKind::Control,
                from_row: 0,
                to_row: 0,
            });
        }
        let mut outgoing = vec![0u32; nodes.len()];
        let mut incoming = vec![0u32; nodes.len()];
        for edge in &mut edges {
            edge.from_row = 1 + outgoing[edge.from] % (NODE_HEIGHT - 2);
            edge.to_row = 1 + incoming[edge.to] % (NODE_HEIGHT - 2);
            outgoing[edge.from] += 1;
            incoming[edge.to] += 1;
        }

        let vertices: Vec<_> = (0..nodes.len())
            .map(|index| (index as u32, (NODE_HEIGHT as f64, NODE_WIDTH as f64)))
            .collect();
        let topology: Vec<_> = topology.into_iter().collect();
        let mut components = from_vertices_and_edges(
            &vertices,
            &topology,
            &Config {
                vertex_spacing: 2.0,
                transpose: false,
                ..Config::default()
            },
        );
        components.sort_by_key(|(vertices, _, _)| {
            vertices
                .iter()
                .map(|(id, _)| *id)
                .min()
                .unwrap_or(usize::MAX)
        });

        let mut placed = vec![false; nodes.len()];
        let mut component_top = MARGIN;
        let mut right = MARGIN;
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
            let rows = layers.iter().map(Vec::len).max().unwrap_or(0) as u32;
            for (column, layer) in layers.iter_mut().enumerate() {
                layer.sort_by(|left, right| {
                    left.1
                        .total_cmp(&right.1)
                        .then_with(|| left.0.cmp(&right.0))
                });
                let centering = (rows - layer.len() as u32) * (NODE_HEIGHT + ROW_GAP) / 2;
                for (row, (id, _)) in layer.iter().enumerate() {
                    if let Some(node) = nodes.get_mut(*id) {
                        node.rect.x = MARGIN + column as u32 * (NODE_WIDTH + COLUMN_GAP);
                        node.rect.y =
                            component_top + centering + row as u32 * (NODE_HEIGHT + ROW_GAP);
                        right = right.max(node.rect.x + NODE_WIDTH);
                        placed[*id] = true;
                    }
                }
            }
            component_top += rows * (NODE_HEIGHT + ROW_GAP) + COMPONENT_GAP;
        }
        for (index, node) in nodes.iter_mut().enumerate() {
            if !placed[index] {
                node.rect.x = MARGIN;
                node.rect.y = component_top;
                component_top += NODE_HEIGHT + ROW_GAP;
                right = right.max(node.rect.x + NODE_WIDTH);
            }
        }
        let height = nodes
            .iter()
            .map(|node| node.rect.y + NODE_HEIGHT + MARGIN)
            .max()
            .unwrap_or(MARGIN * 2);
        Ok(Self {
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

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

pub struct GraphView<'a> {
    layout: &'a GraphLayout,
    snapshot: Option<&'a StateSnapshot>,
    offset: (u32, u32),
    elapsed_ns: u64,
}

impl<'a> GraphView<'a> {
    pub fn new(layout: &'a GraphLayout) -> Self {
        Self {
            layout,
            snapshot: None,
            offset: (0, 0),
            elapsed_ns: 0,
        }
    }

    pub fn snapshot(mut self, snapshot: &'a StateSnapshot) -> Self {
        self.snapshot = Some(snapshot);
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
            cell.set_symbol(symbol).set_style(style);
        }
    }

    fn draw_edge(&self, area: Rect, buffer: &mut Buffer, edge: &GraphEdge) {
        let source = self.layout.nodes[edge.from].rect;
        let target = self.layout.nodes[edge.to].rect;
        let from_x = source.x + source.width;
        let to_x = target.x.saturating_sub(1);
        if from_x > to_x {
            return;
        }
        let from_y = source.y + edge.from_row;
        let to_y = target.y + edge.to_row;
        let middle = from_x + (to_x - from_x) / 2;
        let (horizontal, vertical, down_start, down_end, up_start, up_end) = match edge.kind {
            EdgeKind::Data => ("═", "║", "╗", "╚", "╝", "╔"),
            EdgeKind::Control => ("─", "│", "┐", "└", "┘", "┌"),
        };
        let style = self.edge_style(edge);
        self.horizontal(area, buffer, from_x..=middle, from_y, horizontal, style);
        if from_y != to_y {
            self.vertical(area, buffer, middle, from_y..=to_y, vertical, style);
        }
        self.horizontal(area, buffer, middle..=to_x, to_y, horizontal, style);
        if from_y < to_y {
            self.put(area, buffer, middle, from_y, down_start, style);
            self.put(area, buffer, middle, to_y, down_end, style);
        } else if from_y > to_y {
            self.put(area, buffer, middle, from_y, up_start, style);
            self.put(area, buffer, middle, to_y, up_end, style);
        }
        self.put(area, buffer, to_x, to_y, "▶", style);
        if edge.kind == EdgeKind::Control {
            let label_x = from_x + 1;
            let available = middle.saturating_sub(label_x);
            if let (Some(local_x), Some(local_y)) = (
                label_x.checked_sub(self.offset.0),
                from_y.checked_sub(self.offset.1),
            ) && local_x < u32::from(area.width)
                && local_y < u32::from(area.height)
            {
                let visible = available.min(u32::from(area.width) - local_x);
                buffer.set_stringn(
                    area.x + local_x as u16,
                    area.y + local_y as u16,
                    display_line(&edge.from_output),
                    visible as usize,
                    style,
                );
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
            .snapshot
            .and_then(|snapshot| snapshot.nodes.get(edge.from))
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
            .snapshot
            .and_then(|snapshot| snapshot.nodes.get(index).filter(|item| item.id == node.id));
        let status = observed.map_or(NodeStatus::Pending, |item| item.status);
        let style = node_style(status);
        for x in rect.x + 1..rect.x + rect.width - 1 {
            self.put(area, buffer, x, rect.y, "─", style);
            self.put(area, buffer, x, rect.y + rect.height - 1, "─", style);
        }
        for y in rect.y + 1..rect.y + rect.height - 1 {
            self.put(area, buffer, rect.x, y, "│", style);
            self.put(area, buffer, rect.x + rect.width - 1, y, "│", style);
        }
        for (x, y, symbol) in [
            (rect.x, rect.y, "┌"),
            (rect.x + rect.width - 1, rect.y, "┐"),
            (rect.x, rect.y + rect.height - 1, "└"),
            (rect.x + rect.width - 1, rect.y + rect.height - 1, "┘"),
        ] {
            self.put(area, buffer, x, y, symbol, style);
        }

        let Some(local_x) = rect.x.checked_sub(self.offset.0) else {
            return;
        };
        let Some(local_y) = rect.y.checked_sub(self.offset.1) else {
            return;
        };
        if local_x + rect.width > u32::from(area.width)
            || local_y + rect.height > u32::from(area.height)
        {
            return;
        }
        let marker = status_marker(status, observed, self.elapsed_ns);
        let title = format!("{marker} {}", display_line(&node.id));
        let detail = display_line(&node_detail(node, observed, self.elapsed_ns));
        let x = area.x + local_x as u16 + 1;
        let y = area.y + local_y as u16 + 1;
        buffer.set_stringn(x, y, title, (NODE_WIDTH - 2) as usize, style);
        buffer.set_stringn(
            x,
            y + 1,
            detail,
            (NODE_WIDTH - 2) as usize,
            Style::default().fg(Color::Gray),
        );
    }
}

impl Widget for GraphView<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for edge in &self.layout.edges {
            self.draw_edge(area, buffer, edge);
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
        |duration| format!("{:.1}s | {}", duration as f64 / 1e9, node.kind),
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
