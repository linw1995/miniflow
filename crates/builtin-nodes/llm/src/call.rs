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
        Self::ToolResult {
            tool_call_id: result.tool_call_id,
            content: result.content,
            metadata: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, NodeValue)]
pub struct ToolCallEvent {
    pub model_call_id: String,
    pub call: ToolCall,
}

impl ToolCall {
    /// Preserve declared argument sensitivity before publishing model outputs or stream events.
    pub fn mark_arguments(&mut self, definitions: &[ToolDefinition]) {
        if let Some(definition) = definitions.iter().find(|tool| tool.name == self.name) {
            self.arguments = mark_schema(&self.arguments, &definition.input_schema);
        }
    }

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

fn mark_schema(value: &ValueRef, schema: &ValueRef) -> ValueRef {
    if value.is_secret() || schema["writeOnly"].as_bool() == Some(true) {
        return value.clone().into_secret();
    }
    if let (Some(values), Some(properties)) = (value.as_object(), schema["properties"].as_object())
    {
        return ValueRef::object(values.iter().map(|(name, value)| {
            let value = properties
                .get(name)
                .map_or_else(|| value.clone(), |schema| mark_schema(value, schema));
            (name.clone(), value)
        }));
    }
    if let Some(values) = value.as_array()
        && let Some(items) = schema.get("items")
    {
        return ValueRef::array(values.iter().map(|value| mark_schema(value, items)));
    }
    if (schema["properties"].as_object().is_some() && !value.is_object()
        || schema.get("items").is_some() && !value.is_array())
        && schema_contains_private(schema)
    {
        return value.clone().into_secret();
    }
    value.clone()
}

fn schema_contains_private(schema: &ValueRef) -> bool {
    schema["writeOnly"].as_bool() == Some(true)
        || schema["properties"]
            .as_object()
            .is_some_and(|properties| properties.values().any(schema_contains_private))
        || schema.get("items").is_some_and(schema_contains_private)
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
        if !matches!(message, ChatMessage::ToolResult { .. }) {
            require(
                pending.is_empty(),
                &path,
                "all pending tool results before another system, user or assistant message",
            )?;
        }
        let converted = match message {
            ChatMessage::System { content, .. } | ChatMessage::User { content, .. } => {
                require(
                    !content.is_empty(),
                    &format!("{path}/content"),
                    "nonempty text",
                )?;
                if matches!(message, ChatMessage::System { .. }) {
                    rig::Message::System {
                        content: content.clone(),
                    }
                } else {
                    rig::Message::user(content)
                }
            }
            ChatMessage::Assistant {
                content: text,
                tool_calls,
                ..
            } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(rig::AssistantContent::Text(text.clone().into()));
                }
                for call in tool_calls.as_deref().unwrap_or_default() {
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
            ChatMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => rig::Message::tool_results(vec![encode_tool_result(
                tool_call_id,
                content,
                &mut pending,
                &path,
            )?]),
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
    ChatMessage::Assistant {
        content: result.text.clone(),
        metadata: None,
        tool_calls: (!result.tool_calls.is_empty()).then(|| result.tool_calls.clone()),
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

pub fn valid_name(name: &str) -> bool {
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
    use mf_runtime::{InputValue, OutputValue};
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
        let encoded_message = assistant_message(&result).encode().unwrap();
        assert!(
            !encoded_message
                .as_object()
                .unwrap()
                .contains_key("reasoning")
        );
        assert_eq!(
            encoded_message["tool_calls"][0],
            json!({"id":"one", "name":"lookup", "arguments":{"id":42}})
        );
        let request = prepare_request(&LlmInputs {
            messages: vec![
                ChatMessage::User {
                    content: "Hello".into(),
                    metadata: None,
                },
                ChatMessage::decode(encoded_message).unwrap(),
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
    #[test]
    fn nested_private_arguments_are_marked_before_observation_even_on_shape_errors() {
        let definitions = vec![ToolDefinition {
            name: "lookup".into(),
            description: String::new(),
            input_schema: json!({
                "properties": {
                    "account": {"properties": {"token": {"writeOnly": true}}},
                    "tokens": {"items": {"writeOnly": true}},
                    "public": {"type": "string"}
                }
            })
            .into(),
        }];
        let mut call = ToolCall {
            id: "one".into(), name: "lookup".into(),
            arguments: json!({"account": {"token": "private", "label": "visible"}, "tokens": ["first", "second"], "public": "visible"}).into(),
        };
        call.mark_arguments(&definitions);
        assert!(call.arguments["account"]["token"].is_secret());
        assert!(!call.arguments["account"]["label"].is_secret());
        assert!(
            call.arguments["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .all(ValueRef::is_secret)
        );
        assert!(!call.arguments["public"].is_secret());
        for malformed in [json!({"account": "private"}), json!({"tokens": "private"})] {
            let key = malformed
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            call.arguments = malformed.into();
            call.mark_arguments(&definitions);
            assert!(call.arguments[key.as_str()].is_secret());
        }
        call.arguments = ValueRef::from("already private").into_secret();
        call.mark_arguments(&definitions);
        assert!(call.arguments.is_secret());
    }
}
