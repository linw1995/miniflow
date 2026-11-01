use crate::{ToolCall, ToolDefinition, ToolResult};
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, NodeValue, Outputs, PortSpec,
    PreparedNode, TaskNode, TypedNodeResult, TypedTaskNode, ValueRef, ValueType,
    deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};

struct Catalog {
    definition: ToolDefinition,
    startup_node: String,
    inputs: mf_runtime::WorkflowInputSchema,
    plain_content: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    name: String,
    annotation: String,
    #[serde(rename = "subgraph")]
    _subgraph: Value,
}

fn value_schema(value_type: &ValueType) -> Value {
    match value_type {
        ValueType::Any => json!({}),
        ValueType::Null => json!({"type": "null"}),
        ValueType::Boolean => json!({"type": "boolean"}),
        ValueType::Number | ValueType::Float32 | ValueType::Float64 => {
            json!({"type": "number"})
        }
        ValueType::Int64 | ValueType::Uint64 | ValueType::Usize => json!({"type": "integer"}),
        ValueType::String => json!({"type": "string"}),
        ValueType::Array => json!({"type": "array"}),
        ValueType::Object => json!({"type": "object"}),
        ValueType::List(inner) => json!({"type": "array", "items": value_schema(inner)}),
        ValueType::Map(inner) => {
            json!({"type": "object", "additionalProperties": value_schema(inner)})
        }
        ValueType::Nullable(inner) => {
            json!({"anyOf": [value_schema(inner), {"type": "null"}]})
        }
    }
}

fn prepare_tool(
    name: String,
    annotation: &str,
    body: mf_runtime::SubgraphMetadata,
) -> Result<Catalog, NodeBuildError> {
    ensure!(
        body.stream_outputs.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: format!("tool `{name}` cannot expose stream outputs")
        }
    );
    ensure!(
        body.inputs.stdin.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: format!("tool `{name}` cannot read stdin")
        }
    );
    ensure!(
        body.inputs.inputs.len() == 1,
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: format!("tool `{name}` must have exactly one startup node")
        }
    );
    let (startup_node, inputs) = body.inputs.inputs.iter().next().unwrap();
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (port, input) in inputs {
        ensure!(
            !input.binding.secret || input.binding.env.is_some() || input.binding.default.is_some(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!(
                    "tool `{name}` secret input `{startup_node}.{port}` requires an environment variable or default"
                )
            }
        );
        if input.binding.secret {
            continue;
        }
        properties.insert(port.clone(), value_schema(&input.value_type));
        if input.required && input.binding.env.is_none() && input.binding.default.is_none() {
            required.push(Value::String(port.clone()));
        }
    }
    let input_schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    });
    Ok(Catalog {
        definition: ToolDefinition {
            name,
            description: annotation.to_owned(),
            input_schema: input_schema.into(),
        },
        startup_node: startup_node.clone(),
        inputs: body.inputs.clone(),
        plain_content: matches!(body.outputs.as_slice(), [output]
            if output.name == "content" && output.required && output.value_type == ValueType::String),
    })
}

struct DefinitionNode(ToolDefinition);
#[derive(NodeValue)]
struct Empty {}
#[derive(NodeValue)]
struct DefinitionOutput {
    tool: ToolDefinition,
}
impl TypedTaskNode for DefinitionNode {
    type Input = Empty;
    type Output = DefinitionOutput;
    fn execute(
        &self,
        _: Empty,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<DefinitionOutput>, NodeExecutionError> {
        Ok(DefinitionOutput {
            tool: self.0.clone(),
        }
        .into())
    }
}

struct ArgumentsNode(Catalog);
impl NodePortContract for ArgumentsNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("call", ValueType::Object, true)],
            outputs: vec![
                PortSpec::new("arguments", ValueType::Object, false),
                PortSpec::new("error", ValueType::Object, false),
                PortSpec::new("plain_content", ValueType::Boolean, true),
            ],
        }
    }
}
impl TaskNode for ArgumentsNode {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let call =
            mf_runtime::decode_required_input::<ToolCall>("call", Some(inputs["call"].clone()))
                .boxed()?;
        ensure!(
            !call.id.trim().is_empty(),
            mf_runtime::NodeExecutionFailedSnafu {
                message: "tool call ID must not be blank"
            }
        );
        let mut arguments = mf_runtime::WorkflowArguments::default();
        let mut error = None;
        if let Some(values) = call.arguments.as_object() {
            let ports = &self.0.inputs.inputs[&self.0.startup_node];
            for (name, value) in values {
                match ports.get(name.as_ref()) {
                    Some(input) if !input.binding.secret => {
                        arguments.0.insert(
                            mf_runtime::output_id(&self.0.startup_node, name),
                            value.clone(),
                        );
                    }
                    _ => {
                        error = Some(format!("unknown or secret argument `{name}`"));
                        break;
                    }
                }
            }
            if error.is_none() {
                error = self
                    .0
                    .inputs
                    .validate(&arguments)
                    .err()
                    .map(|error| error.to_string());
            }
        } else {
            error = Some("arguments must be a JSON object".into());
        }
        let (name, value, skipped) = match error {
            Some(error) => ("error", json!({"error": error}), "arguments"),
            None => (
                "arguments",
                serde_json::to_value(arguments.0).expect("values serialize"),
                "error",
            ),
        };
        Ok(NodeResult {
            outputs: Outputs::from([
                (name.into(), value.into()),
                ("plain_content".into(), self.0.plain_content.into()),
            ]),
            skipped: [skipped.into()].into_iter().collect(),
            ..Default::default()
        })
    }
}

