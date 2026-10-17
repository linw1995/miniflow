//! Integer geometry and ownership for orthogonal terminal routes.

use super::{EdgeKind, GraphRect};
use snafu::Snafu;
use std::collections::{BTreeMap, BTreeSet};

pub const CROSSING_RADIUS: u32 = 1;
pub const CROSSING_CLEARANCE: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: Point,
    pub end: Point,
}

impl Segment {
    pub fn axis(self) -> Axis {
        assert!(self.start.x == self.end.x || self.start.y == self.end.y);
        if self.start.y == self.end.y {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    }

    pub fn low(self) -> Point {
        Point::new(self.start.x.min(self.end.x), self.start.y.min(self.end.y))
    }

    pub fn high(self) -> Point {
        Point::new(self.start.x.max(self.end.x), self.start.y.max(self.end.y))
    }

    pub fn length(self) -> u32 {
        self.start.x.abs_diff(self.end.x) + self.start.y.abs_diff(self.end.y)
    }

    pub fn contains(self, point: Point) -> bool {
        let low = self.low();
        let high = self.high();
        (low.x..=high.x).contains(&point.x) && (low.y..=high.y).contains(&point.y)
    }

    pub fn interior_clearance(self, point: Point) -> u32 {
        if !self.contains(point) {
            return 0;
        }
        match self.axis() {
            Axis::Horizontal => point
                .x
                .abs_diff(self.start.x)
                .min(point.x.abs_diff(self.end.x)),
            Axis::Vertical => point
                .y
                .abs_diff(self.start.y)
                .min(point.y.abs_diff(self.end.y)),
        }
    }

    pub fn intersects_rect(self, rect: GraphRect) -> bool {
        let low = self.low();
        let high = self.high();
        rect.width > 0
            && rect.height > 0
            && low.x < rect.x.saturating_add(rect.width)
            && high.x >= rect.x
            && low.y < rect.y.saturating_add(rect.height)
            && high.y >= rect.y
    }

    pub fn intersection(self, other: Self) -> Option<Intersection> {
        let low = self.low();
        let high = self.high();
        let other_low = other.low();
        let other_high = other.high();
        if self.axis() == other.axis() {
            let (fixed, other_fixed, start, end) = match self.axis() {
                Axis::Horizontal => (
                    low.y,
                    other_low.y,
                    low.x.max(other_low.x),
                    high.x.min(other_high.x),
                ),
                Axis::Vertical => (
                    low.x,
                    other_low.x,
                    low.y.max(other_low.y),
                    high.y.min(other_high.y),
                ),
            };
            if fixed != other_fixed || start > end {
                return None;
            }
            if start < end {
                return Some(Intersection::Overlap);
            }
            let point = match self.axis() {
                Axis::Horizontal => Point::new(start, fixed),
                Axis::Vertical => Point::new(fixed, start),
            };
            return Some(Intersection::Contact(point));
        }
        let (horizontal, vertical) = if self.axis() == Axis::Horizontal {
            (self, other)
        } else {
            (other, self)
        };
        let point = Point::new(vertical.start.x, horizontal.start.y);
        if !self.contains(point) || !other.contains(point) {
            return None;
        }
        Some(
            if self.interior_clearance(point) == 0 || other.interior_clearance(point) == 0 {
                Intersection::Contact(point)
            } else {
                Intersection::Crossing(point)
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intersection {
    Overlap,
    Contact(Point),
    Crossing(Point),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeKey {
    pub source: String,
    pub destination: String,
    pub output: String,
    pub input: Option<String>,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeLabel {
    pub rect: GraphRect,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
pub enum UnresolvedReason {
    #[snafu(display("no safe route within the available channels"))]
    Congestion,
    #[snafu(display("routing search budget exhausted"))]
    SearchLimit,
    #[snafu(display("routing geometry budget exhausted"))]
    GeometryLimit,
    #[snafu(display("routing conflict budget exhausted"))]
    ConflictLimit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeRoute {
    pub points: Vec<Point>,
    pub label: Option<EdgeLabel>,
    pub unresolved: Option<UnresolvedReason>,
}

impl EdgeRoute {
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.points.windows(2).map(|pair| Segment {
            start: pair[0],
            end: pair[1],
        })
    }

    pub fn from_points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut simplified: Vec<Point> = Vec::new();
        for point in points {
            if simplified.last() == Some(&point) {
                continue;
            }
            while simplified.len() >= 2 {
                let last = simplified.len() - 1;
                let a = simplified[last - 1];
                let b = simplified[last];
                if (a.x == b.x
                    && b.x == point.x
                    && (a.y <= b.y && b.y <= point.y || a.y >= b.y && b.y >= point.y))
                    || (a.y == b.y
                        && b.y == point.y
                        && (a.x <= b.x && b.x <= point.x || a.x >= b.x && b.x >= point.x))
                {
                    simplified.pop();
                } else {
                    break;
                }
            }
            simplified.push(point);
        }
        Self {
            points: simplified,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crossing {
    pub point: Point,
    pub horizontal: usize,
    pub vertical: usize,
    pub upper: usize,
}

impl Crossing {
    pub fn footprint(&self) -> GraphRect {
        let upper_horizontal = self.upper == self.horizontal;
        GraphRect {
            x: self
                .point
                .x
                .saturating_sub(if upper_horizontal { 0 } else { CROSSING_RADIUS }),
            y: self
                .point
                .y
                .saturating_sub(if upper_horizontal { CROSSING_RADIUS } else { 0 }),
            width: if upper_horizontal { 1 } else { 3 },
            height: if upper_horizontal { 3 } else { 1 },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerCell {
    pub dx: i32,
    pub dy: i32,
    pub symbol: &'static str,
    pub upper: bool,
}

pub fn line_symbols(kind: EdgeKind) -> [&'static str; 6] {
    match kind {
        EdgeKind::Data | EdgeKind::Control => ["─", "│", "┌", "┐", "└", "┘"],
    }
}

pub fn crossing_cells(
    horizontal: EdgeKind,
    vertical: EdgeKind,
    upper_horizontal: bool,
) -> [MarkerCell; 3] {
    let lower = if upper_horizontal {
        vertical
    } else {
        horizontal
    };
    let line = line_symbols(lower)[usize::from(upper_horizontal)];
    let cell = |along, symbol, upper| MarkerCell {
        dx: if upper == upper_horizontal { along } else { 0 },
        dy: if upper == upper_horizontal { 0 } else { along },
        symbol,
        upper,
    };
    [
        // Only the crossing doubles the upper stroke, keeping both paths continuous.
        cell(0, if upper_horizontal { "╪" } else { "╫" }, true),
        cell(-1, line, false),
        cell(1, line, false),
    ]
}

#[derive(Clone, Copy, Debug)]
pub struct IndexedSegment {
    pub edge: usize,
    pub segment: Segment,
}

#[derive(Clone, Debug)]
struct IntervalNode {
    item: IndexedSegment,
    low: u32,
    high: u32,
    max_high: u32,
    height: i16,
    left: Option<Box<IntervalNode>>,
    right: Option<Box<IntervalNode>>,
}

impl IntervalNode {
    fn key(&self) -> (u32, u32, usize) {
        (self.low, self.high, self.item.edge)
    }
    fn height(node: &Option<Box<Self>>) -> i16 {
        node.as_ref().map_or(0, |node| node.height)
    }
    fn update(&mut self) {
        self.height = 1 + Self::height(&self.left).max(Self::height(&self.right));
        self.max_high = self
            .high
            .max(self.left.as_ref().map_or(0, |node| node.max_high))
            .max(self.right.as_ref().map_or(0, |node| node.max_high));
    }
    fn rotate_left(mut root: Box<Self>) -> Box<Self> {
        let mut next = root.right.take().expect("right child exists");
        root.right = next.left.take();
        root.update();
        next.left = Some(root);
        next.update();
        next
    }
    fn rotate_right(mut root: Box<Self>) -> Box<Self> {
        let mut next = root.left.take().expect("left child exists");
        root.left = next.right.take();
        root.update();
        next.right = Some(root);
        next.update();
        next
    }
    fn insert(root: Option<Box<Self>>, item: IndexedSegment, low: u32, high: u32) -> Box<Self> {
        let Some(mut root) = root else {
            return Box::new(Self {
                item,
                low,
                high,
                max_high: high,
                height: 1,
                left: None,
                right: None,
            });
        };
        let key = (low, high, item.edge);
        match key.cmp(&root.key()) {
            std::cmp::Ordering::Less => {
                root.left = Some(Self::insert(root.left.take(), item, low, high))
            }
            std::cmp::Ordering::Greater => {
                root.right = Some(Self::insert(root.right.take(), item, low, high))
            }
            std::cmp::Ordering::Equal => {
                root.item = item;
                return root;
            }
        }
        root.update();
        if Self::height(&root.left) - Self::height(&root.right) > 1 {
            if key > root.left.as_ref().expect("left child exists").key() {
                root.left = root.left.take().map(Self::rotate_left);
            }
            return Self::rotate_right(root);
        }
        if Self::height(&root.right) - Self::height(&root.left) > 1 {
            if key < root.right.as_ref().expect("right child exists").key() {
                root.right = root.right.take().map(Self::rotate_right);
            }
            return Self::rotate_left(root);
        }
        root
    }
}

#[derive(Clone, Debug, Default)]
struct IntervalTree {
    root: Option<Box<IntervalNode>>,
}

impl IntervalTree {
    fn insert(&mut self, item: IndexedSegment) {
        let (low, high) = match item.segment.axis() {
            Axis::Horizontal => (item.segment.low().x, item.segment.high().x),
            Axis::Vertical => (item.segment.low().y, item.segment.high().y),
        };
        self.insert_range(item, low, high);
    }
    fn insert_range(&mut self, item: IndexedSegment, low: u32, high: u32) {
        self.root = Some(IntervalNode::insert(self.root.take(), item, low, high));
    }
    fn query(&self, low: u32, high: u32) -> IntervalQuery<'_> {
        IntervalQuery {
            stack: self.root.as_deref().into_iter().collect(),
            low,
            high,
        }
    }
    fn remove(&mut self, edges: &BTreeSet<usize>) {
        let mut stack: Vec<_> = self.root.take().into_iter().collect();
        while let Some(mut node) = stack.pop() {
            stack.extend(node.left.take());
            stack.extend(node.right.take());
            if !edges.contains(&node.item.edge) {
                self.insert(node.item);
            }
        }
    }
}

struct IntervalQuery<'a> {
    stack: Vec<&'a IntervalNode>,
    low: u32,
    high: u32,
}

impl<'a> Iterator for IntervalQuery<'a> {
    type Item = &'a IndexedSegment;
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            if node.max_high < self.low {
                continue;
            }
            if node.low <= self.high
                && let Some(right) = node.right.as_deref()
            {
                self.stack.push(right);
            }
            if let Some(left) = node.left.as_deref() {
                self.stack.push(left);
            }
            if node.low <= self.high && node.high >= self.low {
                return Some(&node.item);
            }
        }
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct SegmentIndex {
    horizontal: BTreeMap<u32, IntervalTree>,
    vertical: BTreeMap<u32, IntervalTree>,
}

impl SegmentIndex {
    pub fn insert(&mut self, edge: usize, route: &EdgeRoute) {
        for segment in route.segments() {
            let (index, fixed) = match segment.axis() {
                Axis::Horizontal => (&mut self.horizontal, segment.start.y),
                Axis::Vertical => (&mut self.vertical, segment.start.x),
            };
            index
                .entry(fixed)
                .or_default()
                .insert(IndexedSegment { edge, segment });
        }
    }
    pub fn remove(&mut self, edges: &BTreeSet<usize>) {
        for index in [&mut self.horizontal, &mut self.vertical] {
            index.retain(|_, tree| {
                tree.remove(edges);
                tree.root.is_some()
            });
        }
    }
    pub fn candidates(&self, segment: Segment) -> impl Iterator<Item = &IndexedSegment> {
        let low = segment.low();
        let high = segment.high();
        let (same, opposite, fixed, first, last) = match segment.axis() {
            Axis::Horizontal => (&self.horizontal, &self.vertical, low.y, low.x, high.x),
            Axis::Vertical => (&self.vertical, &self.horizontal, low.x, low.y, high.y),
        };
        same.get(&fixed)
            .into_iter()
            .flat_map(move |tree| tree.query(first, last))
            .chain(
                opposite
                    .range(first..=last)
                    .flat_map(move |(_, tree)| tree.query(fixed, fixed)),
            )
    }
}

pub const ATTACHMENT_SPACING: u32 = 4;
pub const TRACK_SPACING: u32 = 4;
pub const LABEL_WIDTH: usize = 12;

pub fn label_text(text: &str) -> String {
    let clean = super::display_line(text);
    let span = ratatui::text::Span::raw(clean);
    let mut result = String::new();
    let mut width = 0;
    for grapheme in span.styled_graphemes(ratatui::style::Style::default()) {
        let next_width = ratatui::text::Span::raw(grapheme.symbol).width();
        if width + next_width > LABEL_WIDTH {
            break;
        }
        result.push_str(grapheme.symbol);
        width += next_width;
    }
    result
}

#[derive(Clone, Copy)]
struct TrackRequest {
    edge: usize,
    target: bool,
    low: u32,
    high: u32,
    start_y: u32,
    end_y: u32,
}

pub fn channel_routes(
    nodes: &mut [super::GraphNode],
    edges: &[super::GraphEdge],
    components: &[usize],
    component_bottoms: &mut [u32],
) -> Vec<EdgeRoute> {
    let columns: Vec<_> = nodes
        .iter()
        .map(|node| (node.rect.x - super::MARGIN) / (super::NODE_WIDTH + super::COLUMN_GAP))
        .collect();
    let column_count = columns.iter().copied().max().map_or(0, |last| last + 1) as usize;
    let mut order: Vec<_> = (0..edges.len()).collect();
    order.sort_by_key(|&index| &edges[index].key);
    let mut pass_rows = vec![None; edges.len()];
    let mut extra = vec![0; component_bottoms.len()];
    for &index in &order {
        let edge = &edges[index];
        if columns[edge.to] > columns[edge.from] + 1 {
            let component = components[edge.from];
            pass_rows[index] = Some(component_bottoms[component] + 4 + extra[component]);
            extra[component] += TRACK_SPACING;
        }
    }
    // Reserve passages before moving later components, including isolated nodes.
    let mut offsets = Vec::with_capacity(extra.len());
    let mut cumulative = 0;
    for &height in &extra {
        offsets.push(cumulative);
        cumulative += height;
    }
    for (index, node) in nodes.iter_mut().enumerate() {
        node.rect.y += offsets[components[index]];
    }
    for (index, row) in pass_rows.iter_mut().enumerate() {
        if let Some(row) = row {
            *row += offsets[components[edges[index].from]];
        }
    }
    let mut shift = 0;
    for (component, bottom) in component_bottoms.iter_mut().enumerate() {
        *bottom += shift + extra[component];
        shift += extra[component];
    }
    let mut channels: BTreeMap<(usize, u32), Vec<TrackRequest>> = BTreeMap::new();
    let mut stubs = vec![2u32; column_count];
    for &index in &order {
        let edge = &edges[index];
        let from_y = nodes[edge.from].rect.y + edge.from_row;
        let to_y = nodes[edge.to].rect.y + edge.to_row;
        let source_column = columns[edge.from];
        if edge.kind == EdgeKind::Control {
            stubs[source_column as usize] = stubs[source_column as usize]
                .max(ratatui::text::Span::raw(label_text(&edge.from_output)).width() as u32 + 2);
        }
        let component = components[edge.from];
        let end = pass_rows[index].unwrap_or(to_y);
        channels
            .entry((component, source_column))
            .or_default()
            .push(TrackRequest {
                edge: index,
                target: false,
                low: from_y.min(end),
                high: from_y.max(end),
                start_y: from_y,
                end_y: end,
            });
        if let Some(pass) = pass_rows[index] {
            channels
                .entry((component, columns[edge.to] - 1))
                .or_default()
                .push(TrackRequest {
                    edge: index,
                    target: true,
                    low: pass.min(to_y),
                    high: pass.max(to_y),
                    start_y: pass,
                    end_y: to_y,
                });
        }
    }
    let mut source_tracks = vec![0; edges.len()];
    let mut target_tracks = vec![0; edges.len()];
    let mut capacities = vec![0u32; column_count];
    for ((_, column), requests) in &mut channels {
        requests.sort_by(|a, b| {
            (a.low, a.high, &edges[a.edge].key, a.target).cmp(&(
                b.low,
                b.high,
                &edges[b.edge].key,
                b.target,
            ))
        });
        let mut count = 0;
        let mut first = 0;
        while first < requests.len() {
            let mut last = first + 1;
            let mut end = requests[first].high;
            while last < requests.len()
                && requests[last].low <= end.saturating_add(CROSSING_CLEARANCE)
            {
                end = end.max(requests[last].high);
                last += 1;
            }
            // Reuse tracks between disjoint regions, preserving order inside each region.
            requests[first..last].sort_by_key(|request| {
                if request.start_y >= request.end_y {
                    (0, request.start_y, request.end_y, &edges[request.edge].key)
                } else {
                    (
                        1,
                        u32::MAX - request.start_y,
                        u32::MAX - request.end_y,
                        &edges[request.edge].key,
                    )
                }
            });
            for (track, request) in requests[first..last].iter().enumerate() {
                if request.target {
                    target_tracks[request.edge] = track as u32;
                } else {
                    source_tracks[request.edge] = track as u32;
                }
            }
            count = count.max((last - first) as u32);
            first = last;
        }
        capacities[*column as usize] = capacities[*column as usize].max(count);
    }
    let mut column_x = vec![super::MARGIN; column_count];
    for column in 1..column_count {
        let gap = (stubs[column - 1] + capacities[column - 1] * TRACK_SPACING + 4).max(10);
        column_x[column] = column_x[column - 1] + super::NODE_WIDTH + gap;
    }
    for (node, column) in nodes.iter_mut().zip(&columns) {
        node.rect.x = column_x[*column as usize];
    }
    edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            let source = nodes[edge.from].rect;
            let target = nodes[edge.to].rect;
            let from = Point::new(source.x + source.width, source.y + edge.from_row);
            let to = Point::new(target.x - 1, target.y + edge.to_row);
            let first_x =
                from.x + stubs[columns[edge.from] as usize] + source_tracks[index] * TRACK_SPACING;
            let mut route = if let Some(pass) = pass_rows[index] {
                let last_column = columns[edge.to] - 1;
                let last_x = column_x[last_column as usize]
                    + 24
                    + stubs[last_column as usize]
                    + target_tracks[index] * TRACK_SPACING;
                EdgeRoute::from_points([
                    from,
                    Point::new(first_x, from.y),
                    Point::new(first_x, pass),
                    Point::new(last_x, pass),
                    Point::new(last_x, to.y),
                    to,
                ])
            } else {
                EdgeRoute::from_points([
                    from,
                    Point::new(first_x, from.y),
                    Point::new(first_x, to.y),
                    to,
                ])
            };
            if edge.kind == EdgeKind::Control {
                let text = label_text(&edge.from_output);
                let width = ratatui::text::Span::raw(text.as_str()).width() as u32;
                route.label = Some(EdgeLabel {
                    rect: GraphRect {
                        x: from.x + 1,
                        y: from.y,
                        width,
                        height: 1,
                    },
                    text,
                });
            }
            route
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct ObstacleIndex {
    by_x: BTreeMap<u32, IntervalTree>,
    items: Vec<(Option<usize>, GraphRect)>,
    max_width: u32,
}

impl ObstacleIndex {
    pub fn insert(&mut self, owner: Option<usize>, rect: GraphRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.max_width = self.max_width.max(rect.width);
        let edge = self.items.len();
        self.items.push((owner, rect));
        self.by_x.entry(rect.x).or_default().insert_range(
            IndexedSegment {
                edge,
                segment: Segment {
                    start: Point::new(rect.x, rect.y),
                    end: Point::new(rect.x, rect.y.saturating_add(rect.height - 1)),
                },
            },
            rect.y,
            rect.y.saturating_add(rect.height - 1),
        );
    }

    fn rect_candidates(
        &self,
        rect: GraphRect,
    ) -> impl Iterator<Item = &(Option<usize>, GraphRect)> {
        self.by_x
            .range(rect.x.saturating_sub(self.max_width)..=rect.x.saturating_add(rect.width))
            .flat_map(move |(_, tree)| {
                tree.query(rect.y, rect.y.saturating_add(rect.height.saturating_sub(1)))
            })
            .map(|item| &self.items[item.edge])
    }

    pub fn candidates(
        &self,
        segment: Segment,
    ) -> impl Iterator<Item = &(Option<usize>, GraphRect)> {
        let low = segment.low();
        let high = segment.high();
        self.by_x
            .range(low.x.saturating_sub(self.max_width)..=high.x)
            .flat_map(move |(_, tree)| tree.query(low.y, high.y))
            .map(|item| &self.items[item.edge])
    }
}

pub fn rects_intersect(a: GraphRect, b: GraphRect) -> bool {
    a.width > 0
        && a.height > 0
        && b.width > 0
        && b.height > 0
        && a.x < b.x.saturating_add(b.width)
        && b.x < a.x.saturating_add(a.width)
        && a.y < b.y.saturating_add(b.height)
        && b.y < a.y.saturating_add(a.height)
}

impl Router<'_> {
    fn validate(
        &mut self,
        edge: usize,
        route: &EdgeRoute,
    ) -> Result<Vec<Crossing>, UnresolvedReason> {
        let definition = &self.edges[edge];
        let source = self.nodes[definition.from].rect;
        let target = self.nodes[definition.to].rect;
        if route.points.first()
            != Some(&Point::new(
                source.x + source.width,
                source.y + definition.from_row,
            ))
            || route.points.last() != Some(&Point::new(target.x - 1, target.y + definition.to_row))
        {
            return CongestionSnafu.fail();
        }
        if route.points.len() < 2 {
            return CongestionSnafu.fail();
        }
        let from = route.points[0];
        let after = route.points[1];
        let to = route.points[route.points.len() - 1];
        let before = route.points[route.points.len() - 2];
        if after.y != from.y || after.x <= from.x || before.y != to.y || before.x >= to.x {
            return CongestionSnafu.fail();
        }
        let edges = self.edges;
        let obstacles = &self.obstacles;
        let index = &self.index;
        let markers = &self.markers;
        let budget = &mut self.budget;
        if route.points.len() > self.limits.max_route_points
            || route
                .points
                .iter()
                .any(|point| point.x > self.limits.max_extent || point.y > self.limits.max_extent)
        {
            return GeometryLimitSnafu.fail();
        }
        let mut crossings: Vec<Crossing> = Vec::new();
        let segments: Vec<_> = route.segments().collect();
        for (position, segment) in segments.iter().copied().enumerate() {
            if segments
                .get(position + 1)
                .is_some_and(|next| next.axis() == segment.axis())
            {
                return CongestionSnafu.fail();
            }
            for &(owner, rect) in obstacles.candidates(segment) {
                budget.check()?;
                if owner != Some(edge) && segment.intersects_rect(rect) {
                    return CongestionSnafu.fail();
                }
            }
            for other in segments.iter().skip(position + 2) {
                budget.check()?;
                if segment.intersection(*other).is_some() {
                    return CongestionSnafu.fail();
                }
            }
            for other in index.candidates(segment) {
                budget.check()?;
                if other.edge == edge {
                    continue;
                }
                match segment.intersection(other.segment) {
                    None => {}
                    Some(Intersection::Overlap | Intersection::Contact(_)) => {
                        return CongestionSnafu.fail();
                    }
                    Some(Intersection::Crossing(point)) => {
                        if segment.interior_clearance(point) < CROSSING_CLEARANCE
                            || other.segment.interior_clearance(point) < CROSSING_CLEARANCE
                        {
                            return CongestionSnafu.fail();
                        }
                        let horizontal = if segment.axis() == Axis::Horizontal {
                            edge
                        } else {
                            other.edge
                        };
                        let vertical = if segment.axis() == Axis::Vertical {
                            edge
                        } else {
                            other.edge
                        };
                        let upper = if (edges[edge].kind, &edges[edge].key)
                            < (edges[other.edge].kind, &edges[other.edge].key)
                        {
                            edge
                        } else {
                            other.edge
                        };
                        let crossing = Crossing {
                            point,
                            horizontal,
                            vertical,
                            upper,
                        };
                        let footprint = crossing.footprint();
                        for &(_, rect) in obstacles
                            .rect_candidates(footprint)
                            .chain(markers.rect_candidates(footprint))
                        {
                            budget.check()?;
                            if rects_intersect(footprint, rect) {
                                return CongestionSnafu.fail();
                            }
                        }
                        for old in &crossings {
                            budget.check()?;
                            if rects_intersect(old.footprint(), footprint) {
                                return CongestionSnafu.fail();
                            }
                        }
                        for candidate in index.candidates(Segment {
                            start: Point::new(footprint.x, footprint.y),
                            end: Point::new(
                                footprint.x + footprint.width - 1,
                                footprint.y + footprint.height - 1,
                            ),
                        }) {
                            budget.check()?;
                            if candidate.edge != other.edge
                                && candidate.segment.intersects_rect(footprint)
                            {
                                return CongestionSnafu.fail();
                            }
                        }
                        if self.crossings.len() + crossings.len() >= self.limits.max_crossings {
                            return ConflictLimitSnafu.fail();
                        }
                        crossings.push(crossing);
                    }
                }
            }
            for &(_, rect) in markers.candidates(segment) {
                budget.check()?;
                if segment.intersects_rect(rect) {
                    return CongestionSnafu.fail();
                }
            }
        }
        for crossing in &crossings {
            if segments
                .iter()
                .filter(|segment| segment.intersects_rect(crossing.footprint()))
                .count()
                != 1
            {
                return CongestionSnafu.fail();
            }
        }
        Ok(crossings)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RoutingLimits {
    pub max_geometry_items: usize,
    pub max_search_vertices: usize,
    pub max_search_arcs: usize,
    pub max_search_expansions: usize,
    pub max_total_expansions: usize,
    pub max_conflict_checks: usize,
    pub max_crossings: usize,
    pub max_route_points: usize,
    pub retry_rounds: usize,
    pub max_extent: u32,
}

impl Default for RoutingLimits {
    fn default() -> Self {
        Self {
            max_geometry_items: 250_000,
            max_search_vertices: 4_096,
            max_search_arcs: 16_384,
            max_search_expansions: 8_192,
            max_total_expansions: 65_536,
            max_conflict_checks: 2_000_000,
            max_crossings: 16_384,
            max_route_points: 128,
            retry_rounds: 2,
            max_extent: 2_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoutingStats {
    pub geometry_items: usize,
    pub search_expansions: usize,
    pub conflict_checks: usize,
    pub retry_rounds: usize,
}

struct WorkBudget {
    limits: RoutingLimits,
    stats: RoutingStats,
}

impl WorkBudget {
    fn check(&mut self) -> Result<(), UnresolvedReason> {
        if self.stats.conflict_checks >= self.limits.max_conflict_checks {
            return ConflictLimitSnafu.fail();
        }
        self.stats.conflict_checks += 1;
        Ok(())
    }

    fn geometry(&mut self, items: usize) -> Result<(), UnresolvedReason> {
        if items
            > self
                .limits
                .max_geometry_items
                .saturating_sub(self.stats.geometry_items)
        {
            return GeometryLimitSnafu.fail();
        }
        self.stats.geometry_items += items;
        Ok(())
    }

    fn expansion(&mut self, local: usize) -> Result<(), UnresolvedReason> {
        if local >= self.limits.max_search_expansions
            || self.stats.search_expansions >= self.limits.max_total_expansions
        {
            return SearchLimitSnafu.fail();
        }
        self.stats.search_expansions += 1;
        Ok(())
    }
}

struct Router<'a> {
    nodes: &'a [super::GraphNode],
    edges: &'a [super::GraphEdge],
    limits: RoutingLimits,
    obstacles: ObstacleIndex,
    index: SegmentIndex,
    markers: ObstacleIndex,
    crossings: Vec<Crossing>,
    budget: WorkBudget,
}

impl<'a> Router<'a> {
    fn new(
        nodes: &'a [super::GraphNode],
        edges: &'a [super::GraphEdge],
        routes: &[EdgeRoute],
        limits: RoutingLimits,
    ) -> Self {
        let mut obstacles = ObstacleIndex::default();
        for node in nodes {
            obstacles.insert(None, node.rect);
        }
        for (edge, route) in routes.iter().enumerate() {
            if let Some(label) = &route.label {
                obstacles.insert(Some(edge), label.rect);
            }
            let target = nodes[edges[edge].to].rect;
            obstacles.insert(
                Some(edge),
                GraphRect {
                    x: target.x - 1,
                    y: target.y + edges[edge].to_row,
                    width: 1,
                    height: 1,
                },
            );
        }
        Self {
            nodes,
            edges,
            limits,
            obstacles,
            index: SegmentIndex::default(),
            markers: ObstacleIndex::default(),
            crossings: Vec::new(),
            budget: WorkBudget {
                limits,
                stats: RoutingStats::default(),
            },
        }
    }

    fn insert(&mut self, edge: usize, route: &EdgeRoute, crossings: Vec<Crossing>) {
        self.index.insert(edge, route);
        for crossing in crossings {
            self.markers.insert(None, crossing.footprint());
            self.crossings.push(crossing);
        }
    }

    fn remove(&mut self, edges: &BTreeSet<usize>) {
        self.index.remove(edges);
        self.crossings.retain(|crossing| {
            !edges.contains(&crossing.horizontal) && !edges.contains(&crossing.vertical)
        });
        self.markers = ObstacleIndex::default();
        for crossing in &self.crossings {
            self.markers.insert(None, crossing.footprint());
        }
    }

    fn step_cost(
        &mut self,
        edge: usize,
        segment: Segment,
    ) -> Result<Option<u64>, UnresolvedReason> {
        for &(owner, rect) in self
            .obstacles
            .candidates(segment)
            .chain(self.markers.candidates(segment))
        {
            self.budget.check()?;
            if owner != Some(edge) && segment.intersects_rect(rect) {
                return Ok(None);
            }
        }
        let mut crossings = 0;
        for item in self.index.candidates(segment) {
            self.budget.check()?;
            match segment.intersection(item.segment) {
                None => {}
                Some(Intersection::Overlap | Intersection::Contact(_)) => return Ok(None),
                Some(Intersection::Crossing(point)) => {
                    // The search edge may end before its final collinear route is simplified.
                    if item.segment.interior_clearance(point) < CROSSING_CLEARANCE {
                        return Ok(None);
                    }
                    let marker = Crossing {
                        point,
                        horizontal: if segment.axis() == Axis::Horizontal {
                            edge
                        } else {
                            item.edge
                        },
                        vertical: if segment.axis() == Axis::Vertical {
                            edge
                        } else {
                            item.edge
                        },
                        upper: if (self.edges[edge].kind, &self.edges[edge].key)
                            < (self.edges[item.edge].kind, &self.edges[item.edge].key)
                        {
                            edge
                        } else {
                            item.edge
                        },
                    }
                    .footprint();
                    for &(_, rect) in self
                        .obstacles
                        .rect_candidates(marker)
                        .chain(self.markers.rect_candidates(marker))
                    {
                        self.budget.check()?;
                        if rects_intersect(marker, rect) {
                            return Ok(None);
                        }
                    }
                    crossings += 1;
                }
            }
        }
        Ok(Some(u64::from(segment.length()) + 40 * crossings))
    }

    fn outer_detour(
        &mut self,
        edge: usize,
        preferred: &EdgeRoute,
        bounds: GraphRect,
    ) -> Result<EdgeRoute, UnresolvedReason> {
        let definition = &self.edges[edge];
        let source = self.nodes[definition.from].rect;
        let target = self.nodes[definition.to].rect;
        let from = Point::new(source.x + source.width, source.y + definition.from_row);
        let to = Point::new(target.x - 1, target.y + definition.to_row);
        let stub = preferred
            .label
            .as_ref()
            .map_or(4, |label| label.rect.width + 4);
        let mut failure = UnresolvedReason::Congestion;
        for offset in [0, 4, 8] {
            for y in [bounds.y, bounds.y.saturating_add(bounds.height)] {
                self.budget.geometry(6)?;
                let first = from.x.saturating_add(stub + offset);
                let last = to.x.saturating_sub(4 + offset);
                if first >= last
                    || y > self.limits.max_extent
                    || to.x > self.limits.max_extent
                    || first > self.limits.max_extent
                {
                    continue;
                }
                let mut route = EdgeRoute::from_points([
                    from,
                    Point::new(first, from.y),
                    Point::new(first, y),
                    Point::new(last, y),
                    Point::new(last, to.y),
                    to,
                ]);
                route.label = preferred.label.clone();
                match self.validate(edge, &route) {
                    Ok(_) => return Ok(route),
                    Err(reason) => {
                        failure = reason;
                        if reason != UnresolvedReason::Congestion {
                            return Err(reason);
                        }
                    }
                }
            }
        }
        Err(failure)
    }

    fn recover(
        &mut self,
        edge: usize,
        preferred: &EdgeRoute,
        members: &[usize],
        bounds: GraphRect,
    ) -> Result<EdgeRoute, UnresolvedReason> {
        match self.search(edge, preferred, members, bounds) {
            Ok(route) => Ok(route),
            Err(search_reason) => match self.outer_detour(edge, preferred, bounds) {
                Ok(route) => Ok(route),
                Err(UnresolvedReason::Congestion) => Err(search_reason),
                Err(reason) => Err(reason),
            },
        }
    }

    fn search(
        &mut self,
        edge: usize,
        preferred: &EdgeRoute,
        members: &[usize],
        bounds: GraphRect,
    ) -> Result<EdgeRoute, UnresolvedReason> {
        use std::{cmp::Reverse, collections::BinaryHeap};
        let definition = &self.edges[edge];
        let source = self.nodes[definition.from].rect;
        let target = self.nodes[definition.to].rect;
        let from = Point::new(source.x + source.width, source.y + definition.from_row);
        let to = Point::new(target.x - 1, target.y + definition.to_row);
        let stub = preferred
            .label
            .as_ref()
            .map_or(4, |label| label.rect.width + 4);
        if from.x > self.limits.max_extent
            || to.x > self.limits.max_extent
            || from.y > self.limits.max_extent
            || to.y > self.limits.max_extent
        {
            return GeometryLimitSnafu.fail();
        }
        let start = Point::new(from.x + stub, from.y);
        if start.x >= to.x {
            return CongestionSnafu.fail();
        }
        if self
            .step_cost(
                edge,
                Segment {
                    start: from,
                    end: start,
                },
            )?
            .is_none()
        {
            return CongestionSnafu.fail();
        }
        let max_x = bounds
            .x
            .saturating_add(bounds.width)
            .min(self.limits.max_extent);
        let max_y = bounds
            .y
            .saturating_add(bounds.height)
            .min(self.limits.max_extent);
        if start.x > max_x
            || to.x > max_x
            || from.y > max_y
            || to.y > max_y
            || bounds.x > max_x
            || bounds.y > max_y
        {
            return GeometryLimitSnafu.fail();
        }
        let mut occupied_x = BTreeSet::new();
        let mut occupied_y = BTreeSet::new();
        let mut xs = BTreeSet::from([start.x, to.x, bounds.x, max_x]);
        let mut ys = BTreeSet::from([from.y, to.y, bounds.y, max_y]);
        for &member in members {
            self.budget.geometry(4)?;
            let rect = self.nodes[member].rect;
            xs.extend([rect.x.saturating_sub(4), rect.x + rect.width + 4]);
            ys.extend([rect.y.saturating_sub(4), rect.y + rect.height + 4]);
        }
        for point in &preferred.points {
            xs.insert(point.x);
            ys.insert(point.y);
        }
        for (&x, items) in self.index.vertical.range(bounds.x..=max_x) {
            self.budget.geometry(3)?;
            if items.query(bounds.y, max_y).next().is_some() {
                occupied_x.insert(x);
                xs.extend([x.saturating_sub(4), x.saturating_add(4)]);
            }
        }
        for (&y, items) in self.index.horizontal.range(bounds.y..=max_y) {
            self.budget.geometry(3)?;
            if items.query(bounds.x, max_x).next().is_some() {
                occupied_y.insert(y);
                ys.extend([y.saturating_sub(4), y.saturating_add(4)]);
            }
        }
        let xs: Vec<_> = xs
            .into_iter()
            .filter(|&x| {
                x >= bounds.x
                    && x <= max_x
                    && (x == start.x || x == to.x || !occupied_x.contains(&x))
            })
            .collect();
        let ys: Vec<_> = ys
            .into_iter()
            .filter(|&y| {
                y >= bounds.y
                    && y <= max_y
                    && (y == from.y || y == to.y || !occupied_y.contains(&y))
            })
            .collect();
        let vertices = xs
            .len()
            .checked_mul(ys.len())
            .ok_or_else(|| GeometryLimitSnafu.build())?;
        if vertices > self.limits.max_search_vertices
            || vertices.saturating_mul(4) > self.limits.max_search_arcs
        {
            return GeometryLimitSnafu.fail();
        }
        self.budget.geometry(vertices)?;
        let Ok(start_x) = xs.binary_search(&start.x) else {
            return GeometryLimitSnafu.fail();
        };
        let Ok(start_y) = ys.binary_search(&start.y) else {
            return GeometryLimitSnafu.fail();
        };
        let Ok(goal_x) = xs.binary_search(&to.x) else {
            return GeometryLimitSnafu.fail();
        };
        let Ok(goal_y) = ys.binary_search(&to.y) else {
            return GeometryLimitSnafu.fail();
        };
        let encode = |x: usize, y: usize, axis: usize| (y * xs.len() + x) * 2 + axis;
        let decode = |state: usize| ((state / 2) % xs.len(), (state / 2) / xs.len(), state % 2);
        let start_state = encode(start_x, start_y, 0);
        let goal_state = encode(goal_x, goal_y, 0);
        let mut distances = vec![u64::MAX; vertices * 2];
        let mut previous = vec![usize::MAX; vertices * 2];
        distances[start_state] = 0;
        let mut heap = BinaryHeap::from([Reverse((
            u64::from(start.x.abs_diff(to.x) + start.y.abs_diff(to.y)),
            0u64,
            start_state,
        ))]);
        let mut expansions = 0;
        while let Some(Reverse((_, cost, state))) = heap.pop() {
            if cost != distances[state] {
                continue;
            }
            self.budget.expansion(expansions)?;
            expansions += 1;
            if state == goal_state {
                let mut points = Vec::new();
                let mut current = state;
                loop {
                    let (x, y, _) = decode(current);
                    points.push(Point::new(xs[x], ys[y]));
                    if current == start_state {
                        break;
                    }
                    current = previous[current];
                }
                points.reverse();
                points.insert(0, from);
                let mut route = EdgeRoute::from_points(points);
                route.label = preferred.label.clone();
                self.validate(edge, &route)?;
                return Ok(route);
            }
            let (x, y, axis) = decode(state);
            let point = Point::new(xs[x], ys[y]);
            let neighbors = [
                (x.checked_sub(1), Some(y), 0),
                (x.checked_add(1).filter(|&next| next < xs.len()), Some(y), 0),
                (Some(x), y.checked_sub(1), 1),
                (Some(x), y.checked_add(1).filter(|&next| next < ys.len()), 1),
            ];
            for (next_x, next_y, next_axis) in neighbors {
                let (Some(next_x), Some(next_y)) = (next_x, next_y) else {
                    continue;
                };
                let next = Point::new(xs[next_x], ys[next_y]);
                if next == to && (next_axis != 0 || point.x >= to.x) {
                    continue;
                }
                let Some(step) = self.step_cost(
                    edge,
                    Segment {
                        start: point,
                        end: next,
                    },
                )?
                else {
                    continue;
                };
                let next_cost = cost + step + if axis == next_axis { 0 } else { 12 };
                let next_state = encode(next_x, next_y, next_axis);
                if next_cost < distances[next_state] {
                    distances[next_state] = next_cost;
                    previous[next_state] = state;
                    let heuristic = u64::from(next.x.abs_diff(to.x) + next.y.abs_diff(to.y));
                    heap.push(Reverse((next_cost + heuristic, next_cost, next_state)));
                }
            }
        }
        CongestionSnafu.fail()
    }
}

fn route_score(routes: &[EdgeRoute], crossings: &[Crossing]) -> (usize, usize, usize, u64) {
    (
        routes
            .iter()
            .filter(|route| route.unresolved.is_some())
            .count(),
        crossings.len(),
        routes
            .iter()
            .map(|route| route.points.len().saturating_sub(2))
            .sum(),
        routes
            .iter()
            .flat_map(EdgeRoute::segments)
            .map(|segment| u64::from(segment.length()))
            .sum(),
    )
}

pub fn route_all(
    nodes: &[super::GraphNode],
    edges: &[super::GraphEdge],
    routes: &mut [EdgeRoute],
    components: &[usize],
    limits: RoutingLimits,
) -> (Vec<Crossing>, RoutingStats) {
    let estimate = nodes
        .len()
        .saturating_mul(4)
        .saturating_add(edges.len().saturating_mul(10));
    if estimate > limits.max_geometry_items {
        for route in routes {
            route.points.clear();
            route.unresolved = Some(UnresolvedReason::GeometryLimit);
        }
        return (Vec::new(), RoutingStats::default());
    }
    let preferred = routes.to_vec();
    let mut router = Router::new(nodes, edges, routes, limits);
    router.budget.stats.geometry_items = estimate;
    let mut order: Vec<_> = (0..edges.len()).collect();
    // Route producers before downstream nodes so opaque IDs do not consume upstream channels first.
    order.sort_by_key(|&edge| (edges[edge].from, &edges[edge].key));
    let mut members = BTreeMap::<usize, Vec<usize>>::new();
    for (node, &component) in components.iter().enumerate() {
        members.entry(component).or_default().push(node);
    }
    let mut bounds = BTreeMap::new();
    for (&component, nodes_in_scope) in &members {
        let left = nodes_in_scope
            .iter()
            .map(|&index| nodes[index].rect.x)
            .min()
            .unwrap_or(2)
            .saturating_sub(2);
        let top = nodes_in_scope
            .iter()
            .map(|&index| nodes[index].rect.y)
            .min()
            .unwrap_or(2)
            .saturating_sub(2);
        let right = nodes_in_scope
            .iter()
            .map(|&index| nodes[index].rect.x + nodes[index].rect.width + 4)
            .max()
            .unwrap_or(4);
        let bottom = nodes_in_scope
            .iter()
            .map(|&index| nodes[index].rect.y + nodes[index].rect.height + 4)
            .max()
            .unwrap_or(4);
        bounds.insert(
            component,
            GraphRect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
        );
    }
    for (edge, route) in preferred.iter().enumerate() {
        let rect = bounds
            .get_mut(&components[edges[edge].from])
            .expect("component exists");
        for point in &route.points {
            rect.width = rect
                .width
                .max(point.x.saturating_add(4).saturating_sub(rect.x));
            rect.height = rect
                .height
                .max(point.y.saturating_add(4).saturating_sub(rect.y));
        }
    }
    for &edge in &order {
        let result = if routes[edge]
            .points
            .iter()
            .any(|point| point.x > limits.max_extent || point.y > limits.max_extent)
        {
            Err(UnresolvedReason::GeometryLimit)
        } else {
            router.validate(edge, &routes[edge])
        };
        match result {
            Ok(new) => router.insert(edge, &routes[edge], new),
            Err(reason) => {
                routes[edge].points.clear();
                routes[edge].unresolved = Some(reason);
            }
        }
    }
    for &edge in &order {
        if routes[edge].unresolved.is_none() {
            continue;
        }
        let component = components[edges[edge].from];
        match router.recover(
            edge,
            &preferred[edge],
            &members[&component],
            bounds[&component],
        ) {
            Ok(route) => match router.validate(edge, &route) {
                Ok(crossings) => {
                    router.insert(edge, &route, crossings);
                    routes[edge] = route;
                }
                Err(reason) => routes[edge].unresolved = Some(reason),
            },
            Err(reason) => routes[edge].unresolved = Some(reason),
        }
    }
    let mut best_routes = routes.to_vec();
    let mut best_crossings = router.crossings.clone();
    for round in 0..limits.retry_rounds {
        if routes.iter().all(|route| route.unresolved.is_none()) {
            break;
        }
        if router.budget.stats.conflict_checks >= limits.max_conflict_checks
            || router.budget.stats.search_expansions >= limits.max_total_expansions
        {
            break;
        }
        router.budget.stats.retry_rounds = round + 1;
        let mut affected = BTreeSet::new();
        for &edge in &order {
            if routes[edge].unresolved.is_none() {
                continue;
            }
            affected.insert(edge);
            for segment in preferred[edge].segments() {
                for item in router.index.candidates(segment) {
                    if router.budget.check().is_err() {
                        break;
                    }
                    if segment.intersection(item.segment).is_some() {
                        affected.insert(item.edge);
                    }
                    if affected.len() >= 16 {
                        break;
                    }
                }
                if affected.len() >= 16 {
                    break;
                }
            }
            if affected.len() >= 16 {
                break;
            }
        }
        if router
            .budget
            .geometry(router.crossings.len().saturating_add(affected.len()))
            .is_err()
        {
            break;
        }
        router.remove(&affected);
        let mut retry_order: Vec<_> = order
            .iter()
            .copied()
            .filter(|edge| affected.contains(edge))
            .collect();
        retry_order.sort_by_key(|&edge| (routes[edge].unresolved.is_none(), &edges[edge].key));
        for edge in retry_order {
            routes[edge].points.clear();
            routes[edge].unresolved = Some(UnresolvedReason::Congestion);
            let component = components[edges[edge].from];
            let route = router
                .validate(edge, &preferred[edge])
                .map(|_| preferred[edge].clone())
                .or_else(|_| {
                    router.recover(
                        edge,
                        &preferred[edge],
                        &members[&component],
                        bounds[&component],
                    )
                });
            match route {
                Ok(route) => match router.validate(edge, &route) {
                    Ok(crossings) => {
                        router.insert(edge, &route, crossings);
                        routes[edge] = route;
                    }
                    Err(reason) => routes[edge].unresolved = Some(reason),
                },
                Err(reason) => routes[edge].unresolved = Some(reason),
            }
        }
        if route_score(routes, &router.crossings) < route_score(&best_routes, &best_crossings) {
            best_routes = routes.to_vec();
            best_crossings = router.crossings.clone();
        }
    }
    routes.clone_from_slice(&best_routes);
    (best_crossings, router.budget.stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(x: u32, y: u32, to_x: u32, to_y: u32) -> Segment {
        Segment {
            start: Point::new(x, y),
            end: Point::new(to_x, to_y),
        }
    }

    fn search_fixture() -> (
        Vec<super::super::GraphNode>,
        Vec<super::super::GraphEdge>,
        EdgeRoute,
    ) {
        let nodes = vec![
            super::super::GraphNode {
                id: "source".into(),
                kind: "fixture".into(),
                rect: GraphRect {
                    x: 2,
                    y: 10,
                    width: 24,
                    height: 6,
                },
            },
            super::super::GraphNode {
                id: "target".into(),
                kind: "fixture".into(),
                rect: GraphRect {
                    x: 82,
                    y: 10,
                    width: 24,
                    height: 6,
                },
            },
            super::super::GraphNode {
                id: "obstacle".into(),
                kind: "fixture".into(),
                rect: GraphRect {
                    x: 45,
                    y: 8,
                    width: 24,
                    height: 12,
                },
            },
        ];
        let edge = super::super::GraphEdge {
            key: EdgeKey {
                source: "source".into(),
                destination: "target".into(),
                output: "value".into(),
                input: Some("input".into()),
                kind: EdgeKind::Data,
            },
            from: 0,
            to: 1,
            from_output: "value".into(),
            to_input: Some("input".into()),
            kind: EdgeKind::Data,
            from_row: 1,
            to_row: 4,
        };
        let route = EdgeRoute::from_points([
            Point::new(26, 11),
            Point::new(41, 11),
            Point::new(41, 14),
            Point::new(81, 14),
        ]);
        (nodes, vec![edge], route)
    }

    #[test]
    fn completed_routes_leave_and_enter_ports_horizontally() {
        let (mut nodes, edges, preferred) = search_fixture();
        nodes.truncate(2);
        let mut router = Router::new(
            &nodes,
            &edges,
            std::slice::from_ref(&preferred),
            RoutingLimits::default(),
        );
        let vertical_departure =
            EdgeRoute::from_points([Point::new(26, 11), Point::new(26, 14), Point::new(81, 14)]);
        let vertical_arrival = EdgeRoute::from_points([
            Point::new(26, 11),
            Point::new(70, 11),
            Point::new(70, 18),
            Point::new(81, 18),
            Point::new(81, 14),
        ]);
        assert_eq!(
            router.validate(0, &vertical_departure),
            Err(UnresolvedReason::Congestion)
        );
        assert_eq!(
            router.validate(0, &vertical_arrival),
            Err(UnresolvedReason::Congestion)
        );
    }

    #[test]
    fn directional_search_finds_an_obstacle_detour_with_few_bends() {
        let (nodes, edges, preferred) = search_fixture();
        let mut router = Router::new(
            &nodes,
            &edges,
            std::slice::from_ref(&preferred),
            RoutingLimits::default(),
        );
        assert_eq!(
            router.validate(0, &preferred),
            Err(UnresolvedReason::Congestion)
        );
        let route = router
            .search(
                0,
                &preferred,
                &[0, 1, 2],
                GraphRect {
                    x: 0,
                    y: 4,
                    width: 110,
                    height: 20,
                },
            )
            .unwrap();
        assert_eq!(route.points.first(), preferred.points.first());
        assert_eq!(route.points.last(), preferred.points.last());
        assert!(route.points.len() <= 6);
        assert!(
            route
                .segments()
                .all(|segment| nodes.iter().all(|node| !segment.intersects_rect(node.rect)))
        );
        assert!(router.budget.stats.search_expansions > 0);
    }

    #[test]
    fn crossing_validation_rejects_corners_labels_and_third_participants() {
        let (mut nodes, mut edges, route) = search_fixture();
        nodes.truncate(2);
        let mut other = edges[0].clone();
        other.key.output = "other".into();
        other.to_row = 7;
        edges.push(other.clone());
        other.key.output = "third".into();
        other.to_row = 8;
        edges.push(other);
        let horizontal = EdgeRoute::from_points([
            Point::new(26, 11),
            Point::new(70, 11),
            Point::new(70, 14),
            Point::new(81, 14),
        ]);
        let vertical = EdgeRoute::from_points([Point::new(60, 0), Point::new(60, 30)]);
        let mut router = Router::new(
            &nodes,
            &edges,
            &[route.clone(), EdgeRoute::default(), EdgeRoute::default()],
            RoutingLimits::default(),
        );
        router.index.insert(1, &vertical);
        assert_eq!(router.validate(0, &horizontal).unwrap().len(), 1);
        router.index.insert(
            2,
            &EdgeRoute::from_points([Point::new(58, 12), Point::new(66, 12)]),
        );
        assert_eq!(router.validate(0, &horizontal).unwrap().len(), 1);
        router.index.remove(&BTreeSet::from([2]));
        router.index.insert(
            2,
            &EdgeRoute::from_points([Point::new(59, 8), Point::new(59, 16)]),
        );
        assert_eq!(
            router.validate(0, &horizontal),
            Err(UnresolvedReason::Congestion)
        );
        router.index.remove(&BTreeSet::from([2]));
        router.obstacles.insert(
            Some(1),
            GraphRect {
                x: 59,
                y: 11,
                width: 3,
                height: 1,
            },
        );
        assert_eq!(
            router.validate(0, &horizontal),
            Err(UnresolvedReason::Congestion)
        );
        router.obstacles = ObstacleIndex::default();
        router.index.remove(&BTreeSet::from([1]));
        router.index.insert(
            1,
            &EdgeRoute::from_points([Point::new(60, 11), Point::new(60, 30)]),
        );
        assert_eq!(
            router.validate(0, &horizontal),
            Err(UnresolvedReason::Congestion)
        );
    }

    #[test]
    fn interval_queries_prune_distant_segments_and_handle_nested_ranges() {
        let mut index = SegmentIndex::default();
        for edge in 0..10_000 {
            index.insert(
                edge,
                &EdgeRoute::from_points([
                    Point::new(edge as u32 * 8, 6),
                    Point::new(edge as u32 * 8 + 4, 6),
                ]),
            );
        }
        assert_eq!(
            index
                .candidates(segment(40_001, 6, 40_003, 6))
                .map(|item| item.edge)
                .collect::<Vec<_>>(),
            vec![5_000]
        );
        index.insert(
            20_000,
            &EdgeRoute::from_points([Point::new(0, 6), Point::new(80_000, 6)]),
        );
        assert_eq!(index.candidates(segment(40_001, 6, 40_003, 6)).count(), 2);
        index.remove(&BTreeSet::from([5_000, 20_000]));
        assert_eq!(index.candidates(segment(40_001, 6, 40_003, 6)).count(), 0);
        assert!(index.horizontal[&6].root.as_ref().unwrap().height < 20);
    }

    #[test]
    fn obstacle_queries_prune_distant_rows_in_shared_columns() {
        let mut index = ObstacleIndex::default();
        for edge in 0..10_000 {
            index.insert(
                Some(edge),
                GraphRect {
                    x: 40,
                    y: edge as u32 * 8,
                    width: 24,
                    height: 6,
                },
            );
        }
        let candidates: Vec<_> = index.candidates(segment(28, 40_003, 70, 40_003)).collect();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, Some(5_000));
        index.insert(
            Some(20_000),
            GraphRect {
                x: 28,
                y: 7,
                width: 1,
                height: 1,
            },
        );
        assert!(
            index
                .candidates(segment(26, 7, 30, 7))
                .any(|item| item.0 == Some(20_000))
        );
    }

    #[test]
    fn search_and_conflict_limits_stop_before_exceeding_counters() {
        let (nodes, edges, preferred) = search_fixture();
        let bounds = GraphRect {
            x: 0,
            y: 4,
            width: 110,
            height: 20,
        };
        for (limits, expected) in [
            (
                RoutingLimits {
                    max_search_vertices: 0,
                    ..RoutingLimits::default()
                },
                UnresolvedReason::GeometryLimit,
            ),
            (
                RoutingLimits {
                    max_search_expansions: 0,
                    ..RoutingLimits::default()
                },
                UnresolvedReason::SearchLimit,
            ),
            (
                RoutingLimits {
                    max_total_expansions: 0,
                    ..RoutingLimits::default()
                },
                UnresolvedReason::SearchLimit,
            ),
            (
                RoutingLimits {
                    max_conflict_checks: 0,
                    ..RoutingLimits::default()
                },
                UnresolvedReason::ConflictLimit,
            ),
        ] {
            let mut router = Router::new(&nodes, &edges, std::slice::from_ref(&preferred), limits);
            assert_eq!(
                router.search(0, &preferred, &[0, 1, 2], bounds),
                Err(expected)
            );
            assert!(router.budget.stats.search_expansions <= limits.max_total_expansions);
            assert!(router.budget.stats.conflict_checks <= limits.max_conflict_checks);
            assert!(router.budget.stats.geometry_items <= limits.max_geometry_items);
        }
    }

    #[test]
    fn outer_detours_recover_routes_when_search_geometry_is_exhausted() {
        let (nodes, edges, preferred) = search_fixture();
        let mut routes = vec![preferred];
        let limits = RoutingLimits {
            max_search_vertices: 0,
            ..RoutingLimits::default()
        };
        let (crossings, stats) = route_all(&nodes, &edges, &mut routes, &[0, 0, 0], limits);
        assert!(crossings.is_empty());
        assert!(routes[0].unresolved.is_none());
        assert!(
            routes[0]
                .segments()
                .all(|segment| nodes.iter().all(|node| !segment.intersects_rect(node.rect)))
        );
        assert_eq!(stats.retry_rounds, 0);
    }

    #[test]
    fn classify_reversed_overlap_contact_and_crossing() {
        let h = segment(12, 6, 2, 6);
        assert_eq!(
            h.intersection(segment(4, 6, 8, 6)),
            Some(Intersection::Overlap)
        );
        assert_eq!(
            h.intersection(segment(12, 6, 15, 6)),
            Some(Intersection::Contact(Point::new(12, 6)))
        );
        assert_eq!(
            h.intersection(segment(8, 10, 8, 2)),
            Some(Intersection::Crossing(Point::new(8, 6)))
        );
        assert_eq!(
            h.intersection(segment(2, 2, 2, 6)),
            Some(Intersection::Contact(Point::new(2, 6)))
        );
        assert_eq!(h.intersection(segment(1, 8, 13, 8)), None);
        assert!(h.intersects_rect(GraphRect {
            x: 8,
            y: 6,
            width: 2,
            height: 2
        }));
        assert!(!h.intersects_rect(GraphRect {
            x: 8,
            y: 7,
            width: 2,
            height: 2
        }));
    }

    #[test]
    fn route_simplification_keeps_reversals_and_index_ownership() {
        let route = EdgeRoute::from_points([
            Point::new(2, 3),
            Point::new(4, 3),
            Point::new(8, 3),
            Point::new(8, 3),
            Point::new(6, 3),
            Point::new(6, 10),
        ]);
        assert_eq!(route.points.len(), 4);
        let mut index = SegmentIndex::default();
        index.insert(7, &route);
        assert!(
            index
                .candidates(segment(5, 2, 5, 8))
                .any(|item| item.edge == 7
                    && item.segment.intersection(segment(5, 2, 5, 8)).is_some())
        );
        index.remove(&BTreeSet::from([7]));
        assert_eq!(index.candidates(segment(5, 2, 5, 8)).count(), 0);
    }
}
