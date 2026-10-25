use crate::markdown::{Document, Position};
use ratatui::{Frame, layout::Rect, style::Style, text::Line};
use std::collections::BTreeMap;

#[derive(Default)]
struct Entry {
    document: Document,
    seen: bool,
}

#[derive(Default)]
pub struct Transcript {
    entries: BTreeMap<String, Entry>,
    order: Vec<String>,
    ends: Vec<usize>,
    width: u16,
    anchor: Option<(String, Position)>,
    requested_scroll: usize,
}

impl Transcript {
    pub fn begin(&mut self, width: u16) {
        self.width = width;
        self.order.clear();
        self.ends.clear();
        for entry in self.entries.values_mut() {
            entry.seen = false;
        }
    }

    pub fn markdown(&mut self, key: String, source: &str) {
        self.entries
            .entry(key.clone())
            .or_default()
            .document
            .update(source);
        self.push(key);
    }

    pub fn plain(&mut self, key: String, lines: Vec<Line<'static>>) {
        self.entries
            .entry(key.clone())
            .or_default()
            .document
            .plain(lines);
        self.push(key);
    }

    pub fn literal(&mut self, key: String, source: &str, style: Style) {
        self.entries
            .entry(key.clone())
            .or_default()
            .document
            .literal(source, style);
        self.push(key);
    }

    fn push(&mut self, key: String) {
        let entry = self.entries.get_mut(&key).unwrap();
        entry.seen = true;
        let height = entry.document.layout(self.width);
        self.ends
            .push(self.ends.last().copied().unwrap_or_default() + height);
        self.order.push(key);
    }

    pub fn is_empty(&self) -> bool {
        self.ends.last().copied().unwrap_or_default() == 0
    }

    fn row(&self, row: usize) -> Option<(&str, Position, &Line<'static>)> {
        let index = self.ends.partition_point(|end| *end <= row);
        let key = self.order.get(index)?;
        let start = index.checked_sub(1).map_or(0, |i| self.ends[i]);
        let (position, line) = self.entries[key].document.row(row - start)?;
        Some((key, position, line))
    }

    fn top(&mut self, height: u16, scroll: usize) -> usize {
        let tail = self
            .ends
            .last()
            .copied()
            .unwrap_or_default()
            .saturating_sub(height as usize);
        let anchored = self.anchor.as_ref().and_then(|(key, position)| {
            let index = self.order.iter().position(|candidate| candidate == key)?;
            let start = index.checked_sub(1).map_or(0, |i| self.ends[i]);
            Some(start + self.entries[key].document.locate(*position))
        });
        let top = if scroll == 0 {
            tail
        } else if let Some(anchored) = anchored {
            if scroll >= self.requested_scroll {
                anchored.saturating_sub(scroll - self.requested_scroll)
            } else {
                anchored.saturating_add(self.requested_scroll - scroll)
            }
            .min(tail)
        } else {
            tail.saturating_sub(scroll)
        };
        self.requested_scroll = if scroll == 0 { 0 } else { tail - top };
        self.anchor = if self.requested_scroll == 0 {
            None
        } else {
            self.row(top)
                .map(|(key, position, _)| (key.to_owned(), position))
        };
        self.entries.retain(|_, entry| entry.seen);
        top
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, scroll: usize) -> usize {
        let top = self.top(area.height, scroll);
        for offset in 0..area.height {
            if let Some((_, _, line)) = self.row(top + offset as usize) {
                frame.render_widget(line, Rect::new(area.x, area.y + offset, area.width, 1));
            }
        }
        self.requested_scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(transcript: &Transcript, top: usize, height: usize) -> Vec<String> {
        (top..top + height)
            .filter_map(|row| transcript.row(row).map(|(_, _, line)| line.to_string()))
            .collect()
    }

    #[test]
    fn browsing_stays_anchored_while_tail_following_advances() {
        let mut transcript = Transcript::default();
        transcript.begin(20);
        transcript.literal(
            "reply".into(),
            "one\ntwo\nthree\nfour\nfive",
            Style::default(),
        );
        let top = transcript.top(2, 2);
        let before = screen(&transcript, top, 2);
        assert_eq!(before, ["two", "three"]);
        transcript.begin(20);
        transcript.literal(
            "reply".into(),
            "one\ntwo\nthree\nfour\nfive\nsix",
            Style::default(),
        );
        let top = transcript.top(2, 2);
        assert_eq!(screen(&transcript, top, 2), before);
        let top = transcript.top(2, 0);
        assert_eq!(screen(&transcript, top, 2), ["five", "six"]);
    }

    #[test]
    fn browsing_tracks_distance_to_the_growing_tail_and_survives_resize() {
        let mut transcript = Transcript::default();
        transcript.begin(20);
        transcript.literal(
            "reply".into(),
            "one\ntwo\nthree\nfour\nfive",
            Style::default(),
        );
        let top = transcript.top(2, 2);
        assert_eq!(screen(&transcript, top, 1), ["two"]);
        transcript.begin(20);
        transcript.literal(
            "reply".into(),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight",
            Style::default(),
        );
        let top = transcript.top(2, 2);
        assert_eq!(screen(&transcript, top, 1), ["two"]);
        assert_eq!(transcript.requested_scroll, 5);
        let top = transcript.top(2, 4);
        assert_eq!(screen(&transcript, top, 1), ["three"]);
        transcript.begin(3);
        transcript.literal(
            "reply".into(),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight",
            Style::default(),
        );
        let top = transcript.top(2, 4);
        assert_eq!(screen(&transcript, top, 1), ["thr"]);
    }

    #[test]
    fn canonical_replacements_and_session_changes_evict_obsolete_documents() {
        let mut transcript = Transcript::default();
        transcript.begin(40);
        transcript.markdown("preview:old-call".into(), "**draft**");
        transcript.top(10, 0);
        transcript.begin(40);
        transcript.markdown("final:old-call".into(), "**canonical**");
        transcript.top(10, 0);
        assert_eq!(transcript.entries.len(), 1);
        assert_eq!(screen(&transcript, 0, 10), ["canonical"]);
        transcript.begin(40);
        transcript.markdown("preview:new-call".into(), "new reply");
        transcript.top(10, 0);
        assert_eq!(transcript.entries.len(), 1);
        assert_eq!(screen(&transcript, 0, 10), ["new reply"]);
    }

    #[test]
    fn transcript_offsets_are_not_limited_to_u16() {
        let mut transcript = Transcript::default();
        transcript.begin(10);
        let source = format!("{}last", "x\n".repeat(u16::MAX as usize + 1));
        transcript.literal("long".into(), &source, Style::default());
        let top = transcript.top(1, 0);
        assert!(top > u16::MAX as usize);
        assert_eq!(screen(&transcript, top, 1), ["last"]);
    }
}
