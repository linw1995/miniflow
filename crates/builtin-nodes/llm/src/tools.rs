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
use std::collections::BTreeMap;

struct ToolInterface {
    startup_node: String,
    inputs: mf_runtime::WorkflowInputSchema,
    plain_content: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    name: String,
    description: String,
    #[serde(default)]
    parameters: BTreeMap<String, mf_runtime::FlowParameterDescription>,
    metadata: mf_runtime::SubgraphMetadata,
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

fn prepare_tool_interface(
    name: &str,
    body: mf_runtime::SubgraphMetadata,
) -> Result<ToolInterface, NodeBuildError> {
    body.validate().boxed().map_err(NodeBuildError::from)?;
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
    for (port, input) in inputs {
        ensure!(
            !input.binding.secret || input.binding.env.is_some() || input.binding.default.is_some(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!(
                    "tool `{name}` secret input `{startup_node}.{port}` requires an environment variable or default"
                )
            }
        );
    }
    Ok(ToolInterface {
        startup_node: startup_node.clone(),
        inputs: body.inputs,
        plain_content: matches!(body.outputs.as_slice(), [output]
            if output.name == "content" && output.required && output.value_type == ValueType::String),
    })
}

fn validate_binding(config: &Config, interface: &ToolInterface) -> Result<(), NodeBuildError> {
    ensure!(
        !config.description.trim().is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: format!(
                "tool `{}` requires a nonblank binding description",
                config.name
            )
        }
    );
    for parameter in config.parameters.keys() {
        ensure!(
            interface.inputs.parameter(parameter).is_some(),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!(
                    "tool `{}` describes unknown input `{parameter}`",
                    config.name
                )
            }
        );
    }
    Ok(())
}

fn tool_definition(
    name: String,
    description: String,
    parameters: &BTreeMap<String, mf_runtime::FlowParameterDescription>,
    interface: &ToolInterface,
) -> ToolDefinition {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (port, input) in &interface.inputs.inputs[&interface.startup_node] {
        if input.binding.secret {
            continue;
        }
        let mut schema = value_schema(&input.value_type);
        if let Some(parameter) =
            parameters.get(&mf_runtime::output_id(&interface.startup_node, port))
        {
            schema["description"] = json!(parameter.description);
        }
        properties.insert(port.clone(), schema);
        if input.required && input.binding.env.is_none() && input.binding.default.is_none() {
            required.push(Value::String(port.clone()));
        }
    }
    ToolDefinition {
        name,
        description,
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
        .into(),
    }
}

/// Runtime projection of linked interface data; no source lookup or environment resolution.
struct DefinitionNode;
#[derive(NodeValue)]
struct DefinitionInputs {
    binding: ValueRef,
}
#[derive(NodeValue)]
struct DefinitionOutputs {
    definition: ToolDefinition,
}
impl TypedTaskNode for DefinitionNode {
    type Input = DefinitionInputs;
    type Output = DefinitionOutputs;

