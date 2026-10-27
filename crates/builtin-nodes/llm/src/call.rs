//! OpenAI-compatible function tools and conversation conversion for the LLM node.

use crate::{
    ChatMessage, InvalidInputSnafu, LlmExecutionError, LlmInputs, UnsupportedOutputSnafu,
    finish_reason, reported_usage,
};
use mf_runtime::{NodeEnum, NodeValue, ValueRef};
use rig_core::{completion::CompletionRequest, message as rig};
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

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
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
}

impl From<ToolResult> for ChatMessage {
    fn from(result: ToolResult) -> Self {
        Self {
            role: "tool".into(),
            content: result.content,
            metadata: None,
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ToolCallEvent {
    pub model_call_id: String,
    pub call: ToolCall,
}

impl ToolCall {
    pub fn from_provider(call: rig::ToolCall) -> Result<Self, LlmExecutionError> {
        let id = call.id.wire().into_owned();
        ensure!(
            !id.trim().is_empty() && valid_name(call.function.name.as_str()),
            crate::InvalidResponseSnafu {
                requirement: "nonblank tool call IDs and valid tool names"
            }
        );
        Ok(Self {
            id,
            name: call.function.name.into(),
            arguments: call.function.arguments.into(),
        })
    }
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
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

pub fn prepare_request(inputs: &LlmInputs) -> Result<CompletionRequest, LlmExecutionError> {
    let mut names = BTreeSet::new();
    let mut tools = Vec::new();
    for (index, tool) in inputs.tools.iter().enumerate() {
        let path = format!("/tools/{index}");
        require(
            valid_name(&tool.name),
            &path,
            "a tool name of 1-64 ASCII letters, digits, underscores or hyphens",
        )?;
        require(names.insert(&tool.name), &path, "unique tool names")?;
        require(
            tool.input_schema.as_object().is_some()
                && tool.input_schema["type"].as_str() == Some("object"),
            &path,
            "an object JSON Schema with type `object`",
        )?;
        tools.push(rig_core::completion::ToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: serde_json::to_value(&tool.input_schema)
                .context(crate::EncodeRequestSnafu)?,
        });
    }

    let mut pending = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut messages = Vec::new();
    for (index, message) in inputs.messages.iter().enumerate() {
        let path = format!("/messages/{index}");
        let converted = match message.role.as_str() {
            "system" | "user" | "assistant" => {
                require(
                    message.tool_call_id.is_none(),
                    &path,
                    "tool_call_id only on tool messages",
                )?;
                require(
                    pending.is_empty(),
                    &path,
                    "all pending tool results before another system, user or assistant message",
                )?;
                if message.role != "assistant" {
                    require(
                        message.tool_calls.is_none(),
                        &path,
                        "plain system or user text",
                    )?;
                    require(
                        !message.content.is_empty(),
                        &format!("{path}/content"),
                        "nonempty text",
                    )?;
                    if message.role == "system" {
                        rig::Message::System {
                            content: message.content.clone(),
                        }
                    } else {
                        rig::Message::user(&message.content)
                    }
                } else {
                    let mut content = Vec::new();
                    if !message.content.is_empty() {
                        content.push(rig::AssistantContent::Text(message.content.clone().into()));
                    }
                    for call in message.tool_calls.as_deref().unwrap_or_default() {
                        require(
                            !call.id.trim().is_empty() && seen.insert(&call.id),
                            &path,
                            "nonblank unique tool call IDs",
                        )?;
                        require(valid_name(&call.name), &path, "a valid tool name")?;
                        pending.insert(call.id.as_str(), call.name.as_str());
                        content.push(rig::AssistantContent::ToolCall(rig::ToolCall::from_wire(
                            call.id.clone(),
                            rig::ToolFunction::new(
                                rig::ToolName::new(call.name.clone()).expect("validated tool name"),
                                serde_json::to_value(&call.arguments)
                                    .context(crate::EncodeRequestSnafu)?,
                            ),
                        )));
                    }
                    require(
                        !content.is_empty(),
                        &format!("{path}/content"),
                        "nonempty text or tool calls",
                    )?;
                    rig::Message::Assistant { id: None, content }
                }
            }
            "tool" => {
                require(
                    message.tool_calls.is_none(),
                    &path,
                    "tool result content without assistant fields",
                )?;
                rig::Message::tool_results(vec![encode_tool_result(
                    message.tool_call_id.as_deref().unwrap_or_default(),
                    &message.content,
                    &mut pending,
                    &path,
                )?])
            }
            _ => {
                return InvalidInputSnafu {
                    path: format!("{path}/role"),
                    requirement: "role `system`, `user`, `assistant` or `tool`",
                }
                .fail();
            }
        };
        messages.push(converted);
    }
    if !inputs.tool_results.is_empty() {
        let results = inputs
            .tool_results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                encode_tool_result(
                    &result.tool_call_id,
                    &result.content,
                    &mut pending,
                    &format!("/tool_results/{index}"),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        messages.push(rig::Message::tool_results(results));
    }
    require(
        pending.is_empty(),
        if inputs.tool_results.is_empty() {
            "/messages"
        } else {
            "/tool_results"
        },
        "a result for every pending tool call",
    )?;
    require(!messages.is_empty(), "/messages", "a nonempty message list")?;
    Ok(CompletionRequest::from(messages).tools(tools))
}

fn encode_tool_result(
    id: &str,
    content: &str,
    pending: &mut BTreeMap<&str, &str>,
    path: &str,
) -> Result<rig::ToolResult, LlmExecutionError> {
    let name = pending.remove(id).context(InvalidInputSnafu {
        path: format!("{path}/tool_call_id"),
        requirement: "a tool_call_id matching one pending assistant call",
    })?;
    Ok(rig::ToolResult {
        call: rig::CallId::from_wire(id),
        name: rig::ToolName::new(name).expect("validated pending tool name"),
        content: vec![rig::ToolResultContent::Text(content.into())],
    })
}

pub fn assistant_message(result: &CallResult) -> ChatMessage {
    ChatMessage {
        role: "assistant".into(),
        content: result.text.clone(),
        metadata: None,
        tool_calls: (!result.tool_calls.is_empty()).then(|| result.tool_calls.clone()),
        tool_call_id: None,
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

pub fn terminal_result(
    response: rig_core::completion::CompletionResponse,
) -> Result<CallResult, LlmExecutionError> {
    // Validate counters even when callers only consume the structured response.
    reported_usage(&response.usage)?;
    let finish_reason = finish_reason(&response);
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut seen = BTreeSet::new();
    for block in response.choice {
        match block {
            rig::AssistantContent::Text(part) => {
                text.push_str(&part.text);
            }
            rig::AssistantContent::ToolCall(call) => {
                let call = ToolCall::from_provider(call)?;
                ensure!(
                    seen.insert(call.id.clone()),
                    crate::InvalidResponseSnafu {
                        requirement: "unique tool call IDs"
                    }
                );
                tool_calls.push(call);
            }
            rig::AssistantContent::Reasoning(_) => {}
            _ => return UnsupportedOutputSnafu.fail(),
        }
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
    fn assistant_messages_round_trip_with_tool_results() {
        let call = rig::ToolCall::from_wire(
            "one",
            rig::ToolFunction::new(rig::ToolName::new("lookup").unwrap(), json!({"id":42})),
        );
        let choice = vec![
            rig::AssistantContent::Text("Checking".into()),
            rig::AssistantContent::ToolCall(call),
        ];
        let mut response_content = vec![rig::AssistantContent::reasoning("openai", "hidden")];
        response_content.extend(choice.clone());
        let response =
            CompletionResponse::new(response_content, Usage::default(), "openai", Value::Null)
                .with_finish_reason(FinishReason::ToolCalls);
        let result = terminal_result(response).unwrap();
        assert_eq!(result.outcome, CallOutcome::ToolCalls);
        assert_eq!(result.text, "Checking");
        let encoded_message = assistant_message(&result).into_values().unwrap();
        assert!(!encoded_message.contains_key("reasoning"));
        assert_eq!(
            encoded_message["tool_calls"][0],
            json!({"id":"one", "name":"lookup", "arguments":{"id":42}})
        );
        let request = prepare_request(&LlmInputs {
            messages: vec![
                ChatMessage {
                    role: "user".into(),
                    content: "Hello".into(),
                    metadata: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage::from_values(encoded_message).unwrap(),
                ToolResult {
                    content: "42".into(),
                    tool_call_id: "one".into(),
                }
                .into(),
            ],
            tools: Vec::new(),
            tool_results: Vec::new(),
        })
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

    #[test]
    fn explicit_results_match_call_ids_in_one_rig_batch() {
        let input = LlmInputs::from_values(
            [
                (
                    "messages".into(),
                    json!([{"role":"assistant","content":"","tool_calls":[
                        {"id":"first","name":"lookup","arguments":{"key":"a"}},
                        {"id":"second","name":"lookup","arguments":{"key":"b"}},
                        {"id":"third","name":"store","arguments":{"key":"c"}}
                    ]}])
                    .into(),
                ),
                (
                    "tool_results".into(),
                    json!([
                        {"tool_call_id":"second","content":"B"},
                        {"tool_call_id":"third","content":"C"},
                        {"tool_call_id":"first","content":"A"}
                    ])
                    .into(),
                ),
            ]
            .into(),
        )
        .unwrap();
        let request = prepare_request(&input).unwrap();
        assert_eq!(request.chat_history.len(), 2);
        let rig::Message::Assistant { content, .. } = &request.chat_history[0] else {
            panic!("expected one assistant call batch");
        };
        assert_eq!(content.len(), 3);
        let rig::Message::User { content } = &request.chat_history[1] else {
            panic!("expected one tool result batch");
        };
        assert_eq!(content.len(), 3);
        for (item, (id, name, text)) in content.iter().zip([
            ("second", "lookup", "B"),
            ("third", "store", "C"),
            ("first", "lookup", "A"),
        ]) {
            let rig::UserContent::ToolResult(result) = item else {
                panic!("expected a tool result without synthetic user text");
            };
            assert_eq!(result.call.wire(), id);
            assert_eq!(result.name.as_str(), name);
            assert_eq!(
                result.content,
                vec![rig::ToolResultContent::Text(text.into())]
            );
        }
    }
}
