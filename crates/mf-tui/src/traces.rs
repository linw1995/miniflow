//! Bounded OTLP span retention and terminal trace inspection.

use crate::duration::format_duration_ns;
use crossterm::event::KeyCode;
use opentelemetry_proto::tonic::{
    common::v1::{InstrumentationScope, KeyValue},
    resource::v1::Resource,
    trace::v1::{Span, span::SpanKind, status::StatusCode},
};
use prost::Message;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Write},
    sync::Arc,
};

const MAX_SPANS: usize = 4096;
const MAX_TRACE_BYTES: usize = 16 * 1024 * 1024;
const PREVIEW_BYTES: usize = 64 * 1024;
type SpanKey = (Vec<u8>, Vec<u8>);

#[derive(Clone, Debug)]
pub struct TraceSpan {
    pub span: Span,
    pub resource: Option<Arc<Resource>>,
    pub scope: Option<Arc<InstrumentationScope>>,
}

impl TraceSpan {
    fn key(&self) -> SpanKey {
        (self.span.trace_id.clone(), self.span.span_id.clone())
    }

    fn status(&self) -> &'static str {
        match self.span.status.as_ref().map(|status| status.code) {
            Some(code) if code == StatusCode::Ok as i32 => "OK",
            Some(code) if code == StatusCode::Error as i32 => "ERROR",
            _ => "Unset",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TraceRow {
    pub depth: usize,
    pub observation: Arc<TraceSpan>,
}

#[derive(Clone, Debug, Default)]
pub struct TraceSnapshot {
    pub rows: Vec<TraceRow>,
}

#[derive(Default)]
pub struct TraceStore {
    spans: BTreeMap<SpanKey, Arc<TraceSpan>>,
    bytes: usize,
    cached: Option<Arc<TraceSnapshot>>,
}

impl TraceStore {
    /// Duplicate exports are idempotent. Retained spans never disappear during inspection.
    pub fn admit(&mut self, observation: TraceSpan) -> bool {
        let key = observation.key();
        if self.spans.contains_key(&key) {
            return true;
        }
        let bytes = observation.span.encoded_len()
            + observation
                .resource
                .as_ref()
                .map_or(0, |resource| resource.encoded_len())
            + observation
                .scope
                .as_ref()
                .map_or(0, |scope| scope.encoded_len());
        if self.spans.len() >= MAX_SPANS || bytes > MAX_TRACE_BYTES.saturating_sub(self.bytes) {
            return false;
        }
        self.bytes += bytes;
        self.spans.insert(key, Arc::new(observation));
        self.cached = None;
        true
    }

    pub fn snapshot(&mut self) -> Arc<TraceSnapshot> {
        if let Some(snapshot) = &self.cached {
            return snapshot.clone();
        }
        let mut children: BTreeMap<SpanKey, Vec<SpanKey>> = BTreeMap::new();
        let mut roots = Vec::new();
        for (key, observation) in &self.spans {
            let parent = (key.0.clone(), observation.span.parent_span_id.clone());
            if parent != *key && self.spans.contains_key(&parent) {
                children.entry(parent).or_default().push(key.clone());
            } else {
                roots.push(key.clone());
            }
        }
        let sort = |keys: &mut Vec<SpanKey>| {
            keys.sort_by_key(|key| (self.spans[key].span.start_time_unix_nano, key.clone()));
        };
        roots.sort_by_key(|key| {
            (
                key.0.clone(),
                self.spans[key].span.start_time_unix_nano,
                key.1.clone(),
            )
        });
        for keys in children.values_mut() {
            sort(keys);
        }
        let mut rows = Vec::new();
        let mut seen = BTreeSet::new();
        // The fallback also exposes malformed cycles instead of hiding their spans.
        for root in roots.into_iter().chain(self.spans.keys().cloned()) {
            let mut pending = vec![(root, 0)];
            while let Some((key, depth)) = pending.pop() {
                if !seen.insert(key.clone()) {
                    continue;
                }
                rows.push(TraceRow {
                    depth,
                    observation: self.spans[&key].clone(),
                });
                if let Some(children) = children.get(&key) {
                    pending.extend(
                        children
                            .iter()
                            .rev()
                            .map(|child| (child.clone(), depth + 1)),
                    );
                }
            }
        }
        let snapshot = Arc::new(TraceSnapshot { rows });
        self.cached = Some(snapshot.clone());
        snapshot
    }
}

#[derive(Default)]
pub struct TraceView {
    pub visible: bool,
    selected: Option<SpanKey>,
    scroll: u16,
    preview: Option<(SpanKey, String)>,
}

impl TraceView {
    fn position(&self, snapshot: &TraceSnapshot) -> usize {
        self.selected
            .as_ref()
            .and_then(|key| {
                snapshot
                    .rows
                    .iter()
                    .position(|row| row.observation.key() == *key)
            })
            .unwrap_or(snapshot.rows.len().saturating_sub(1))
    }

    pub fn handle_key(&mut self, key: KeyCode, snapshot: &TraceSnapshot) -> bool {
        if key == KeyCode::Char('v') {
            self.visible = !self.visible;
            return true;
        }
        if !self.visible {
            return false;
        }
        let current = self.position(snapshot);
        let index = match key {
            KeyCode::Esc => {
                self.visible = false;
                None
            }
            KeyCode::Up | KeyCode::Char('k') => Some(current.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                Some((current + 1).min(snapshot.rows.len().saturating_sub(1)))
            }
            KeyCode::Home => Some(0),
            KeyCode::End | KeyCode::Char('f') => {
                self.selected = None;
                None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                None
            }
            _ => None,
        };
        if let Some(index) = index {
            self.selected = snapshot.rows.get(index).map(|row| row.observation.key());
        }
        true
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        snapshot: &TraceSnapshot,
        observed: u64,
        dropped: u64,
        finished: bool,
    ) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "OTEL Tracing: {} retained | {observed} received | {dropped} drops | {}",
                snapshot.rows.len(),
                if finished { "Finished" } else { "Collecting" }
            )),
            header,
        );
        let [entries, details] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .areas(body);
        let selected = self.position(snapshot);
        let rows = usize::from(entries.height.saturating_sub(2)).max(1);
        let first = selected
            .saturating_sub(rows / 2)
            .min(snapshot.rows.len().saturating_sub(rows));
        let items: Vec<_> = snapshot
            .rows
            .iter()
            .skip(first)
            .take(rows)
            .map(|row| {
                let span = &row.observation.span;
                let node = span
                    .attributes
                    .iter()
                    .find(|attribute| attribute.key == "mf.node.id")
                    .and_then(|attribute| attribute.value.as_ref())
                    .and_then(|value| match value.value.as_ref() {
                        Some(
                            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                value,
                            ),
                        ) => Some(value.as_str()),
                        _ => None,
                    });
                ListItem::new(format!(
                    "{}{}{} {} {}",
                    "  ".repeat(row.depth.min(16)),
                    label(&span.name),
                    node.map_or(String::new(), |node| format!(" [{}]", label(node))),
                    row.observation.status(),
                    format_duration_ns(
                        span.end_time_unix_nano
                            .saturating_sub(span.start_time_unix_nano)
                    )
                ))
                .style(Style::default().fg(match row.observation.status() {
                    "ERROR" => Color::Red,
                    "OK" => Color::Green,
                    _ => Color::Gray,
                }))
            })
            .collect();
        let mut list_state =
            ListState::default().with_selected((!items.is_empty()).then_some(selected - first));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .title("Spans (parent / child)")
                        .borders(Borders::ALL),
                )
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            entries,
            &mut list_state,
        );
        if let Some(row) = snapshot.rows.get(selected) {
            let key = row.observation.key();
            if self
                .preview
                .as_ref()
                .is_none_or(|(previous, _)| *previous != key)
            {
                self.preview = Some((key, span_preview(&row.observation)));
                self.scroll = 0;
            }
        }
        let empty = if finished {
            "No trace spans were received.\nThe runner must support OTLP tracing with sampled spans."
        } else {
            "Waiting for exported spans.\nSpans appear after they finish and the exporter sends them."
        };
        let paragraph = Paragraph::new(
            self.preview
                .as_ref()
                .map_or(empty, |(_, text)| text.as_str()),
        )
        .block(Block::default().title("Span details").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
        self.scroll = self.scroll.min(
            paragraph
                .line_count(details.width.saturating_sub(2))
                .saturating_sub(usize::from(details.height.saturating_sub(2)))
                .min(u16::MAX as usize) as u16,
        );
        frame.render_widget(paragraph.scroll((self.scroll, 0)), details);
        frame.render_widget(Paragraph::new("j/k or arrows: span | Home: first | End/f: follow last\nPgUp/PgDn: scroll details | v/Esc: graph | Ctrl-C: interrupt/close"), footer);
    }
}

