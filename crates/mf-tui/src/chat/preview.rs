use mf_chat::{
    ChatDatabase, Invocation,
    display::{DraftWriter, PreviewSegment},
};
use snafu::Snafu;
use std::{collections::VecDeque, sync::Arc};

#[derive(Debug, Snafu)]
pub enum PreviewError {
    #[snafu(transparent)]
    Storage { source: mf_chat::ChatError },
    #[snafu(display("application preview does not match the staged draft"))]
    ReceiptMismatch,
}

#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    pub invocation: Option<Invocation>,
    pub segments: Vec<Arc<PreviewSegment>>,
    pub warning: Option<Arc<PreviewError>>,
    pub incomplete: bool,
    pub revision: u64,
}

// Only the runner owns a writer. Published frames and body handles are read-only.
#[derive(Default)]
pub struct Capture {
    pub frame: LivePreview,
    writer: Option<DraftWriter>,
    // The application Flow delivers receipts in emission order. Text/argument handles
    // refer to durable prefixes; replaceable output frames carry a bounded snapshot.
    pending: VecDeque<PreviewSegment>,
}

impl std::ops::Deref for Capture {
    type Target = LivePreview;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl Capture {
    pub fn begin(&mut self, db: &ChatDatabase, invocation: Invocation) {
        let (writer, warning) = match DraftWriter::open(db, invocation.clone()) {
            Ok(writer) => (Some(writer), None),
            Err(error) => (None, Some(Arc::new(error.into()))),
        };
        let revision = self.revision.saturating_add(1);
        self.writer = writer;
        self.pending.clear();
        self.frame = LivePreview {
            invocation: Some(invocation),
            warning,
            revision,
            ..Default::default()
        };
    }

    #[cfg(test)]
    pub fn apply(&mut self, event: mf_chat::PreviewEvent) -> Result<(), mf_chat::ChatError> {
        if self.invocation.as_ref() == Some(&event.invocation) {
            self.stage(std::slice::from_ref(&event.delta));
        }
        self.deliver(event)
    }

    pub fn deliver(&mut self, event: mf_chat::PreviewEvent) -> Result<(), mf_chat::ChatError> {
        if self.invocation.as_ref() != Some(&event.invocation) {
            return Ok(());
        }
        event.validate()?;
        let matches_part = |s: &PreviewSegment| match &event.delta {
            mf_chat::PreviewDelta::Text { delta } => {
                s.model_call_id == delta.model_call_id && s.tool.is_none() && s.output.is_none()
            }
            mf_chat::PreviewDelta::ToolCall { delta } => {
                s.model_call_id == delta.model_call_id
                    && s.tool.as_ref().is_some_and(|t| t.index == delta.index)
            }
            mf_chat::PreviewDelta::ToolOutput { delta } => {
                s.output.as_ref() == Some(&delta.context)
            }
        };
        let Some(segment) = self.pending.pop_front() else {
            return Ok(());
        };
        self.frame.revision = self.revision.saturating_add(1);
        let matches_snapshot = match &event.delta {
            mf_chat::PreviewDelta::ToolOutput { delta } if segment.output.is_some() => {
                segment.text.load()? == delta.value
            }
            _ => true,
        };
        if !matches_part(&segment) || !matches_snapshot {
            self.pending.clear();
            self.frame.warning = Some(Arc::new(ReceiptMismatchSnafu.build()));
            return Ok(());
        }
        let previous = self
            .frame
            .segments
            .iter_mut()
            .rev()
            .find(|old| matches_part(old));
        if let Some(previous) = previous {
            *previous = Arc::new(segment);
        } else {
            self.frame.segments.push(Arc::new(segment));
        }
        Ok(())
    }

    pub fn stage(&mut self, deltas: &[mf_chat::PreviewDelta]) {
        if deltas.is_empty() || self.warning.is_some() {
            return;
        }
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        match writer.append(deltas) {
            Ok(segments) => self.pending.extend(segments),
            // Do not publish undurable content or fail the runner on a display error.
            Err(error) => self.warn(error),
        }
    }

    pub fn seal(&mut self) {
        if let Some(writer) = self.writer.take()
            && !(self.segments.is_empty() && self.pending.is_empty())
            && let Err(error) = writer.seal()
        {
            self.warn(error);
        }
    }

    fn warn(&mut self, error: mf_chat::ChatError) {
        self.frame.warning = Some(Arc::new(error.into()));
        self.frame.revision = self.revision.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{capture, preview_turn};
    use super::*;
    use mf_chat::{PreviewDelta, PreviewEvent, TextDelta, ToolCallDelta};

    fn event(invocation: &Invocation, delta: impl Into<PreviewDelta>) -> PreviewEvent {
        PreviewEvent {
            invocation: invocation.clone(),
            delta: delta.into(),
        }
    }

    fn text(invocation: &Invocation, model: &str, fragment: &str) -> PreviewEvent {
        event(
            invocation,
            TextDelta {
                text: fragment.into(),
                model_call_id: model.into(),
            },
        )
    }

    fn call(invocation: &Invocation, model: &str, index: u64, arguments: &str) -> PreviewEvent {
        event(
            invocation,
            ToolCallDelta {
                model_call_id: model.into(),
                index,
                arguments: arguments.into(),
                id: None,
                name: None,
            },
        )
    }

    #[test]
    fn same_call_output_receipts_cannot_publish_a_different_staged_snapshot() {
        let (_root, db, inv) = preview_turn();
        let mut preview = capture(&db, &inv);
        let event = |value: &str| {
            event(
                &inv,
                PreviewDelta::ToolOutput {
                    delta: mf_chat::ToolOutputUpdate {
                        context: mf_chat::ToolOutputContext {
                            model_call_id: "m".into(),
                            tool_call_id: "t".into(),
                            name: "bash".into(),
                        },
                        value: value.into(),
                    },
                },
            )
        };
        let first = event("before");
        let last = event("after");
        preview.stage(&[first.delta.clone(), last.delta.clone()]);
        preview.deliver(first.clone()).unwrap();
        assert_eq!(preview.segments[0].text.load().unwrap(), "before");
        preview.deliver(first).unwrap();
        assert!(matches!(
            preview.warning.as_deref(),
            Some(PreviewError::ReceiptMismatch)
        ));
        assert_eq!(preview.segments[0].text.load().unwrap(), "before");
    }

    #[test]
    fn previews_are_grouped_by_model_call_and_ignore_other_turns() {
        let (_root, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        for (model, fragment) in [("first", "one"), ("second", "two"), ("first", "-tail")] {
            preview.apply(text(&invocation, model, fragment)).unwrap();
        }
        preview
            .apply(text(&Invocation::new("session"), "third", "stale"))
            .unwrap();
        assert_eq!(preview.segments.len(), 2);
        assert_eq!(preview.segments[0].text.load().unwrap(), "one-tail");
        assert_eq!(preview.segments[1].text.load().unwrap(), "two");
    }

    #[test]
    fn interleaved_tool_arguments_keep_their_raw_text_and_model_part_identity() {
        let (_root, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        for event in [
            text(&invocation, "first", "Preamble"),
            event(
                &invocation,
                ToolCallDelta {
                    model_call_id: "first".into(),
                    index: 0,
                    arguments: "{ \"query\":".into(),
                    id: Some("first-call".into()),
                    name: Some("lookup".into()),
                },
            ),
            call(&invocation, "first", 1, "{}"),
            call(&invocation, "second", 0, "[]"),
            call(&Invocation::new("session"), "first", 0, "ignored"),
            call(&invocation, "first", 0, " \"a\" }"),
        ] {
            preview.apply(event).unwrap();
        }

        assert_eq!(
            preview
                .segments
                .iter()
                .map(|segment| segment.text.load().unwrap())
                .collect::<Vec<_>>(),
            ["Preamble", "{ \"query\": \"a\" }", "{}", "[]"]
        );
        let tool = preview.segments[1].tool.as_ref().unwrap();
        assert_eq!(tool.id.as_deref(), Some("first-call"));
        assert_eq!(tool.name.as_deref(), Some("lookup"));
    }

    #[test]
    fn long_turns_keep_every_segment_and_complete_utf8_payloads() {
        let (_root, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        let answer = "é".repeat(70 * 1024);
        let argument = "ü".repeat(5 * 1024);
        preview.apply(text(&invocation, "model", &answer)).unwrap();
        for index in 0..40 {
            preview
                .apply(call(&invocation, "model", index, &argument))
                .unwrap();
        }
        let first = preview.clone();
        preview
            .apply(call(&invocation, "model", 0, "-end"))
            .unwrap();
        preview.apply(text(&invocation, "last", "Done")).unwrap();

        assert_eq!(preview.segments.len(), 42);
        assert_eq!(preview.segments[0].text.load().unwrap(), answer);
        let updated = format!("{argument}-end");
        for (index, segment) in preview.segments[1..41].iter().enumerate() {
            assert_eq!(segment.tool.as_ref().unwrap().index, index as u64);
            assert_eq!(
                &segment.text.load().unwrap(),
                if index == 0 { &updated } else { &argument }
            );
        }
        assert_eq!(preview.segments[41].text.load().unwrap(), "Done");
        assert_eq!(first.segments.len(), 41);
        assert_eq!(first.segments[1].text.load().unwrap(), argument);
    }

    #[test]
    fn a_rejected_draft_write_is_visible_but_does_not_reject_delivery() {
        let (_root, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        preview.apply(text(&invocation, "model", "prefix")).unwrap();
        db.fail_turn(invocation.clone(), "cancelled", "stopped")
            .unwrap();
        preview.apply(text(&invocation, "model", "more")).unwrap();
        let Some(PreviewError::Storage { source }) = preview.warning.as_deref() else {
            panic!("expected a storage warning");
        };
        assert!(matches!(source, mf_chat::ChatError::Conflict { id } if id == &invocation.call_id));
        let revision = preview.revision;
        preview
            .apply(text(&invocation, "model", "after failure"))
            .unwrap();
        assert_eq!(preview.revision, revision);
        assert_eq!(preview.segments[0].text.load().unwrap(), "prefix");
        preview.begin(&db, Invocation::new("another session"));
        assert!(preview.warning.is_none());
        assert!(preview.segments.is_empty());
    }

    #[test]
    fn a_failed_draft_open_keeps_the_database_error_in_the_published_frame() {
        let (_root, db, invocation) = preview_turn();
        std::fs::remove_file(db.path()).unwrap();
        std::fs::create_dir(db.path()).unwrap();
        let mut capture = capture(&db, &invocation);
        capture
            .apply(text(&invocation, "model", "not stored"))
            .unwrap();
        let frame = capture.frame.clone();
        drop(capture);
        let warning = frame.warning.as_deref().unwrap();
        assert!(matches!(warning, PreviewError::Storage {
            source: mf_chat::ChatError::Connect { path, .. }
        } if path == db.path()));
        assert!(std::error::Error::source(warning).is_some());
        assert!(frame.segments.is_empty());
    }

    #[test]
    fn only_matching_application_receipts_publish_staged_handles_even_after_settlement() {
        let (_root, db, invocation) = preview_turn();
        let mut capture = capture(&db, &invocation);
        let a = text(&invocation, "model", "A");
        let b = call(&invocation, "model", 0, "B");
        capture.stage(&[a.delta.clone(), b.delta.clone()]);
        capture
            .deliver(text(&Invocation::new("other"), "model", "A"))
            .unwrap();
        assert!(capture.frame.segments.is_empty());
        capture.seal();
        db.complete_turn(invocation, mf_chat::ConversationResult::assistant("final"))
            .unwrap();
        capture.deliver(a).unwrap();
        assert_eq!(capture.frame.segments.len(), 1);
        assert_eq!(capture.frame.segments[0].text.load().unwrap(), "A");
        capture.deliver(b).unwrap();
        assert_eq!(capture.frame.segments[1].text.load().unwrap(), "B");
    }

    #[test]
    fn inconsistent_receipts_never_publish_another_parts_body() {
        let (_root, db, invocation) = preview_turn();
        let mut capture = capture(&db, &invocation);
        capture.stage(&[call(&invocation, "first", 1, "other").delta]);
        capture
            .deliver(call(&invocation, "first", 2, "not staged"))
            .unwrap();
        let warning = capture.warning.as_deref().unwrap();
        assert!(matches!(warning, PreviewError::ReceiptMismatch));
        assert_eq!(
            warning.to_string(),
            "application preview does not match the staged draft"
        );
        assert!(capture.segments.is_empty());
    }

    #[test]
    fn long_identities_and_late_hints_do_not_drop_or_merge_calls() {
        let (_root, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        let prefix = "m".repeat(256);
        let model = format!("{prefix}-first");
        let other = format!("{prefix}-second");
        let id = "i".repeat(300);
        let name = "n".repeat(300);
        preview
            .apply(call(&invocation, &model, 0, "first"))
            .unwrap();
        preview
            .apply(call(&invocation, &other, 0, "second"))
            .unwrap();
        preview
            .apply(event(
                &invocation,
                ToolCallDelta {
                    model_call_id: model.clone(),
                    index: 0,
                    arguments: String::new(),
                    id: Some(id.clone()),
                    name: Some(name.clone()),
                },
            ))
            .unwrap();
        assert_eq!(preview.segments.len(), 2);
        let first = &preview.segments[0];
        assert_eq!(first.model_call_id, model);
        assert_eq!(first.text.load().unwrap(), "first");
        let tool = first.tool.as_ref().unwrap();
        assert_eq!(tool.id.as_ref(), Some(&id));
        assert_eq!(tool.name.as_ref(), Some(&name));
        assert_eq!(preview.segments[1].model_call_id, other);
        assert_eq!(preview.segments[1].text.load().unwrap(), "second");
        assert!(preview.segments[1].tool.as_ref().unwrap().id.is_none());
    }
}
