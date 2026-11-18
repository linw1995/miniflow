use mf_chat::{Invocation, PreviewDelta, PreviewEvent};
use std::collections::VecDeque;

pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
pub const MAX_PREVIEW_SEGMENTS: usize = 32;
pub const MAX_TOOL_ARGUMENT_PREVIEW_BYTES: usize = 8 * 1024;
const MAX_ID_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: String,
    pub tool: Option<ToolCallPreview>,
}

#[derive(Clone, Debug, Default)]
struct ArgumentWindow {
    // The text itself lives only in PreviewSegment; record byte lengths to count evicted deltas.
    lengths: VecDeque<usize>,
    total_bytes: usize,
    omitted_deltas: u64,
    partial_head: bool,
}

impl ArgumentWindow {
    fn append(&mut self, text: &mut String, fragment: &str, limit: usize) {
        if fragment.is_empty() {
            return;
        }
        let start = fragment.ceil_char_boundary(fragment.len().saturating_sub(limit));
        let tail = &fragment[start..];
        // If the new fragment alone fills the window, retaining any previous bytes would leave a gap.
        self.trim(text, if start == 0 { limit - tail.len() } else { 0 });
        if start > 0 {
            self.omitted_deltas = self.omitted_deltas.saturating_add(1);
            self.partial_head = !tail.is_empty();
        }
        self.total_bytes = self.total_bytes.saturating_add(fragment.len());
        if !tail.is_empty() {
            self.lengths.push_back(tail.len());
            text.push_str(tail);
        }
    }

    fn trim(&mut self, text: &mut String, limit: usize) {
        let start = text.ceil_char_boundary(text.len().saturating_sub(limit));
        let mut remaining = start;
        while remaining > 0 {
            let head = self.lengths.front_mut().expect("retained delta length");
            if !self.partial_head {
                self.omitted_deltas = self.omitted_deltas.saturating_add(1);
            }
            if remaining < *head {
                *head -= remaining;
                self.partial_head = true;
                break;
            }
            remaining -= *head;
            self.lengths.pop_front();
            self.partial_head = false;
        }
        text.drain(..start);
    }
}

#[derive(Clone, Debug)]
pub struct ToolCallPreview {
    pub index: u64,
    pub id: Option<String>,
    pub name: Option<String>,
    arguments: ArgumentWindow,
}

impl ToolCallPreview {
    pub fn argument_bytes(&self) -> usize {
        self.arguments.total_bytes
    }

    pub fn omitted_deltas(&self) -> u64 {
        self.arguments.omitted_deltas
    }
}

impl PreviewSegment {
    fn metadata_bytes(&self) -> usize {
        self.model_call_id.len()
            + self.tool.as_ref().map_or(0, |tool| {
                tool.id.as_ref().map_or(0, String::len) + tool.name.as_ref().map_or(0, String::len)
            })
    }

    fn trim(&mut self, limit: usize) {
        if let Some(tool) = &mut self.tool {
            tool.arguments.trim(&mut self.text, limit);
        } else {
            self.text.truncate(self.text.floor_char_boundary(limit));
        }
        self.text.shrink_to_fit();
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
                    arguments: ArgumentWindow::default(),
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
            if segment.text.len() > available {
                segment.trim(available);
                self.omitted = true;
            }
            available -= segment.text.len();
        }
        let segment = &mut self.segments[index];
        if let Some(tool) = &mut segment.tool {
            let budget = available + segment.text.len();
            self.omitted |= budget < MAX_TOOL_ARGUMENT_PREVIEW_BYTES && fragment.len() > available;
            tool.arguments.append(
                &mut segment.text,
                &fragment,
                budget.min(MAX_TOOL_ARGUMENT_PREVIEW_BYTES),
            );
        } else {
            let length = fragment.floor_char_boundary(available);
            segment.text.push_str(&fragment[..length]);
            self.omitted |= length < fragment.len();
        }
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
    fn tool_argument_history_discards_older_deltas_once_per_delta() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        let fragments = [
            "{\"first\":\"",
            &"a".repeat(5000),
            "é",
            &"b".repeat(5000),
            "\",\"latest\":true}",
        ];
        let full = fragments.concat();
        for fragment in fragments {
            preview
                .apply(call(&invocation, "model", 0, fragment))
                .unwrap();
        }
        let segment = &preview.segments[0];
        let tool = segment.tool.as_ref().unwrap();
        assert_eq!(tool.argument_bytes(), full.len());
        assert_eq!(tool.omitted_deltas(), 2);
        assert_eq!(
            segment.text,
            full[full.len() - MAX_TOOL_ARGUMENT_PREVIEW_BYTES..]
        );
        assert!(!preview.omitted);
    }

    #[test]
    fn shared_budget_keeps_text_prefix_and_rotates_the_remaining_tool_window() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        let prefix_limit = MAX_PREVIEW_BYTES - "text".len() - "model".len() - 5;
        let prefix = format!("{}é", "t".repeat(prefix_limit - "é".len()));
        preview.apply(text(&invocation, "text", &prefix)).unwrap();
        preview
            .apply(call(&invocation, "model", 0, "xéabc"))
            .unwrap();
        assert_eq!(preview.segments[1].text, "éabc");
        preview.apply(call(&invocation, "model", 0, "d")).unwrap();
        assert_eq!(preview.segments[1].text, "abcd");
        assert_eq!(
            preview.segments[1].tool.as_ref().unwrap().omitted_deltas(),
            1
        );
        // A late identity can consume the remaining budget without being counted as a delta.
        let mut hint = call(&invocation, "model", 0, "");
        if let PreviewDelta::ToolCall { delta } = &mut hint.delta {
            delta.id = Some("named".into());
        }
        preview.apply(hint).unwrap();
        assert!(preview.segments[1].text.is_empty());
        let tool = preview.segments[1].tool.as_ref().unwrap();
        assert_eq!(tool.argument_bytes(), "xéabcd".len());
        assert_eq!(tool.omitted_deltas(), 2);
        assert_eq!(preview.segments[0].text, prefix);
        assert!(preview.omitted);
        preview
            .apply(event(
                &invocation,
                ToolCallDelta {
                    model_call_id: "model".into(),
                    index: 0,
                    arguments: String::new(),
                    id: None,
                    name: Some("x".into()),
                },
            ))
            .unwrap();
        assert_eq!(preview.segments[0].text, prefix.strip_suffix('é').unwrap());
        assert_eq!(
            preview.segments[1].tool.as_ref().unwrap().omitted_deltas(),
            2
        );
    }

    #[test]
    fn bounded_tool_and_text_previews_preserve_recent_utf8_and_ignore_oversized_identities() {
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
        assert_eq!(
            preview.segments[0].text,
            "é".repeat(MAX_TOOL_ARGUMENT_PREVIEW_BYTES / "é".len())
        );
        let tool = preview.segments[0].tool.as_ref().unwrap();
        assert_eq!(tool.argument_bytes(), MAX_PREVIEW_BYTES * 2);
        assert_eq!(tool.omitted_deltas(), 1);
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
