//! Shared business value history received through OTLP logs.
use crate::traces::SpanKey;
use crossterm::event::KeyCode;
use mf_runtime::{NodeSnapshot, SnapshotEntry, SnapshotRecord, SnapshotStore, ValueRef};
use mf_telemetry::snapshot::Assembler;
use opentelemetry_proto::tonic::{common::v1::any_value, logs::v1::LogRecord};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Arc,
};

#[derive(Debug, Snafu)]
pub enum CaptureError {
    #[snafu(transparent)]
    Transport {
        source: mf_telemetry::snapshot::SnapshotError,
    },
    #[snafu(display("invalid snapshot record: {source}"))]
    Record { source: serde_json::Error },
    #[snafu(transparent)]
    Store { source: mf_runtime::SnapshotError },
    #[snafu(display("snapshot sequence {sequence} has conflicting trace context"))]
    ConflictingContext { sequence: usize },
    #[snafu(display("snapshot capture is closed"))]
    Closed,
    #[snafu(display("{message}"))]
    Unavailable { message: String },
    #[snafu(display(
        "snapshot history is incomplete at sequence {sequence}; received history remains available"
    ))]
    Incomplete { sequence: usize },
}

const PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct SnapshotCapture {
    pub store: SnapshotStore,
    pub diagnostic: Option<Arc<CaptureError>>,
    assembler: Assembler,
    finished: bool,
    pending_contexts: BTreeMap<usize, Option<SpanKey>>,
    nodes_by_span: BTreeMap<SpanKey, Arc<NodeSnapshot>>,
    revision: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SpanData {
    pub node: Option<Arc<NodeSnapshot>>,
    pub status: String,
    pub revision: usize,
}

#[derive(Clone, Debug)]
pub struct HistorySnapshot {
    pub history_len: usize,
    pub value_count: usize,
    pub selected: usize,
    pub first: usize,
    pub entries: Vec<SnapshotEntry>,
    pub status: String,
}

impl SnapshotCapture {
    pub fn admit(&mut self, record: LogRecord) -> Result<(), Arc<CaptureError>> {
        if self.finished {
            return Err(Arc::new(ClosedSnafu.build()));
        }
        if let Some(error) = &self.diagnostic {
            return Err(error.clone());
        }
        self.revision = self.revision.saturating_add(1);
        let result = (|| -> Result<(), CaptureError> {
            let first = self.assembler.next_sequence();
            let sequence = record
                .attributes
                .iter()
                .find(|attribute| attribute.key == "mf.snapshot.sequence")
                .and_then(|attribute| attribute.value.as_ref())
                .and_then(|value| value.value.as_ref())
                .and_then(|value| match value {
                    any_value::Value::IntValue(sequence) => usize::try_from(*sequence).ok(),
                    _ => None,
                });
            let correlation = (record.trace_id.len() == 16
                && record.span_id.len() == 8
                && record.trace_id.iter().any(|byte| *byte != 0)
                && record.span_id.iter().any(|byte| *byte != 0))
            .then(|| (record.trace_id.clone(), record.span_id.clone()));
            if let Some(sequence) = sequence.filter(|sequence| *sequence >= first) {
                ensure!(
                    self.pending_contexts
                        .get(&sequence)
                        .is_none_or(|previous| previous == &correlation),
                    ConflictingContextSnafu { sequence }
                );
                self.pending_contexts.insert(sequence, correlation);
            }
            for (offset, body) in self.assembler.push(record)?.into_iter().enumerate() {
                let record: SnapshotRecord = serde_json::from_value(body).context(RecordSnafu)?;
                let node = match &record {
                    SnapshotRecord::Node { scope, node, .. } => Some((scope.clone(), node.clone())),
                    _ => None,
                };
                self.store.apply(record)?;
                let correlation = self.pending_contexts.remove(&(first + offset)).flatten();
                if let (Some((scope, node)), Some(correlation)) = (node, correlation) {
                    let snapshot = self
                        .store
                        .current()
                        .node(&scope, &node)
                        .expect("admitted node snapshots exist in the store")
                        .clone();
                    self.nodes_by_span.insert(correlation, snapshot);
                }
            }
            Ok(())
        })()
        .map_err(Arc::new);
        if let Err(error) = &result {
            self.diagnostic = Some(error.clone());
        }
        result
    }

    pub fn fail(&mut self, error: &str) {
        self.revision = self.revision.saturating_add(1);
        self.diagnostic
            .get_or_insert_with(|| Arc::new(UnavailableSnafu { message: error }.build()));
    }

    pub fn finish(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.finished = true;
        if self.diagnostic.is_none()
            && (self.assembler.has_pending()
                || (self.store.is_initialized() && !self.store.is_complete()))
        {
            self.diagnostic = Some(Arc::new(
                IncompleteSnafu {
                    sequence: self.assembler.next_sequence(),
                }
                .build(),
            ));
        }
    }

    pub fn data(&self, trace_id: &[u8], span_id: &[u8]) -> SpanData {
        SpanData {
            node: self
                .nodes_by_span
                .get(&(trace_id.to_vec(), span_id.to_vec()))
                .cloned(),
            status: self.view(None, 1).status,
            revision: self.revision,
        }
    }

    pub fn view(&self, selected: Option<usize>, rows: usize) -> HistorySnapshot {
        let history = self.store.history();
        let selected = selected
            .unwrap_or(history.len().saturating_sub(1))
            .min(history.len().saturating_sub(1));
        let rows = rows.max(1);
        let first = selected
            .saturating_sub(rows / 2)
            .min(history.len().saturating_sub(rows));
        let status = if let Some(error) = &self.diagnostic {
            error.to_string()
        } else if self.store.is_complete() {
            "Complete".into()
        } else if self.finished && !self.store.is_initialized() {
            "Data snapshots unavailable; the runner must support telemetry and snapshot capture"
                .into()
        } else {
            "Collecting".into()
        };
        HistorySnapshot {
            history_len: history.len(),
            value_count: self.store.value_count(),
            selected,
            first,
            entries: history.iter().skip(first).take(rows).cloned().collect(),
            status,
        }
    }
}

#[derive(Default)]
pub struct HistoryView {
    pub visible: bool,
    selected: Option<usize>,
    scroll: u16,
    preview: Option<(usize, String)>,
}

impl HistoryView {
    pub fn handle_key(&mut self, key: KeyCode, count: usize) -> bool {
        if key == KeyCode::Char('v') {
            self.visible = !self.visible;
            return true;
        }
        if !self.visible {
            return false;
        }
        let current = self.selected.unwrap_or(count.saturating_sub(1));
        match key {
            KeyCode::Esc => self.visible = false,
            KeyCode::Up | KeyCode::Char('k') => self.selected = Some(current.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = Some((current + 1).min(count.saturating_sub(1)))
            }
            KeyCode::Home => self.selected = Some(0),
            KeyCode::End | KeyCode::Char('f') => self.selected = None,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            _ => {}
        }
        true
    }

    pub fn selection(&self) -> Option<usize> {
        self.selected
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>, capture: &HistorySnapshot) {
        let selected = capture.selected;
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "Data history: {} | {} changes | {} shared values",
                capture.status, capture.history_len, capture.value_count
            )),
            header,
        );
        let [entries, values] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(body);
        let start = capture.first;
        let items: Vec<_> = capture
            .entries
            .iter()
            .enumerate()
            .map(|(offset, entry)| {
                let node = entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .expect("history entries reference an existing node");
                ListItem::new(Line::from(format!(
                    "{} {} {:?}",
                    start + offset + 1,
                    entry_label(entry),
                    node.outcome
                )))
            })
            .collect();
        let mut list_state = ListState::default().with_selected(Some(selected - start));
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().title("Changes").borders(Borders::ALL))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            entries,
            &mut list_state,
        );
        if self
            .preview
            .as_ref()
            .is_none_or(|(index, _)| *index != selected)
        {
            self.preview = capture
                .entries
                .get(selected - start)
                .map(|entry| (selected, entry_preview(entry)));
            self.scroll = 0;
        }
        let text = self.preview.as_ref().map_or(
            "No node inputs or outputs have been received",
            |(_, text)| text.as_str(),
        );
        let paragraph = Paragraph::new(text)
            .block(
                Block::default()
                    .title("Inputs / Outputs")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false });
        self.scroll = self.scroll.min(
            paragraph
                .line_count(values.width.saturating_sub(2))
                .saturating_sub(usize::from(values.height.saturating_sub(2)))
                .min(u16::MAX as usize) as u16,
        );
        frame.render_widget(paragraph.scroll((self.scroll, 0)), values);
        frame.render_widget(Paragraph::new("j/k or arrows: change | Home: first | End/f: follow latest\nPgUp/PgDn: scroll values | v/Esc: graph | Ctrl-C: interrupt/close"), footer);
    }
}

