use mf_chat::Invocation;
use mf_runtime::{LiveOutputEvent, LiveOutputKind, StreamInvocation};

pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
pub const MAX_PREVIEW_SEGMENTS: usize = 32;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub invocation: StreamInvocation,
    pub text: String,
    pub closed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    pub invocation: Option<Invocation>,
    pub segments: Vec<PreviewSegment>,
    pub omitted: bool,
    pub incomplete: bool,
    pub revision: u64,
    bytes: usize,
}

impl LivePreview {
    pub fn begin(&mut self, invocation: Invocation) {
        let revision = self.revision.saturating_add(1);
        *self = Self {
            invocation: Some(invocation),
            revision,
            ..Default::default()
        };
    }

    pub fn apply(
        &mut self,
        invocation: &Invocation,
        event: LiveOutputEvent,
    ) -> Result<(), mf_runtime::StreamError> {
        if self.invocation.as_ref() != Some(invocation) {
            return Ok(());
        }
        match event.event {
            LiveOutputKind::Open => {
                if self.segments.len() < MAX_PREVIEW_SEGMENTS {
                    self.segments.push(PreviewSegment {
                        invocation: event.invocation,
                        text: String::new(),
                        closed: false,
                    });
                } else {
                    self.omitted = true;
                }
            }
            LiveOutputKind::Value { value } => {
                use snafu::ResultExt;
                let delta = mf_chat::contract::decode_answer_delta(&value)
                    .context(mf_runtime::StreamOutputDecodeSnafu)?;
                if let Some(segment) = self
                    .segments
                    .iter_mut()
                    .find(|segment| segment.invocation == event.invocation)
                {
                    let available = MAX_PREVIEW_BYTES.saturating_sub(self.bytes);
                    let mut length = available.min(delta.text.len());
                    while !delta.text.is_char_boundary(length) {
                        length -= 1;
                    }
                    segment.text.push_str(&delta.text[..length]);
                    self.bytes += length;
                    self.omitted |= length < delta.text.len();
                }
            }
            LiveOutputKind::Close { completed } => {
                if let Some(segment) = self
                    .segments
                    .iter_mut()
                    .find(|segment| segment.invocation == event.invocation)
                {
                    segment.closed = true;
                }
                self.incomplete |= !completed;
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: u64, event: LiveOutputKind) -> LiveOutputEvent {
        LiveOutputEvent {
            export: "answer".into(),
            source: vec!["llm".into()],
            invocation: StreamInvocation {
                id,
                node: "llm".into(),
                path: vec![],
            },
            event,
        }
    }
    fn value(id: u64, text: &str) -> LiveOutputEvent {
        event(
            id,
            LiveOutputKind::Value {
                value: serde_json::json!({"text":text,"model_call_id":format!("call-{id}")}).into(),
            },
        )
    }
    #[test]
    fn repeated_invocations_keep_distinct_provisional_segments() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        for (id, text) in [(1, "first"), (2, "second")] {
            preview
                .apply(&invocation, event(id, LiveOutputKind::Open))
                .unwrap();
            preview.apply(&invocation, value(id, text)).unwrap();
            preview
                .apply(
                    &invocation,
                    event(id, LiveOutputKind::Close { completed: true }),
                )
                .unwrap();
        }
        assert_eq!(preview.segments.len(), 2);
        assert_eq!(preview.segments[0].text, "first");
        assert_eq!(preview.segments[1].text, "second");
        assert!(preview.segments.iter().all(|segment| segment.closed));
        assert!(!preview.incomplete);
    }
    #[test]
    fn previews_bound_segments_and_utf8_bytes_while_continuing_to_consume() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        for id in 0..100 {
            preview
                .apply(&invocation, event(id, LiveOutputKind::Open))
                .unwrap();
        }
        for _ in 0..100 {
            preview
                .apply(&invocation, value(0, &"\u{e9}".repeat(2000)))
                .unwrap();
        }
        assert_eq!(preview.segments.len(), MAX_PREVIEW_SEGMENTS);
        assert_eq!(preview.bytes, MAX_PREVIEW_BYTES);
        assert_eq!(preview.segments[0].text.len(), MAX_PREVIEW_BYTES);
        assert!(preview.omitted);
        preview
            .apply(
                &invocation,
                event(0, LiveOutputKind::Close { completed: false }),
            )
            .unwrap();
        assert!(preview.incomplete);
    }
    #[test]
    fn a_new_turn_rejects_stale_fragments() {
        let old = Invocation::new("session");
        let new = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(new);
        let revision = preview.revision;
        preview.apply(&old, event(1, LiveOutputKind::Open)).unwrap();
        preview.apply(&old, value(1, "stale")).unwrap();
        assert!(preview.segments.is_empty());
        assert_eq!(preview.revision, revision);
    }
}
