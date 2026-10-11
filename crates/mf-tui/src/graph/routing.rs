//! Integer geometry and ownership for orthogonal terminal routes.

use super::{EdgeKind, GraphRect};
use std::collections::{BTreeMap, BTreeSet};

pub const CROSSING_RADIUS: u32 = 1;
pub const CROSSING_CLEARANCE: u32 = 3;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedReason {
    Congestion,
    SearchLimit,
    GeometryLimit,
    ConflictLimit,
}

impl std::fmt::Display for UnresolvedReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Congestion => "no safe route within the available channels",
            Self::SearchLimit => "routing search budget exhausted",
            Self::GeometryLimit => "routing geometry budget exhausted",
            Self::ConflictLimit => "routing conflict budget exhausted",
        })
    }
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
        GraphRect {
            x: self.point.x.saturating_sub(CROSSING_RADIUS),
            y: self.point.y.saturating_sub(CROSSING_RADIUS),
            width: 3,
            height: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingPattern {
    Bridge,
    Underpass,
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
        EdgeKind::Data => ["═", "║", "╔", "╗", "╚", "╝"],
        EdgeKind::Control => ["─", "│", "┌", "┐", "└", "┘"],
    }
}

pub fn crossing_cells(
    horizontal: EdgeKind,
    vertical: EdgeKind,
    upper_horizontal: bool,
    pattern: CrossingPattern,
) -> Vec<MarkerCell> {
    let upper_kind = if upper_horizontal {
        horizontal
    } else {
        vertical
    };
    let lower_kind = if upper_horizontal {
        vertical
    } else {
        horizontal
    };
    let [h, v, tl, tr, bl, br] = line_symbols(upper_kind);
    let lower_line = line_symbols(lower_kind)[usize::from(upper_horizontal)];
    let mut cells = Vec::new();
    let mut put = |dx, dy, symbol, upper| {
        cells.push(MarkerCell {
            dx,
            dy,
            symbol,
            upper,
        })
    };
    match pattern {
        CrossingPattern::Underpass => {
            for along in -1..=1 {
                put(
                    if upper_horizontal { along } else { 0 },
                    if upper_horizontal { 0 } else { along },
                    if upper_horizontal { h } else { v },
                    true,
                );
            }
            let (before, after) = match (upper_horizontal, lower_kind) {
                (true, EdgeKind::Control) => ("┴", "┬"),
                (true, EdgeKind::Data) => ("╧", "╤"),
                (false, EdgeKind::Control) => ("┤", "├"),
                (false, EdgeKind::Data) => ("╢", "╟"),
            };
            put(
                if upper_horizontal { 0 } else { -1 },
                if upper_horizontal { -1 } else { 0 },
                before,
                false,
            );
            put(
                if upper_horizontal { 0 } else { 1 },
                if upper_horizontal { 1 } else { 0 },
                after,
                false,
            );
        }
        CrossingPattern::Bridge => {
            if upper_horizontal {
                for (dx, dy, symbol) in [
                    (-2, 0, br),
                    (-2, -1, tl),
                    (-1, -1, h),
                    (0, -1, h),
                    (1, -1, h),
                    (2, -1, tr),
                    (2, 0, bl),
                ] {
                    put(dx, dy, symbol, true);
                }
                put(0, -2, lower_line, false);
                put(0, 1, lower_line, false);
                put(0, 2, lower_line, false);
            } else {
                for (dx, dy, symbol) in [
                    (0, -2, bl),
                    (1, -2, tr),
                    (1, -1, v),
                    (1, 0, v),
                    (1, 1, v),
                    (1, 2, br),
                    (0, 2, tl),
                ] {
                    put(dx, dy, symbol, true);
                }
                put(-2, 0, lower_line, false);
                put(-1, 0, lower_line, false);
            }
        }
    }
    cells
}

#[derive(Clone, Copy, Debug)]
pub struct IndexedSegment {
    pub edge: usize,
    pub segment: Segment,
}

