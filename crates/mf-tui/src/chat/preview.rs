use mf_chat::{Invocation, PreviewDelta, PreviewEvent, ToolActivity};

pub const MAX_PREVIEW_BYTES: usize = 128 * 1024;
pub const MAX_PREVIEW_SEGMENTS: usize = 32;
const MAX_ID_BYTES: usize = 256;

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub model_call_id: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Receiving,
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl ToolState {
    pub fn label(self, incomplete: bool) -> &'static str {
        match (self, incomplete) {
            (Self::Receiving | Self::Queued | Self::Running, true) => "Interrupted",
            (Self::Receiving, false) => "Receiving arguments",
            (Self::Queued, false) => "Queued",
            (Self::Running, false) => "Running",
            (Self::Succeeded, _) => "Succeeded",
            (Self::Failed, _) => "Failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolPreview {
    pub source: String,
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub content: Option<String>,
    pub state: ToolState,
}

#[derive(Clone, Copy, Debug)]
pub enum PreviewItem {
    Text(usize),
    Tool(usize),
}

#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    pub invocation: Option<Invocation>,
    pub segments: Vec<PreviewSegment>,
    pub tools: Vec<ToolPreview>,
    pub order: Vec<PreviewItem>,
    pub omitted: bool,
    pub incomplete: bool,
    pub revision: u64,
}

