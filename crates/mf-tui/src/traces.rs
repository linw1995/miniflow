//! Bounded OTLP span retention and terminal trace inspection.

use crate::{
    SpanData,
    duration::format_duration_ns,
    logs::{LogObservation, LogSnapshot},
    snapshots::{HistorySnapshot, SnapshotMetadata, span_data_preview},
    state::TraceAvailability,
};
use crossterm::event::KeyCode;
use opentelemetry_proto::tonic::{
    common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value},
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
pub type SpanKey = (Vec<u8>, Vec<u8>);

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pane {
    #[default]
    Details,
    Data,
    Logs,
}

#[derive(PartialEq, Eq)]
struct PreviewKey {
    pane: Pane,
    correlation: Option<SpanKey>,
    log_id: Option<usize>,
}

pub struct TraceViewSnapshot<'a> {
    pub traces: &'a TraceSnapshot,
    pub logs: &'a LogSnapshot,
    pub data: &'a HistorySnapshot,
    pub spans: TraceAvailability,
    pub finished: bool,
}

#[derive(Default)]
pub struct TraceView {
    pub visible: bool,
    selected: Option<SpanKey>,
    scroll: u16,
    preview: Option<(PreviewKey, String)>,
    pane: Pane,
    session_logs: bool,
    focus_logs: bool,
    selected_log: Option<usize>,
    data_list: DataList,
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

    fn log_records(&self, traces: &TraceSnapshot, logs: &LogSnapshot) -> Vec<Arc<LogObservation>> {
        if self.session_logs {
            logs.records.clone()
        } else {
            traces
                .rows
                .get(self.position(traces))
                .map_or_else(Vec::new, |row| {
                    logs.related(
                        &row.observation.span.trace_id,
                        &row.observation.span.span_id,
                    )
                })
        }
    }

    fn log_position(&self, records: &[Arc<LogObservation>]) -> usize {
        self.selected_log
            .and_then(|selected| records.iter().position(|record| record.id == selected))
            .unwrap_or(records.len().saturating_sub(1))
    }

    pub fn correlation(&self, traces: &TraceSnapshot, logs: &LogSnapshot) -> Option<SpanKey> {
        if self.session_logs {
            let records = self.log_records(traces, logs);
            let record = &records.get(self.log_position(&records))?.record;
            (record.trace_id.len() == 16
                && record.span_id.len() == 8
                && record.trace_id.iter().any(|byte| *byte != 0)
                && record.span_id.iter().any(|byte| *byte != 0))
            .then(|| (record.trace_id.clone(), record.span_id.clone()))
        } else {
            traces
                .rows
                .get(self.position(traces))
                .map(|row| row.observation.key())
        }
    }

    pub fn handle_data_key(
        &mut self,
        key: KeyCode,
        traces: &TraceSnapshot,
        data: &HistorySnapshot,
    ) -> bool {
        if !self.visible || self.pane != Pane::Data {
            return false;
        }
        if key == KeyCode::Char('l') && !self.session_logs {
            self.data_list.focused = !self.data_list.focused;
            return true;
        }
        if !self.session_logs && !self.data_list.focused {
            return false;
        }
        let correlation = traces
            .rows
            .get(self.position(traces))
            .map(|row| row.observation.key());
        let rows = self.data_list.rows(self.session_logs, correlation, data);
        let handled = self.data_list.navigate(key, &rows, data);
        if handled {
            self.scroll = 0;
        }
        handled
    }

