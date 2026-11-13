use crate::{ToolCall, ToolResult};
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, Shared, TypedNodeResult, TypedTaskNode, ValueRef,
    deserialize_config,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use snafu::{ResultExt, Snafu, ensure};

/// Documentation attached to a startup input, not an executable binding.
///
/// Tool properties carry these descriptions as plain JSON, so a caller can join
/// them with the body's own interface without re-declaring types or requiredness.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDescription {
    pub description: String,
}

#[derive(Debug, Snafu)]
pub enum ToolExecutionError {
    #[snafu(display("tool call ID must not be blank"))]
    BlankCallId,
}

impl From<ToolExecutionError> for NodeExecutionError {
    fn from(source: ToolExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct EncodeNode;
#[derive(NodeValue)]
struct EncodeInputs {
    call: Shared<ToolCall>,
    result: ValueRef,
    plain_content: Option<bool>,
}
#[derive(NodeValue)]
struct EncodeOutputs {
    result: Shared<ToolResult>,
}
impl TypedTaskNode for EncodeNode {
    type Input = EncodeInputs;
    type Output = EncodeOutputs;

    fn execute(
        &self,
        input: EncodeInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EncodeOutputs>, NodeExecutionError> {
        let call = input.call.decode().boxed()?;
        ensure!(!call.id.trim().is_empty(), BlankCallIdSnafu);
        let content = if let Some(error) = input.result.get("error") {
            serde_json::to_string(&json!({"error": error})).boxed()?
        } else if input.plain_content.unwrap_or(false)
            && let Some(content) = input
                .result
                .get("outputs")
                .and_then(|value| value.get("content"))
                .and_then(ValueRef::as_str)
        {
            content.to_owned()
        } else {
            serde_json::to_string(input.result.get("outputs").unwrap_or(&input.result)).boxed()?
        };
        // Business encoding retains the payload; only observation formatting redacts.
        // Owned strings do not carry sensitivity, so mark the published object explicitly.
        let result = ValueRef::object([
            ("tool_call_id".into(), call.id.into()),
            ("content".into(), content.into()),
        ])
        .inherit_secrets([&input.result, input.call.value()]);
        Ok(EncodeOutputs {
            result: Shared::new(result).boxed()?,
        }
        .into())
    }
}

fn encode_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(EncodeNode, NodeMetadata::default())
}

inventory::submit! { NodeRegistration { kind: "llm.tool_result", factory: NodeFactory::Plain(encode_factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{Inputs, PortSpec, ValueType};
    fn encode_value(call: ValueRef, result: ValueRef, plain_content: Option<bool>) -> ValueRef {
        let mut inputs = Inputs::from([("call".into(), call), ("result".into(), result)]);
        if let Some(plain_content) = plain_content {
            inputs.insert("plain_content".into(), plain_content.into());
        }
        mf_runtime::execute_typed_task(&EncodeNode, inputs, &mut ExecutionContext::default())
            .unwrap()
            .outputs
            .remove("result")
            .unwrap()
    }

    fn encode(result: Value, plain_content: Option<bool>) -> String {
        Shared::<ToolResult>::new(encode_value(
            json!({"id":"id", "name":"lookup", "arguments":{}}).into(),
            result.into(),
            plain_content,
        ))
        .unwrap()
        .decode()
        .unwrap()
        .content
    }

    #[test]
    fn result_encoding_preserves_business_values_and_secret_metadata_on_every_path() {
        for secret in [false, true] {
            let payload = ValueRef::from("private");
            let payload = if secret {
                payload.into_secret()
            } else {
                payload
            };
            let outputs = ValueRef::object([("content".into(), payload.clone())]);
            for (result, plain_content, expected) in [
                (
                    ValueRef::object([("outputs".into(), outputs.clone())]),
                    Some(true),
                    "private",
                ),
                (
                    ValueRef::object([("outputs".into(), outputs)]),
                    None,
                    "{\"content\":\"private\"}",
                ),
                (
                    ValueRef::object([
                        ("error".into(), payload.clone()),
                        ("outputs".into(), json!({"content":"ignored"}).into()),
                    ]),
                    Some(true),
                    "{\"error\":\"private\"}",
                ),
                (
                    ValueRef::object([("answer".into(), payload)]),
                    None,
                    "{\"answer\":\"private\"}",
                ),
            ] {
                let result = encode_value(
                    json!({"id":"id", "name":"lookup", "arguments":{}}).into(),
                    result,
                    plain_content,
                );
                assert!(result.is_object());
                assert_eq!(result.is_secret(), secret);
                assert_eq!(result["content"].is_secret(), secret);
                let decoded = Shared::<ToolResult>::new(result.clone())
                    .unwrap()
                    .decode()
                    .unwrap();
                assert_eq!(decoded.tool_call_id, "id");
                assert_eq!(decoded.content, expected);
                let business = serde_json::to_value(&result).unwrap();
                assert_eq!(business, json!({"tool_call_id":"id", "content":expected}));
                assert_eq!(
                    serde_json::to_value(result.redacted()).unwrap(),
                    if secret {
                        json!("[REDACTED]")
                    } else {
                        business
                    }
                );
            }
        }
    }

    #[test]
    fn result_encoding_inherits_whole_and_nested_call_secrets() {
        let public_call: ValueRef = json!({"id":"id", "name":"lookup", "arguments":{}}).into();
        let nested_call = ValueRef::object([
            ("id".into(), "id".into()),
            ("name".into(), "lookup".into()),
            (
                "arguments".into(),
                ValueRef::object([("token".into(), ValueRef::from("credential").into_secret())]),
            ),
        ]);
        for call in [public_call.into_secret(), nested_call] {
            for (result, expected) in [
                (json!({"outputs":{"content":"text"}}), "text"),
                (json!({"outputs":{"answer":12}}), "{\"answer\":12}"),
                (json!({"error":"bad"}), "{\"error\":\"bad\"}"),
            ] {
                let result = encode_value(call.clone(), result.into(), Some(true));
                assert!(result.is_object());
                assert!(result.is_secret());
                assert!(result["tool_call_id"].is_secret());
                assert!(result["content"].is_secret());
                assert_eq!(
                    serde_json::to_value(&result).unwrap(),
                    json!({"tool_call_id":"id", "content":expected})
                );
                assert_eq!(result.redacted(), ValueRef::from("[REDACTED]"));
            }
        }
    }

    #[test]
    fn result_encoding_keeps_object_ports_and_strict_call_decoding() {
        let node = encode_factory(json!({})).unwrap();
        assert_eq!(
            node.metadata.ports.inputs[0],
            PortSpec::new("call", ValueType::Object, true)
        );
        assert_eq!(
            node.metadata.ports.outputs,
            [PortSpec::new("result", ValueType::Object, true)]
        );
        for call in [
            json!({"name":"lookup", "arguments":{}}),
            json!({"id":42, "name":"lookup", "arguments":{}}),
            json!({"id":"id", "name":"lookup", "arguments":{}, "extra":true}),
        ] {
            let call = Shared::new(call.into()).unwrap();
            let error = EncodeNode
                .execute(
                    EncodeInputs {
                        call,
                        result: json!({}).into(),
                        plain_content: None,
                    },
                    &mut ExecutionContext::default(),
                )
                .err()
                .unwrap();
            assert!(std::error::Error::source(&error).is_some());
        }
    }

    #[test]
    fn result_encoding_preserves_errors_and_json_fallback_and_rejects_blank_ids() {
        for (result, expected) in [
            (json!({"outputs":{"content":"text"}}), "text"),
            (json!({"outputs":{"content":12}}), "{\"content\":12}"),
            (
                json!({"error":"bad", "outputs":{"content":"ignored"}}),
                "{\"error\":\"bad\"}",
            ),
            (json!({"anything":true}), "{\"anything\":true}"),
        ] {
            assert_eq!(encode(result, Some(true)), expected);
        }
        assert_eq!(
            encode(json!({"outputs":{"content":"text"}}), None),
            "{\"content\":\"text\"}"
        );
        for id in ["", " \n"] {
            assert!(
                EncodeNode
                    .execute(
                        EncodeInputs {
                            call: Shared::new(
                                json!({"id":id, "name":"lookup", "arguments":{}}).into(),
                            )
                            .unwrap(),
                            result: json!({}).into(),
                            plain_content: None,
                        },
                        &mut ExecutionContext::default()
                    )
                    .is_err()
            );
        }
    }
}
