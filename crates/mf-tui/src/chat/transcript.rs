use super::preview::MAX_TOOL_ARGUMENT_PREVIEW_BYTES;
use crate::{
    images::Images,
    markdown::{Document, Position},
};
use mf_runtime::ValueRef;
use ratatui::{Frame, layout::Rect, style::Style, text::Line};
use ratatui_image::picker::Picker;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    path::PathBuf,
    rc::Rc,
};

#[derive(Default)]
struct Entry {
    document: Document,
    canonical_arguments: Option<Rc<CanonicalArguments>>,
    seen: bool,
}

pub struct CanonicalArguments {
    value: ValueRef,
    pub text: String,
    pub total_bytes: usize,
}

impl CanonicalArguments {
    fn new(value: &ValueRef) -> Self {
        let mut tail = JsonTail::default();
        let (text, total_bytes) = match serde_json::to_writer_pretty(&mut tail, value) {
            Ok(()) => (tail.text(), tail.total_bytes),
            Err(error) => {
                let text = format!("[Arguments unavailable: {error}]");
                let total_bytes = text.len();
                (text, total_bytes)
            }
        };
        Self {
            value: value.clone(),
            text,
            total_bytes,
        }
    }
}

#[derive(Default)]
struct JsonTail {
    bytes: VecDeque<u8>,
    total_bytes: usize,
}

impl JsonTail {
    fn text(&mut self) -> String {
        let bytes = self.bytes.make_contiguous();
        // Serialization emits UTF-8, but the byte budget may cut through the first character.
        let start = bytes
            .iter()
            .take_while(|byte| **byte & 0xc0 == 0x80)
            .count();
        String::from_utf8(bytes[start..].to_vec()).expect("serialized JSON is UTF-8")
    }
}

impl Write for JsonTail {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let limit = MAX_TOOL_ARGUMENT_PREVIEW_BYTES;
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let tail = &bytes[bytes.len().saturating_sub(limit)..];
        self.bytes
            .drain(..(self.bytes.len() + tail.len()).saturating_sub(limit));
        self.bytes.extend(tail);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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
}

impl Transcript {
    pub fn enable_images(&mut self, root: PathBuf, picker: Picker) -> io::Result<()> {
        self.images.enable(root, picker)
    }

    pub fn update_image_picker(&mut self, picker: Picker) {
        self.images.update_picker(picker);
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

    pub fn canonical_arguments(&mut self, key: String, value: &ValueRef) -> Rc<CanonicalArguments> {
        let entry = self.entries.entry(key).or_default();
        entry.seen = true;
        if let Some(cached) = &entry.canonical_arguments
            && cached.value.ptr_eq(value)
        {
            return Rc::clone(cached);
        }
        let arguments = Rc::new(CanonicalArguments::new(value));
        entry.canonical_arguments = Some(Rc::clone(&arguments));
        arguments
    }

    pub fn json(&mut self, key: String, source: &str) {
        self.entries
            .entry(key.clone())
            .or_default()
            .document
            .json(source);
        self.push(key);
    }

    fn push(&mut self, key: String) {
        let entry = self.entries.get_mut(&key).unwrap();
        entry.seen = true;
        let height = entry.document.layout_images(self.width, &mut self.images);
        self.ends
            .push(self.ends.last().copied().unwrap_or_default() + height);
        self.order.push(key);
    }

    pub fn focus(&mut self, key: &str) -> bool {
        let Some((position, _)) = self
            .entries
            .get(key)
            .and_then(|entry| entry.document.row(0))
        else {
            return false;
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

    pub fn render_images(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        scroll: usize,
        show_images: bool,
    ) -> usize {
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
    fn canonical_argument_cache_serializes_only_once_per_visible_value() {
        let value = ValueRef::from(serde_json::json!({
            "text": format!("{}🦀{}", "a".repeat(9000), "b".repeat(8187))
        }));
        let serialized = serde_json::to_string_pretty(&value).unwrap();
        let limit = MAX_TOOL_ARGUMENT_PREVIEW_BYTES;
        let start = serialized.len() - limit;
        assert!(!serialized.is_char_boundary(start));
        let mut transcript = Transcript::default();
        transcript.begin(80);
        let key = "tool:arguments";
        let first = transcript.canonical_arguments(key.into(), &value);
        assert_eq!(first.total_bytes, serialized.len());
        assert_eq!(
            first.text,
            serialized[serialized.ceil_char_boundary(start)..]
        );
        assert!(
            transcript.is_empty(),
            "collapsed arguments add no visible rows"
        );
        transcript.top(10, 0);
        transcript.begin(60);
        let cached = transcript.canonical_arguments(key.into(), &value.clone());
        assert!(Rc::ptr_eq(&first, &cached));
        let replacement = transcript.canonical_arguments(key.into(), &ValueRef::from(42));
        assert_eq!(replacement.text, "42");
        assert!(!Rc::ptr_eq(&first, &replacement));
        transcript.top(10, 0);
        transcript.begin(60);
        transcript.top(10, 0);
        assert!(transcript.entries.is_empty());
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
