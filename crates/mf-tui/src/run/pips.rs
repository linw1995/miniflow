//! Picture-in-picture placement and motion, independent of workflow lifecycle state.

use super::{ExecutionPip, PipKey, centered_offset};
use crate::{
    graph::{GraphLayout, GraphView},
    receiver::ObservationReader,
    state::NodeObservation,
};
use mf_telemetry::identity::RunId;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, Clear, Widget},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const MIN_WIDTH: u16 = 32;
const TILE_HEIGHT: u16 = 8;
pub const LINGER: Duration = Duration::from_millis(600);
pub const MOTION: Duration = Duration::from_millis(200);
const FRAME_INTERVAL: Duration = Duration::from_millis(32);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PipSurface {
    Graph,
    ChatOverview,
}

#[derive(Clone, Copy)]
pub struct PipScene<'a> {
    pub layout: &'a GraphLayout,
    pub nodes: &'a [NodeObservation],
    pub reader: &'a ObservationReader,
    pub elapsed_ns: u64,
}

#[derive(Default)]
pub struct PipGrid {
    surface: Option<(RunId, PipSurface)>,
    tiles: Vec<PipTile>,
}

impl PipGrid {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn frame_wait(&self, idle: Duration, now: Instant) -> Duration {
        self.tiles.iter().fold(idle, |wait, tile| {
            if now.saturating_duration_since(tile.entered) < MOTION {
                wait.min(FRAME_INTERVAL)
            } else if let Some(left) = tile.left {
                let until_exit = (left + LINGER).saturating_duration_since(now);
                wait.min(if until_exit.is_zero() {
                    FRAME_INTERVAL
                } else {
                    until_exit
                })
            } else {
                wait
            }
        })
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        surface: PipSurface,
        scene: PipScene<'_>,
        views: &[ExecutionPip<'_>],
        now: Instant,
    ) -> Vec<Rect> {
        let context = Some((scene.reader.run_id(), surface));
        if self.surface != context {
            self.tiles.clear();
            self.surface = context;
        }
        // The compact overview keeps orchestration folded even when there is only one active child.
        let eligible = if views.len() >= 2
            || (surface == PipSurface::ChatOverview
                && views
                    .first()
                    .is_some_and(|view| !scene.layout.node_path(view.display).is_empty()))
        {
            views
        } else {
            &[]
        };
        let slots = PipSlots::new(area);
        let visible = eligible.len().min(slots.capacity());
        let overflow = eligible.len() - visible;
        let desired = self.reconcile(&eligible[..visible], slots.capacity(), now);
        let count = self.tiles.len();
        self.tiles
            .iter_mut()
            .zip(desired)
            .zip(slots.areas(count))
            .enumerate()
            .map(|(index, ((tile, view), slot))| {
                if let Some(view) = view {
                    tile.paint_graph(view, scene, slot);
                }
                let rect = tile.occupied_area(slot, now);
                tile.draw(frame, rect, if index + 1 == count { overflow } else { 0 });
                rect
            })
            .collect()
    }

    fn reconcile<'a>(
        &mut self,
        active: &'a [ExecutionPip<'a>],
        capacity: usize,
        now: Instant,
    ) -> Vec<Option<&'a ExecutionPip<'a>>> {
        let mut wanted: BTreeMap<_, _> = active.iter().map(|view| (&view.key, view)).collect();
        let mut spare = capacity - active.len();
        let mut views = Vec::new();
        // Keep existing order through the hold period, but never use a slot needed by live work.
        self.tiles.retain_mut(|tile| {
            if tile
                .left
                .is_some_and(|left| now.saturating_duration_since(left) >= LINGER + MOTION)
            {
                return false;
            }
            if let Some(view) = wanted.remove(&tile.key) {
                tile.resume(now);
                views.push(Some(view));
            } else if spare > 0 {
                tile.left.get_or_insert(now);
                spare -= 1;
                views.push(None);
            } else {
                return false;
            }
            true
        });
        for view in active {
            if wanted.remove(&view.key).is_some() {
                self.tiles.push(PipTile::new(view, now));
                views.push(Some(view));
            }
        }
        views
    }
}

