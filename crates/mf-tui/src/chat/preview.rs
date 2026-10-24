use mf_chat::{Invocation, PreviewEvent};

pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
pub const MAX_PREVIEW_SEGMENTS: usize = 32;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: String,
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
        event: PreviewEvent,
    ) -> Result<(), mf_chat::ChatError> {
        if self.invocation.as_ref() != Some(invocation) || &event.invocation != invocation {
            return Ok(());
        }
        event.validate()?;
        let index = self
            .segments
            .iter()
            .position(|segment| segment.model_call_id == event.delta.model_call_id);
        let index = if let Some(index) = index {
            index
        } else if self.segments.len() < MAX_PREVIEW_SEGMENTS {
            self.segments.push(PreviewSegment {
                model_call_id: event.delta.model_call_id,
                text: String::new(),
            });
            self.segments.len() - 1
        } else {
            self.omitted = true;
            self.revision = self.revision.saturating_add(1);
            return Ok(());
        };
        let available = MAX_PREVIEW_BYTES.saturating_sub(self.bytes);
        let mut length = available.min(event.delta.text.len());
        while !event.delta.text.is_char_boundary(length) {
            length -= 1;
        }
        self.segments[index]
            .text
            .push_str(&event.delta.text[..length]);
        self.bytes += length;
        self.omitted |= length < event.delta.text.len();
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(invocation: &Invocation, call: &str, text: &str) -> PreviewEvent {
        PreviewEvent {
            invocation: invocation.clone(),
            delta: mf_chat::TextDelta {
                text: text.into(),
                model_call_id: call.into(),
            },
        }
    }

    #[test]
    fn previews_are_grouped_by_model_call_and_ignore_other_turns() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        for (call, text) in [("first", "one"), ("second", "two"), ("first", "-tail")] {
            preview
                .apply(&invocation, event(&invocation, call, text))
                .unwrap();
        }
        assert_eq!(preview.segments[0].text, "one-tail");
        assert_eq!(preview.segments[1].text, "two");
        let other = Invocation::new("session");
        preview
            .apply(&invocation, event(&other, "third", "stale"))
            .unwrap();
        assert_eq!(preview.segments.len(), 2);
    }

    #[test]
    fn bounded_previews_preserve_utf8_and_discard_excess_segments() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        preview
            .apply(
                &invocation,
                event(&invocation, "first", &"é".repeat(MAX_PREVIEW_BYTES)),
            )
            .unwrap();
        assert_eq!(preview.segments[0].text.len(), MAX_PREVIEW_BYTES);
        for index in 0..MAX_PREVIEW_SEGMENTS + 2 {
            preview
                .apply(
                    &invocation,
                    event(&invocation, &format!("call-{index}"), "x"),
                )
                .unwrap();
        }
        assert!(preview.omitted);
        assert_eq!(preview.segments.len(), MAX_PREVIEW_SEGMENTS);
    }
}
