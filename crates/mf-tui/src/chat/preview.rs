use mf_chat::{
    Invocation, PreviewDelta, PreviewEvent,
    display::{Fragments, Text},
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: Text,
    pub tool: Option<ToolCallPreview>,
}

#[derive(Clone, Debug)]
pub struct ToolCallPreview {
    pub index: u64,
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    pub invocation: Option<Invocation>,
    pub segments: Vec<Arc<PreviewSegment>>,
    pub storage_error: Option<String>,
    fragments: Option<Fragments>,
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
        if self.storage_error.is_some() {
            return Ok(());
        }
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
        let id = tool_id.filter(|hint| !hint.trim().is_empty());
        let name = tool_name.filter(|hint| !hint.trim().is_empty());
        let previous = self.segments.iter_mut().rev().find(|segment| {
            segment.model_call_id == model_call_id
                && segment.tool.as_ref().map(|tool| tool.index) == tool_index
        });
        let mut text = previous
            .as_ref()
            .map_or_else(Text::default, |segment| segment.text.clone());
        if !fragment.is_empty() {
            let appended = match &self.fragments {
                Some(store) => store.append(&text, &fragment),
                None => Fragments::open()
                    .and_then(|store| self.fragments.insert(store).append(&text, &fragment)),
            };
            match appended {
                Ok(next) => text = next,
                Err(error) => {
                    // A terminal storage failure must not fail model delivery or settlement.
                    self.storage_error = Some(error.to_string());
                    return Ok(());
                }
            }
        }
        if let Some(segment) = previous {
            let segment = Arc::make_mut(segment);
            segment.text = text;
            if let Some(tool) = &mut segment.tool {
                if id.is_some() {
                    tool.id = id;
                }
                if name.is_some() {
                    tool.name = name;
                }
            }
        } else {
            self.segments.push(Arc::new(PreviewSegment {
                model_call_id,
                text,
                tool: tool_index.map(|index| ToolCallPreview { index, id, name }),
            }));
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
        assert_eq!(preview.segments[0].text.load().unwrap(), "one-tail");
        assert_eq!(preview.segments[1].text.load().unwrap(), "two");
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
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
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
    fn scratch_write_failure_is_visible_but_does_not_reject_delivery() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        preview.apply(text(&invocation, "model", "prefix")).unwrap();
        // A different scratch database cannot extend this immutable prefix.
        preview.fragments = Some(Fragments::open().unwrap());
        preview.apply(text(&invocation, "model", "more")).unwrap();
        assert!(preview.storage_error.is_some());
        let revision = preview.revision;
        preview
            .apply(text(&invocation, "model", "after failure"))
            .unwrap();
        assert_eq!(preview.revision, revision);
        assert_eq!(preview.segments[0].text.load().unwrap(), "prefix");
        preview.begin(Invocation::new("another session"));
        assert!(preview.storage_error.is_none());
        assert!(preview.segments.is_empty());
    }

    #[test]
    fn long_identities_and_late_hints_do_not_drop_or_merge_calls() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
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
