//! A single frontend-independent reducer for semantic messages and execution observations.

use super::*;
use crate::{ChatMessage, MessageBatch};
use snafu::ResultExt;
use std::collections::BTreeMap;

pub fn message_id(invocation: &Invocation, ordinal: usize) -> String {
    format!("{}:message:{ordinal}", invocation.call_id)
}
pub fn call_id(invocation: &Invocation, message: usize, ordinal: usize) -> String {
    format!("{}:tool:{message}:{ordinal}", invocation.call_id)
}

pub struct ToolUseReducer {
    history: ToolUseHistory,
    messages: Vec<ChatMessage>,
    origins: BTreeMap<usize, String>,
    pending: BTreeMap<i64, MessageBatch>,
    running: BTreeSet<String>,
}
impl ToolUseReducer {
    pub fn new(invocation: Invocation) -> Self {
        Self {
            history: ToolUseHistory {
                invocation,
                calls: Vec::new(),
                runs: Vec::new(),
                incomplete: false,
                finished: false,
            },
            messages: Vec::new(),
            origins: BTreeMap::new(),
            pending: BTreeMap::new(),
            running: BTreeSet::new(),
        }
    }
    pub fn history(&self) -> &ToolUseHistory {
        &self.history
    }
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn apply_messages(&mut self, batch: MessageBatch) -> Result<bool, ChatError> {
        ensure!(
            batch.offset >= 0
                && batch
                    .model_call_id
                    .as_ref()
                    .is_none_or(|id| !id.trim().is_empty()),
            InvalidSnafu {
                field: "message event",
                reason: "requires a nonnegative offset and a nonblank origin"
            }
        );
        if usize::try_from(batch.offset)
            .ok()
            .is_none_or(|offset| offset > self.messages.len())
        {
            ensure!(
                self.pending.len() < 32 || self.pending.contains_key(&batch.offset),
                InvalidSnafu {
                    field: "message events",
                    reason: "too many out-of-order message batches"
                }
            );
            if let Some(old) = self.pending.get(&batch.offset) {
                ensure!(
                    old == &batch,
                    ConflictSnafu {
                        id: &self.history.invocation.call_id
                    }
                );
            }
            self.pending.insert(batch.offset, batch);
            return Ok(false);
        }
        let before = self.history.clone();
        self.append(batch)?;
        while let Some((&offset, _)) = self.pending.first_key_value() {
            if usize::try_from(offset)
                .ok()
                .is_none_or(|offset| offset > self.messages.len())
            {
                break;
            }
            let batch = self.pending.remove(&offset).expect("pending key exists");
            self.append(batch)?;
        }
        self.project();
        Ok(before != self.history)
    }

    fn append(&mut self, batch: MessageBatch) -> Result<(), ChatError> {
        let offset = usize::try_from(batch.offset).context(crate::error::IntegerSnafu {
            operation: "index source messages",
        })?;
        let mut messages = self.messages.clone();
        let mut origins = self.origins.clone();
        for (index, message) in batch.messages.into_iter().enumerate() {
            let ordinal = offset + index;
            if let Some(old) = messages.get(ordinal) {
                ensure!(
                    old == &message,
                    ConflictSnafu {
                        id: &self.history.invocation.call_id
                    }
                );
            } else {
                messages.push(message);
            }
            if let Some(origin) = &batch.model_call_id {
                ensure!(
                    origins.get(&ordinal).is_none_or(|old| old == origin),
                    ConflictSnafu {
                        id: &self.history.invocation.call_id
                    }
                );
                origins.insert(ordinal, origin.clone());
            }
        }
        crate::message_log::validate(&messages, false)?;
        self.messages = messages;
        self.origins = origins;
        Ok(())
    }

    /// Canonical recovery keeps original messages, grouped only by their explicit source identity.
    pub fn batches(&self) -> Vec<MessageBatch> {
        let mut batches: Vec<MessageBatch> = Vec::new();
        for (index, message) in self.messages.iter().enumerate() {
            let origin = self.origins.get(&index).cloned();
            if let Some(last) = batches.last_mut()
                && last.model_call_id == origin
            {
                last.messages.push(message.clone());
            } else {
                batches.push(MessageBatch {
                    offset: index as i64,
                    model_call_id: origin,
                    messages: vec![message.clone()],
                });
            }
        }
        batches
    }