    fn execute(
        &self,
        input: DefinitionInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<DefinitionOutputs>, NodeExecutionError> {
        let definition =
            project_definition(serde_json::to_value(&input.binding).boxed()?).boxed()?;
        Ok(DefinitionOutputs { definition }.into())
    }
}

struct ArgumentsNode(ToolInterface);
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
            Some(error) => (
                "error",
                ValueRef::from(json!({"error": error})),
                "arguments",
            ),
            None => (
                "arguments",
                ValueRef::object(
                    arguments
                        .0
                        .into_iter()
                        .map(|(name, value)| (name.into(), value)),
                ),
                "error",
            ),
        };
        Ok(NodeResult {
            outputs: Outputs::from([
                (name.into(), value),
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

/// Validate a linked interface and project it into the catalog and public adapter metadata.
/// Real bindings remain exclusively in the executable child definition. The adapter needs only
/// fallback presence, not environment names or literal defaults.
pub fn project_tool(
    name: &str,
    description: &str,
    parameters: &BTreeMap<String, mf_runtime::FlowParameterDescription>,
    metadata: &mf_runtime::SubgraphMetadata,
) -> Result<(ToolDefinition, Value), NodeBuildError> {
    let original = json!({
        "name": name,
        "description": description,
        "parameters": parameters,
        "metadata": metadata,
    });
    // Validate before redaction so malformed defaults, unbound secrets, and unknown
    // description keys cannot disappear unnoticed.
    let definition = project_definition(original)?;
    let mut metadata = metadata.clone();
    for ports in metadata.inputs.inputs.values_mut() {
        ports.retain(|_, input| !input.binding.secret);
        for input in ports.values_mut() {
            let has_source = input.binding.env.is_some() || input.binding.default.is_some();
            input.binding.env = has_source.then(|| "MF_TOOL_BINDING_SOURCE_PRESENT".into());
            input.binding.default = None;
        }
    }
    let parameters: BTreeMap<_, _> = parameters
        .iter()
        .filter(|(name, _)| metadata.inputs.parameter(name).is_some())
        .collect();
    let config = json!({
        "name": name,
        "description": description,
        "parameters": parameters,
        "metadata": metadata,
    });
    Ok((definition, config))
}

fn project_definition(config: Value) -> Result<ToolDefinition, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    let interface = prepare_tool_interface(&config.name, config.metadata.clone())?;
    validate_binding(&config, &interface)?;
    Ok(tool_definition(
        config.name,
        config.description,
        &config.parameters,
        &interface,
    ))
}

struct RuntimeArgumentsNode;

impl NodePortContract for RuntimeArgumentsNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("call", ValueType::Object, true),
                PortSpec::new("binding", ValueType::Object, true),
            ],
            outputs: vec![
                PortSpec::new("arguments", ValueType::Object, false),
                PortSpec::new("error", ValueType::Object, false),
                PortSpec::new("plain_content", ValueType::Boolean, true),
            ],
        }
    }
}

impl TaskNode for RuntimeArgumentsNode {
    fn execute(
        &self,
        mut inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let binding = inputs.remove("binding").expect("validated binding");
        let config: Config =
            serde_json::from_value(serde_json::to_value(binding).boxed()?).boxed()?;
        let interface = prepare_tool_interface(&config.name, config.metadata.clone()).boxed()?;
        validate_binding(&config, &interface).boxed()?;
        ArgumentsNode(interface).execute(inputs, context)
    }
}