fn truncate(text: &mut String, available: &mut usize) -> bool {
    let mut length = (*available).min(text.len());
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    let omitted = length < text.len();
    text.truncate(length);
    *available -= length;
    omitted
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
        match event.delta {
            PreviewDelta::Text { delta } => {
                if delta.model_call_id.len() > MAX_ID_BYTES {
                    self.omitted = true;
                } else if let Some(index) = self
                    .segments
                    .iter()
                    .position(|s| s.model_call_id == delta.model_call_id)
                {
                    self.segments[index].text.push_str(&delta.text);
                } else if self.order.len() < MAX_PREVIEW_SEGMENTS {
                    self.order.push(PreviewItem::Text(self.segments.len()));
                    self.segments.push(PreviewSegment {
                        model_call_id: delta.model_call_id,
                        text: delta.text,
                    });
                } else {
                    self.omitted = true;
                }
            }
            PreviewDelta::Tool { activity } => self.tool(activity),
        }
        // Include correlation metadata in the bound, not only visible business text.
        let metadata = self
            .segments
            .iter()
            .map(|s| s.model_call_id.len())
            .sum::<usize>()
            + self
                .tools
                .iter()
                .map(|t| t.source.len() + t.id.len() + t.name.len())
                .sum::<usize>();
        let mut available = MAX_PREVIEW_BYTES.saturating_sub(metadata);
        for item in &self.order {
            match *item {
                PreviewItem::Text(index) => {
                    self.omitted |= truncate(&mut self.segments[index].text, &mut available)
                }
                PreviewItem::Tool(index) => {
                    let tool = &mut self.tools[index];
                    self.omitted |= truncate(&mut tool.arguments, &mut available);
                    if let Some(content) = &mut tool.content {
                        self.omitted |= truncate(content, &mut available);
                    }
                }
            }
        }
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    fn tool(&mut self, activity: ToolActivity) {
        let (source, id, name) = match &activity {
            ToolActivity::Call {
                model_call_id,
                part,
                call,
                ..
            } => {
                if model_call_id.len() > MAX_ID_BYTES {
                    self.omitted = true;
                    return;
                }
                (
                    format!("{model_call_id}:{part}"),
                    call.as_ref().map_or("", |c| c.id.as_str()),
                    call.as_ref().map_or("", |c| c.name.as_str()),
                )
            }
            ToolActivity::Running { tool_call_id, name } => {
                (String::new(), tool_call_id.as_str(), name.as_str())
            }
            ToolActivity::Result { tool_call_id, .. } => (String::new(), tool_call_id.as_str(), ""),
        };
        if source.len() > MAX_ID_BYTES + 21 || id.len() > MAX_ID_BYTES || name.len() > MAX_ID_BYTES
        {
            self.omitted = true;
            return;
        }
        let index = self.tools.iter().position(|t| {
            (!source.is_empty() && t.source == source) || (!id.is_empty() && t.id == id)
        });
        let index = if let Some(index) = index {
            index
        } else if self.order.len() < MAX_PREVIEW_SEGMENTS {
            self.order.push(PreviewItem::Tool(self.tools.len()));
            self.tools.push(ToolPreview {
                source,
                id: id.into(),
                name: name.into(),
                arguments: String::new(),
                content: None,
                state: ToolState::Receiving,
            });
            self.tools.len() - 1
        } else {
            self.omitted = true;
            return;
        };
        let tool = &mut self.tools[index];
        match activity {
            ToolActivity::Call {
                arguments, call, ..
            } => {
                if let Some(call) = call {
                    tool.id = call.id;
                    tool.name = call.name;
                    tool.arguments =
                        serde_json::to_string_pretty(&call.arguments).unwrap_or_default();
                    if tool.state == ToolState::Receiving {
                        tool.state = ToolState::Queued;
                    }
                } else if tool.state == ToolState::Receiving {
                    tool.arguments.push_str(&arguments);
                }
            }
            ToolActivity::Running { name, .. } => {
                tool.name = name;
                if tool.content.is_none() {
                    tool.state = ToolState::Running;
                }
            }
            ToolActivity::Result {
                failed, content, ..
            } => {
                tool.content = Some(content);
                tool.state = if failed {
                    ToolState::Failed
                } else {
                    ToolState::Succeeded
                };
            }
        }
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
            }
            .into(),
        }
    }

    #[test]
    fn previews_are_grouped_by_model_call_and_ignore_other_turns() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        for (call, text) in [("first", "one"), ("second", "two"), ("first", "-tail")] {
            preview.apply(event(&invocation, call, text)).unwrap();
        }
        assert_eq!(preview.segments[0].text, "one-tail");
        assert_eq!(preview.segments[1].text, "two");
        preview
            .apply(event(&Invocation::new("session"), "third", "stale"))
            .unwrap();
        assert_eq!(preview.segments.len(), 2);
    }

    #[test]
    fn bounded_previews_preserve_utf8_and_discard_excess_segments() {
        let invocation = Invocation::new("session");
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        preview
            .apply(event(&invocation, "first", &"é".repeat(MAX_PREVIEW_BYTES)))
            .unwrap();
        assert!(preview.segments[0].text.len() <= MAX_PREVIEW_BYTES - 5);
        for index in 0..MAX_PREVIEW_SEGMENTS + 2 {
            preview
                .apply(event(&invocation, &format!("call-{index}"), "x"))
                .unwrap();
        }
        assert!(preview.omitted);
        assert_eq!(preview.segments.len(), MAX_PREVIEW_SEGMENTS);
    }

    fn activity(preview: &mut LivePreview, activity: ToolActivity) {
        preview
            .apply(PreviewEvent {
                invocation: preview.invocation.clone().unwrap(),
                delta: activity.into(),
            })
            .unwrap();
    }

    #[test]
    fn tool_fragments_parallel_states_and_late_events_remain_independent() {
        let mut preview = LivePreview::default();
        preview.begin(Invocation::new("session"));
        for (part, id) in [(0, "first"), (1, "second")] {
            activity(
                &mut preview,
                ToolActivity::Call {
                    model_call_id: "model".into(),
                    part,
                    arguments: "{\"x\":".into(),
                    call: None,
                },
            );
            activity(
                &mut preview,
                ToolActivity::Call {
                    model_call_id: "model".into(),
                    part,
                    arguments: "1}".into(),
                    call: None,
                },
            );
            assert_eq!(preview.tools[part as usize].arguments, "{\"x\":1}");
            activity(
                &mut preview,
                ToolActivity::Call {
                    model_call_id: "model".into(),
                    part,
                    arguments: String::new(),
                    call: Some(
                        serde_json::from_value(
                            serde_json::json!({"id":id,"name":"lookup","arguments":{"x":1}}),
                        )
                        .unwrap(),
                    ),
                },
            );
            assert_eq!(preview.tools[part as usize].state, ToolState::Queued);
            activity(
                &mut preview,
                ToolActivity::Running {
                    tool_call_id: id.into(),
                    name: "lookup".into(),
                },
            );
        }
        activity(
            &mut preview,
            ToolActivity::Result {
                tool_call_id: "second".into(),
                failed: true,
                content: "error".into(),
            },
        );
        assert_eq!(preview.tools[0].state, ToolState::Running);
        assert_eq!(preview.tools[1].state, ToolState::Failed);
        activity(
            &mut preview,
            ToolActivity::Running {
                tool_call_id: "second".into(),
                name: "lookup".into(),
            },
        );
        assert_eq!(preview.tools[1].state, ToolState::Failed);
        activity(
            &mut preview,
            ToolActivity::Result {
                tool_call_id: "first".into(),
                failed: false,
                content: String::new(),
            },
        );
        assert_eq!(preview.tools[0].state, ToolState::Succeeded);
        for state in [ToolState::Receiving, ToolState::Queued, ToolState::Running] {
            assert_eq!(state.label(true), "Interrupted");
            assert_ne!(state.label(false), "Interrupted");
        }
        assert_eq!(ToolState::Succeeded.label(true), "Succeeded");
        assert_eq!(ToolState::Failed.label(false), "Failed");
    }

    #[test]
    fn tool_only_previews_are_bounded_without_stalling_status_updates() {
        let mut preview = LivePreview::default();
        preview.begin(Invocation::new("session"));
        activity(
            &mut preview,
            ToolActivity::Running {
                tool_call_id: "id".into(),
                name: "lookup".into(),
            },
        );
        activity(
            &mut preview,
            ToolActivity::Result {
                tool_call_id: "id".into(),
                failed: false,
                content: "é".repeat(MAX_PREVIEW_BYTES),
            },
        );
        assert_eq!(preview.tools[0].state, ToolState::Succeeded);
        assert!(
            preview.omitted && preview.tools[0].content.as_ref().unwrap().len() < MAX_PREVIEW_BYTES
        );
        activity(
            &mut preview,
            ToolActivity::Call {
                model_call_id: "x".repeat(MAX_ID_BYTES + 1),
                part: 0,
                arguments: "huge identity".into(),
                call: None,
            },
        );
        assert_eq!(preview.tools.len(), 1);
        for index in 0..MAX_PREVIEW_SEGMENTS + 2 {
            activity(
                &mut preview,
                ToolActivity::Running {
                    tool_call_id: format!("id-{index}"),
                    name: "lookup".into(),
                },
            );
        }
        assert_eq!(preview.tools.len(), MAX_PREVIEW_SEGMENTS);
        activity(
            &mut preview,
            ToolActivity::Result {
                tool_call_id: "id-0".into(),
                failed: true,
                content: "failure".into(),
            },
        );
        assert_eq!(preview.tools[1].state, ToolState::Failed);
        assert!(
            preview
                .apply(PreviewEvent {
                    invocation: preview.invocation.clone().unwrap(),
                    delta: mf_chat::TextDelta {
                        model_call_id: "x".repeat(MAX_ID_BYTES + 1),
                        text: "ignored".into()
                    }
                    .into()
                })
                .is_ok()
        );
        assert!(preview.segments.is_empty());
        preview.begin(Invocation::new("session"));
        assert!(preview.tools.is_empty() && preview.order.is_empty() && !preview.omitted);
    }
}