    pub fn observe(&mut self, incoming: Vec<ToolUse>, incomplete: bool) -> bool {
        let before = self.history.clone();
        self.history.incomplete |= incomplete;
        for mut run in incoming {
            if matches!(run.phase, ToolUsePhase::Running) {
                self.running.insert(run.id.clone());
            }
            if let Some(old) = self.history.runs.iter_mut().find(|old| old.id == run.id) {
                if run.name == "tool (name unobserved)" {
                    run.name = old.name.clone();
                }
                if run.arguments.is_empty() {
                    run.arguments = old.arguments.clone();
                }
                if run.output.is_empty() {
                    run.output = old.output.clone();
                }
                if run.duration_ns.is_none() {
                    run.duration_ns = old.duration_ns;
                }
                if run.origin.is_none() {
                    run.origin = old.origin.clone();
                }
                if matches!(
                    old.phase,
                    ToolUsePhase::Succeeded
                        | ToolUsePhase::Failed
                        | ToolUsePhase::Skipped
                        | ToolUsePhase::Interrupted
                ) && matches!(
                    run.phase,
                    ToolUsePhase::Unknown | ToolUsePhase::Preparing | ToolUsePhase::Running
                ) {
                    run.phase = old.phase;
                }
                *old = run;
            } else if self.history.runs.len() < MAX_TOOL_USES {
                self.history.runs.push(run);
            } else {
                self.history.incomplete = true;
            }
        }
        self.project();
        before != self.history
    }
    pub fn finish(&mut self, interrupted: bool) -> bool {
        let before = self.history.clone();
        for run in &mut self.history.runs {
            if matches!(run.phase, ToolUsePhase::Preparing | ToolUsePhase::Running)
                || interrupted
                    && run.phase == ToolUsePhase::Unknown
                    && self.running.contains(&run.id)
            {
                run.phase = if interrupted {
                    ToolUsePhase::Interrupted
                } else {
                    ToolUsePhase::Unknown
                };
                self.history.incomplete = true;
            }
        }
        self.history.incomplete |= !self.pending.is_empty()
            || self
                .history
                .runs
                .iter()
                .any(|run| run.phase == ToolUsePhase::Unknown);
        self.history.finished = true;
        before != self.history
    }
    fn project(&mut self) {
        fn text(value: &str, limit: usize) -> String {
            value[..value.floor_char_boundary(value.len().min(limit))].into()
        }
        let mut calls: Vec<ChatToolUse> = Vec::new();
        let mut identities: BTreeMap<_, usize> = BTreeMap::new();
        for (index, message) in self.messages.iter().enumerate() {
            if let ChatMessage::Assistant {
                tool_calls: Some(tools),
                ..
            } = message
                && let Some(model) = self.origins.get(&index)
            {
                for tool in tools {
                    *identities.entry((model, &tool.id)).or_default() += 1;
                }
            }
        }
        let mut pending = BTreeMap::new();
        for (index, message) in self.messages.iter().enumerate() {
            match message {
                ChatMessage::Assistant {
                    tool_calls: Some(tools),
                    ..
                } => {
                    for (ordinal, tool) in tools.iter().enumerate() {
                        pending.insert(&tool.id, calls.len());
                        let model_call_id = self.origins.get(&index).cloned();
                        let execution_ids = self
                            .history
                            .runs
                            .iter()
                            .filter(|run| {
                                run.origin.as_ref().is_some_and(|origin| {
                                    Some(&origin.model_call_id) == model_call_id.as_ref()
                                        && origin.tool_call_id == tool.id
                                        && identities
                                            .get(&(&origin.model_call_id, &origin.tool_call_id))
                                            == Some(&1)
                                })
                            })
                            .map(|run| run.id.clone())
                            .collect();
                        let arguments = serde_json::to_string(&tool.arguments.redacted())
                            .unwrap_or_else(|_| "[unavailable]".into());
                        calls.push(ChatToolUse {
                            id: call_id(&self.history.invocation, index, ordinal),
                            assistant_message_id: message_id(&self.history.invocation, index),
                            model_call_id,
                            tool_call_id: tool.id.clone(),
                            name: text(&tool.name, MAX_TOOL_NAME_BYTES),
                            arguments: text(&arguments, MAX_TOOL_TEXT_BYTES),
                            output: None,
                            execution_ids,
                        });
                    }
                }
                ChatMessage::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => {
                    if let Some(index) = pending.remove(tool_call_id) {
                        calls[index].output = Some(text(content, MAX_TOOL_TEXT_BYTES));
                    }
                }
                _ => {}
            }
        }
        if calls.len() > MAX_TOOL_USES {
            self.history.incomplete = true;
            calls.truncate(MAX_TOOL_USES);
        }
        self.history.calls = calls;
    }
}
