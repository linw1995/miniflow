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
