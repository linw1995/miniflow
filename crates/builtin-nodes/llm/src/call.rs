//! Structured tool and reasoning messages for the LLM node.

use crate::{
    ChatMessage, InvalidInputSnafu, LlmExecutionError, LlmInputs, UnsupportedOutputSnafu,
    finish_reason, reported_usage,
};
use mf_runtime::{NodeEnum, NodeValue, ValueRef};
use rig_core::{completion::CompletionRequest, message as rig};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: ValueRef,
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: ValueRef,
    pub signature: Option<String>,
    pub additional_params: Option<ValueRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, NodeEnum)]
#[value(tag = "type")]
pub enum ReasoningBlock {
    #[value(rename = "text")]
    Text {
        text: String,
        signature: Option<String>,
    },
    #[value(rename = "summary")]
    Summary { text: String },
    #[value(rename = "encrypted")]
    Encrypted { data: String },
    #[value(rename = "redacted")]
    Redacted { data: String },
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct Reasoning {
    pub issuer: String,
    pub id: Option<String>,
    pub content: Vec<ReasoningBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, NodeEnum)]
#[value(tag = "type")]
pub enum ContentBlock {
    #[value(rename = "text")]
    Text { text: String },
    #[value(rename = "tool_call")]
    ToolCall { call: ToolCall },
    #[value(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        name: String,
        content: String,
    },
    #[value(rename = "reasoning")]
    Reasoning {
        issuer: String,
        id: Option<String>,
        content: Vec<ReasoningBlock>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Clone, Debug, NodeValue)]
pub struct CallInputs {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    #[value(default)]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, NodeEnum)]
pub enum CallOutcome {
    #[value(rename = "completed")]
    Completed,
    #[value(rename = "tool_calls")]
    ToolCalls,
    #[value(rename = "truncated")]
    Truncated,
    #[value(rename = "refused")]
    Refused,
    #[value(rename = "incomplete")]
    Incomplete,
    #[value(rename = "failed")]
    Failed,
}

#[derive(Clone, Debug, NodeValue)]
pub struct CallResult {
    pub outcome: CallOutcome,
    pub message: Message,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

pub fn prepare_request(inputs: &LlmInputs) -> Result<CompletionRequest, LlmExecutionError> {
    let mut messages = Vec::new();
    let mut names = BTreeMap::new();
    for (index, message) in inputs
        .history
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let path = format!("/history/{index}");
        let mut content = Vec::new();
        match message.role.as_str() {
            "user" | "assistant" => {
                require(
                    message.tool_call_id.is_none(),
                    &path,
                    "tool_call_id only on tool messages",
                )?;
                if message.role == "user" {
                    require(
                        message.tool_calls.is_none() && message.reasoning.is_none(),
                        &path,
                        "plain user text",
                    )?;
                }
                if let Some(reasoning) = &message.reasoning {
                    for block in reasoning {
                        content.push(ContentBlock::Reasoning {
                            issuer: block.issuer.clone(),
                            id: block.id.clone(),
                            content: block.content.clone(),
                        });
                    }
                }
                if !message.content.is_empty() {
                    content.push(ContentBlock::Text {
                        text: message.content.clone(),
                    });
                }
                if let Some(calls) = &message.tool_calls {
                    for call in calls {
                        names.insert(call.id.clone(), call.name.clone());
                        content.push(ContentBlock::ToolCall { call: call.clone() });
                    }
                }
                require(
                    !content.is_empty(),
                    &format!("{path}/content"),
                    "nonempty text, reasoning or tool calls",
                )?;
                messages.push(Message {
                    role: message.role.clone(),
                    content,
                });
            }
            "tool" => {
                require(
                    message.tool_calls.is_none() && message.reasoning.is_none(),
                    &path,
                    "tool result content without assistant fields",
                )?;
                let id = message.tool_call_id.as_deref().unwrap_or_default();
                let name = names.get(id).ok_or_else(|| {
                    InvalidInputSnafu {
                        path: format!("{path}/tool_call_id"),
                        requirement: "a tool_call_id from an earlier assistant message",
                    }
                    .build()
                })?;
                messages.push(Message {
                    role: "user".into(),
                    content: vec![ContentBlock::ToolResult {
                        tool_call_id: id.into(),
                        name: name.clone(),
                        content: message.content.clone(),
                    }],
                });
            }
            _ => {
                return InvalidInputSnafu {
                    path: format!("{path}/role"),
                    requirement: "role `user`, `assistant` or `tool`",
                }
                .fail();
            }
        }
    }
    if let Some(prompt) = &inputs.prompt {
        require(!prompt.is_empty(), "/prompt", "nonempty text when supplied")?;
        messages.push(Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: prompt.clone(),
            }],
        });
    }
    CallInputs {
        messages,
        system: inputs.system.clone(),
        tools: inputs.tools.clone(),
    }
    .into_request()
}

