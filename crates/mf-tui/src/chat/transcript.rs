use crate::{
    images::Images,
    markdown::{Document, Position},
};
use ratatui::{Frame, layout::Rect, style::Style, text::Line};
use ratatui_image::picker::Picker;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Markdown,
    Literal,
    Json,
}

#[derive(Default)]
struct Entry {
    document: Document,
    source: Option<(mf_chat::display::Text, Format)>,
    loaded: bool,
    laid_out: bool,
    height: usize,
    seen: bool,
}

impl Entry {
    fn evict_body(&mut self) {
        if self.source.is_some() {
            self.document = Document::default();
            self.loaded = false;
            self.laid_out = false;
        }
    }
}

#[derive(Default)]
pub struct Transcript {
    entries: BTreeMap<String, Entry>,
    order: Vec<String>,
    ends: Vec<usize>,
    width: u16,
    anchor: Option<(String, Position)>,
    requested_scroll: usize,
    images: Images,
    #[cfg(test)]
    body_reads: usize,
}

impl Transcript {
    pub fn enable_images(&mut self, root: PathBuf, picker: Picker) -> io::Result<()> {
        self.images.enable(root, picker)
    }

    pub fn update_image_picker(&mut self, picker: Picker) {
        self.images.update_picker(picker);
    }

    pub fn clear(&mut self) {
        self.clear_images();
        self.entries.clear();
        self.order.clear();
        self.ends.clear();
        self.anchor = None;
        self.requested_scroll = 0;
    }

    pub fn evict_bodies(&mut self) {
        self.entries.values_mut().for_each(Entry::evict_body);
    }

    pub fn clear_images(&mut self) {
        self.images.clear();
    }

    pub fn flush_images(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        self.images.poll();
        self.images.flush_retired(writer)
    }

    pub fn begin(&mut self, width: u16) {
        self.images.begin_frame();
        self.width = width;
        self.order.clear();
        self.ends.clear();
        for entry in self.entries.values_mut() {
            entry.seen = false;
            entry.laid_out = false;
        }
    }