fn entry_label(entry: &SnapshotEntry) -> String {
    let mut label = String::new();
    for scope in &entry.scope {
        label.push_str(&format!("{}[{}]/", scope.loop_id, scope.index.get()));
    }
    label.push_str(&entry.node);
    label
}

fn entry_preview(entry: &SnapshotEntry) -> String {
    let node = entry
        .snapshot
        .node(&entry.scope, &entry.node)
        .expect("history entries reference an existing node");
    let mut text = format!("{}\nStatus: {:?}\n", entry_label(entry), node.outcome);
    if let Some(error) = &node.error {
        text.push_str(&format!("Error: {error}\n"));
    }
    if !node.skipped.is_empty() {
        text.push_str(&format!("Skipped outputs: {:?}\n", node.skipped));
    }
    text.push_str(&format!(
        "\nInputs:\n{}\n\nOutputs:\n{}",
        value_preview(&node.inputs),
        value_preview(&node.outputs)
    ));
    text
}

fn value_preview(value: &ValueRef) -> String {
    struct Preview(Vec<u8>);
    impl Write for Preview {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let remaining = PREVIEW_BYTES.saturating_sub(self.0.len());
            if remaining == 0 {
                return Err(io::Error::other("preview limit"));
            }
            let count = remaining.min(bytes.len());
            self.0.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut preview = Preview(Vec::new());
    let truncated = serde_json::to_writer_pretty(&mut preview, value).is_err();
    let mut text = String::from_utf8_lossy(&preview.0).into_owned();
    if truncated {
        text.push_str("\n[Preview truncated at 64 KiB; the complete value remains in history.]");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeSnapshot, SnapshotOutcome, SnapshotRecord, SnapshotRecorder};
    use mf_telemetry::snapshot::packets;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn snapshot(value: ValueRef) -> NodeSnapshot {
        NodeSnapshot {
            inputs: ValueRef::object([]),
            outputs: ValueRef::object([("value".into(), value)]),
            skipped: Arc::from([]),
            outcome: SnapshotOutcome::Succeeded,
            error: None,
        }
    }
    fn recorder(capture: Arc<Mutex<SnapshotCapture>>) -> SnapshotRecorder {
        let mut sequence = 0;
        SnapshotRecorder::with_sink(move |record| {
            for packet in packets(sequence, serde_json::to_value(record).unwrap())? {
                capture
                    .lock()
                    .unwrap()
                    .admit(packet.into_log_record("workflow", "run"))?;
            }
            sequence += 1;
            Ok(())
        })
        .unwrap()
    }

    #[test]
    fn capture_retains_record_decode_sources_until_presentation() {
        use std::error::Error;
        let mut capture = SnapshotCapture::default();
        let record = packets(0, json!({"record":"unknown"}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        let error = capture.admit(record.clone()).unwrap_err();
        assert!(error.source().unwrap().is::<serde_json::Error>());
        let repeated = capture.admit(record).unwrap_err();
        assert!(repeated.source().unwrap().is::<serde_json::Error>());
        capture.finish();
        assert_eq!(capture.view(None, 1).status, error.to_string());
    }

    #[test]
    fn otlp_history_shares_payloads_and_preserves_historical_roots() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let recorder = recorder(capture.clone());
        let value = ValueRef::from(json!({"payload": "unique payload", "count": 1}));
        recorder.record(vec![], "first", snapshot(value.clone()));
        let first = capture.lock().unwrap().store.current().clone();
        recorder.record(
            vec![],
            "second",
            snapshot(ValueRef::from(serde_json::to_value(&value).unwrap())),
        );
        recorder.record(
            vec![],
            "first",
            snapshot(json!({"payload": "unique payload", "count": 2}).into()),
        );
        recorder.finish();
        let mut capture = capture.lock().unwrap();
        capture.finish();
        assert!(capture.diagnostic.is_none());
        assert!(capture.store.is_complete());
        assert_eq!(capture.store.history().len(), 3);
        assert_eq!(
            first.node(&[], "first").unwrap().outputs["value"]["count"],
            json!(1)
        );
        let latest = capture.store.current();
        assert!(
            first.node(&[], "first").unwrap().outputs["value"]["payload"]
                .ptr_eq(&latest.node(&[], "first").unwrap().outputs["value"]["payload"])
        );
        let window = capture.view(None, 1);
        assert_eq!(window.entries.len(), 1);
        assert_eq!(window.history_len, 3);
        assert!(window.entries[0].snapshot.ptr_eq(latest));
    }

    #[test]
    fn missing_records_and_value_references_leave_explicit_incomplete_history() {
        let mut capture = SnapshotCapture::default();
        let header = serde_json::to_value(SnapshotRecord::Header {
            version: mf_runtime::SNAPSHOT_VERSION,
        })
        .unwrap();
        for packet in packets(0, header).unwrap() {
            capture
                .admit(packet.into_log_record("workflow", "run"))
                .unwrap();
        }
        let end = serde_json::to_value(SnapshotRecord::End).unwrap();
        for packet in packets(2, end).unwrap() {
            capture
                .admit(packet.into_log_record("workflow", "run"))
                .unwrap();
        }
        capture.finish();
        assert!(
            capture
                .diagnostic
                .as_ref()
                .unwrap()
                .to_string()
                .contains("sequence 1")
        );
        assert!(!capture.store.is_complete());

        let mut capture = SnapshotCapture::default();
        for (sequence, record) in [
            SnapshotRecord::Header {
                version: mf_runtime::SNAPSHOT_VERSION,
            },
            SnapshotRecord::Value {
                id: 0,
                value: mf_runtime::ValueDefinition::Array(vec![1]),
            },
        ]
        .into_iter()
        .enumerate()
        {
            for packet in packets(sequence, serde_json::to_value(record).unwrap()).unwrap() {
                let _ = capture.admit(packet.into_log_record("workflow", "run"));
            }
        }
        assert!(
            capture
                .diagnostic
                .as_ref()
                .unwrap()
                .to_string()
                .contains("unknown snapshot value")
        );
        assert_eq!(capture.store.value_count(), 0);
    }

    #[test]
    fn history_navigation_preserves_selection_and_renders_values() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let recorder = recorder(capture.clone());
        recorder.record(vec![], "node", snapshot(42.into()));
        recorder.finish();
        capture.lock().unwrap().finish();
        let mut view = HistoryView::default();
        assert!(view.handle_key(KeyCode::Char('v'), 1));
        assert!(view.handle_key(KeyCode::Home, 1));
        assert_eq!(view.selected, Some(0));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let window = capture.lock().unwrap().view(view.selection(), 24);
        terminal.draw(|frame| view.draw(frame, &window)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Inputs / Outputs"));
        assert!(text.contains("42"));
        view.handle_key(KeyCode::End, 2);
        assert!(view.selected.is_none());
        view.handle_key(KeyCode::Esc, 2);
        assert!(!view.visible);
    }

    #[test]
    fn large_previews_are_bounded_without_truncating_stored_values() {
        let value = ValueRef::from("x".repeat(PREVIEW_BYTES * 2));
        let preview = value_preview(&value);
        assert!(preview.contains("Preview truncated"));
        assert_eq!(value.as_str().unwrap().len(), PREVIEW_BYTES * 2);
    }
    #[test]
    fn reordered_records_keep_repeated_nodes_associated_with_their_own_spans() {
        let mut capture = SnapshotCapture::default();
        let bodies = [
            json!({"record":"header","version":1}),
            json!({"record":"value","id":0,"value":{"kind":"object","data":{}}}),
            json!({"record":"value","id":1,"value":{"kind":"string","data":"first invocation"}}),
            json!({"record":"value","id":2,"value":{"kind":"object","data":{"result":1}}}),
            json!({"record":"node","scope":[],"node":"same","inputs":0,"outputs":2,"skipped":[],"outcome":"succeeded"}),
            json!({"record":"value","id":3,"value":{"kind":"string","data":"second invocation"}}),
            json!({"record":"value","id":4,"value":{"kind":"object","data":{"result":3}}}),
            json!({"record":"node","scope":[],"node":"same","inputs":0,"outputs":4,"skipped":[],"outcome":"succeeded"}),
            json!({"record":"end"}),
        ];
        for (sequence, body) in bodies.into_iter().enumerate().rev() {
            for packet in packets(sequence, body).unwrap() {
                let mut record = packet.into_log_record("workflow", "run");
                record.trace_id = vec![1; 16];
                record.span_id = vec![if sequence < 5 { 1 } else { 2 }; 8];
                capture.admit(record).unwrap();
            }
        }
        capture.finish();
        assert_eq!(
            capture.data(&[1; 16], &[1; 8]).node.unwrap().outputs["result"],
            json!("first invocation")
        );
        assert_eq!(
            capture.data(&[1; 16], &[2; 8]).node.unwrap().outputs["result"],
            json!("second invocation")
        );
        assert!(capture.data(&[2; 16], &[1; 8]).node.is_none());
        assert!(capture.pending_contexts.is_empty());
        assert_eq!(capture.data(&[1; 16], &[1; 8]).status, "Complete");
    }

    #[test]
    fn fragmented_node_records_preserve_context_and_reject_conflicting_fragments() {
        let mut capture = SnapshotCapture::default();
        for (sequence, body) in [
            json!({"record":"header","version":1}),
            json!({"record":"value","id":0,"value":{"kind":"object","data":{}}}),
        ]
        .into_iter()
        .enumerate()
        {
            for packet in packets(sequence, body).unwrap() {
                capture
                    .admit(packet.into_log_record("workflow", "run"))
                    .unwrap();
            }
        }
        let body = json!({"record":"node","scope":[],"node":"step","inputs":0,"outputs":0,"skipped":[],"outcome":"failed","error":"x".repeat(100_000)});
        let mut records: Vec<_> = packets(2, body.clone())
            .unwrap()
            .map(|packet| {
                let mut record = packet.into_log_record("workflow", "run");
                record.trace_id = vec![1; 16];
                record.span_id = vec![2; 8];
                record
            })
            .collect();
        assert!(records.len() > 1);
        for record in records.iter().rev() {
            capture.admit(record.clone()).unwrap();
        }
        assert_eq!(
            capture
                .data(&[1; 16], &[2; 8])
                .node
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .len(),
            100_000
        );
        assert!(capture.pending_contexts.is_empty());
        let mut capture = SnapshotCapture::default();
        capture.admit(records.remove(0)).unwrap();
        records[0].span_id = vec![3; 8];
        let error = capture.admit(records[0].clone()).unwrap_err();
        assert!(matches!(
            error.as_ref(),
            CaptureError::ConflictingContext { sequence: 2 }
        ));
        assert!(matches!(
            capture.admit(records[0].clone()).unwrap_err().as_ref(),
            CaptureError::ConflictingContext { sequence: 2 }
        ));
    }

    #[test]
    fn missing_context_does_not_guess_a_span_and_retries_do_not_grow_pending_contexts() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let recorder = recorder(capture.clone());
        recorder.record(vec![], "step", snapshot(42.into()));
        recorder.finish();
        let mut capture = capture.lock().unwrap();
        capture.finish();
        assert!(capture.data(&[1; 16], &[2; 8]).node.is_none());
        let duplicate = packets(0, json!({"record":"header","version":1}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        let mut capture = SnapshotCapture::default();
        for _ in 0..100 {
            capture.admit(duplicate.clone()).unwrap();
        }
        assert!(capture.pending_contexts.is_empty());
    }
}