struct EncodeNode;
#[derive(NodeValue)]
struct EncodeInputs {
    call: ToolCall,
    result: ValueRef,
    plain_content: Option<bool>,
}
#[derive(NodeValue)]
struct EncodeOutputs {
    result: ToolResult,
}
impl TypedTaskNode for EncodeNode {
    type Input = EncodeInputs;
    type Output = EncodeOutputs;
    fn execute(
        &self,
        input: EncodeInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<EncodeOutputs>, NodeExecutionError> {
        ensure!(
            !input.call.id.trim().is_empty(),
            mf_runtime::NodeExecutionFailedSnafu {
                message: "tool call ID must not be blank"
            }
        );
        let content = if let Some(error) = input.result.get("error") {
            json!({"error": error}).to_string()
        } else if input.plain_content.unwrap_or(false)
            && let Some(content) = input
                .result
                .get("outputs")
                .and_then(|value| value.get("content"))
                .and_then(ValueRef::as_str)
        {
            content.to_owned()
        } else {
            input
                .result
                .get("outputs")
                .unwrap_or(&input.result)
                .to_string()
        };
        Ok(EncodeOutputs {
            result: ToolResult {
                tool_call_id: input.call.id,
                content,
            },
        }
        .into())
    }
}

fn definition_factory(
    config: Value,
    body: mf_runtime::SubgraphMetadata,
) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    let catalog = prepare_tool(config.name, &config.annotation, body)?;
    PreparedNode::typed_task(DefinitionNode(catalog.definition), NodeMetadata::default())
}
fn arguments_factory(
    config: Value,
    body: mf_runtime::SubgraphMetadata,
) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    let node = ArgumentsNode(prepare_tool(config.name, &config.annotation, body)?);
    PreparedNode::new(
        node,
        NodeMetadata {
            exclusive_outputs: vec!["arguments".into(), "error".into()],
            ..Default::default()
        },
    )
}
fn encode_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(EncodeNode, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "llm.tool_definition", factory: NodeFactory::Metadata(definition_factory) } }
inventory::submit! { NodeRegistration { kind: "llm.tool_arguments", factory: NodeFactory::Metadata(arguments_factory) } }
inventory::submit! { NodeRegistration { kind: "llm.tool_result", factory: NodeFactory::Plain(encode_factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{InputBinding, SubgraphMetadata, WorkflowInput, WorkflowInputSchema};
    use std::collections::BTreeMap;

    fn metadata() -> SubgraphMetadata {
        SubgraphMetadata {
            inputs: WorkflowInputSchema {
                inputs: BTreeMap::from([(
                    "params".into(),
                    BTreeMap::from([
                        (
                            "visible".into(),
                            WorkflowInput {
                                value_type: ValueType::String,
                                required: true,
                                binding: InputBinding::default(),
                            },
                        ),
                        (
                            "secret".into(),
                            WorkflowInput {
                                value_type: ValueType::String,
                                required: true,
                                binding: InputBinding {
                                    secret: true,
                                    default: Some(json!("hidden")),
                                    ..Default::default()
                                },
                            },
                        ),
                    ]),
                )]),
                ..Default::default()
            },
            outputs: vec![],
            stream_outputs: vec![],
        }
    }

    #[test]
    fn schemas_cover_all_value_contracts_and_hide_secret_defaults() {
        for (ty, expected) in [
            (ValueType::Any, json!({})),
            (ValueType::Null, json!({"type":"null"})),
            (ValueType::Boolean, json!({"type":"boolean"})),
            (ValueType::Number, json!({"type":"number"})),
            (ValueType::Float32, json!({"type":"number"})),
            (ValueType::Float64, json!({"type":"number"})),
            (ValueType::Int64, json!({"type":"integer"})),
            (ValueType::Uint64, json!({"type":"integer"})),
            (ValueType::Usize, json!({"type":"integer"})),
            (ValueType::Array, json!({"type":"array"})),
            (ValueType::Object, json!({"type":"object"})),
            (
                ValueType::List(Box::new(ValueType::String)),
                json!({"type":"array", "items":{"type":"string"}}),
            ),
            (
                ValueType::Map(Box::new(ValueType::Boolean)),
                json!({"type":"object", "additionalProperties":{"type":"boolean"}}),
            ),
            (
                ValueType::Nullable(Box::new(ValueType::String)),
                json!({"anyOf":[{"type":"string"},{"type":"null"}]}),
            ),
        ] {
            assert_eq!(value_schema(&ty), expected);
        }
        let catalog = prepare_tool("example".into(), "Description", metadata()).unwrap();
        assert_eq!(
            serde_json::to_value(&catalog.definition.input_schema["required"]).unwrap(),
            json!(["visible"])
        );
        assert!(
            catalog.definition.input_schema["properties"]
                .get("secret")
                .is_none()
        );
        let arguments = ArgumentsNode(catalog).execute(Inputs::from([("call".into(), json!({"id":"id", "name":"example", "arguments":{"visible":"x", "secret":"override"}}).into())]), &mut ExecutionContext::default()).unwrap();
        assert!(arguments.outputs.contains_key("error"));
    }

    #[test]
    fn metadata_validation_rejects_stdin_streams_and_unbound_secrets() {
        let mut body = metadata();
        body.stream_outputs
            .push(PortSpec::new("stream", ValueType::String, true));
        assert!(prepare_tool("x".into(), "x", body).is_err());
        let mut body = metadata();
        body.inputs
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        assert!(prepare_tool("x".into(), "x", body).is_err());
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("secret")
            .unwrap()
            .binding
            .default = None;
        assert!(prepare_tool("x".into(), "x", body).is_err());
    }

    #[test]
    fn plain_content_requires_one_required_string_output() {
        for (outputs, expected) in [
            (
                vec![PortSpec::new("content", ValueType::String, true)],
                true,
            ),
            (
                vec![PortSpec::new("content", ValueType::String, false)],
                false,
            ),
            (vec![PortSpec::new("content", ValueType::Any, true)], false),
            (
                vec![PortSpec::new("content", ValueType::Object, true)],
                false,
            ),
            (vec![PortSpec::new("other", ValueType::String, true)], false),
            (
                vec![
                    PortSpec::new("content", ValueType::String, true),
                    PortSpec::new("extra", ValueType::Int64, true),
                ],
                false,
            ),
            (vec![], false),
        ] {
            let mut body = metadata();
            body.outputs = outputs;
            let catalog = prepare_tool("example".into(), "Description", body).unwrap();
            assert_eq!(catalog.plain_content, expected);
            let arguments = ArgumentsNode(catalog)
                .execute(
                    Inputs::from([(
                        "call".into(),
                        json!({"id":"id", "name":"example", "arguments":{"visible":"x"}}).into(),
                    )]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(arguments.outputs["plain_content"].as_bool(), Some(expected));
            let result = EncodeNode
                .execute(
                    EncodeInputs {
                        call: ToolCall {
                            id: "id".into(),
                            name: "example".into(),
                            arguments: json!({}).into(),
                        },
                        result: json!({"outputs":{"content":"text"}}).into(),
                        plain_content: Some(expected),
                    },
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(
                result.outputs.result.content,
                if expected {
                    "text"
                } else {
                    "{\"content\":\"text\"}"
                }
            );
        }
    }

    #[test]
    fn result_encoding_preserves_json_fallback_and_rejects_blank_ids() {
        let node = EncodeNode;
        for (result, expected) in [
            (json!({"outputs":{"content":"text"}}), "text"),
            (json!({"outputs":{"content":12}}), "{\"content\":12}"),
            (json!({"error":"bad"}), "{\"error\":\"bad\"}"),
        ] {
            let result = node
                .execute(
                    EncodeInputs {
                        call: ToolCall {
                            id: "id".into(),
                            name: "x".into(),
                            arguments: json!({}).into(),
                        },
                        result: result.into(),
                        plain_content: Some(true),
                    },
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(result.outputs.result.content, expected);
        }
        assert!(
            node.execute(
                EncodeInputs {
                    call: ToolCall {
                        id: " ".into(),
                        name: "x".into(),
                        arguments: json!({}).into()
                    },
                    result: json!({}).into(),
                    plain_content: None,
                },
                &mut ExecutionContext::default()
            )
            .is_err()
        );
    }
}