    #[cfg(test)]
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
        entry.source = None;
        entry.laid_out = true;
        entry.height = entry.document.layout_images(self.width, &mut self.images);
        self.register(key);
    }

    pub fn deferred(&mut self, key: String, text: &mf_chat::display::Text, format: Format) {
        let entry = self.entries.entry(key.clone()).or_default();
        if !entry
            .source
            .as_ref()
            .is_some_and(|(old, kind)| old == text && *kind == format)
        {
            entry.source = Some((text.clone(), format));
            entry.loaded = false;
            // Unknown bodies occupy a row until the viewport measures them. Keep the
            // previous height of a growing body so a reader's anchor need not jump.
            entry.height = entry.height.max(1);
        }
        self.register(key);
    }

    fn register(&mut self, key: String) {
        let entry = self.entries.get_mut(&key).unwrap();
        entry.seen = true;
        self.ends
            .push(self.ends.last().copied().unwrap_or_default() + entry.height);
        self.order.push(key);
    }

    fn measure(&mut self, index: usize) {
        let entry = self.entries.get_mut(&self.order[index]).unwrap();
        if entry.laid_out {
            return;
        }
        if !entry.loaded {
            #[cfg(test)]
            {
                self.body_reads += 1;
            }
            let (source, format) = entry.source.as_ref().expect("deferred document");
            match source.load() {
                Ok(text) => match format {
                    Format::Markdown => entry.document.update(&text),
                    Format::Literal => entry.document.literal(&text, Style::default()),
                    Format::Json => entry.document.json(&text),
                },
                Err(error) => entry.document.literal(
                    &format!("[Display unavailable: {error}]"),
                    Style::default().fg(ratatui::style::Color::Yellow),
                ),
            }
            entry.loaded = true;
        }
        let height = entry.document.layout_images(self.width, &mut self.images);
        if height != entry.height {
            for end in &mut self.ends[index..] {
                *end = *end - entry.height + height;
            }
            entry.height = height;
        }
        entry.laid_out = true;
    }

    pub fn focus(&mut self, key: &str) -> bool {
        let Some(entry) = self.entries.get(key) else {
            return false;
        };
        let position = match entry.document.row(0) {
            Some((position, _)) => position,
            None if entry.source.is_some() => Position::default(),
            None => return false,
        };
        self.anchor = Some((key.to_owned(), position));
        self.requested_scroll = 1;
        true
    }

    pub fn remap_anchor(&mut self, key_for: impl FnOnce(&str) -> Option<String>) {
        if let Some((key, _)) = &mut self.anchor
            && let Some(mapped) = key_for(key)
        {
            *key = mapped;
        }
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
        self.entries.retain(|_, entry| entry.seen);
        let anchor = self
            .anchor
            .as_ref()
            .and_then(|(key, position)| {
                Some((self.order.iter().position(|item| item == key)?, *position))
            })
            .filter(|_| scroll != 0);
        if let Some((index, _)) = anchor {
            self.measure(index);
        }
        // Measure from the reading position in the direction of travel. Following
        // the tail starts at the last body, not at page one. Heights of evicted
        // bodies are hints; resolve them before crossing them or painting rows.
        let (tail, top, visible) = loop {
            let total = self.ends.last().copied().unwrap_or_default();
            let tail = total.saturating_sub(height as usize);
            let origin = anchor.map_or(tail, |(index, position)| {
                let start = index.checked_sub(1).map_or(0, |i| self.ends[i]);
                start + self.entries[&self.order[index]].document.locate(position)
            });
            let old_scroll = if anchor.is_some() {
                self.requested_scroll
            } else {
                0
            };
            let top = if scroll >= old_scroll {
                origin.saturating_sub(scroll - old_scroll)
            } else {
                origin.saturating_add(old_scroll - scroll)
            }
            .min(tail);
            let first = self.ends.partition_point(|&end| end <= top);
            let end = self
                .ends
                .partition_point(|&end| end < top + height as usize)
                .saturating_add(1)
                .min(self.order.len());
            let origin_index = anchor.map_or(self.order.len().saturating_sub(1), |(i, _)| i);
            let range = first.min(origin_index)..end.max(origin_index + 1).min(self.order.len());
            let pending = |&i: &usize| !self.entries[&self.order[i]].laid_out;
            let next = if anchor.is_none() || scroll > old_scroll {
                range.rev().find(pending)
            } else {
                range.into_iter().find(pending)
            };
            match next {
                Some(i) => self.measure(i),
                None => break (tail, top, first..end),
            }
        };
        self.requested_scroll = if scroll == 0 { 0 } else { tail - top };
        self.anchor = if self.requested_scroll == 0 {
            None
        } else {
            self.row(top)
                .map(|(key, position, _)| (key.to_owned(), position))
        };
        for (index, key) in self.order.iter().enumerate() {
            let entry = self.entries.get_mut(key).unwrap();
            if !visible.contains(&index) {
                entry.evict_body();
            }
        }
        top
    }

    pub fn render_images(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        scroll: usize,
        show_images: bool,
    ) -> usize {
        if area.is_empty() {
            self.entries.retain(|_, entry| entry.seen);
            self.evict_bodies();
            return scroll;
        }
        let top = self.top(area.height, scroll);
        let mut offset = 0;
        while offset < area.height {
            let image = self
                .row(top + offset as usize)
                .and_then(|(key, position, _)| {
                    self.entries[key]
                        .document
                        .image(position)
                        .map(|(image, indent, height)| {
                            (image.clone(), indent, height, position.byte)
                        })
                });
            if let Some((image, indent, height, skip)) = image {
                let indent = indent.min(area.width.saturating_sub(1) as usize) as u16;
                let visible = height
                    .saturating_sub(skip)
                    .min((area.height - offset) as usize)
                    .max(1) as u16;
                let rect = Rect::new(
                    area.x + indent,
                    area.y + offset,
                    area.width.saturating_sub(indent),
                    visible,
                );
                if show_images {
                    self.images.request(&image, rect.width);
                    if self.images.render(&image, rect.width, frame, rect, skip) {
                        offset += visible;
                        continue;
                    }
                }
            }
            if let Some((_, _, line)) = self.row(top + offset as usize) {
                frame.render_widget(line, Rect::new(area.x, area.y + offset, area.width, 1));
            }
            offset += 1;
        }
        self.requested_scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::{Duration, Instant};

    fn screen(transcript: &Transcript, top: usize, height: usize) -> Vec<String> {
        (top..top + height)
            .filter_map(|row| transcript.row(row).map(|(_, _, line)| line.to_string()))
            .collect()
    }

    #[test]
    fn tool_focus_tolerates_not_yet_rendered_keys_and_anchors_existing_headers() {
        let mut transcript = Transcript::default();
        assert!(!transcript.focus("missing"));
        transcript.begin(20);
        transcript.literal(
            "tool".into(),
            "Tool result\none\ntwo\nthree",
            Style::default(),
        );
        assert_eq!(transcript.top(2, 0), 2);
        assert!(transcript.focus("tool"));
        assert_eq!(transcript.top(2, 1), 0);
        assert_eq!(screen(&transcript, 0, 1), ["Tool result"]);
    }

    #[test]
    fn rekeyed_group_keeps_a_focused_header_at_the_top() {
        let mut transcript = Transcript::default();
        transcript.begin(24);
        transcript.literal("prefix".into(), "earlier", Style::default());
        transcript.literal("anonymous".into(), "Tool call [index 0]", Style::default());
        transcript.literal("tail".into(), "one\ntwo\nthree", Style::default());
        assert!(transcript.focus("anonymous"));
        transcript.remap_anchor(|key| (key == "anonymous").then(|| "known".into()));
        transcript.begin(24);
        transcript.literal("prefix".into(), "earlier\nbefore", Style::default());
        transcript.literal("known".into(), "Tool call [known]", Style::default());
        transcript.literal("tail".into(), "one\ntwo\nthree\nfour", Style::default());
        let top = transcript.top(2, 1);
        assert_eq!(screen(&transcript, top, 1), ["Tool call [known]"]);
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
    fn viewport_loads_only_intersecting_bodies_and_reloads_evicted_rows() {
        let mut expected = Vec::new();
        let messages = (0..64)
            .map(|id| {
                let lines: Vec<_> = (0..8)
                    .map(|line| format!("body-{id}: line-{line}"))
                    .collect();
                let content = lines.join("\n");
                expected.extend(lines);
                mf_chat::ChatMessage::User {
                    content,
                    metadata: None,
                }
            })
            .collect();
        let (bodies, warning) = mf_chat::display::stage(messages);
        assert!(warning.is_none());
        let mut t = Transcript::default();
        let frame = |t: &mut Transcript, scroll| {
            t.begin(32);
            for (id, body) in bodies.iter().enumerate() {
                t.deferred(format!("body-{id}"), body.content(), Format::Literal);
            }
            let top = t.top(3, scroll);
            screen(t, top, 3)
        };
        assert_eq!(frame(&mut t, 0), expected[509..]);
        assert_eq!(frame(&mut t, 0), expected[509..]);
        assert_eq!(
            t.body_reads, 1,
            "drawing the tail must not read preceding bodies or reread a hot body"
        );
        for distance in 1..=24 {
            let scroll = t.requested_scroll + 1;
            let end = expected.len() - distance;
            assert_eq!(frame(&mut t, scroll), expected[end - 3..end]);
            assert!(
                t.entries
                    .values()
                    .filter(|e| e.document.row(0).is_some())
                    .count()
                    <= 2
            );
        }
        assert!(t.entries["body-63"].document.row(0).is_none());
        let before = t.body_reads;
        assert_eq!(frame(&mut t, 0), expected[509..]);
        assert_eq!(
            t.body_reads,
            before + 1,
            "evicted tail must be fetched again"
        );
        t.evict_bodies();
        assert!(t.entries.values().all(|e| e.document.row(0).is_none()));
        assert!(t.focus("body-0"));
        assert_eq!(frame(&mut t, 1), expected[..3]);
        assert_eq!(
            t.body_reads,
            before + 2,
            "jumping to a key must not materialize the gap"
        );
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

    #[test]
    fn images_participate_in_scrolling_resizing_and_popup_visibility() {
        let root = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(64, 128, image::Rgb([40u8, 120, 200]))
            .save(root.path().join("chart.png"))
            .unwrap();
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut transcript = Transcript::default();
        transcript
            .enable_images(root.path().to_owned(), picker)
            .unwrap();
        let source = "Before\n\n![chart](chart.png)\n\nAfter";
        let mut terminal = Terminal::new(TestBackend::new(12, 5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            transcript.begin(12);
            transcript.markdown("reply".into(), source);
            terminal
                .draw(|frame| {
                    transcript.render_images(frame, frame.area(), 0, true);
                })
                .unwrap();
            if terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains('\u{10eeee}'))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "A")
        );
        transcript.begin(12);
        transcript.markdown("reply".into(), source);
        terminal
            .draw(|frame| {
                transcript.render_images(frame, frame.area(), 0, false);
            })
            .unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains('\u{10eeee}')
                    || cell.symbol().contains("\u{1b}_G"))
        );
        terminal
            .draw(|frame| {
                transcript.render_images(frame, frame.area(), 0, true);
            })
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains('\u{10eeee}'))
        );
        std::fs::remove_file(root.path().join("chart.png")).unwrap();
        terminal.resize(Rect::new(0, 0, 4, 5)).unwrap();
        loop {
            transcript.begin(4);
            transcript.markdown("reply".into(), source);
            terminal
                .draw(|frame| {
                    transcript.render_images(frame, frame.area(), 0, true);
                })
                .unwrap();
            if terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol().contains('\u{10eeee}'))
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        transcript.clear_images();
        let mut cleanup = vec![];
        transcript.flush_images(&mut cleanup).unwrap();
        assert!(String::from_utf8(cleanup).unwrap().contains("a=d,d=I"));
    }
}