struct PipTile {
    key: PipKey,
    title: String,
    // Keep only the last painted viewport, not the invocation's observation history.
    graph: Buffer,
    entered: Instant,
    left: Option<Instant>,
}

impl PipTile {
    fn new(view: &ExecutionPip<'_>, now: Instant) -> Self {
        Self {
            key: view.key.clone(),
            title: view.title.clone(),
            graph: Buffer::default(),
            entered: now,
            left: None,
        }
    }

    fn resume(&mut self, now: Instant) {
        if let Some(left) = self.left.take()
            && let Some(exiting) = now.checked_duration_since(left + LINGER)
        {
            // Play the same curve forward from the current height when work returns mid-exit.
            self.entered = now - MOTION.saturating_sub(exiting);
        }
    }

    fn occupied_area(&self, slot: Rect, now: Instant) -> Rect {
        let animation = self
            .left
            .and_then(|left| now.checked_duration_since(left + LINGER))
            .map_or_else(
                || now.saturating_duration_since(self.entered),
                |exiting| MOTION.saturating_sub(exiting),
            );
        let progress = animation.min(MOTION).as_secs_f32() / MOTION.as_secs_f32();
        let height = 2 + (f32::from(slot.height - 2) * progress * (2.0 - progress)).round() as u16;
        Rect::new(slot.x, slot.bottom() - height, slot.width, height)
    }

    fn paint_graph(&mut self, view: &ExecutionPip<'_>, scene: PipScene<'_>, slot: Rect) {
        let area = Rect::new(0, 0, slot.width - 2, slot.height - 2);
        self.graph.resize(area);
        self.graph.reset();
        let node = &scene.layout.nodes()[view.display];
        let offset = centered_offset(node.rect, (area.width, area.height));
        let scoped = view.observations(scene.layout, scene.nodes);
        GraphView::new(scene.layout)
            .reader(scene.reader)
            .nodes(scoped.as_deref().unwrap_or(scene.nodes))
            .selected(view.display)
            .offset(offset.0, offset.1)
            .elapsed_ns(scene.elapsed_ns)
            .render(area, &mut self.graph);
    }

    fn draw(&self, frame: &mut Frame<'_>, rect: Rect, overflow: usize) {
        let (label, color) = if self.left.is_some() {
            ("Recent", Color::DarkGray)
        } else {
            ("Running", Color::Yellow)
        };
        let mut block = Block::bordered()
            .title(format!(" {label}: {} ", self.title))
            .border_style(Style::default().fg(color));
        if overflow > 0 {
            block = block.title_bottom(format!(" +{overflow} more "));
        }
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let buffer = frame.buffer_mut();
        let width = inner.width.min(self.graph.area.width);
        for y in 0..inner.height.min(self.graph.area.height) {
            for x in 0..width {
                let cell = &self.graph[(x, y)];
                // Reflow can narrow a retained tile. Do not split a wide glyph at the new edge.
                if width == self.graph.area.width
                    || Span::raw(cell.symbol()).width() <= usize::from(width - x)
                {
                    buffer[(inner.x + x, inner.y + y)].clone_from(cell);
                }
            }
        }
    }
}

struct PipSlots {
    area: Rect,
    columns: usize,
    rows: usize,
}

impl PipSlots {
    fn new(area: Rect) -> Self {
        Self {
            area,
            columns: usize::from(area.width / MIN_WIDTH),
            rows: usize::from(area.height.div_ceil(2) / TILE_HEIGHT),
        }
    }

    fn capacity(&self) -> usize {
        self.columns * self.rows
    }