    pub fn handle_key(&mut self, key: KeyCode, traces: &TraceSnapshot, logs: &LogSnapshot) -> bool {
        if key == KeyCode::Char('v') {
            self.visible = !self.visible;
            return true;
        }
        if !self.visible {
            return false;
        }
        match key {
            KeyCode::Esc => self.visible = false,
            KeyCode::Tab => {
                self.pane = match (self.session_logs, self.pane) {
                    (_, Pane::Details) => Pane::Data,
                    (_, Pane::Data) => Pane::Logs,
                    (true, Pane::Logs) => Pane::Data,
                    (false, Pane::Logs) => Pane::Details,
                };
                self.focus_logs = false;
                self.data_list.focused = false;
            }
            KeyCode::Char('g') => {
                self.session_logs = !self.session_logs;
                self.pane = if self.session_logs {
                    Pane::Logs
                } else {
                    Pane::Details
                };
                self.focus_logs = false;
                self.data_list.focused = false;
            }
            KeyCode::Char('l') if self.pane == Pane::Logs => self.focus_logs = !self.focus_logs,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Up
            | KeyCode::Char('k')
            | KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char('f') => {
                let logs_active = self.session_logs || (self.pane == Pane::Logs && self.focus_logs);
                let records = if logs_active {
                    self.log_records(traces, logs)
                } else {
                    Vec::new()
                };
                let count = if logs_active {
                    records.len()
                } else {
                    traces.rows.len()
                };
                let current = if logs_active {
                    self.log_position(&records)
                } else {
                    self.position(traces)
                };
                let index = match key {
                    KeyCode::Up | KeyCode::Char('k') => Some(current.saturating_sub(1)),
                    KeyCode::Down | KeyCode::Char('j') => {
                        Some((current + 1).min(count.saturating_sub(1)))
                    }
                    KeyCode::Home => Some(0),
                    _ => None,
                };
                if logs_active {
                    self.selected_log =
                        index.and_then(|index| records.get(index).map(|record| record.id));
                } else {
                    self.selected = index
                        .and_then(|index| traces.rows.get(index).map(|row| row.observation.key()));
                    self.selected_log = None;
                }
            }
            _ => {}
        }
        true
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>, snapshot: &TraceViewSnapshot<'_>) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        frame.render_widget(Paragraph::new(format!(
            "OTEL Tracing: {} spans | {} received | {} span drops\nLogs: {} retained | {} received | {} log drops | {}",
            snapshot.traces.rows.len(), snapshot.spans.observed_spans, snapshot.spans.local_drops,
            snapshot.logs.records.len(), snapshot.logs.observed_records, snapshot.logs.local_drops,
            if snapshot.finished { "Finished" } else { "Collecting" }
        )), header);
        let [entries, details] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .areas(body);
        let selected = self.position(snapshot.traces);
        let records = self.log_records(snapshot.traces, snapshot.logs);
        let log_selected = self.log_position(&records);
        let data = (self.pane == Pane::Data).then_some(snapshot.data);
        if self.session_logs && data.is_none() {
            draw_logs(frame, entries, &records, log_selected, "Session LogRecords");
        } else if !self.session_logs {
            let rows = usize::from(entries.height.saturating_sub(2)).max(1);
            let first = selected
                .saturating_sub(rows / 2)
                .min(snapshot.traces.rows.len().saturating_sub(rows));
            let items: Vec<_> = snapshot.traces.rows.iter().skip(first).take(rows).map(|row| {
                let span = &row.observation.span;
                let node = span.attributes.iter().find(|attribute| attribute.key == "mf.node.id")
                    .and_then(|attribute| attribute.value.as_ref()).and_then(|value| match value.value.as_ref() {
                        Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value)) => Some(value.as_str()), _ => None,
                    });
                ListItem::new(format!("{}{}{} {} {}", "  ".repeat(row.depth.min(16)), label(&span.name),
                    node.map_or(String::new(), |node| format!(" [{}]", label(node))), row.observation.status(),
                    format_duration_ns(span.end_time_unix_nano.saturating_sub(span.start_time_unix_nano))))
                    .style(Style::default().fg(match row.observation.status() { "ERROR" => Color::Red, "OK" => Color::Green, _ => Color::Gray }))
            }).collect();
            let mut state =
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
                &mut state,
            );
        }
        if let Some(data) = data {
            let correlation = snapshot
                .traces
                .rows
                .get(selected)
                .map(|row| row.observation.key());
            let rows = self.data_list.rows(self.session_logs, correlation, data);
            let selected = self.data_list.position(&rows, data);
            let sequence = rows
                .get(selected)
                .map(|index| data.metadata[*index].sequence);
            if self.data_list.preview_sequence != sequence {
                self.scroll = 0;
                self.data_list.preview_sequence = sequence;
            }
            let preview_area = if self.session_logs {
                draw_data_rows(
                    frame,
                    entries,
                    &rows,
                    data,
                    selected,
                    "Session Data (j/k: select)",
                );
                details
            } else {
                let [list_area, preview_area] = Layout::vertical([
                    Constraint::Length((details.height / 3).clamp(3, 10)),
                    Constraint::Min(1),
                ])
                .areas(details);
                draw_data_rows(
                    frame,
                    list_area,
                    &rows,
                    data,
                    selected,
                    if self.data_list.focused {
                        "Invocation Data (focused)"
                    } else {
                        "Invocation Data (l: focus)"
                    },
                );
                preview_area
            };
            let text = data_preview(
                rows.get(selected).copied(),
                data,
                self.data_list.selected.is_none(),
                snapshot.finished,
            );
            let paragraph = Paragraph::new(text)
                .block(
                    Block::default()
                        .title("[Data] | Tab: pane")
                        .borders(Borders::ALL),
                )
                .wrap(Wrap { trim: false });
            self.scroll = self.scroll.min(
                paragraph
                    .line_count(preview_area.width.saturating_sub(2))
                    .saturating_sub(usize::from(preview_area.height.saturating_sub(2)))
                    .min(u16::MAX as usize) as u16,
            );
            frame.render_widget(paragraph.scroll((self.scroll, 0)), preview_area);
            frame.render_widget(Paragraph::new("Tab: pane | g: spans/session | l: focus related Data | j/k: select | Home/End/f: first/follow\nPgUp/PgDn: page focused list / scroll preview | v/Esc: graph | Ctrl-C: interrupt/close"), footer);
            return;
        }
        let (details, title) = if self.pane == Pane::Logs && !self.session_logs {
            let [logs_area, details] = Layout::vertical([
                Constraint::Length((details.height / 3).clamp(3, 8)),
                Constraint::Min(1),
            ])
            .areas(details);
            draw_logs(
                frame,
                logs_area,
                &records,
                log_selected,
                if self.focus_logs {
                    "LogRecords (focused)"
                } else {
                    "LogRecords (l: focus)"
                },
            );
            (details, "Details | Data | [Logs]")
        } else {
            (
                details,
                match (self.session_logs, self.pane) {
                    (true, Pane::Data) => "[Data] | Logs",
                    (true, _) => "Data | [Logs]",
                    (false, Pane::Details) => "[Details] | Data | Logs",
                    (false, Pane::Data) => "Details | [Data] | Logs",
                    (false, Pane::Logs) => "Details | Data | [Logs]",
                },
            )
        };
        let log_id = (self.pane == Pane::Logs || self.session_logs)
            .then(|| records.get(log_selected).map(|record| record.id))
            .flatten();
        let key = PreviewKey {
            pane: self.pane,
            correlation: self.correlation(snapshot.traces, snapshot.logs),
            log_id,
        };
        // Data and absence messages can change without changing the selected identity.
        if self.pane == Pane::Data
            || (self.pane == Pane::Details && key.correlation.is_none())
            || self
                .preview
                .as_ref()
                .is_none_or(|(previous, _)| *previous != key)
        {
            let text = match self.pane {
                Pane::Data => unreachable!("Data is rendered from the shared history list"),
                Pane::Logs => records.get(log_selected).map_or_else(|| "No matching LogRecords were received.".into(), |record| log_preview(record)),
                Pane::Details => snapshot.traces.rows.get(selected).map_or_else(|| {
                    if snapshot.finished { "No trace spans were received.\nUse g to inspect session logs; spans may be unsampled or unavailable.".into() }
                    else { "Waiting for exported spans.\nSpans appear after completion and export. Use g for live session logs.".into() }
                }, |row| span_preview(&row.observation)),
            };
            if self.preview.as_ref().is_none_or(|(previous, _)| {
                previous.pane != key.pane
                    || previous.correlation != key.correlation
                    || previous.log_id != key.log_id
            }) {
                self.scroll = 0;
            }
            self.preview = Some((key, text));
        }
        let text = &self.preview.as_ref().expect("preview is initialized").1;
        let paragraph = Paragraph::new(text.as_str())
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        self.scroll = self.scroll.min(
            paragraph
                .line_count(details.width.saturating_sub(2))
                .saturating_sub(usize::from(details.height.saturating_sub(2)))
                .min(u16::MAX as usize) as u16,
        );
        frame.render_widget(paragraph.scroll((self.scroll, 0)), details);
        frame.render_widget(
            Paragraph::new(
                "Up/Down select | Tab pane | ? help | Esc graph\nCtrl-C interrupt/close",
            ),
            footer,
        );
    }
}

