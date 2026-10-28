use crate::{ChatMessage, ChatToolCall, InvalidInputSnafu, LlmExecutionError};
use rig_core::{
    completion::{AssistantContent, Message},
    message::{ToolCall, ToolFunction, ToolName},
};
use snafu::{OptionExt, ensure};
use std::collections::BTreeMap;

pub fn messages(history: &[ChatMessage]) -> Result<Vec<Message>, LlmExecutionError> {
    let mut messages = Vec::new();
    let mut pending = BTreeMap::<String, ToolCall>::new();
    for (index, message) in history.iter().enumerate() {
        let path = format!("/history/{index}");
        let calls = message.tool_calls.as_deref().unwrap_or(&[]);
        ensure!(
            pending.is_empty() || message.role == "tool",
            InvalidInputSnafu {
                path: &path,
                requirement: "results for every pending tool call before another message"
            }
        );
        match message.role.as_str() {
            "user" => {
                ensure!(
                    !message.content.is_empty(),
                    InvalidInputSnafu {
                        path: format!("{path}/content"),
                        requirement: "nonempty text"
                    }
                );
                ensure!(
                    calls.is_empty() && message.tool_call_id.is_none(),
                    InvalidInputSnafu {
                        path,
                        requirement: "no tool fields on user messages"
                    }
                );
                messages.push(Message::user(&message.content));
            }
            "assistant" => {
                ensure!(
                    message.tool_call_id.is_none(),
                    InvalidInputSnafu {
                        path: &path,
                        requirement: "tool_call_id only on tool results"
                    }
                );
                ensure!(
                    !message.content.is_empty() || !calls.is_empty(),
                    InvalidInputSnafu {
                        path: format!("{path}/content"),
                        requirement: "text or tool calls"
                    }
                );
                ensure!(
                    calls.len() <= crate::tools::MAX_TOOL_CALLS,
                    InvalidInputSnafu {
                        path: &path,
                        requirement: "a bounded tool batch"
                    }
                );
                let mut content = Vec::new();
                if !message.content.is_empty() {
                    content.push(AssistantContent::text(&message.content));
                }
                for (call_index, call) in calls.iter().enumerate() {
                    let path = format!("{path}/tool_calls/{call_index}");
                    ensure!(
                        !call.id.trim().is_empty()
                            && call.id.len() <= 256
                            && !pending.contains_key(&call.id),
                        InvalidInputSnafu {
                            path: format!("{path}/id"),
                            requirement: "a nonblank unique tool call ID of at most 256 bytes"
                        }
                    );
                    ensure!(
                        !call.name.trim().is_empty() && call.name.len() <= 256,
                        InvalidInputSnafu {
                            path: format!("{path}/name"),
                            requirement: "a nonblank tool name of at most 256 bytes"
                        }
                    );
                    let encoded = crate::tools::bounded_json(
                        &call.arguments,
                        crate::tools::MAX_TOOL_ARGUMENT_BYTES,
                    )
                    .ok()
                    .context(InvalidInputSnafu {
                        path: format!("{path}/arguments"),
                        requirement: "bounded JSON arguments",
                    })?;
                    let arguments = mf_runtime::decode_unique_json(encoded.as_bytes())
                        .expect("serialized JSON is unambiguous");
                    let call = ToolCall::from_wire(
                        &call.id,
                        ToolFunction::new(
                            ToolName::new(&call.name).expect("validated name"),
                            arguments,
                        ),
                    );
                    pending.insert(call.id.to_string(), call.clone());
                    content.push(AssistantContent::ToolCall(call));
                }
                messages.push(Message::Assistant { id: None, content });
            }
            "tool" => {
                ensure!(
                    calls.is_empty(),
                    InvalidInputSnafu {
                        path: &path,
                        requirement: "tool results cannot request calls"
                    }
                );
                let id = message.tool_call_id.as_deref().context(InvalidInputSnafu {
                    path: format!("{path}/tool_call_id"),
                    requirement: "a pending tool call ID",
                })?;
                let call = pending.remove(id).context(InvalidInputSnafu {
                    path: format!("{path}/tool_call_id"),
                    requirement: "one result for a pending tool call",
                })?;
                messages.push(Message::tool_result(
                    call.id,
                    call.function.name,
                    &message.content,
                ));
            }
            _ => {
                return InvalidInputSnafu {
                    path: format!("{path}/role"),
                    requirement: "role `user`, `assistant`, or `tool`",
                }
                .fail();
            }
        }
    }
    ensure!(
        pending.is_empty(),
        InvalidInputSnafu {
            path: "/history",
            requirement: "results for every pending tool call"
        }
    );
    Ok(messages)
}

pub fn plain(role: &str, content: String) -> ChatMessage {
    ChatMessage {
        role: role.into(),
        content,
        metadata: None,
        tool_calls: None,
        tool_call_id: None,
    }
}
pub fn assistant(content: String, calls: Vec<ChatToolCall>) -> ChatMessage {
    ChatMessage {
        tool_calls: (!calls.is_empty()).then_some(calls),
        ..plain("assistant", content)
    }
}
pub fn tool(id: String, content: String) -> ChatMessage {
    ChatMessage {
        tool_call_id: Some(id),
        ..plain("tool", content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ChatMessage {
        assistant(
            String::new(),
            vec![ChatToolCall {
                id: "a".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query":"text"}).into(),
            }],
        )
    }

    #[test]
    fn history_requires_exactly_one_result_per_pending_call_before_advancing() {
        let request = request();
        assert!(messages(std::slice::from_ref(&request)).is_err());
        assert!(messages(&[request.clone(), plain("user", "next".into())]).is_err());
        assert!(messages(&[request.clone(), tool("unknown".into(), "result".into())]).is_err());
        assert!(
            messages(&[
                request.clone(),
                tool("a".into(), "result".into()),
                tool("a".into(), "duplicate".into())
            ])
            .is_err()
        );
        assert!(
            messages(&[
                request.clone(),
                tool("a".into(), String::new()),
                plain("assistant", "done".into())
            ])
            .is_ok()
        );
        let mut duplicate = request;
        let call = duplicate.tool_calls.as_ref().unwrap()[0].clone();
        duplicate.tool_calls.as_mut().unwrap().push(call);
        assert!(messages(&[duplicate]).is_err());
    }
    #[test]
    fn history_rejects_cross_role_fields_empty_assistants_and_unbounded_calls() {
        let call = request().tool_calls.unwrap().remove(0);
        let mut user = plain("user", "question".into());
        user.tool_calls = Some(vec![call.clone()]);
        let mut assistant_result = plain("assistant", "answer".into());
        assistant_result.tool_call_id = Some("a".into());
        let mut unnamed = call.clone();
        unnamed.name = " ".into();
        let mut result_with_calls = tool("a".into(), "result".into());
        result_with_calls.tool_calls = Some(vec![call.clone()]);
        for history in [
            vec![user],
            vec![assistant_result],
            vec![plain("assistant", String::new())],
            vec![assistant(
                String::new(),
                vec![call; crate::tools::MAX_TOOL_CALLS + 1],
            )],
            vec![assistant(String::new(), vec![unnamed])],
            vec![request(), result_with_calls],
        ] {
            let error = messages(&history).unwrap_err();
            assert!(matches!(error, LlmExecutionError::InvalidInput { .. }));
            assert!(
                error.to_string().contains("/history/0")
                    || error.to_string().contains("/history/1")
            );
        }
    }
}