    fn areas(&self, count: usize) -> impl Iterator<Item = Rect> + use<> {
        let area = self.area;
        let columns = if count == 0 {
            1
        } else {
            let ideal_columns = (1..=count)
                .find(|columns| *columns >= count.div_ceil(*columns))
                .unwrap_or(count);
            ideal_columns
                .max(count.div_ceil(self.rows))
                .min(self.columns)
        };
        let rows = count.div_ceil(columns);
        let column_width = area.width / columns as u16;
        let top = area.bottom() - rows as u16 * TILE_HEIGHT;
        (0..count).map(move |index| {
            let row = index / columns;
            let column = index % columns;
            let x = column as u16 * column_width;
            Rect::new(
                area.x + x,
                top + row as u16 * TILE_HEIGHT,
                if column + 1 == columns {
                    area.width - x
                } else {
                    column_width
                },
                TILE_HEIGHT,
            )
        })
    }
}

#[cfg(test)]
pub fn parallel_pip_areas(count: usize, area: Rect) -> (Vec<Rect>, usize) {
    if count < 2 {
        return (Vec::new(), 0);
    }
    let slots = PipSlots::new(area);
    let visible = count.min(slots.capacity());
    (slots.areas(visible).collect(), count - visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::PipSource;

    fn invocation(number: u64) -> ExecutionPip<'static> {
        ExecutionPip {
            key: PipKey::Call(Some(number), Vec::new(), "task".into()),
            display: 0,
            title: "task".into(),
            source: PipSource::Static,
        }
    }

    #[test]
    fn returning_tile_expands_from_its_current_height() {
        let mut grid = PipGrid::default();
        let views = [invocation(1)];
        let start = Instant::now();
        let idle = Duration::from_millis(100);
        let slot = Rect::new(0, 0, MIN_WIDTH, TILE_HEIGHT);
        grid.reconcile(&views, 2, start);
        assert_eq!(grid.frame_wait(idle, start), FRAME_INTERVAL);
        assert_eq!(grid.frame_wait(idle, start + MOTION), idle);

        let left = start + MOTION;
        grid.reconcile(&[], 2, left);
        assert_eq!(grid.frame_wait(idle, left), idle);
        let exit_start = left + LINGER;
        assert_eq!(
            grid.frame_wait(idle, exit_start - Duration::from_millis(10)),
            Duration::from_millis(10)
        );
        let returns = exit_start + MOTION * 3 / 4;
        let exiting = grid.tiles[0].occupied_area(slot, returns);
        assert!(exiting.height < slot.height);
        assert_eq!(grid.frame_wait(idle, returns), FRAME_INTERVAL);

        let present = grid.reconcile(&views, 2, returns);
        assert_eq!(present.len(), 1);
        assert!(present[0].is_some());
        assert_eq!(grid.tiles[0].occupied_area(slot, returns), exiting);
        assert_eq!(grid.tiles[0].occupied_area(slot, returns + MOTION), slot);
    }

    #[test]
    fn resize_and_new_invocations_give_live_tiles_priority_over_recent_ones() {
        let views = [invocation(1), invocation(2), invocation(3)];
        let mut grid = PipGrid::default();
        let start = Instant::now();
        grid.reconcile(&views[..2], 3, start);

        // All invocations share a title and display node, but only the first leaves.
        let left = start + MOTION;
        let presented = grid.reconcile(&views[1..], 3, left);
        assert_eq!(presented.len(), 3);
        assert!(presented[0].is_none());
        assert!(grid.tiles[1].key == views[1].key);
        assert_eq!(grid.tiles[1].entered, start);
        assert_eq!(grid.tiles[2].entered, left);

        let resized = grid.reconcile(&views[1..], 2, left + FRAME_INTERVAL);
        assert_eq!(resized.len(), 2);
        assert!(resized.iter().all(Option::is_some));
        assert!(grid.tiles[0].key == views[1].key);
        assert!(grid.tiles[1].key == views[2].key);
    }
}