fn arguments_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    if config.is_null() || config.as_object().is_some_and(serde_json::Map::is_empty) {
        return PreparedNode::new(
            RuntimeArgumentsNode,
            NodeMetadata {
                exclusive_outputs: vec!["arguments".into(), "error".into()],
                ..Default::default()
            },
        );
    }
    let config: Config = deserialize_config(config)?;
    let interface = prepare_tool_interface(&config.name, config.metadata.clone())?;
    validate_binding(&config, &interface)?;
    let node = ArgumentsNode(interface);
    PreparedNode::new(
        node,
        NodeMetadata {
            exclusive_outputs: vec!["arguments".into(), "error".into()],
            ..Default::default()
        },
    )
}
fn definition_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(DefinitionNode, NodeMetadata::default())
}
fn encode_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(EncodeNode, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "llm.tool_definition", factory: NodeFactory::Plain(definition_factory) } }
inventory::submit! { NodeRegistration { kind: "llm.tool_arguments", factory: NodeFactory::Plain(arguments_factory) } }
inventory::submit! { NodeRegistration { kind: "llm.tool_result", factory: NodeFactory::Plain(encode_factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{InputBinding, SubgraphMetadata, WorkflowInput, WorkflowInputSchema};
    use std::collections::BTreeMap;

    fn with_metadata(mut config: Value, body: SubgraphMetadata) -> Value {
        if let Some(object) = config.as_object_mut()
            && object.remove("subgraph").is_some()
        {
            object.insert("metadata".into(), serde_json::to_value(body).unwrap());
        }
        config
    }

    fn definition_projection(
        config: Value,
        body: SubgraphMetadata,
    ) -> Result<ToolDefinition, NodeBuildError> {
        project_definition(with_metadata(config, body))
    }

    fn arguments_adapter(
        config: Value,
        body: SubgraphMetadata,
    ) -> Result<PreparedNode, NodeBuildError> {
        arguments_factory(with_metadata(config, body))
    }

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
    fn single_node_addition_has_exact_described_schema_and_qualified_arguments() {
        let body = SubgraphMetadata {
            inputs: WorkflowInputSchema {
                inputs: BTreeMap::from([(
                    "calculate".into(),
                    ["left", "right"]
                        .into_iter()
                        .map(|port| {
                            (
                                port.into(),
                                WorkflowInput {
                                    value_type: ValueType::Int64,
                                    required: true,
                                    binding: InputBinding::default(),
                                },
                            )
                        })
                        .collect(),
                )]),
                ..Default::default()
            },
            outputs: vec![PortSpec::new("sum", ValueType::Any, true)],
            stream_outputs: vec![],
        };
        let config = json!({
            "name": "add",
            "description": "Add two integers.",
            "parameters": {
                "calculate.left": {"description": "Left operand."},
                "calculate.right": {"description": "Right operand."}
            },
            "subgraph": {}
        });
        assert!(definition_projection(config.clone(), body.clone()).is_ok());
        assert!(arguments_adapter(config.clone(), body.clone()).is_ok());
        let config: Config = deserialize_config(with_metadata(config, body.clone())).unwrap();
        let interface = prepare_tool_interface(&config.name, body).unwrap();
        let definition = tool_definition(
            config.name,
            config.description,
            &config.parameters,
            &interface,
        );
        assert_eq!(definition.name, "add");
        assert_eq!(definition.description, "Add two integers.");
        assert_eq!(
            serde_json::to_value(&definition.input_schema).unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "left": {"type": "integer", "description": "Left operand."},
                    "right": {"type": "integer", "description": "Right operand."}
                },
                "required": ["left", "right"],
                "additionalProperties": false
            })
        );
        let result = ArgumentsNode(interface)
            .execute(
                Inputs::from([(
                    "call".into(),
                    json!({"id": "id", "name": "add", "arguments": {"left": 2, "right": 3}}).into(),
                )]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.outputs["arguments"]).unwrap(),
            json!({"calculate.left": 2, "calculate.right": 3})
        );
    }

    #[test]
    fn parameter_descriptions_require_known_qualified_input_names() {
        for parameter in [
            "visible",
            "other.visible",
            "params.missing",
            "params.visible.extra",
        ] {
            let config = json!({
                "name": "x", "description": "Description", "subgraph": {},
                "parameters": {parameter: {"description": "Not an input"}}
            });
            for error in [
                definition_projection(config.clone(), metadata())
                    .err()
                    .unwrap(),
                arguments_adapter(config, metadata()).err().unwrap(),
            ] {
                assert!(error.to_string().contains("describes unknown input"));
                assert!(error.to_string().contains(parameter));
            }
        }
    }

    #[test]
    fn descriptions_preserve_env_default_requiredness_and_hide_secrets() {
        let mut body = metadata();
        let ports = body.inputs.inputs.get_mut("params").unwrap();
        for (port, binding) in [
            (
                "environment",
                InputBinding {
                    env: Some("MF_TOOL_TEST_VISIBLE".into()),
                    ..Default::default()
                },
            ),
            (
                "defaulted",
                InputBinding {
                    default: Some(json!(4)),
                    ..Default::default()
                },
            ),
            (
                "secret_environment",
                InputBinding {
                    secret: true,
                    env: Some("MF_TOOL_TEST_SECRET".into()),
                    ..Default::default()
                },
            ),
        ] {
            ports.insert(
                port.into(),
                WorkflowInput {
                    value_type: ValueType::Int64,
                    required: true,
                    binding,
                },
            );
        }
        ports.insert(
            "optional".into(),
            WorkflowInput {
                value_type: ValueType::Boolean,
                required: false,
                binding: InputBinding::default(),
            },
        );
        let parameters: BTreeMap<String, mf_runtime::FlowParameterDescription> = body.inputs.inputs
            ["params"]
            .keys()
            .map(|port| {
                (
                    format!("params.{port}"),
                    mf_runtime::FlowParameterDescription {
                        description: format!("Description of {port}"),
                    },
                )
            })
            .collect();
        let config = json!({"name": "x", "description": "Static description", "parameters": parameters, "subgraph": {}});
        assert!(definition_projection(config.clone(), body.clone()).is_ok());
        assert!(arguments_adapter(config, body.clone()).is_ok());
        let interface = prepare_tool_interface("x", body).unwrap();
        let definition = tool_definition(
            "x".into(),
            "Static description".into(),
            &parameters,
            &interface,
        );
        assert_eq!(
            serde_json::to_value(&definition.input_schema).unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "visible": {"type": "string", "description": "Description of visible"},
                    "environment": {"type": "integer", "description": "Description of environment"},
                    "defaulted": {"type": "integer", "description": "Description of defaulted"},
                    "optional": {"type": "boolean", "description": "Description of optional"}
                },
                "required": ["visible"],
                "additionalProperties": false
            })
        );
        let node = ArgumentsNode(interface);
        let call = |arguments| {
            Inputs::from([(
                "call".into(),
                json!({"id": "id", "name": "x", "arguments": arguments}).into(),
            )])
        };
        let result = node
            .execute(
                call(json!({"visible": "x"})),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.outputs["arguments"]).unwrap(),
            json!({"params.visible": "x"})
        );
        for secret in ["secret", "secret_environment"] {
            let result = node
                .execute(
                    call(json!({"visible": "x", secret: 1})),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert!(result.outputs.contains_key("error"));
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
        let interface = prepare_tool_interface("example", metadata()).unwrap();
        let definition = tool_definition(
            "example".into(),
            "Description".into(),
            &BTreeMap::new(),
            &interface,
        );
        assert_eq!(
            serde_json::to_value(&definition.input_schema["required"]).unwrap(),
            json!(["visible"])
        );
        assert!(
            definition.input_schema["properties"]
                .get("secret")
                .is_none()
        );
        let arguments = ArgumentsNode(interface).execute(Inputs::from([("call".into(), json!({"id":"id", "name":"example", "arguments":{"visible":"x", "secret":"override"}}).into())]), &mut ExecutionContext::default()).unwrap();
        assert!(arguments.outputs.contains_key("error"));
    }

    fn assert_invalid_metadata(body: SubgraphMetadata) {
        let config = json!({"name":"x", "description":"x", "subgraph":{}});
        assert!(definition_projection(config.clone(), body.clone()).is_err());
        assert!(arguments_adapter(config, body).is_err());
    }

    #[test]
    fn runtime_projection_reads_supplied_binding_data_and_preserves_schema_rules() {
        assert!(definition_factory(json!({})).is_ok());
        assert!(definition_factory(json!({"binding": {}})).is_err());
        let mut body = metadata();
        let ports = body.inputs.inputs.get_mut("params").unwrap();
        ports.get_mut("visible").unwrap().value_type = ValueType::Map(Box::new(ValueType::List(
            Box::new(ValueType::Nullable(Box::new(ValueType::Int64))),
        )));
        ports.insert(
            "environment".into(),
            WorkflowInput {
                value_type: ValueType::String,
                required: true,
                binding: InputBinding {
                    env: Some("MF_RUNTIME_CATALOG_UNRESOLVED".into()),
                    ..Default::default()
                },
            },
        );
        for name in ["search-web", "9lookup"] {
            let binding = json!({
                "name": name, "description": "Runtime supplied description.",
                "parameters": {"params.visible": {"description": "Nested values."}},
                "metadata": body,
            });
            let expected = project_definition(binding.clone()).unwrap();
            let result = DefinitionNode
                .execute(
                    DefinitionInputs {
                        binding: binding.into(),
                    },
                    &mut ExecutionContext::default(),
                )
                .unwrap()
                .outputs
                .definition;
            assert_eq!(result.name, expected.name);
            assert_eq!(result.description, expected.description);
            assert_eq!(result.input_schema, expected.input_schema);
            let schema = serde_json::to_value(result.input_schema).unwrap();
            assert!(schema["properties"].get("secret").is_none());
            assert_eq!(schema["required"], json!(["visible"]));
            assert_eq!(
                schema["properties"]["visible"]["additionalProperties"]["items"]["anyOf"],
                json!([{"type": "integer"}, {"type": "null"}])
            );
        }
        for binding in [
            json!({}),
            json!({"name": "x", "description": "x", "metadata": body,
                "parameters": {"params.missing": {"description": "Unknown"}}}),
        ] {
            let error = DefinitionNode
                .execute(
                    DefinitionInputs {
                        binding: binding.into(),
                    },
                    &mut ExecutionContext::default(),
                )
                .err()
                .unwrap();
            assert!(std::error::Error::source(&error).is_some());
        }
    }

    #[test]
    fn public_binding_projection_removes_secrets_and_preserves_fallback_semantics() {
        let mut body = metadata();
        let ports = body.inputs.inputs.get_mut("params").unwrap();
        ports.get_mut("visible").unwrap().binding.env = Some("PRIVATE_ENV_NAME".into());
        ports.insert(
            "defaulted".into(),
            WorkflowInput {
                value_type: ValueType::Int64,
                required: true,
                binding: InputBinding {
                    default: Some(json!(197531)),
                    ..Default::default()
                },
            },
        );
        let parameters = BTreeMap::from([(
            "params.secret".into(),
            mf_runtime::FlowParameterDescription {
                description: "PRIVATE_SECRET_DESCRIPTION".into(),
            },
        )]);
        let (definition, binding) = project_tool("x", "Description", &parameters, &body).unwrap();
        for literal in [
            "hidden",
            "PRIVATE_ENV_NAME",
            "197531",
            "secret",
            "PRIVATE_SECRET_DESCRIPTION",
        ] {
            assert!(!binding.to_string().contains(literal), "{binding}");
        }
        let projected = project_definition(binding.clone()).unwrap();
        assert_eq!(definition.input_schema, projected.input_schema);
        assert_eq!(definition.input_schema["required"], json!([]));
        let config: Config = deserialize_config(binding).unwrap();
        let interface = prepare_tool_interface("x", config.metadata).unwrap();
        let node = ArgumentsNode(interface);
        for (arguments, output) in [
            (json!({}), "arguments"),
            (json!({"secret": "override"}), "error"),
        ] {
            let result = node
                .execute(
                    Inputs::from([(
                        "call".into(),
                        json!({"id": "id", "name": "x", "arguments": arguments}).into(),
                    )]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert!(result.outputs.contains_key(output));
        }

        // Removing the final secret port must not remove the sole startup node.
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .retain(|port, _| port == "secret");
        let (_, binding) = project_tool("x", "Description", &parameters, &body).unwrap();
        assert_eq!(binding["metadata"]["inputs"]["inputs"]["params"], json!({}));
        assert!(project_definition(binding.clone()).is_ok());
        assert!(arguments_factory(binding).is_ok());
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("secret")
            .unwrap()
            .binding
            .default = None;
        assert!(project_tool("x", "Description", &parameters, &body).is_err());
    }

    #[test]
    fn factories_preserve_config_validation() {
        let config = json!({"name":"x", "description":"x", "subgraph":{}});
        assert!(definition_projection(config.clone(), metadata()).is_ok());
        assert!(arguments_adapter(config, metadata()).is_ok());
        for config in [
            json!({"description":"x", "subgraph":{}}),
            json!({"name":"x", "subgraph":{}}),
            json!({"name":1, "description":"x", "subgraph":{}}),
            json!({"name":"x", "description":1, "subgraph":{}}),
            json!({"name":"x", "description":"x"}),
            json!({"name":"x", "description":"x", "subgraph":{}, "extra":true}),
            json!({"name":"x", "description":"", "subgraph":{}}),
            json!({"name":"x", "description":" \n\t", "subgraph":{}}),
            json!({"name":"x", "description":"x", "parameters":[], "subgraph":{}}),
            json!({"name":"x", "description":"x", "parameters":{"params.visible":{}}, "subgraph":{}}),
        ] {
            assert!(definition_projection(config.clone(), metadata()).is_err());
            assert!(arguments_adapter(config, metadata()).is_err());
        }
    }

    #[test]
    fn projection_and_arguments_reject_malformed_frozen_metadata() {
        let mut duplicate_outputs = metadata();
        duplicate_outputs.outputs = vec![
            PortSpec::new("content", ValueType::String, true),
            PortSpec::new("content", ValueType::String, true),
        ];
        let mut invalid_default = metadata();
        invalid_default
            .inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .binding
            .default = Some(json!(42));
        for body in [duplicate_outputs, invalid_default] {
            let config = json!({"name": "x", "description": "Description", "metadata": body});
            assert!(project_tool("x", "Description", &BTreeMap::new(), &body).is_err());
            assert!(arguments_factory(config).is_err());
        }
    }

    #[test]
    fn metadata_validation_rejects_stdin_streams_and_unbound_secrets() {
        let mut body = metadata();
        body.stream_outputs
            .push(PortSpec::new("stream", ValueType::String, true));
        assert_invalid_metadata(body);
        let mut body = metadata();
        body.inputs
            .stdin
            .insert("read".into(), mf_runtime::StdinRequirement::Always);
        assert_invalid_metadata(body);
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("secret")
            .unwrap()
            .binding
            .default = None;
        assert_invalid_metadata(body);
        let mut body = metadata();
        body.inputs.inputs.clear();
        assert_invalid_metadata(body);
        let mut body = metadata();
        body.inputs.inputs.insert("other".into(), BTreeMap::new());
        assert_invalid_metadata(body);
    }

    #[test]
    fn precomputed_metadata_preserves_map_argument_validation() {
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .value_type = ValueType::Map(Box::new(ValueType::Boolean));
        let (definition, config) =
            project_tool("lookup", "Look up flags.", &BTreeMap::new(), &body).unwrap();
        assert_eq!(
            serde_json::to_value(&definition.input_schema["properties"]["visible"]).unwrap(),
            json!({"type": "object", "additionalProperties": {"type": "boolean"}})
        );
        assert!(arguments_factory(config.clone()).is_ok());
        assert!(config.get("subgraph").is_none());
        let config: Config = deserialize_config(config).unwrap();
        let interface = prepare_tool_interface(&config.name, config.metadata).unwrap();
        let node = ArgumentsNode(interface);
        for (visible, valid) in [
            (json!({"flag": true}), true),
            (json!({"flag": "not boolean"}), false),
        ] {
            let result = node
                .execute(
                    Inputs::from([(
                        "call".into(),
                        json!({"id":"id", "name":"lookup", "arguments":{"visible":visible}}).into(),
                    )]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(result.outputs.contains_key("arguments"), valid);
            assert_eq!(result.outputs.contains_key("error"), !valid);
        }
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
            let interface = prepare_tool_interface("example", body).unwrap();
            assert_eq!(interface.plain_content, expected);
            let arguments = ArgumentsNode(interface)
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
