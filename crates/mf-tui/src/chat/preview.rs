use mf_chat::{Invocation, PreviewDelta, PreviewEvent};

pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
pub const MAX_PREVIEW_SEGMENTS: usize = 32;
const MAX_ID_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: String,
    pub tool: Option<ToolCallPreview>,
}

#[derive(Clone, Debug)]
pub struct ToolCallPreview {
    pub index: u64,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl PreviewSegment {
    fn metadata_bytes(&self) -> usize {
        self.model_call_id.len()
            + self.tool.as_ref().map_or(0, |tool| {
                tool.id.as_ref().map_or(0, String::len) + tool.name.as_ref().map_or(0, String::len)
            })
    }
}

#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    pub invocation: Option<Invocation>,
    pub segments: Vec<PreviewSegment>,
    pub omitted: bool,
    pub incomplete: bool,
    pub revision: u64,
}

fn bounded_prefix(text: &str, available: &mut usize) -> usize {
    let mut length = (*available).min(text.len());
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    *available -= length;
    length
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

    pub fn apply(&mut self, event: PreviewEvent) -> Result<(), mf_chat::ChatError> {
        if self.invocation.as_ref() != Some(&event.invocation) {
            return Ok(());
        }
        event.validate()?;
        self.revision = self.revision.saturating_add(1);
        let (model_call_id, fragment, tool_index, tool_id, tool_name) = match event.delta {
            PreviewDelta::Text { delta } => (delta.model_call_id, delta.text, None, None, None),
            PreviewDelta::ToolCall { delta } => (
                delta.model_call_id,
                delta.arguments,
                Some(delta.index),
                delta.id,
                delta.name,
            ),
        };
        if model_call_id.len() > MAX_ID_BYTES {
            self.omitted = true;
            return Ok(());
        }
        let index = if let Some(index) = self.segments.iter().position(|segment| {
            segment.model_call_id == model_call_id
                && segment.tool.as_ref().map(|tool| tool.index) == tool_index
        }) {
            index
        } else if self.segments.len() < MAX_PREVIEW_SEGMENTS {
            self.segments.push(PreviewSegment {
                model_call_id,
                text: String::new(),
                tool: tool_index.map(|index| ToolCallPreview {
                    index,
                    id: None,
                    name: None,
                }),
            });
            self.segments.len() - 1
        } else {
            self.omitted = true;
            return Ok(());
        };
        if let Some(tool) = &mut self.segments[index].tool {
            for (stored, hint) in [(&mut tool.id, tool_id), (&mut tool.name, tool_name)] {
                if let Some(hint) = hint.filter(|hint| !hint.trim().is_empty()) {
                    if hint.len() <= MAX_ID_BYTES {
                        *stored = Some(hint);
                    } else {
                        self.omitted = true;
                    }
                }
            }
        }
        let mut available = MAX_PREVIEW_BYTES.saturating_sub(
            self.segments
                .iter()
                .map(PreviewSegment::metadata_bytes)
                .sum(),
        );
        for segment in &mut self.segments {
            let length = bounded_prefix(&segment.text, &mut available);
            if length < segment.text.len() {
                segment.text.truncate(length);
                segment.text.shrink_to_fit();
                self.omitted = true;
            }
        }
        let length = bounded_prefix(&fragment, &mut available);
        self.segments[index].text.push_str(&fragment[..length]);
        self.omitted |= length < fragment.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_chat::{TextDelta, ToolCallDelta};

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
    fn previews_are_grouped_by_model_call_and_ignore_other_turns() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        for (model, fragment) in [("first", "one"), ("second", "two"), ("first", "-tail")] {
            preview.apply(text(&invocation, model, fragment)).unwrap();
        }
        preview
            .apply(text(&Invocation::new("session"), "third", "stale"))
            .unwrap();
        assert_eq!(preview.segments.len(), 2);
        assert_eq!(preview.segments[0].text, "one-tail");
        assert_eq!(preview.segments[1].text, "two");
    }

    #[test]
    fn interleaved_tool_arguments_keep_their_raw_text_and_model_part_identity() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
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
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            ["Preamble", "{ \"query\": \"a\" }", "{}", "[]"]
        );
        let tool = preview.segments[1].tool.as_ref().unwrap();
        assert_eq!(tool.id.as_deref(), Some("first-call"));
        assert_eq!(tool.name.as_deref(), Some("lookup"));
    }

    #[test]
    fn bounded_tool_and_text_previews_preserve_utf8_and_ignore_oversized_identities() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        preview
            .apply(call(
                &invocation,
                "model",
                0,
                &"é".repeat(MAX_PREVIEW_BYTES),
            ))
            .unwrap();
        assert!(preview.omitted);
        assert_eq!(
            preview.segments[0].text.len(),
            MAX_PREVIEW_BYTES - "model".len() - 1
        );
        for index in 0..MAX_PREVIEW_SEGMENTS + 2 {
            preview
                .apply(text(&invocation, &format!("call-{index}"), "x"))
                .unwrap();
        }
        assert_eq!(preview.segments.len(), MAX_PREVIEW_SEGMENTS);
        assert!(
            preview
                .segments
                .iter()
                .map(|segment| segment.text.len() + segment.metadata_bytes())
                .sum::<usize>()
                <= MAX_PREVIEW_BYTES
        );
        preview.begin(invocation.clone());
        preview
            .apply(call(&invocation, &"m".repeat(MAX_ID_BYTES + 1), 0, "{}"))
            .unwrap();
        assert!(preview.omitted && preview.segments.is_empty());
        preview
            .apply(event(
                &invocation,
                ToolCallDelta {
                    model_call_id: "model".into(),
                    index: 0,
                    arguments: "{}".into(),
                    id: Some("x".repeat(MAX_ID_BYTES + 1)),
                    name: Some("lookup".into()),
                },
            ))
            .unwrap();
        let tool = preview.segments[0].tool.as_ref().unwrap();
        assert!(tool.id.is_none());
        assert_eq!(tool.name.as_deref(), Some("lookup"));
    }
}