pub fn history_message(result: &CallResult) -> ChatMessage {
    let mut reasoning = Vec::new();
    {
        for block in &result.message.content {
            if let ContentBlock::Reasoning {
                issuer,
                id,
                content,
            } = block
            {
                reasoning.push(Reasoning {
                    issuer: issuer.clone(),
                    id: id.clone(),
                    content: content.clone(),
                });
            }
        }
    }
    ChatMessage {
        role: "assistant".into(),
        content: result.text.clone(),
        metadata: None,
        tool_calls: (!result.tool_calls.is_empty()).then(|| result.tool_calls.clone()),
        tool_call_id: None,
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
    }
}

impl CallInputs {
    fn into_request(self) -> Result<CompletionRequest, LlmExecutionError> {
        require(
            !self.messages.is_empty(),
            "/history",
            "a prompt or nonempty history",
        )?;
        let mut names = std::collections::BTreeSet::new();
        let mut tools = Vec::new();
        for (index, tool) in self.tools.into_iter().enumerate() {
            let path = format!("/tools/{index}");
            require(
                valid_name(&tool.name),
                &path,
                "a tool name of 1-64 ASCII letters, digits, underscores or hyphens",
            )?;
            require(names.insert(tool.name.clone()), &path, "unique tool names")?;
            require(
                tool.input_schema.as_object().is_some()
                    && tool.input_schema["type"].as_str() == Some("object"),
                &path,
                "an object JSON Schema with type `object`",
            )?;
            tools.push(rig_core::completion::ToolDefinition {
                name: tool.name,
                description: tool.description,
                parameters: serde_json::to_value(tool.input_schema)
                    .context(crate::EncodeRequestSnafu)?,
            });
        }
        let mut pending = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut messages = Vec::new();
        for (index, message) in self.messages.into_iter().enumerate() {
            let path = format!("/history/{index}");
            require(
                !message.content.is_empty(),
                &path,
                "nonempty content blocks",
            )?;
            let converted = match message.role.as_str() {
                "user" => {
                    let mut content = Vec::new();
                    for block in message.content {
                        content.push(match block {
                            ContentBlock::Text { text } => {
                                require(
                                    pending.is_empty(),
                                    &path,
                                    "all pending tool results before user text",
                                )?;
                                rig::UserContent::text(text)
                            }
                            ContentBlock::ToolResult {
                                tool_call_id,
                                name,
                                content,
                            } => {
                                require(
                                    pending.remove(&tool_call_id).as_ref() == Some(&name),
                                    &path,
                                    "a result matching one pending tool call ID and name",
                                )?;
                                rig::UserContent::ToolResult(rig::ToolResult {
                                    call: rig::CallId::from_wire(tool_call_id),
                                    name: rig::ToolName::new(name)
                                        .expect("validated pending tool name"),
                                    content: vec![rig::ToolResultContent::Text(content.into())],
                                })
                            }
                            _ => {
                                return InvalidInputSnafu {
                                    path,
                                    requirement: "user text or tool results",
                                }
                                .fail();
                            }
                        });
                    }
                    rig::Message::User { content }
                }
                "assistant" => {
                    require(
                        pending.is_empty(),
                        &path,
                        "all pending tool results before another assistant message",
                    )?;
                    let mut content = Vec::new();
                    for block in message.content {
                        if let ContentBlock::ToolCall { call } = &block {
                            require(
                                !call.id.trim().is_empty() && seen.insert(call.id.clone()),
                                &path,
                                "nonblank unique tool call IDs",
                            )?;
                            require(valid_name(&call.name), &path, "a valid tool name")?;
                            pending.insert(call.id.clone(), call.name.clone());
                        }
                        content.push(block.into_assistant(&path)?);
                    }
                    rig::Message::Assistant { id: None, content }
                }
                _ => {
                    return InvalidInputSnafu {
                        path,
                        requirement: "role `user` or `assistant`",
                    }
                    .fail();
                }
            };
            messages.push(converted);
        }
        require(
            pending.is_empty(),
            "/history",
            "a result for every pending tool call",
        )?;
        let mut request = CompletionRequest::from(messages).tools(tools);
        if let Some(system) = self.system.filter(|system| !system.is_empty()) {
            request = request.preamble(system);
        }
        Ok(request)
    }
}