#[derive(Default)]
struct DataList {
    scope: Option<(bool, Option<SpanKey>)>,
    selected: Option<usize>,
    focused: bool,
    preview_sequence: Option<usize>,
}

impl DataList {
    fn rows(
        &mut self,
        session: bool,
        correlation: Option<SpanKey>,
        snapshot: &HistorySnapshot,
    ) -> Vec<usize> {
        let scope = (session, if session { None } else { correlation });
        if self.scope.as_ref() != Some(&scope) {
            self.scope = Some(scope.clone());
            self.selected = None;
        }
        snapshot
            .metadata
            .iter()
            .enumerate()
            .filter_map(|(index, metadata)| {
                (session
                    || scope
                        .1
                        .as_ref()
                        .is_some_and(|key| metadata.correlation.as_ref() == Some(key)))
                .then_some(index)
            })
            .collect()
    }
    fn position(&self, rows: &[usize], snapshot: &HistorySnapshot) -> usize {
        self.selected
            .and_then(|sequence| {
                rows.iter()
                    .position(|index| snapshot.metadata[*index].sequence == sequence)
            })
            .unwrap_or(rows.len().saturating_sub(1))
    }
    fn navigate(&mut self, key: KeyCode, rows: &[usize], snapshot: &HistorySnapshot) -> bool {
        let current = self.position(rows, snapshot);
        let index = match key {
            KeyCode::Up | KeyCode::Char('k') => Some(current.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                Some((current + 1).min(rows.len().saturating_sub(1)))
            }
            KeyCode::PageUp => Some(current.saturating_sub(10)),
            KeyCode::PageDown => Some((current + 10).min(rows.len().saturating_sub(1))),
            KeyCode::Home => Some(0),
            KeyCode::End | KeyCode::Char('f') => None,
            _ => return false,
        };
        self.selected = index.and_then(|index| {
            rows.get(index)
                .map(|index| snapshot.metadata[*index].sequence)
        });
        true
    }
}