#[derive(Clone, Debug, Default)]
pub struct SegmentIndex {
    horizontal: BTreeMap<u32, Vec<IndexedSegment>>,
    vertical: BTreeMap<u32, Vec<IndexedSegment>>,
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
                .push(IndexedSegment { edge, segment });
        }
    }

    pub fn remove(&mut self, edges: &BTreeSet<usize>) {
        for index in [&mut self.horizontal, &mut self.vertical] {
            index.retain(|_, items| {
                items.retain(|item| !edges.contains(&item.edge));
                !items.is_empty()
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
            .flatten()
            .chain(opposite.range(first..=last).flat_map(|(_, items)| items))
    }
}

pub const ATTACHMENT_SPACING: u32 = 6;
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
    by_x: BTreeMap<u32, Vec<(Option<usize>, GraphRect)>>,
    max_width: u32,
}

impl ObstacleIndex {
    pub fn insert(&mut self, owner: Option<usize>, rect: GraphRect) {
        self.max_width = self.max_width.max(rect.width);
        self.by_x.entry(rect.x).or_default().push((owner, rect));
    }

    pub fn candidates(
        &self,
        segment: Segment,
    ) -> impl Iterator<Item = &(Option<usize>, GraphRect)> {
        self.by_x
            .range(segment.low().x.saturating_sub(self.max_width)..=segment.high().x)
            .flat_map(|(_, items)| items)
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

pub fn route_crossings(
    edge: usize,
    route: &EdgeRoute,
    edges: &[super::GraphEdge],
    obstacles: &ObstacleIndex,
    index: &SegmentIndex,
    existing: &[Crossing],
) -> Result<Vec<Crossing>, UnresolvedReason> {
    let mut crossings = Vec::new();
    let segments: Vec<_> = route.segments().collect();
    for (position, segment) in segments.iter().copied().enumerate() {
        if obstacles
            .candidates(segment)
            .any(|&(owner, rect)| owner != Some(edge) && segment.intersects_rect(rect))
        {
            return Err(UnresolvedReason::Congestion);
        }
        for other in segments.iter().skip(position + 2) {
            if segment.intersection(*other).is_some() {
                return Err(UnresolvedReason::Congestion);
            }
        }
        for other in index.candidates(segment) {
            if other.edge == edge {
                continue;
            }
            match segment.intersection(other.segment) {
                None => {}
                Some(Intersection::Overlap | Intersection::Contact(_)) => {
                    return Err(UnresolvedReason::Congestion);
                }
                Some(Intersection::Crossing(point)) => {
                    if segment.interior_clearance(point) < CROSSING_CLEARANCE
                        || other.segment.interior_clearance(point) < CROSSING_CLEARANCE
                    {
                        return Err(UnresolvedReason::Congestion);
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
                    if obstacles
                        .by_x
                        .range(
                            footprint.x.saturating_sub(obstacles.max_width)
                                ..=footprint.x + footprint.width,
                        )
                        .flat_map(|(_, items)| items)
                        .any(|&(_, rect)| rects_intersect(footprint, rect))
                    {
                        return Err(UnresolvedReason::Congestion);
                    }
                    if existing
                        .iter()
                        .chain(&crossings)
                        .any(|old: &Crossing| rects_intersect(old.footprint(), footprint))
                    {
                        return Err(UnresolvedReason::Congestion);
                    }
                    for candidate in index.candidates(Segment {
                        start: Point::new(footprint.x, footprint.y),
                        end: Point::new(footprint.x + 2, footprint.y),
                    }) {
                        if candidate.edge != other.edge
                            && candidate.segment.intersects_rect(footprint)
                        {
                            return Err(UnresolvedReason::Congestion);
                        }
                    }
                    for candidate in index.candidates(Segment {
                        start: Point::new(footprint.x, footprint.y + 2),
                        end: Point::new(footprint.x + 2, footprint.y + 2),
                    }) {
                        if candidate.edge != other.edge
                            && candidate.segment.intersects_rect(footprint)
                        {
                            return Err(UnresolvedReason::Congestion);
                        }
                    }
                    crossings.push(crossing);
                }
            }
        }
        if existing.iter().any(|crossing| {
            edge != crossing.horizontal
                && edge != crossing.vertical
                && segment.intersects_rect(crossing.footprint())
        }) {
            return Err(UnresolvedReason::Congestion);
        }
    }
    for crossing in &crossings {
        if segments
            .iter()
            .filter(|segment| segment.intersects_rect(crossing.footprint()))
            .count()
            != 1
        {
            return Err(UnresolvedReason::Congestion);
        }
    }
    Ok(crossings)
}

pub fn validate_routes(
    nodes: &[super::GraphNode],
    edges: &[super::GraphEdge],
    routes: &mut [EdgeRoute],
) -> Vec<Crossing> {
    let mut obstacles = ObstacleIndex::default();
    for node in nodes {
        obstacles.insert(None, node.rect);
    }
    for (edge, route) in routes.iter().enumerate() {
        if let Some(label) = &route.label {
            obstacles.insert(Some(edge), label.rect);
        }
    }
    let mut order: Vec<_> = (0..edges.len()).collect();
    order.sort_by_key(|&index| &edges[index].key);
    let mut index = SegmentIndex::default();
    let mut crossings = Vec::new();
    for edge in order {
        match route_crossings(edge, &routes[edge], edges, &obstacles, &index, &crossings) {
            Ok(new) => {
                crossings.extend(new);
                index.insert(edge, &routes[edge]);
            }
            Err(reason) => {
                routes[edge].points.clear();
                routes[edge].unresolved = Some(reason);
            }
        }
    }
    crossings
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

    #[test]
    fn marker_footprints_exclude_nearby_bends_and_labels() {
        let crossing = Crossing {
            point: Point::new(8, 6),
            horizontal: 0,
            vertical: 1,
            upper: 0,
        };
        let h = segment(2, 6, 12, 6);
        assert!(h.interior_clearance(crossing.point) >= CROSSING_CLEARANCE);
        assert!(segment(9, 4, 9, 8).intersects_rect(crossing.footprint()));
        assert_eq!(segment(8, 6, 8, 10).interior_clearance(crossing.point), 0);
        assert!(h.intersects_rect(GraphRect {
            x: 3,
            y: 6,
            width: 4,
            height: 1
        }));
    }

    #[test]
    fn underpass_cells_have_single_cell_width_and_explicit_lower_caps() {
        for h in [EdgeKind::Data, EdgeKind::Control] {
            for v in [EdgeKind::Data, EdgeKind::Control] {
                for upper in [true, false] {
                    let cells = crossing_cells(h, v, upper, CrossingPattern::Underpass);
                    assert_eq!(cells.iter().filter(|cell| cell.upper).count(), 3);
                    assert_eq!(cells.iter().filter(|cell| !cell.upper).count(), 2);
                    for cell in cells {
                        assert_eq!(ratatui::text::Span::raw(cell.symbol).width(), 1);
                    }
                }
            }
        }
    }
}