fn require(
    condition: bool,
    path: &str,
    requirement: &'static str,
) -> Result<(), LlmExecutionError> {
    ensure!(condition, InvalidInputSnafu { path, requirement });
    Ok(())
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

impl ContentBlock {
    fn into_assistant(self, path: &str) -> Result<rig::AssistantContent, LlmExecutionError> {
        Ok(match self {
            Self::Text { text } => rig::AssistantContent::Text(text.into()),
            Self::ToolCall { call } => rig::AssistantContent::ToolCall(
                rig::ToolCall::from_wire(
                    call.id,
                    rig::ToolFunction::new(
                        rig::ToolName::new(call.name).expect("validated tool name"),
                        serde_json::to_value(call.arguments).context(crate::EncodeRequestSnafu)?,
                    ),
                )
                .with_signature(call.signature)
                .with_additional_params(
                    call.additional_params
                        .map(serde_json::to_value)
                        .transpose()
                        .context(crate::EncodeRequestSnafu)?,
                ),
            ),
            Self::Reasoning {
                issuer,
                id,
                content,
            } => {
                require(
                    !issuer.trim().is_empty() && !content.is_empty(),
                    path,
                    "reasoning with an issuer and content",
                )?;
                rig::AssistantContent::Reasoning(
                    rig::Reasoning {
                        id,
                        content: content
                            .into_iter()
                            .map(|block| match block {
                                ReasoningBlock::Text { text, signature } => {
                                    rig::ReasoningContent::Text { text, signature }
                                }
                                ReasoningBlock::Summary { text } => {
                                    rig::ReasoningContent::Summary(text)
                                }
                                ReasoningBlock::Encrypted { data } => {
                                    rig::ReasoningContent::Encrypted(data)
                                }
                                ReasoningBlock::Redacted { data } => {
                                    rig::ReasoningContent::Redacted { data }
                                }
                            })
                            .collect(),
                    }
                    .sealed(issuer),
                )
            }
            Self::ToolResult { .. } => {
                return InvalidInputSnafu {
                    path,
                    requirement: "assistant text, reasoning or tool calls",
                }
                .fail();
            }
        })
    }
}

pub fn terminal_result(
    response: rig_core::completion::CompletionResponse,
) -> Result<CallResult, LlmExecutionError> {
    // Validate counters even when callers only consume the structured response.
    reported_usage(&response.usage)?;
    let finish_reason = finish_reason(&response);
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut content = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for block in response.choice {
        content.push(match block {
            rig::AssistantContent::Text(part) => {
                text.push_str(&part.text);
                ContentBlock::Text { text: part.text }
            }
            rig::AssistantContent::ToolCall(call) => {
                let id = call.id.wire().into_owned();
                ensure!(
                    !id.trim().is_empty()
                        && seen.insert(id.clone())
                        && valid_name(call.function.name.as_str()),
                    crate::InvalidResponseSnafu {
                        requirement: "unique nonblank tool call IDs and valid tool names"
                    }
                );
                let call = ToolCall {
                    id,
                    name: call.function.name.into(),
                    arguments: call.function.arguments.into(),
                    signature: call.signature,
                    additional_params: call.additional_params.map(Into::into),
                };
                tool_calls.push(call.clone());
                ContentBlock::ToolCall { call }
            }
            rig::AssistantContent::Reasoning(sealed) => {
                let issuer = sealed.issuer().as_str().to_owned();
                let reasoning = sealed.open(sealed.issuer()).expect("matching issuer");
                ContentBlock::Reasoning {
                    issuer,
                    id: reasoning.id.clone(),
                    content: reasoning
                        .content
                        .iter()
                        .cloned()
                        .map(|block| match block {
                            rig::ReasoningContent::Text { text, signature } => {
                                ReasoningBlock::Text { text, signature }
                            }
                            rig::ReasoningContent::Summary(text) => {
                                ReasoningBlock::Summary { text }
                            }
                            rig::ReasoningContent::Encrypted(data) => {
                                ReasoningBlock::Encrypted { data }
                            }
                            rig::ReasoningContent::Redacted { data } => {
                                ReasoningBlock::Redacted { data }
                            }
                        })
                        .collect(),
                }
            }
            _ => return UnsupportedOutputSnafu.fail(),
        });
    }
    let outcome = match finish_reason.as_deref() {
        Some("length") => CallOutcome::Truncated,
        Some("content_filter") => CallOutcome::Refused,
        _ if !tool_calls.is_empty() => CallOutcome::ToolCalls,
        Some("stop") | None if !text.is_empty() => CallOutcome::Completed,
        _ => CallOutcome::Incomplete,
    };
    Ok(CallResult {
        outcome,
        message: Message {
            role: "assistant".into(),
            content,
        },
        text,
        tool_calls,
        finish_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::completion::{CompletionResponse, FinishReason, Usage};
    use serde_json::{Value, json};

    #[test]
    fn reasoning_and_tool_metadata_survive_value_encoding_and_continuation() {
        let reasoning = rig::Reasoning {
            id: Some("reasoning-id".into()),
            content: vec![
                rig::ReasoningContent::Text {
                    text: "thinking".into(),
                    signature: Some("signature".into()),
                },
                rig::ReasoningContent::Summary("summary".into()),
                rig::ReasoningContent::Encrypted("encrypted".into()),
                rig::ReasoningContent::Redacted {
                    data: "redacted".into(),
                },
            ],
        }
        .sealed("openai");
        let call = rig::ToolCall::from_wire(
            "one",
            rig::ToolFunction::new(rig::ToolName::new("lookup").unwrap(), json!({"id":42})),
        )
        .with_signature(Some("call-signature".into()))
        .with_additional_params(Some(json!({"provider":"state"})));
        let choice = vec![
            rig::AssistantContent::Reasoning(reasoning),
            rig::AssistantContent::Text("Checking".into()),
            rig::AssistantContent::ToolCall(call),
        ];
        let response =
            CompletionResponse::new(choice.clone(), Usage::default(), "openai", Value::Null)
                .with_finish_reason(FinishReason::ToolCalls);
        let result = terminal_result(response).unwrap();
        let result = CallResult::from_values(result.into_values().unwrap()).unwrap();
        assert_eq!(result.outcome, CallOutcome::ToolCalls);
        let request = CallInputs {
            messages: vec![
                Message {
                    role: "user".into(),
                    content: vec![ContentBlock::Text {
                        text: "Hello".into(),
                    }],
                },
                result.message,
                Message {
                    role: "user".into(),
                    content: vec![ContentBlock::ToolResult {
                        tool_call_id: "one".into(),
                        name: "lookup".into(),
                        content: "42".into(),
                    }],
                },
            ],
            system: None,
            tools: Vec::new(),
        }
        .into_request()
        .unwrap();
        assert_eq!(
            request.chat_history[1],
            rig::Message::Assistant {
                id: None,
                content: choice
            }
        );
        assert!(request.tool_choice.is_none());
    }

    #[test]
    fn duplicate_calls_fail_and_length_limited_calls_are_never_actionable() {
        let call = rig::AssistantContent::ToolCall(rig::ToolCall::from_wire(
            "one",
            rig::ToolFunction::new(rig::ToolName::new("lookup").unwrap(), json!({})),
        ));
        let duplicate = CompletionResponse::new(
            vec![call.clone(), call.clone()],
            Usage::default(),
            "openai",
            Value::Null,
        );
        assert!(matches!(
            terminal_result(duplicate),
            Err(LlmExecutionError::InvalidResponse { .. })
        ));
        let truncated =
            CompletionResponse::new(vec![call], Usage::default(), "openai", Value::Null)
                .with_finish_reason(FinishReason::Length);
        assert_eq!(
            terminal_result(truncated).unwrap().outcome,
            CallOutcome::Truncated
        );
    }
}