fn data_row(entry: &mf_runtime::SnapshotEntry, metadata: &SnapshotMetadata) -> String {
    let node = entry
        .snapshot
        .node(&entry.scope, &entry.node)
        .expect("history entries contain their node");
    format!(
        "#{} {} {:?} t={}",
        metadata.sequence,
        label(&entry.node),
        node.outcome,
        metadata.time_unix_nano
    )
}

fn draw_data_rows(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    rows: &[usize],
    snapshot: &HistorySnapshot,
    selected: usize,
    title: &str,
) {
    let visible = usize::from(area.height.saturating_sub(2)).max(1);
    let first = selected
        .saturating_sub(visible / 2)
        .min(rows.len().saturating_sub(visible));
    let items: Vec<_> = rows
        .iter()
        .skip(first)
        .take(visible)
        .map(|index| {
            ListItem::new(data_row(
                &snapshot.entries[*index],
                &snapshot.metadata[*index],
            ))
        })
        .collect();
    let mut state =
        ListState::default().with_selected((!items.is_empty()).then_some(selected - first));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().title(title).borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn data_preview(
    index: Option<usize>,
    snapshot: &HistorySnapshot,
    following: bool,
    finished: bool,
) -> String {
    let mut text = format!(
        "{} | {}\nRows={} values={}\n",
        snapshot.status,
        if following {
            "Following tail"
        } else {
            "Inspecting selection"
        },
        snapshot.history_len,
        snapshot.value_count
    );
    if let Some(index) = index {
        let entry = &snapshot.entries[index];
        let metadata = &snapshot.metadata[index];
        text.push_str(&format!("{}\n", data_row(entry, metadata)));
        for scope in &entry.scope {
            text.push_str(&format!(
                "Scope: {}[{}]\n",
                label(&scope.loop_id),
                scope.index.get()
            ));
        }
        if let Some((trace, span)) = &metadata.correlation {
            text.push_str(&format!("Trace={} span={}\n", hex(trace), hex(span)));
        } else {
            text.push_str("Native context absent; no inferred span association.\n");
        }
        text.push_str(&span_data_preview(
            &SpanData {
                node: entry.snapshot.node(&entry.scope, &entry.node).cloned(),
                status: snapshot.status.clone(),
            },
            finished,
        ));
    } else {
        text.push_str("No snapshot entries in this scope. Use g then Tab for session Data.");
    }
    text
}

fn draw_logs(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    records: &[Arc<LogObservation>],
    selected: usize,
    title: &str,
) {
    let rows = usize::from(area.height.saturating_sub(2)).max(1);
    let first = selected
        .saturating_sub(rows / 2)
        .min(records.len().saturating_sub(rows));
    let items: Vec<_> = records
        .iter()
        .skip(first)
        .take(rows)
        .map(|observation| {
            let record = &observation.record;
            ListItem::new(format!(
                "#{} {} {}",
                observation.id + 1,
                label(&record.severity_text),
                label(if !record.event_name.is_empty() {
                    &record.event_name
                } else if let Some(any_value::Value::StringValue(body)) =
                    record.body.as_ref().and_then(|body| body.value.as_ref())
                {
                    body
                } else {
                    "(unnamed log)"
                })
            ))
        })
        .collect();
    let mut state =
        ListState::default().with_selected((!items.is_empty()).then_some(selected - first));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().title(title).borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn log_preview(observation: &LogObservation) -> String {
    let mut preview = Preview(String::new());
    let record = &observation.record;
    let result = (|| -> fmt::Result {
        writeln!(
            preview,
            "LogRecord #{}\nEvent: {}\nTime (unix ns): {}\nObserved (unix ns): {}\nSeverity: {} ({})\nTrace ID: {}\nSpan ID: {}\nFlags: {}",
            observation.id + 1,
            record.event_name,
            record.time_unix_nano,
            record.observed_time_unix_nano,
            record.severity_text,
            record.severity_number,
            hex(&record.trace_id),
            hex(&record.span_id),
            record.flags
        )?;
        if let Some(scope) = &observation.scope {
            writeln!(preview, "Scope: {} {}", scope.name, scope.version)?;
            attributes(&mut preview, "Scope attributes", &scope.attributes)?;
        }
        if let Some(resource) = &observation.resource {
            attributes(&mut preview, "Resource attributes", &resource.attributes)?;
        }
        writeln!(preview, "\nBody:")?;
        write_value(&mut preview, record.body.as_ref())?;
        writeln!(preview)?;
        attributes(&mut preview, "Log attributes", &record.attributes)?;
        writeln!(
            preview,
            "\nExporter omissions: {} attributes",
            record.dropped_attributes_count
        )
    })();
    if result.is_err() {
        preview.0.push_str("\n[Preview truncated at 64 KiB]");
    }
    preview.0
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
        let mut end = available.min(text.len());
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
        if end == text.len() {
            return Ok(());
        }
        Err(fmt::Error)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_value(preview: &mut Preview, value: Option<&AnyValue>) -> fmt::Result {
    match value.and_then(|value| value.value.as_ref()) {
        None => write!(preview, "null"),
        Some(any_value::Value::StringValue(value)) => write!(preview, "{value:?}"),
        Some(any_value::Value::BoolValue(value)) => write!(preview, "{value}"),
        Some(any_value::Value::IntValue(value)) => write!(preview, "{value}"),
        Some(any_value::Value::DoubleValue(value)) => write!(preview, "{value}"),
        Some(any_value::Value::BytesValue(value)) => {
            write!(preview, "bytes(")?;
            for byte in value {
                write!(preview, "{byte:02x}")?;
            }
            write!(preview, ")")
        }
        Some(any_value::Value::ArrayValue(value)) => {
            write!(preview, "[")?;
            for (index, item) in value.values.iter().enumerate() {
                if index != 0 {
                    write!(preview, ", ")?;
                }
                write_value(preview, Some(item))?;
            }
            write!(preview, "]")
        }
        Some(any_value::Value::KvlistValue(value)) => {
            write!(preview, "{{")?;
            for (index, item) in value.values.iter().enumerate() {
                if index != 0 {
                    write!(preview, ", ")?;
                }
                write!(preview, "{:?}: ", item.key)?;
                write_value(preview, item.value.as_ref())?;
            }
            write!(preview, "}}")
        }
        Some(value) => write!(preview, "{value:?}"),
    }
}

fn attributes(preview: &mut Preview, title: &str, attributes: &[KeyValue]) -> fmt::Result {
    if attributes.is_empty() {
        return Ok(());
    }
    writeln!(preview, "\n{title}:")?;
    for attribute in attributes {
        write!(preview, "  {}: ", attribute.key)?;
        write_value(preview, attribute.value.as_ref())?;
        writeln!(preview)?;
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
            if span.parent_span_id.is_empty() || span.parent_span_id.iter().all(|byte| *byte == 0) {
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
        let mut view = TraceView::default();
        view.handle_key(KeyCode::Char('v'), &before, &LogSnapshot::default());
        view.handle_key(KeyCode::End, &before, &LogSnapshot::default());
        view.handle_key(KeyCode::Down, &before, &LogSnapshot::default());
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
        view.handle_key(KeyCode::Home, &after, &LogSnapshot::default());
        assert_eq!(view.position(&after), 0);
        view.handle_key(KeyCode::End, &after, &LogSnapshot::default());
        assert!(view.selected.is_none());
        view.handle_key(KeyCode::Esc, &after, &LogSnapshot::default());
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
            .draw(|frame| {
                view.draw(
                    frame,
                    &TraceViewSnapshot {
                        traces: &store.snapshot(),
                        logs: &LogSnapshot::default(),
                        data: &HistorySnapshot::default(),
                        spans: TraceAvailability {
                            observed_spans: 1,
                            local_drops: 3,
                        },
                        finished: true,
                    },
                )
            })
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
            "3 span drops",
            "Spans (parent / child)",
            "[Details]",
            "ERROR",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        let mut view = TraceView::default();
        for (finished, expected) in [
            (false, "Waiting for exported spans"),
            (true, "No trace spans were received"),
        ] {
            terminal
                .draw(|frame| {
                    view.draw(
                        frame,
                        &TraceViewSnapshot {
                            traces: &TraceSnapshot::default(),
                            logs: &LogSnapshot::default(),
                            data: &HistorySnapshot::default(),
                            spans: TraceAvailability::default(),

                            finished,
                        },
                    )
                })
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
    fn panes_refresh_data_and_keep_log_selection_stable_across_reordering() {
        use crate::logs::LogStore;
        use mf_runtime::{NodeSnapshot, SnapshotOutcome, ValueRef};
        use opentelemetry_proto::tonic::logs::v1::LogRecord;
        let mut spans = TraceStore::default();
        spans.admit(observation(1, 1, None, 0));
        let traces = spans.snapshot();
        let mut logs = LogStore::default();
        for time in [100, 200] {
            logs.admit(
                LogRecord {
                    time_unix_nano: time,
                    trace_id: vec![1; 16],
                    span_id: vec![1; 8],
                    event_name: format!("log-{time}"),
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("log-payload".into())),
                    }),
                    ..Default::default()
                },
                None,
                None,
            );
        }
        let data = |value| {
            let recording = mf_runtime::SnapshotRecording::memory();
            recording.record(
                vec![],
                "node",
                NodeSnapshot {
                    inputs: ValueRef::object([]),
                    outputs: ValueRef::object([("answer".into(), ValueRef::from(value))]),
                    skipped: Arc::from([]),
                    outcome: SnapshotOutcome::Succeeded,
                    error: None,
                },
            );
            HistorySnapshot {
                history_len: 1,
                entries: recording.history(),
                metadata: vec![SnapshotMetadata {
                    sequence: 1,
                    time_unix_nano: 1,
                    correlation: Some((vec![1; 16], vec![1; 8])),
                }],
                status: "Complete".into(),
                ..Default::default()
            }
        };
        let mut view = TraceView::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        view.handle_key(KeyCode::Char('v'), &traces, &logs.snapshot());
        view.handle_key(KeyCode::Tab, &traces, &logs.snapshot());
        for value in [42, 73] {
            terminal
                .draw(|frame| {
                    view.draw(
                        frame,
                        &TraceViewSnapshot {
                            traces: &traces,
                            logs: &logs.snapshot(),
                            data: &data(value),
                            spans: TraceAvailability::default(),
                            finished: true,
                        },
                    )
                })
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(&value.to_string()), "{text}");
            assert!(
                text.contains("Inputs:") && text.contains("Outputs:"),
                "{text}"
            );
        }
        view.handle_key(KeyCode::Tab, &traces, &logs.snapshot());
        view.handle_key(KeyCode::Char('l'), &traces, &logs.snapshot());
        view.handle_key(KeyCode::Home, &traces, &logs.snapshot());
        assert_eq!(view.selected_log, Some(0));
        logs.admit(
            LogRecord {
                time_unix_nano: 50,
                trace_id: vec![1; 16],
                span_id: vec![1; 8],
                ..Default::default()
            },
            None,
            None,
        );
        let records = view.log_records(&traces, &logs.snapshot());
        assert_eq!(records[view.log_position(&records)].id, 0);
        terminal
            .draw(|frame| {
                view.draw(
                    frame,
                    &TraceViewSnapshot {
                        traces: &traces,
                        logs: &logs.snapshot(),
                        data: &data(73),
                        spans: TraceAvailability::default(),
                        finished: true,
                    },
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            text.contains("log-100") && text.contains("log-payload"),
            "{text}"
        );
        view.handle_key(KeyCode::Char('g'), &traces, &logs.snapshot());
        assert!(view.session_logs);
        assert_eq!(
            view.correlation(&traces, &logs.snapshot()),
            Some((vec![1; 16], vec![1; 8]))
        );
        view.handle_key(KeyCode::Esc, &traces, &logs.snapshot());
        assert!(!view.visible);
    }

    #[test]
    fn session_logs_and_data_remain_available_without_exported_spans() {
        use crate::logs::LogStore;
        use opentelemetry_proto::tonic::logs::v1::LogRecord;
        let traces = TraceSnapshot::default();
        let mut logs = LogStore::default();
        logs.admit(
            LogRecord {
                trace_id: vec![1; 16],
                span_id: vec![2; 8],
                event_name: "live-node-log".into(),
                ..Default::default()
            },
            None,
            None,
        );
        let mut view = TraceView::default();
        view.handle_key(KeyCode::Char('v'), &traces, &logs.snapshot());
        view.handle_key(KeyCode::Char('g'), &traces, &logs.snapshot());
        assert_eq!(
            view.correlation(&traces, &logs.snapshot()),
            Some((vec![1; 16], vec![2; 8]))
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| {
                view.draw(
                    frame,
                    &TraceViewSnapshot {
                        traces: &traces,
                        logs: &logs.snapshot(),
                        data: &HistorySnapshot::default(),
                        spans: TraceAvailability::default(),
                        finished: false,
                    },
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            text.contains("Session LogRecords") && text.contains("live-node-log"),
            "{text}"
        );
        view.handle_key(KeyCode::Tab, &traces, &logs.snapshot());
        terminal
            .draw(|frame| {
                view.draw(
                    frame,
                    &TraceViewSnapshot {
                        traces: &traces,
                        logs: &logs.snapshot(),
                        data: &HistorySnapshot::default(),
                        spans: TraceAvailability::default(),
                        finished: false,
                    },
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("No snapshot entries in this scope"), "{text}");
        let mut tiny = Terminal::new(TestBackend::new(1, 1)).unwrap();
        tiny.draw(|frame| {
            view.draw(
                frame,
                &TraceViewSnapshot {
                    traces: &traces,
                    logs: &logs.snapshot(),

                    data: &HistorySnapshot::default(),
                    spans: TraceAvailability::default(),

                    finished: false,
                },
            )
        })
        .unwrap();
    }

    #[test]
    fn data_lists_filter_native_context_and_preserve_selection() {
        let recording = mf_runtime::SnapshotRecording::memory();
        for value in [42, 73, 99] {
            recording.record(
                vec![mf_telemetry::event::LoopPathEntry {
                    loop_id: "repeat".into(),
                    index: 2_i64.try_into().unwrap(),
                }],
                "same",
                mf_runtime::NodeSnapshot {
                    inputs: mf_runtime::ValueRef::object([]),
                    outputs: mf_runtime::ValueRef::from(serde_json::json!({"value": value})),
                    skipped: Arc::from([]),
                    outcome: mf_runtime::SnapshotOutcome::Emitted,
                    error: None,
                },
            );
        }
        let mut snapshot = HistorySnapshot {
            history_len: 3,
            entries: recording.history(),
            metadata: vec![
                SnapshotMetadata {
                    sequence: 4,
                    time_unix_nano: 1,
                    correlation: Some((vec![1; 16], vec![1; 8])),
                },
                SnapshotMetadata {
                    sequence: 7,
                    time_unix_nano: 2,
                    correlation: Some((vec![2; 16], vec![1; 8])),
                },
                SnapshotMetadata {
                    sequence: 9,
                    time_unix_nano: 3,
                    correlation: Some((vec![1; 16], vec![2; 8])),
                },
            ],
            status: "Collecting".into(),
            ..Default::default()
        };
        let mut list = DataList::default();
        assert_eq!(
            list.rows(false, Some((vec![1; 16], vec![1; 8])), &snapshot),
            [0]
        );
        assert_eq!(
            list.rows(false, Some((vec![2; 16], vec![1; 8])), &snapshot),
            [1]
        );
        assert_eq!(
            list.rows(false, Some((vec![1; 16], vec![2; 8])), &snapshot),
            [2]
        );
        let rows = list.rows(true, None, &snapshot);
        assert_eq!(rows, [0, 1, 2]);
        assert_eq!(list.position(&rows, &snapshot), 2);
        assert!(list.navigate(KeyCode::Home, &rows, &snapshot));
        assert!(list.navigate(KeyCode::Down, &rows, &snapshot));
        snapshot.entries.push(snapshot.entries[2].clone());
        snapshot.metadata.push(SnapshotMetadata {
            sequence: 12,
            time_unix_nano: 4,
            correlation: None,
        });
        snapshot.history_len += 1;
        snapshot.status = "snapshot history is incomplete".into();
        let rows = list.rows(true, None, &snapshot);
        assert_eq!(list.position(&rows, &snapshot), 1);
        assert_eq!(list.selected, Some(7));
        let text = data_preview(
            Some(rows[list.position(&rows, &snapshot)]),
            &snapshot,
            false,
            true,
        );
        assert!(
            text.contains("73")
                && text.contains("Scope: repeat[2]")
                && text.contains("incomplete")
                && text.contains("Inspecting selection"),
            "{text}"
        );
        assert!(list.navigate(KeyCode::Char('f'), &rows, &snapshot));
        assert_eq!(list.position(&rows, &snapshot), 3);
        assert!(data_preview(Some(3), &snapshot, true, false).contains("Native context absent"));
    }

    #[test]
    fn log_previews_are_bounded_and_format_structured_values_readably() {
        use opentelemetry_proto::tonic::{
            common::v1::{ArrayValue, KeyValueList},
            logs::v1::LogRecord,
        };
        let mut observation = LogObservation {
            id: 0,
            resource: None,
            scope: None,
            record: LogRecord {
                body: Some(AnyValue {
                    value: Some(any_value::Value::KvlistValue(KeyValueList {
                        values: vec![KeyValue {
                            key: "values".into(),
                            value: Some(AnyValue {
                                value: Some(any_value::Value::ArrayValue(ArrayValue {
                                    values: vec![
                                        AnyValue {
                                            value: Some(any_value::Value::IntValue(42)),
                                        },
                                        AnyValue {
                                            value: Some(any_value::Value::BoolValue(true)),
                                        },
                                    ],
                                })),
                            }),
                            ..Default::default()
                        }],
                    })),
                }),
                ..Default::default()
            },
        };
        assert!(log_preview(&observation).contains("{\"values\": [42, true]}"));
        observation.record.body = Some(AnyValue {
            value: Some(any_value::Value::StringValue("é".repeat(PREVIEW_BYTES))),
        });
        let mut span = self::observation(1, 1, None, 0);
        span.span.name = "é".repeat(PREVIEW_BYTES);
        for preview in [log_preview(&observation), span_preview(&span)] {
            assert!(preview.contains("Preview truncated"));
            assert!(preview.len() < PREVIEW_BYTES + 100);
        }
        assert_eq!(span.span.name.len(), PREVIEW_BYTES * 2);
    }
}