fn label(value: &str) -> String {
    let mut characters = value.chars();
    let mut text: String = characters
        .by_ref()
        .take(80)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if characters.next().is_some() {
        text.push_str("...");
    }
    text
}

struct Preview(String);
impl Write for Preview {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let available = PREVIEW_BYTES.saturating_sub(self.0.len());
        if text.len() <= available {
            self.0.extend(text.chars().map(|character| {
                if character.is_control() && !matches!(character, '\n' | '\t') {
                    ' '
                } else {
                    character
                }
            }));
            return Ok(());
        }
        let mut end = available;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.0.extend(text[..end].chars().map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                ' '
            } else {
                character
            }
        }));
        Err(fmt::Error)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn attributes(preview: &mut Preview, title: &str, attributes: &[KeyValue]) -> fmt::Result {
    writeln!(preview, "\n{title}:")?;
    for attribute in attributes {
        writeln!(
            preview,
            "  {}: {:?}",
            attribute.key,
            attribute
                .value
                .as_ref()
                .and_then(|value| value.value.as_ref())
        )?;
    }
    Ok(())
}

fn span_preview(observation: &TraceSpan) -> String {
    let mut preview = Preview(String::new());
    let span = &observation.span;
    let result = (|| -> fmt::Result {
        writeln!(
            preview,
            "{}\nTrace ID: {}\nSpan ID: {}\nParent span ID: {}\nKind: {:?}\nStatus: {}\nStart (unix ns): {}\nEnd (unix ns): {}\nDuration: {}",
            span.name,
            hex(&span.trace_id),
            hex(&span.span_id),
            if span.parent_span_id.is_empty() {
                "root".into()
            } else {
                hex(&span.parent_span_id)
            },
            SpanKind::try_from(span.kind).unwrap_or(SpanKind::Unspecified),
            observation.status(),
            span.start_time_unix_nano,
            span.end_time_unix_nano,
            format_duration_ns(
                span.end_time_unix_nano
                    .saturating_sub(span.start_time_unix_nano)
            )
        )?;
        if let Some(status) = &span.status {
            writeln!(preview, "Status message: {}", status.message)?;
        }
        if let Some(scope) = &observation.scope {
            writeln!(preview, "Scope: {} {}", scope.name, scope.version)?;
            attributes(&mut preview, "Scope attributes", &scope.attributes)?;
        }
        if let Some(resource) = &observation.resource {
            attributes(&mut preview, "Resource attributes", &resource.attributes)?;
        }
        attributes(&mut preview, "Span attributes", &span.attributes)?;
        writeln!(preview, "\nEvents:")?;
        for event in &span.events {
            writeln!(
                preview,
                "  +{} {}",
                format_duration_ns(
                    event
                        .time_unix_nano
                        .saturating_sub(span.start_time_unix_nano)
                ),
                event.name
            )?;
            attributes(&mut preview, "Event attributes", &event.attributes)?;
        }
        writeln!(preview, "\nLinks:")?;
        for link in &span.links {
            writeln!(
                preview,
                "  trace={} span={}",
                hex(&link.trace_id),
                hex(&link.span_id)
            )?;
            attributes(&mut preview, "Link attributes", &link.attributes)?;
        }
        writeln!(
            preview,
            "\nExporter omissions: {} attributes, {} events, {} links",
            span.dropped_attributes_count, span.dropped_events_count, span.dropped_links_count
        )
    })();
    if result.is_err() {
        preview.0.push_str("\n[Preview truncated at 64 KiB]");
    }
    preview.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        common::v1::{AnyValue, any_value},
        trace::v1::Status,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn observation(trace: u8, id: u8, parent: Option<u8>, start: u64) -> TraceSpan {
        TraceSpan {
            span: Span {
                trace_id: vec![trace; 16],
                span_id: vec![id; 8],
                parent_span_id: parent.map_or_else(Vec::new, |parent| vec![parent; 8]),
                name: format!("span-{id}"),
                start_time_unix_nano: start,
                end_time_unix_nano: start + 42_000,
                ..Default::default()
            },
            resource: None,
            scope: None,
        }
    }

    #[test]
    fn reparents_out_of_order_spans_without_crossing_traces_or_losing_selection() {
        let mut store = TraceStore::default();
        store.admit(observation(1, 2, Some(1), 200));
        store.admit(observation(2, 1, None, 100));
        let before = store.snapshot();
        assert!(Arc::ptr_eq(&before, &store.snapshot()));
        let mut view = TraceView::default();
        view.handle_key(KeyCode::Char('v'), &before);
        view.handle_key(KeyCode::End, &before);
        view.handle_key(KeyCode::Down, &before);
        let selected = view.selected.clone();
        store.admit(observation(1, 1, None, 50));
        let after = store.snapshot();
        assert_eq!(
            after
                .rows
                .iter()
                .map(|row| (
                    row.observation.span.trace_id[0],
                    row.observation.span.span_id[0],
                    row.depth
                ))
                .collect::<Vec<_>>(),
            [(1, 1, 0), (1, 2, 1), (2, 1, 0)]
        );
        assert_eq!(
            after.rows[view.position(&after)].observation.key(),
            selected.unwrap()
        );
        assert_eq!(before.rows.len(), 2);
        view.handle_key(KeyCode::Home, &after);
        assert_eq!(view.position(&after), 0);
        view.handle_key(KeyCode::End, &after);
        assert!(view.selected.is_none());
        view.handle_key(KeyCode::Esc, &after);
        assert!(!view.visible);
    }

    #[test]
    fn retains_orphans_and_cycles_once_and_deduplicates_retries() {
        let mut store = TraceStore::default();
        for (id, parent) in [(1, Some(2)), (2, Some(1)), (3, Some(9)), (4, Some(4))] {
            assert!(store.admit(observation(1, id, parent, 0)));
        }
        assert!(store.admit(observation(1, 1, Some(2), 0)));
        let snapshot = store.snapshot();
        assert_eq!(snapshot.rows.len(), 4);
        assert_eq!(
            snapshot
                .rows
                .iter()
                .map(|row| row.observation.key())
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
    }

    #[test]
    fn bounds_retention_by_count_and_bytes_without_losing_existing_spans() {
        let mut store = TraceStore::default();
        for id in 0..MAX_SPANS {
            let mut item = observation(1, 1, None, id as u64);
            item.span.span_id = (id as u64 + 1).to_be_bytes().to_vec();
            assert!(store.admit(item));
        }
        assert!(!store.admit(observation(2, 1, None, 0)));
        assert_eq!(store.snapshot().rows.len(), MAX_SPANS);
        let mut store = TraceStore::default();
        store.admit(observation(1, 1, None, 0));
        let mut oversized = observation(1, 2, None, 0);
        oversized.span.name = "x".repeat(MAX_TRACE_BYTES);
        assert!(!store.admit(oversized));
        assert_eq!(store.snapshot().rows.len(), 1);
    }

    #[test]
    fn renders_span_details_and_distinguishes_waiting_from_missing_traces() {
        let mut store = TraceStore::default();
        let mut item = observation(1, 1, None, 1_000);
        item.span.status = Some(Status {
            code: StatusCode::Error as i32,
            message: "request failed".into(),
        });
        item.span.attributes.push(KeyValue {
            key: "example.attribute".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue("trace-payload-42".into())),
            }),
            ..Default::default()
        });
        item.span
            .events
            .push(opentelemetry_proto::tonic::trace::v1::span::Event {
                name: "exception".into(),
                time_unix_nano: 2_000,
                ..Default::default()
            });
        item.resource = Some(Arc::new(Resource {
            attributes: vec![KeyValue {
                key: "service.name".into(),
                value: None,
                ..Default::default()
            }],
            ..Default::default()
        }));
        item.scope = Some(Arc::new(InstrumentationScope {
            name: "mf.workflow".into(),
            ..Default::default()
        }));
        let text = span_preview(&item);
        for expected in [
            "Trace ID:",
            "Span ID:",
            "root",
            "42.0µs",
            "ERROR",
            "request failed",
            "example.attribute",
            "trace-payload-42",
            "exception",
            "service.name",
            "mf.workflow",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        store.admit(item);
        let mut view = TraceView::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, &store.snapshot(), 1, 3, true))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "OTEL Tracing",
            "3 drops",
            "Spans (parent / child)",
            "Span details",
            "ERROR",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        for (finished, expected) in [
            (false, "Waiting for exported spans"),
            (true, "No trace spans were received"),
        ] {
            let mut view = TraceView::default();
            terminal
                .draw(|frame| view.draw(frame, &TraceSnapshot::default(), 0, 0, finished))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(expected), "{text}");
        }
    }

    #[test]
    fn bounds_list_labels_and_keeps_control_characters_on_one_line() {
        assert_eq!(label("line\nnext\r\x1b"), "line next  ");
        assert_eq!(
            label(&"x".repeat(100_000)),
            format!("{}...", "x".repeat(80))
        );
        let mut item = observation(1, 1, None, 0);
        item.span.name = "name\x1b\r".into();
        let preview = span_preview(&item);
        assert!(!preview.contains('\x1b'));
        assert!(!preview.contains('\r'));
    }

    #[test]
    fn bounds_utf8_previews_without_truncating_retained_spans() {
        let mut item = observation(1, 1, None, 0);
        item.span.name = "é".repeat(PREVIEW_BYTES);
        let text = span_preview(&item);
        assert!(text.contains("Preview truncated"));
        assert!(text.len() < PREVIEW_BYTES + 100);
        assert_eq!(item.span.name.len(), PREVIEW_BYTES * 2);
    }
}
