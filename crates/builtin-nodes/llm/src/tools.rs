use crate::{ToolCall, ToolDefinition, ToolResult};
use mf_runtime::{
    ExecutionContext, FlowValue, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, NodeValue, Outputs,
    PortSpec, PreparedNode, Shared, TaskNode, TypedNodeResult, TypedTaskNode, ValueRef, ValueType,
    WorkflowInputSchema, deserialize_config,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use snafu::{ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// Documentation attached to a qualified startup input, not an executable binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDescription {
    pub description: String,
}

struct ToolDescription {
    description: String,
    parameters: BTreeMap<String, ParameterDescription>,
}

/// The adapter's interface DTO contains no executable child or graph relationships.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolMetadata {
    pub inputs: WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
}

#[derive(Debug, Snafu)]
pub enum ToolMetadataError {
    #[snafu(display("invalid tool metadata interface: {message}"))]
    InvalidMetadata { message: String },
    #[snafu(display("invalid tool metadata input: {source}"))]
    MetadataInput {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("invalid tool metadata type: {source}"))]
    MetadataTypeDepth { source: mf_runtime::TypeDepthError },
}

impl ToolMetadata {
    /// Checks declarations without resolving the environment, stdin, or child tasks.
    pub fn validate(&self) -> Result<(), ToolMetadataError> {
        for (node, ports) in &self.inputs.inputs {
            for (port, input) in ports {
                ensure!(
                    !node.is_empty() && !port.is_empty(),
                    InvalidMetadataSnafu {
                        message: "input node and port names must not be empty"
                    }
                );
                input
                    .value_type
                    .check_depth()
                    .context(MetadataTypeDepthSnafu)?;
                input
                    .binding
                    .validate(&mf_runtime::output_id(node, port), &input.value_type)
                    .context(MetadataInputSnafu)?;
                ensure!(
                    input.binding.secret || !input.value_type.is_runtime_only(),
                    InvalidMetadataSnafu {
                        message: format!(
                            "model input `{node}.{port}` cannot contain runtime-only types"
                        )
                    }
                );
            }
        }
        for ports in [&self.outputs, &self.stream_outputs] {
            let mut names = BTreeSet::new();
            for port in ports {
                ensure!(
                    !port.name.is_empty() && names.insert(&port.name),
                    InvalidMetadataSnafu {
                        message: "output names must be nonempty and unique within their channel"
                    }
                );
                port.value_type
                    .check_depth()
                    .context(MetadataTypeDepthSnafu)?;
                ensure!(
                    !port.value_type.is_runtime_only(),
                    InvalidMetadataSnafu {
                        message: format!(
                            "model output `{}` cannot contain runtime-only types",
                            port.name
                        )
                    }
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Snafu)]
pub enum ToolBuildError {
    #[snafu(display("tool `{name}`: {message}"))]
    InvalidTool { name: String, message: String },
    #[snafu(display("tool `{name}`: {source}"))]
    Metadata {
        name: String,
        source: ToolMetadataError,
    },
}

impl From<ToolBuildError> for NodeBuildError {
    fn from(source: ToolBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl From<ToolBuildError> for NodeExecutionError {
    fn from(source: ToolBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum ToolExecutionError {
    #[snafu(display("tool call ID must not be blank"))]
    BlankCallId,
    #[snafu(display("tool flow is missing from the selected record"))]
    MissingFlow,
    #[snafu(display("tool description is missing from the selected record"))]
    MissingDescription,
    #[snafu(display("invalid self-contained tool record: {message}"))]
    InvalidRecord { message: String },
    #[snafu(display("invalid tool documentation: {source}"))]
    Description { source: serde_json::Error },
    #[snafu(display("tool adapter input: {source}"))]
    PlumbingInput {
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("tool adapter value `{field}`: {source}"))]
    PlumbingValue {
        field: String,
        source: mf_runtime::TypeMismatch,
    },
}

impl From<ToolExecutionError> for NodeExecutionError {
    fn from(source: ToolExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeBinding {
    name: String,
    description: String,
    #[serde(default)]
    parameters: BTreeMap<String, ParameterDescription>,
    metadata: ToolMetadata,
}

struct ToolInterface {
    startup_node: String,
    inputs: WorkflowInputSchema,
    plain_content: bool,
}

fn validate_name(name: &str) -> Result<(), ToolBuildError> {
    ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
        InvalidToolSnafu {
            name,
            message: "name must be 1-64 ASCII letters, digits, underscores or hyphens"
        }
    );
    Ok(())
}

fn prepare_tool_interface(name: &str, body: ToolMetadata) -> Result<ToolInterface, ToolBuildError> {
    body.validate().context(MetadataSnafu { name })?;
    validate_name(name)?;
    ensure!(
        body.stream_outputs.is_empty(),
        InvalidToolSnafu {
            name,
            message: "cannot expose stream outputs"
        }
    );
    ensure!(
        body.inputs.stdin.is_empty(),
        InvalidToolSnafu {
            name,
            message: "cannot read stdin"
        }
    );
    ensure!(
        body.inputs.inputs.len() == 1,
        InvalidToolSnafu {
            name,
            message: "must have exactly one startup node"
        }
    );
    let (startup_node, inputs) = body.inputs.inputs.iter().next().unwrap();
    for (port, input) in inputs {
        ensure!(
            !input.binding.secret || input.binding.env.is_some() || input.binding.default.is_some(),
            InvalidToolSnafu {
                name,
                message: format!(
                    "secret input `{startup_node}.{port}` requires an environment variable or default"
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

fn validate_binding(
    config: &RuntimeBinding,
    interface: &ToolInterface,
) -> Result<(), ToolBuildError> {
    ensure!(
        !config.description.trim().is_empty(),
        InvalidToolSnafu {
            name: &config.name,
            message: "requires a nonblank binding description"
        }
    );
    for parameter in config.parameters.keys() {
        ensure!(
            interface.inputs.parameter(parameter).is_some(),
            InvalidToolSnafu {
                name: &config.name,
                message: format!("describes unknown input `{parameter}`")
            }
        );
    }
    Ok(())
}

/// Validate before redaction: bad defaults and unbound secrets must not disappear.
/// Only fallback presence survives; source names and literal defaults stay in the child.
fn project_tool(
    name: String,
    description: String,
    parameters: BTreeMap<String, ParameterDescription>,
    metadata: ToolMetadata,
) -> Result<RuntimeBinding, ToolBuildError> {
    let mut binding = RuntimeBinding {
        name,
        description,
        parameters,
        metadata,
    };
    let interface = prepare_tool_interface(&binding.name, binding.metadata.clone())?;
    validate_binding(&binding, &interface)?;
    for ports in binding.metadata.inputs.inputs.values_mut() {
        ports.retain(|_, input| !input.binding.secret);
        for input in ports.values_mut() {
            let has_source = input.binding.env.is_some() || input.binding.default.is_some();
            input.binding.env = has_source.then(|| "MF_TOOL_BINDING_SOURCE_PRESENT".into());
            input.binding.default = None;
        }
    }
    binding
        .parameters
        .retain(|name, _| binding.metadata.inputs.parameter(name).is_some());
    Ok(binding)
}

fn value_schema(value_type: &ValueType) -> Value {
    match value_type {
        ValueType::Any => json!({}),
        ValueType::Null => json!({"type": "null"}),
        ValueType::Boolean => json!({"type": "boolean"}),
        ValueType::Number | ValueType::Float32 | ValueType::Float64 => json!({"type": "number"}),
        ValueType::Int64 | ValueType::Uint64 | ValueType::Usize => json!({"type": "integer"}),
        ValueType::String => json!({"type": "string"}),
        ValueType::Array => json!({"type": "array"}),
        ValueType::Object => json!({"type": "object"}),
        ValueType::Flow => {
            unreachable!("runtime-only tool interfaces are rejected before schema projection")
        }
        ValueType::List(inner) => json!({"type": "array", "items": value_schema(inner)}),
        ValueType::Map(inner) => {
            json!({"type": "object", "additionalProperties": value_schema(inner)})
        }
        ValueType::Nullable(inner) => json!({"anyOf": [value_schema(inner), {"type": "null"}]}),
    }
}

fn tool_definition(
    name: String,
    description: String,
    parameters: &BTreeMap<String, ParameterDescription>,
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

#[cfg(test)]
fn project_definition(config: Value) -> Result<ToolDefinition, NodeBuildError> {
    let config: RuntimeBinding = deserialize_config(config)?;
    let interface = prepare_tool_interface(&config.name, config.metadata.clone())?;
    validate_binding(&config, &interface)?;
    Ok(tool_definition(
        config.name,
        config.description,
        &config.parameters,
        &interface,
    ))
}

/// Inspect the native capability, never an advertised or serialized interface.
fn flow_metadata(name: &str, flow: &FlowValue) -> Result<ToolMetadata, ToolBuildError> {
    let body = flow.body();
    ensure!(
        !body.is_event_stream(),
        InvalidToolSnafu {
            name,
            message: "requires a terminal-only body"
        }
    );
    ensure!(
        body.relations
            .as_ref()
            .is_none_or(|relations| relations.is_callable()),
        InvalidToolSnafu {
            name,
            message: "requires a callable prepared child without an implicit scope frame"
        }
    );
    Ok(ToolMetadata {
        inputs: body.inputs.clone(),
        outputs: body.outputs.clone(),
        stream_outputs: body.stream_outputs.clone(),
    })
}

/// Decode only the documentation fields with Serde; the executable Flow stays native.
fn tool_record(record: &ValueRef) -> Result<(&FlowValue, ToolDescription), ToolExecutionError> {
    ValueType::Object
        .validate_shared(record)
        .context(PlumbingValueSnafu { field: "tool" })?;
    let fields = record.as_object().expect("validated tool record");
    for field in fields.keys() {
        ensure!(
            matches!(field.as_ref(), "flow" | "description" | "parameters"),
            InvalidRecordSnafu {
                message: format!("unknown field `{field}`")
            }
        );
    }
    let flow = fields.get("flow").ok_or_else(|| MissingFlowSnafu.build())?;
    ValueType::Flow
        .validate_shared(flow)
        .context(PlumbingValueSnafu { field: "tool.flow" })?;
    let description = fields
        .get("description")
        .ok_or_else(|| MissingDescriptionSnafu.build())?;
    let description =
        serde_json::from_value(serde_json::to_value(description).context(DescriptionSnafu)?)
            .context(DescriptionSnafu)?;
    let parameters = match fields.get("parameters") {
        Some(parameters) => {
            serde_json::from_value(serde_json::to_value(parameters).context(DescriptionSnafu)?)
                .context(DescriptionSnafu)?
        }
        None => BTreeMap::new(),
    };
    Ok((
        flow.as_flow().expect("validated native tool flow"),
        ToolDescription {
            description,
            parameters,
        },
    ))
}

/// Projects the selected Flow's real interface without invoking or retaining its body.
struct DefinitionNode;
impl NodePortContract for DefinitionNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("tool", ValueType::Object, true),
                PortSpec::new("name", ValueType::String, true),
            ],
            outputs: vec![PortSpec::new("definition", ValueType::Object, true)],
        }
    }
}
impl TaskNode for DefinitionNode {
    fn execute(
        &self,
        mut inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let tool: ValueRef =
            mf_runtime::decode_input(&mut inputs, "tool").context(PlumbingInputSnafu)?;
        let name: Shared<String> =
            mf_runtime::decode_input(&mut inputs, "name").context(PlumbingInputSnafu)?;
        mf_runtime::reject_unknown_inputs(inputs).context(PlumbingInputSnafu)?;
        // The caller supplies Iteration.key, not an alias from the tool record.
        let key = name.value().as_str().expect("decoded name");
        let (flow, description) = tool_record(&tool)?;
        // Validate original typed declarations before redaction. The projected
        // metadata stays local; only the model-facing definition is published.
        let public = project_tool(
            key.to_owned(),
            description.description,
            description.parameters,
            flow_metadata(key, flow)?,
        )?;
        let interface = prepare_tool_interface(&public.name, public.metadata.clone())?;
        let definition = tool_definition(
            public.name.clone(),
            public.description.clone(),
            &public.parameters,
            &interface,
        );
        // Only the selected record participates: private siblings must not taint
        // an otherwise public tool. Never serialize the record containing a Flow.
        let definition = Shared::<ToolDefinition>::new(
            mf_runtime::encode_node_value(definition)
                .context(PlumbingValueSnafu {
                    field: "definition",
                })?
                .inherit_secrets([&tool, name.value()]),
        )
        .context(PlumbingValueSnafu {
            field: "definition",
        })?;
        Ok(Outputs::from([("definition".into(), definition.into_value())]).into())
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
                PortSpec::new("name", ValueType::String, true),
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
        ensure!(!call.id.trim().is_empty(), BlankCallIdSnafu);
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
                // Validation failures are tool-result data, not wrapped runtime errors.
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
                ValueRef::from(json!({"error": error}))
                    .inherit_secrets([&inputs["call"]["arguments"]]),
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
                (
                    "name".into(),
                    inputs["call"]
                        .get("name")
                        .expect("decoded call name")
                        .clone(),
                ),
            ]),
            skipped: [skipped.into()].into_iter().collect(),
            ..Default::default()
        })
    }
}

struct RuntimeArgumentsNode;
impl NodePortContract for RuntimeArgumentsNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("call", ValueType::Object, true),
                PortSpec::new("tools", ValueType::Map(Box::new(ValueType::Object)), true),
            ],
            outputs: vec![
                PortSpec::new("arguments", ValueType::Object, false),
                PortSpec::new("error", ValueType::Object, false),
                PortSpec::new("plain_content", ValueType::Boolean, true),
                PortSpec::new("name", ValueType::String, true),
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
        let call: Shared<ToolCall> =
            mf_runtime::decode_input(&mut inputs, "call").context(PlumbingInputSnafu)?;
        let decoded = call
            .decode()
            .context(PlumbingValueSnafu { field: "call" })?;
        ensure!(!decoded.id.trim().is_empty(), BlankCallIdSnafu);
        let name = call.value().get("name").expect("decoded call name").clone();
        let tools: ValueRef =
            mf_runtime::decode_input(&mut inputs, "tools").context(PlumbingInputSnafu)?;
        ValueType::Map(Box::new(ValueType::Object))
            .validate_shared(&tools)
            .context(PlumbingValueSnafu { field: "tools" })?;
        mf_runtime::reject_unknown_inputs(inputs).context(PlumbingInputSnafu)?;
        let Some(selected) = tools.get(name.as_str().expect("decoded call name")) else {
            // Missing selection depends on the requested name and collection root,
            // never on private arguments or another tool's private metadata.
            let selection = if tools.is_secret() {
                name.clone().into_secret()
            } else {
                name.clone()
            };
            return Ok(NodeResult {
                outputs: Outputs::from([
                    (
                        "error".into(),
                        ValueRef::from(json!({"error":"unknown tool"}))
                            .inherit_secrets([&selection]),
                    ),
                    (
                        "plain_content".into(),
                        ValueRef::from(false).inherit_secrets([&selection]),
                    ),
                    ("name".into(), name),
                ]),
                skipped: ["arguments".into()].into_iter().collect(),
                ..Default::default()
            });
        };
        let (flow, _) = tool_record(selected)?;
        let selected_flow = selected.get("flow").expect("decoded tool flow");
        // Validate against the actual executable capability, not the advertised schema.
        // Its private inputs remain present so model arguments cannot override them.
        let interface = prepare_tool_interface(&decoded.name, flow_metadata(&decoded.name, flow)?)?;
        let mut result = ArgumentsNode(interface)
            .execute(Inputs::from([("call".into(), call.into_value())]), context)?;
        for (output, value) in &mut result.outputs {
            // Keep the original call-name reference. Only the selected capability
            // and route name affect execution, never documentation or siblings.
            if output == "name" {
                continue;
            }
            *value = value.clone().inherit_secrets([selected_flow, &name]);
        }
        Ok(result)
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

fn arguments_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    let metadata = NodeMetadata {
        exclusive_outputs: vec!["arguments".into(), "error".into()],
        ..Default::default()
    };
    PreparedNode::new(RuntimeArgumentsNode, metadata)
}

fn definition_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::new(DefinitionNode, NodeMetadata::default())
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
    use mf_runtime::{InputBinding, PreparedSubgraph, WorkflowInput};

    fn metadata() -> ToolMetadata {
        ToolMetadata {
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

    fn prepared(body: ToolMetadata) -> PreparedSubgraph {
        PreparedSubgraph::streaming(vec![], body.outputs, body.stream_outputs, |_, _| {
            panic!("catalog and adapter nodes must never execute a tool body")
        })
        .with_input_schema(body.inputs)
    }

    fn binding(body: ToolMetadata) -> Value {
        json!({"name": "lookup", "description": "Look up values.", "metadata": body})
    }

    fn documentation() -> Value {
        json!({"description": "Look up values."})
    }

    fn native_flow(body: ToolMetadata) -> ValueRef {
        FlowValue::new(prepared(body)).unwrap().into()
    }

    fn tool(flow: ValueRef, documentation: Value) -> ValueRef {
        ValueRef::object(
            std::iter::once(("flow".into(), flow)).chain(
                documentation
                    .as_object()
                    .expect("test documentation object")
                    .iter()
                    .map(|(name, value)| (name.clone().into(), value.clone().into())),
            ),
        )
    }

    fn tools(selected: ValueRef) -> ValueRef {
        ValueRef::object([("lookup".into(), selected)])
    }

    fn definition_inputs(tool: ValueRef, name: ValueRef) -> Inputs {
        Inputs::from([("tool".into(), tool), ("name".into(), name)])
    }

    fn project(
        name: &str,
        documentation: Value,
        body: PreparedSubgraph,
    ) -> Result<NodeResult, NodeExecutionError> {
        let node = definition_factory(json!({})).boxed()?;
        let flow = FlowValue::new(body).boxed()?;
        execute_with(
            &node,
            definition_inputs(tool(flow.into(), documentation), name.into()),
        )
    }

    fn catalog(body: ToolMetadata) -> Result<NodeResult, NodeExecutionError> {
        project("lookup", documentation(), prepared(body))
    }

    fn execute_with(node: &PreparedNode, inputs: Inputs) -> Result<NodeResult, NodeExecutionError> {
        node.execution
            .as_task_node()
            .unwrap()
            .execute(inputs, &mut ExecutionContext::default())
    }

    fn call(arguments: Value) -> Inputs {
        Inputs::from([(
            "call".into(),
            json!({
                "id": "id", "name": "lookup", "arguments": arguments,
            })
            .into(),
        )])
    }

    #[test]
    fn flow_definition_is_sanitized_without_child_execution() {
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .binding
            .env = Some("PRIVATE_ENV_NAME".into());
        body.inputs.inputs.get_mut("params").unwrap().insert(
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
        for name in ["z-last", "9first"] {
            let documentation = json!({"description":"Description", "parameters": {
                "params.secret": {"description":"PRIVATE_SECRET_DESCRIPTION"}
            }});
            let node = definition_factory(json!({})).unwrap();
            assert_eq!(
                node.metadata.ports.inputs,
                [
                    PortSpec::new("tool", ValueType::Object, true),
                    PortSpec::new("name", ValueType::String, true),
                ]
            );
            assert_eq!(
                node.metadata.ports.outputs,
                [PortSpec::new("definition", ValueType::Object, true)]
            );
            let flow: ValueRef = FlowValue::new(prepared(body.clone())).unwrap().into();
            let selected = tool(flow.clone(), documentation);
            for _ in 0..2 {
                let result =
                    execute_with(&node, definition_inputs(selected.clone(), name.into())).unwrap();
                let public = serde_json::to_value(&result.outputs["definition"]).unwrap();
                assert_eq!(result.outputs.len(), 1);
                let definition =
                    Shared::<ToolDefinition>::new(result.outputs["definition"].clone())
                        .unwrap()
                        .decode()
                        .unwrap();
                assert_eq!(public["name"], name);
                assert_eq!(definition.name, name);
                assert_eq!(definition.input_schema["required"], json!([]));
                for forbidden in [
                    "hidden",
                    "PRIVATE_ENV_NAME",
                    "197531",
                    "secret",
                    "PRIVATE_SECRET_DESCRIPTION",
                    "body",
                ] {
                    assert!(!public.to_string().contains(forbidden), "{public}");
                    assert!(
                        !serde_json::to_string(&result.outputs["definition"])
                            .unwrap()
                            .contains(forbidden)
                    );
                }
                assert!(public.get("metadata").is_none());
                assert!(public.get("flow").is_none());
                let arguments = RuntimeArgumentsNode
                    .execute(
                        Inputs::from([
                            (
                                "tools".into(),
                                ValueRef::object([(name.into(), selected.clone())]),
                            ),
                            (
                                "call".into(),
                                json!({"id":"id", "name": name, "arguments": {}}).into(),
                            ),
                        ]),
                        &mut ExecutionContext::default(),
                    )
                    .unwrap();
                assert!(arguments.outputs.contains_key("arguments"));
                // Projection leaves the original capability and bindings untouched.
                assert_eq!(flow.as_flow().unwrap().body().inputs, body.inputs);
            }
        }
    }

    #[test]
    fn one_static_definition_node_projects_any_catalog_size_using_collection_keys() {
        // One prepared node and fixed ports, reused by the catalog Iteration body.
        // Neither tool names nor their number are configuration or graph structure.
        let node = definition_factory(json!({})).unwrap();
        let ports = node.metadata.ports.clone();
        for count in [0, 1, 3, 32] {
            let tools = ValueRef::object((0..count).map(|index| {
                (
                    format!("tool-{index}").into(),
                    tool(
                        FlowValue::new(prepared(metadata())).unwrap().into(),
                        json!({"description":format!("Tool {index}")}),
                    ),
                )
            }));
            let mut definitions = Vec::new();
            for (key, selected) in tools.as_object().unwrap() {
                let result = execute_with(
                    &node,
                    definition_inputs(selected.clone(), key.as_ref().into()),
                )
                .unwrap();
                assert_eq!(
                    result.outputs["definition"]["name"].as_str(),
                    Some(key.as_ref())
                );
                assert_eq!(result.outputs.len(), 1);
                definitions.push(result.outputs["definition"].clone());
            }
            assert_eq!(definitions.len(), count);
            assert_eq!(node.metadata.ports, ports);
        }
    }

    #[test]
    fn self_contained_records_are_successful_and_strictly_decoded() {
        let node = definition_factory(json!({})).unwrap();
        let flow = native_flow(metadata());
        // Omitting optional parameters is a complete, successful record.
        let result = execute_with(
            &node,
            definition_inputs(tool(flow.clone(), documentation()), "lookup".into()),
        )
        .unwrap();
        assert_eq!(
            result.outputs["definition"]["name"].as_str(),
            Some("lookup")
        );

        for document in [
            json!({"description":42}),
            json!({"description":"Description", "parameters":[]}),
            json!({"description":"Description", "parameters":{"params.visible":{}}}),
            json!({"description":"Description", "parameters":{"params.visible":{"description":"x", "extra":true}}}),
        ] {
            let error = execute_with(
                &node,
                definition_inputs(tool(flow.clone(), document), "lookup".into()),
            )
            .unwrap_err();
            assert!(matches!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<ToolExecutionError>(),
                Some(ToolExecutionError::Description { .. })
            ));
        }

        let error = execute_with(
            &node,
            definition_inputs(Value::Null.into(), "lookup".into()),
        )
        .unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::PlumbingValue { field, .. }) if field == "tool"
        ));

        let missing_description = ValueRef::object([("flow".into(), flow.clone())]);
        let error = execute_with(
            &node,
            definition_inputs(missing_description, "lookup".into()),
        )
        .unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::MissingDescription)
        ));

        let missing_flow = ValueRef::object([("description".into(), "Description".into())]);
        let error =
            execute_with(&node, definition_inputs(missing_flow, "lookup".into())).unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::MissingFlow)
        ));

        let unknown_field = tool(
            flow.clone(),
            json!({"description":"Description", "name":"alias"}),
        );
        let error =
            execute_with(&node, definition_inputs(unknown_field, "lookup".into())).unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::InvalidRecord { .. })
        ));

        let spoofed_flow = ValueRef::object([
            ("flow".into(), json!({"kind":"Flow", "body":{}}).into()),
            ("description".into(), "Description".into()),
        ]);
        let error =
            execute_with(&node, definition_inputs(spoofed_flow, "lookup".into())).unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::PlumbingValue { field, .. }) if field == "tool.flow"
        ));
    }

    #[test]
    fn flow_projection_requires_valid_names_descriptions_and_qualified_parameters() {
        for name in [
            "".to_owned(),
            "has.dot".into(),
            "has/slash".into(),
            "two words".into(),
            "工具".into(),
            "x".repeat(65),
        ] {
            let error = project(&name, documentation(), prepared(metadata())).unwrap_err();
            assert!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<ToolBuildError>()
                    .is_some(),
                "{name}"
            );
        }
        for name in [
            "search-web",
            "9lookup",
            "under_score",
            "else",
            &"x".repeat(64),
        ] {
            assert!(
                project_tool(
                    name.into(),
                    "Description".into(),
                    BTreeMap::new(),
                    metadata()
                )
                .is_ok()
            );
        }
        for description in ["", " \n\t"] {
            let mut config = documentation();
            config["description"] = json!(description);
            assert!(project("lookup", config, prepared(metadata())).is_err());
        }
        for parameter in [
            "visible",
            "other.visible",
            "params.missing",
            "params.visible.extra",
        ] {
            let mut config = documentation();
            config["parameters"] = json!({parameter: {"description": "Unknown"}});
            let error = project("lookup", config, prepared(metadata())).unwrap_err();
            assert!(error.to_string().contains("describes unknown input"));
            assert!(error.to_string().contains(parameter));
        }
    }

    #[test]
    fn flow_projection_rejects_streams_stdin_scoped_children_and_multiple_startups() {
        let mut variants = vec![];
        let mut body = metadata();
        body.stream_outputs
            .push(PortSpec::new("stream", ValueType::String, true));
        variants.push(body);
        let mut body = metadata();
        body.inputs
            .stdin
            .insert("params".into(), mf_runtime::StdinRequirement::Always);
        variants.push(body);
        let mut body = metadata();
        body.inputs.inputs.clear();
        variants.push(body);
        let mut body = metadata();
        body.inputs.inputs.insert("other".into(), BTreeMap::new());
        variants.push(body);
        for body in variants {
            assert!(catalog(body.clone()).is_err());
            let mut inputs = call(json!({"visible":"x"}));
            inputs.insert(
                "tools".into(),
                tools(tool(native_flow(body.clone()), documentation())),
            );
            assert!(
                RuntimeArgumentsNode
                    .execute(inputs, &mut ExecutionContext::default())
                    .is_err()
            );
            assert!(project_definition(binding(body)).is_err());
        }
        let scoped = prepared(metadata()).with_relations(mf_runtime::GraphRelations {
            scope: Some(mf_runtime::ScopeRelations {
                input: "params".into(),
                values: BTreeMap::new(),
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: BTreeSet::new(),
            }),
            ..Default::default()
        });
        assert!(project("lookup", documentation(), scoped).is_err());
        let event =
            PreparedSubgraph::from_stream_factory(vec![], metadata().inputs, vec![], || {
                panic!("event factory must not run during catalog preparation")
            });
        assert!(project("lookup", documentation(), event).is_err());
    }

    #[test]
    fn prepared_secret_sources_and_defaults_are_checked_before_redaction() {
        for secret in [false, true] {
            let mut body = metadata();
            let input = body
                .inputs
                .inputs
                .get_mut("params")
                .unwrap()
                .get_mut("secret")
                .unwrap();
            input.binding.secret = secret;
            input.binding.default = Some(json!(42)); // Declared string, even when hidden.
            let error: NodeExecutionError =
                project_tool("lookup".into(), "Description".into(), BTreeMap::new(), body)
                    .unwrap_err()
                    .into();
            let source = std::error::Error::source(&error).unwrap();
            let build = source.downcast_ref::<ToolBuildError>().unwrap();
            assert!(matches!(
                build,
                ToolBuildError::Metadata {
                    source: ToolMetadataError::MetadataInput {
                        source: mf_runtime::WorkflowInputError::TypeMismatch { .. }
                    },
                    ..
                }
            ));
            assert!(std::error::Error::source(source).is_some());
        }
        for required in [false, true] {
            let mut body = metadata();
            let input = body
                .inputs
                .inputs
                .get_mut("params")
                .unwrap()
                .get_mut("secret")
                .unwrap();
            input.required = required;
            input.binding.default = None;
            assert!(catalog(body.clone()).is_err());
            input_binding(&mut body, "secret").env = Some("UNRESOLVED_SECRET_ENV".into());
            assert!(catalog(body.clone()).is_ok());
            input_binding(&mut body, "secret").env = Some("BAD=ENV".into());
            assert!(catalog(body).is_err());
        }
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .retain(|name, _| name == "secret");
        let projection = project_tool(
            "lookup".into(),
            "Description".into(),
            BTreeMap::new(),
            body.clone(),
        )
        .unwrap();
        assert!(projection.metadata.inputs.inputs["params"].is_empty());
        let result = catalog(body).unwrap();
        assert_eq!(
            result.outputs["definition"]["input_schema"]["properties"],
            json!({})
        );
    }

    fn input_binding<'a>(body: &'a mut ToolMetadata, port: &str) -> &'a mut InputBinding {
        &mut body
            .inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut(port)
            .unwrap()
            .binding
    }

    fn with_field(value: &ValueRef, field: &str, replacement: ValueRef) -> ValueRef {
        ValueRef::object(value.as_object().unwrap().iter().map(|(name, value)| {
            (
                name.clone(),
                if name.as_ref() == field {
                    replacement.clone()
                } else {
                    value.clone()
                },
            )
        }))
    }

    fn with_secret_field(value: &ValueRef, field: &str) -> ValueRef {
        with_field(value, field, value[field].clone().into_secret())
    }

    #[test]
    fn model_visible_runtime_only_types_are_rejected_recursively() {
        for ty in [
            ValueType::Flow,
            ValueType::List(Box::new(ValueType::Flow)),
            ValueType::Map(Box::new(ValueType::Nullable(Box::new(ValueType::Flow)))),
        ] {
            let mut body = metadata();
            body.inputs
                .inputs
                .get_mut("params")
                .unwrap()
                .get_mut("visible")
                .unwrap()
                .value_type = ty.clone();
            let error = catalog(body.clone()).unwrap_err();
            let source = std::error::Error::source(&error).unwrap();
            assert!(matches!(
                source.downcast_ref::<ToolBuildError>(),
                Some(ToolBuildError::Metadata {
                    source: ToolMetadataError::InvalidMetadata { .. },
                    ..
                })
            ));
            assert!(std::error::Error::source(source).is_some());
            assert!(project_definition(binding(body.clone())).is_err());
            let mut inputs = call(json!({"visible":"x"}));
            inputs.insert(
                "tools".into(),
                tools(tool(native_flow(body), documentation())),
            );
            assert!(
                RuntimeArgumentsNode
                    .execute(inputs, &mut ExecutionContext::default())
                    .is_err()
            );
            let mut body = metadata();
            body.outputs.push(PortSpec::new("capability", ty, true));
            assert!(catalog(body.clone()).is_err());
            assert!(project_definition(binding(body)).is_err());
        }
    }

    #[test]
    fn secret_runtime_only_inputs_cannot_bypass_source_validation_before_redaction() {
        for ty in [
            ValueType::Flow,
            ValueType::List(Box::new(ValueType::Flow)),
            ValueType::Map(Box::new(ValueType::Nullable(Box::new(ValueType::Flow)))),
        ] {
            let mut body = metadata();
            let hidden = body
                .inputs
                .inputs
                .get_mut("params")
                .unwrap()
                .get_mut("secret")
                .unwrap();
            hidden.value_type = ty;
            hidden.binding.default = None;
            hidden.binding.env = Some("PRIVATE_FLOW_SOURCE".into());
            let error = project_tool(
                "lookup".into(),
                "Description".into(),
                BTreeMap::new(),
                body.clone(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                ToolBuildError::Metadata {
                    source: ToolMetadataError::MetadataInput { .. },
                    ..
                }
            ));
            assert!(catalog(body.clone()).is_err());
            input_binding(&mut body, "secret").env = None;
            // A hidden capability also cannot be left unbound and then erased.
            assert!(catalog(body).is_err());
        }
    }

    #[test]
    fn metadata_validation_preserves_typed_sources_and_checks_port_contracts() {
        for (node, port) in [("", "visible"), ("params", "")] {
            let mut body = metadata();
            let input = body.inputs.inputs["params"]["visible"].clone();
            body.inputs.inputs =
                BTreeMap::from([(node.into(), BTreeMap::from([(port.into(), input)]))]);
            assert!(matches!(
                body.validate(),
                Err(ToolMetadataError::InvalidMetadata { .. })
            ));
        }
        for ports in [
            vec![PortSpec::new("", ValueType::String, true)],
            vec![
                PortSpec::new("same", ValueType::String, true),
                PortSpec::new("same", ValueType::String, true),
            ],
        ] {
            let mut body = metadata();
            body.outputs = ports.clone();
            assert!(body.validate().is_err());
            assert!(catalog(body).is_err());
            let mut body = metadata();
            body.stream_outputs = ports;
            assert!(body.validate().is_err());
        }
        let mut body = metadata();
        let mut deep = ValueType::String;
        for _ in 0..256 {
            deep = ValueType::List(Box::new(deep));
        }
        body.outputs.push(PortSpec::new("deep", deep, true));
        assert!(matches!(
            body.validate(),
            Err(ToolMetadataError::MetadataTypeDepth { .. })
        ));
        let mut body = metadata();
        body.outputs
            .push(PortSpec::new("same", ValueType::String, true));
        body.stream_outputs
            .push(PortSpec::new("same", ValueType::String, true));
        body.validate().unwrap(); // Separate channels have separate name namespaces.
    }

    #[test]
    fn runtime_definition_preserves_descriptions_fallback_requiredness_and_hidden_secrets() {
        let mut body = metadata();
        let ports = body.inputs.inputs.get_mut("params").unwrap();
        for (port, binding) in [
            (
                "environment",
                InputBinding {
                    env: Some("UNRESOLVED_VISIBLE_ENV".into()),
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
                    env: Some("UNRESOLVED_SECRET_ENV".into()),
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
        let parameters: BTreeMap<_, _> = ports
            .keys()
            .map(|port| {
                (
                    format!("params.{port}"),
                    ParameterDescription {
                        description: format!("Description of {port}"),
                    },
                )
            })
            .collect();
        let result = project(
            "lookup",
            json!({"description":"Description", "parameters":parameters}),
            prepared(body.clone()),
        )
        .unwrap();
        let definition = Shared::<ToolDefinition>::new(result.outputs["definition"].clone())
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(definition.name, "lookup");
        assert_eq!(
            serde_json::to_value(&definition.input_schema).unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "visible": {"type": "string", "description": "Description of visible"},
                    "environment": {"type": "integer", "description": "Description of environment"},
                    "defaulted": {"type": "integer", "description": "Description of defaulted"},
                    "optional": {"type": "boolean", "description": "Description of optional"},
                },
                "required": ["visible"], "additionalProperties": false,
            })
        );
        let node = ArgumentsNode(prepare_tool_interface("lookup", body.clone()).unwrap());
        let result = node
            .execute(
                call(json!({"visible":"x"})),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.outputs["arguments"]).unwrap(),
            json!({"params.visible":"x"})
        );
        assert!(result.skipped.contains("error"));
        for invalid in [
            json!({}),
            json!({"visible":"x", "secret":"override"}),
            json!({"visible":"x", "secret_environment":1}),
            json!({"unknown":true}),
            json!({"visible":42}),
            json!([]),
            json!(null),
        ] {
            let mut inputs = call(invalid);
            inputs.insert(
                "tools".into(),
                tools(tool(native_flow(body.clone()), documentation())),
            );
            let result = RuntimeArgumentsNode
                .execute(inputs, &mut ExecutionContext::default())
                .unwrap();
            assert!(result.outputs.contains_key("error"));
            assert!(result.skipped.contains("arguments"));
        }
    }

    #[test]
    fn definition_inherits_selected_record_and_name_without_execution_document_taint() {
        let node = definition_factory(json!({})).unwrap();
        let public = tool(
            native_flow(metadata()),
            json!({
                "description":"Look up values.",
                "parameters":{"params.visible":{"description":"Visible parameter documentation."}}
            }),
        );
        let baseline =
            execute_with(&node, definition_inputs(public.clone(), "lookup".into())).unwrap();
        for (selected, name, secret) in [
            (public.clone(), ValueRef::from("lookup"), false),
            (with_secret_field(&public, "flow"), "lookup".into(), true),
            (public.clone().into_secret(), "lookup".into(), true),
            (public.clone(), ValueRef::from("lookup").into_secret(), true),
            (
                with_secret_field(&public, "description"),
                "lookup".into(),
                true,
            ),
            (
                with_secret_field(&public, "parameters"),
                "lookup".into(),
                true,
            ),
        ] {
            let result = execute_with(&node, definition_inputs(selected.clone(), name)).unwrap();
            {
                let output = "definition";
                let value = &result.outputs[output];
                assert_eq!(value.is_secret(), secret);
                assert_eq!(
                    serde_json::to_value(value).unwrap(),
                    serde_json::to_value(&baseline.outputs[output]).unwrap()
                );
                assert_eq!(
                    value.redacted(),
                    if secret {
                        ValueRef::from("[REDACTED]")
                    } else {
                        baseline.outputs[output].clone()
                    }
                );
            }
            for field in ["name", "description", "input_schema"] {
                assert_eq!(result.outputs["definition"][field].is_secret(), secret);
            }
            let definition = Shared::<ToolDefinition>::new(result.outputs["definition"].clone())
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(definition.name, "lookup");
            assert_eq!(
                definition.input_schema["properties"]["visible"]["description"],
                json!("Visible parameter documentation.")
            );
            // Documentation sensitivity only affects advertisement, not execution.
            // The selected record root still taints its nested native Flow.
            let catalog = ValueRef::object([
                ("lookup".into(), selected.clone()),
                (
                    "private-sibling".into(),
                    tool(native_flow(metadata()), documentation()).into_secret(),
                ),
            ]);
            let mut inputs = call(json!({"visible":"x"}));
            inputs.insert("tools".into(), catalog);
            let arguments = RuntimeArgumentsNode
                .execute(inputs, &mut ExecutionContext::default())
                .unwrap();
            let execution_secret = selected["flow"].is_secret();
            assert_eq!(arguments.outputs["arguments"].is_secret(), execution_secret);
            assert_eq!(
                arguments.outputs["plain_content"].is_secret(),
                execution_secret
            );
            assert!(selected["flow"].as_flow().is_some());
            assert!(serde_json::to_value(&selected).is_err());
        }
    }

    #[test]
    fn runtime_arguments_inherit_selected_flow_privacy_and_preserve_nested_argument_markers() {
        let mut body = metadata();
        body.inputs.inputs.get_mut("params").unwrap().insert(
            "other".into(),
            WorkflowInput {
                value_type: ValueType::Boolean,
                required: false,
                binding: InputBinding::default(),
            },
        );
        let public = tool(native_flow(body), documentation());
        for (selected, secret_arguments, secret_plain_content) in [
            (public.clone(), false, false),
            (public.clone().into_secret(), true, true),
        ] {
            for (arguments, valid) in [
                (json!({"visible":"x"}), true),
                (json!({}), false),
                (json!({"visible":"x", "secret":"override"}), false),
            ] {
                let mut inputs = call(arguments.clone());
                inputs.insert("tools".into(), tools(selected.clone()));
                let result = RuntimeArgumentsNode
                    .execute(inputs, &mut ExecutionContext::default())
                    .unwrap();
                let (output, skipped) = if valid {
                    ("arguments", "error")
                } else {
                    ("error", "arguments")
                };
                assert_eq!(result.outputs.len(), 3);
                assert_eq!(result.outputs["name"].as_str(), Some("lookup"));
                assert!(!result.outputs["name"].is_secret());
                assert!(result.skipped.contains(skipped));
                assert_eq!(result.outputs[output].is_secret(), secret_arguments);
                assert_eq!(
                    result.outputs["plain_content"].is_secret(),
                    secret_plain_content
                );
                assert_eq!(result.outputs["plain_content"].as_bool(), Some(false));
                for (name, secret) in [
                    (output, secret_arguments),
                    ("plain_content", secret_plain_content),
                ] {
                    if secret {
                        assert_eq!(
                            result.outputs[name].redacted(),
                            ValueRef::from("[REDACTED]")
                        );
                    }
                }
                if valid {
                    assert_eq!(
                        serde_json::to_value(&result.outputs["arguments"]).unwrap(),
                        json!({"params.visible":"x"})
                    );
                } else {
                    assert!(result.outputs["error"]["error"].is_string());
                }
            }
        }

        // Public metadata must not erase a nested argument marker or taint its
        // public siblings and the unrelated plain-content flag.
        let visible = ValueRef::from("private argument").into_secret();
        let result = RuntimeArgumentsNode
            .execute(
                Inputs::from([
                    ("tools".into(), tools(public)),
                    (
                        "call".into(),
                        ValueRef::object([
                            ("id".into(), "id".into()),
                            ("name".into(), "lookup".into()),
                            (
                                "arguments".into(),
                                ValueRef::object([
                                    ("visible".into(), visible.clone()),
                                    ("other".into(), true.into()),
                                ]),
                            ),
                        ]),
                    ),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert!(!result.outputs["arguments"].is_secret());
        assert!(result.outputs["arguments"]["params.visible"].ptr_eq(&visible));
        assert!(!result.outputs["arguments"]["params.other"].is_secret());
        assert!(!result.outputs["plain_content"].is_secret());
    }

    #[test]
    fn independent_calls_keep_argument_secrets_out_of_selection_and_other_calls() {
        let tools = ValueRef::object([
            (
                "lookup".into(),
                tool(native_flow(metadata()), documentation()),
            ),
            (
                "private-tool".into(),
                tool(native_flow(metadata()), documentation()).into_secret(),
            ),
        ]);
        let private_argument = ValueRef::from("private argument").into_secret();
        let first = ValueRef::object([
            ("id".into(), "first".into()),
            ("name".into(), "lookup".into()),
            (
                "arguments".into(),
                ValueRef::object([("visible".into(), private_argument.clone())]),
            ),
        ]);
        let second: ValueRef =
            json!({"id":"second", "name":"lookup", "arguments":{"visible":"public"}}).into();
        let calls = [first, second];
        let mut results = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            let result = RuntimeArgumentsNode
                .execute(
                    Inputs::from([
                        ("call".into(), call.clone()),
                        ("tools".into(), tools.clone()),
                    ]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert!(result.outputs["name"].ptr_eq(&call["name"]));
            assert!(!result.outputs["name"].contains_secret());
            assert!(!result.outputs["plain_content"].contains_secret());
            assert!(!result.outputs["arguments"].is_secret());
            assert_eq!(result.outputs["arguments"].contains_secret(), index == 0);
            assert!(result.skipped.contains("error"));
            if index == 0 {
                assert!(result.outputs["arguments"]["params.visible"].ptr_eq(&private_argument));
            }
            let encoded = encode_value(
                call.clone(),
                json!({"outputs":{"content":"done"}}).into(),
                Some(true),
            );
            assert_eq!(encoded.is_secret(), index == 0);
            results.push(result);
        }
        assert!(results[0].outputs["arguments"].contains_secret());
        assert!(!results[1].outputs["arguments"].contains_secret());
        assert!(!tools["lookup"].contains_secret());
    }

    #[test]
    fn private_call_names_mark_selection_outputs_but_preserve_the_original_name() {
        let catalog = tools(tool(native_flow(metadata()), documentation()));
        for requested in ["lookup", "missing"] {
            for arguments in [json!({"visible":"x"}), json!({})] {
                let public: ValueRef =
                    json!({"id":"id", "name":requested, "arguments":arguments}).into();
                for call in [with_secret_field(&public, "name"), public.into_secret()] {
                    let result = RuntimeArgumentsNode
                        .execute(
                            Inputs::from([
                                ("call".into(), call.clone()),
                                ("tools".into(), catalog.clone()),
                            ]),
                            &mut ExecutionContext::default(),
                        )
                        .unwrap();
                    assert!(result.outputs["name"].ptr_eq(&call["name"]));
                    assert!(result.outputs["name"].is_secret());
                    assert!(result.outputs["plain_content"].is_secret());
                    let valid = requested == "lookup" && call["arguments"].get("visible").is_some();
                    let (output, skipped) = if valid {
                        ("arguments", "error")
                    } else {
                        ("error", "arguments")
                    };
                    assert!(result.outputs[output].is_secret());
                    assert!(result.skipped.contains(skipped));
                    assert_eq!(
                        result.outputs[output].redacted(),
                        ValueRef::from("[REDACTED]")
                    );
                    if requested == "missing" {
                        assert_eq!(result.outputs["error"], json!({"error":"unknown tool"}));
                        assert_eq!(result.outputs["plain_content"].as_bool(), Some(false));
                    }
                    let encoded = encode_value(call, result.outputs[output].clone(), Some(false));
                    assert!(encoded.is_secret());
                    assert_eq!(encoded.redacted(), ValueRef::from("[REDACTED]"));
                }
            }
        }
    }

    #[test]
    fn unknown_tools_do_not_inspect_other_flow_interfaces_or_inherit_their_privacy() {
        // These bodies panic if invoked. An invalid tool interface on another
        // native Flow must not affect unknown calls or leak its privacy.
        let mut invalid_interface = metadata();
        invalid_interface.inputs.inputs.clear();
        let tools = ValueRef::object([
            (
                "lookup".into(),
                tool(native_flow(metadata()), documentation()).into_secret(),
            ),
            (
                "invalid-unused".into(),
                tool(native_flow(invalid_interface), documentation()),
            ),
            (
                "malformed-unused".into(),
                json!({"unknown":"not a tool record"}).into(),
            ),
        ]);
        let public: ValueRef =
            json!({"id":"id", "name":"missing", "arguments":{"private":"payload"}}).into();
        for call in [public.clone(), with_secret_field(&public, "arguments")] {
            for private_map in [false, true] {
                let map = if private_map {
                    tools.clone().into_secret()
                } else {
                    tools.clone()
                };
                let result = RuntimeArgumentsNode
                    .execute(
                        Inputs::from([("call".into(), call.clone()), ("tools".into(), map)]),
                        &mut ExecutionContext::default(),
                    )
                    .unwrap();
                assert_eq!(result.outputs.len(), 3);
                assert_eq!(result.outputs["error"], json!({"error":"unknown tool"}));
                assert_eq!(result.outputs["error"].is_secret(), private_map);
                assert_eq!(result.outputs["plain_content"].as_bool(), Some(false));
                assert_eq!(result.outputs["plain_content"].is_secret(), private_map);
                assert!(result.outputs["name"].ptr_eq(&call["name"]));
                assert!(!result.outputs["name"].is_secret());
                assert!(result.skipped.contains("arguments"));
            }
        }
    }

    #[test]
    fn selected_flow_and_whole_map_secrets_do_not_taint_original_public_names_or_other_calls() {
        let public = tool(native_flow(metadata()), documentation());
        let tools = ValueRef::object([
            ("lookup".into(), public),
            (
                "private-tool".into(),
                tool(native_flow(metadata()), documentation()).into_secret(),
            ),
        ]);
        for private_root in [false, true] {
            let map = if private_root {
                tools.clone().into_secret()
            } else {
                tools.clone()
            };
            for requested in ["lookup", "private-tool"] {
                for valid in [false, true] {
                    let arguments = if valid {
                        json!({"visible":"x"})
                    } else {
                        json!({})
                    };
                    let call: ValueRef =
                        json!({"id":"id", "name":requested, "arguments":arguments}).into();
                    let result = RuntimeArgumentsNode
                        .execute(
                            Inputs::from([
                                ("call".into(), call.clone()),
                                ("tools".into(), map.clone()),
                            ]),
                            &mut ExecutionContext::default(),
                        )
                        .unwrap();
                    let secret = private_root || requested == "private-tool";
                    let output = if valid { "arguments" } else { "error" };
                    assert_eq!(result.outputs[output].is_secret(), secret);
                    assert_eq!(result.outputs["plain_content"].is_secret(), secret);
                    assert!(result.outputs["name"].ptr_eq(&call["name"]));
                    assert!(!result.outputs["name"].is_secret());
                }
            }
        }
    }

    #[test]
    fn runtime_rejects_spoofed_json_flows_and_legacy_protocols() {
        let runtime = arguments_factory(json!({})).unwrap();
        for invalid in [
            json!([]),
            json!({"lookup":"<Flow>"}),
            json!({"lookup":{"kind":"Flow", "body":{}}}),
            json!({"lookup":binding(metadata())}),
            json!([{ "definition":{}, "binding":binding(metadata()) }]),
            json!({"lookup":{"flow":{"kind":"Flow", "body":{}}, "description":"Spoofed"}}),
        ] {
            let mut inputs = call(json!({"visible":"x"}));
            inputs.insert("tools".into(), invalid.into());
            let error = execute_with(&runtime, inputs).unwrap_err();
            assert!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<ToolExecutionError>()
                    .is_some()
            );
        }

        // A direct native Map<Flow> is no longer accepted: entries must be records.
        let mut inputs = call(json!({"visible":"x"}));
        inputs.insert("tools".into(), tools(native_flow(metadata())));
        let error = execute_with(&runtime, inputs).unwrap_err();
        assert!(matches!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<ToolExecutionError>(),
            Some(ToolExecutionError::PlumbingValue { field, .. }) if field == "tools"
        ));

        let mut inputs = call(json!({"visible":"x"}));
        inputs.insert("bindings".into(), json!([]).into());
        assert!(execute_with(&runtime, inputs).is_err());
        let mut inputs = call(json!({"visible":"x"}));
        inputs.insert(
            "tools".into(),
            tools(tool(native_flow(metadata()), documentation())),
        );
        inputs.insert("bindings".into(), json!([]).into());
        assert!(execute_with(&runtime, inputs).is_err());
        let mut inputs = call(json!({"visible":"x"}));
        inputs.insert(
            "tools".into(),
            tools(tool(native_flow(metadata()), documentation())),
        );
        inputs.insert("flows".into(), json!({}).into());
        assert!(execute_with(&runtime, inputs).is_err());
    }

    #[test]
    fn runtime_arguments_strictly_decode_calls_before_unknown_selection() {
        let runtime = arguments_factory(json!({})).unwrap();
        let interface = ArgumentsNode(prepare_tool_interface("lookup", metadata()).unwrap());
        assert_eq!(runtime.metadata.ports.outputs, interface.ports().outputs);
        assert_eq!(
            runtime.metadata.ports.inputs,
            [
                PortSpec::new("call", ValueType::Object, true),
                PortSpec::new("tools", ValueType::Map(Box::new(ValueType::Object)), true,),
            ]
        );
        assert!(runtime.metadata.ports.outputs.contains(&PortSpec::new(
            "name",
            ValueType::String,
            true,
        )));
        for call in [
            json!({}),
            json!({"id":42, "name":"missing", "arguments":{}}),
            json!({"id":"id", "name":42, "arguments":{}}),
            json!({"id":"id", "name":"missing", "arguments":{}, "extra":true}),
            json!({"id":" ", "name":"missing", "arguments":{}}),
        ] {
            let error = execute_with(
                &runtime,
                Inputs::from([
                    ("call".into(), call.into()),
                    ("tools".into(), json!({}).into()),
                ]),
            )
            .unwrap_err();
            assert!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<ToolExecutionError>()
                    .is_some()
            );
        }
    }

    #[test]
    fn single_node_addition_has_exact_described_schema_and_qualified_arguments() {
        let body = ToolMetadata {
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
            outputs: vec![PortSpec::new("sum", ValueType::Int64, true)],
            stream_outputs: vec![],
        };
        let config = json!({"name":"add", "description":"Add two integers.", "metadata":body,
            "parameters": {"calculate.left":{"description":"Left operand."}, "calculate.right":{"description":"Right operand."}}});
        assert!(arguments_factory(config.clone()).is_err());
        let definition = project_definition(config.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&definition.input_schema).unwrap(),
            json!({
                "type":"object", "properties": {
                    "left":{"type":"integer", "description":"Left operand."},
                    "right":{"type":"integer", "description":"Right operand."},
                }, "required":["left","right"], "additionalProperties":false,
            })
        );
        let config: RuntimeBinding = deserialize_config(config).unwrap();
        let result = ArgumentsNode(prepare_tool_interface(&config.name, config.metadata).unwrap())
            .execute(
                call(json!({"left":2,"right":3})),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result.outputs["arguments"]).unwrap(),
            json!({"calculate.left":2,"calculate.right":3})
        );
    }

    #[test]
    fn schemas_cover_every_value_contract() {
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
            (ValueType::String, json!({"type":"string"})),
            (ValueType::Array, json!({"type":"array"})),
            (ValueType::Object, json!({"type":"object"})),
            (
                ValueType::List(Box::new(ValueType::String)),
                json!({"type":"array","items":{"type":"string"}}),
            ),
            (
                ValueType::Map(Box::new(ValueType::Boolean)),
                json!({"type":"object","additionalProperties":{"type":"boolean"}}),
            ),
            (
                ValueType::Nullable(Box::new(ValueType::String)),
                json!({"anyOf":[{"type":"string"},{"type":"null"}]}),
            ),
        ] {
            assert_eq!(value_schema(&ty), expected);
        }
        let nested = ValueType::Map(Box::new(ValueType::List(Box::new(ValueType::Nullable(
            Box::new(ValueType::Int64),
        )))));
        assert_eq!(
            value_schema(&nested)["additionalProperties"]["items"]["anyOf"],
            json!([{"type":"integer"},{"type":"null"}])
        );
    }

    #[test]
    fn runtime_map_arguments_keep_typed_value_validation() {
        let mut body = metadata();
        body.inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .value_type = ValueType::Map(Box::new(ValueType::Boolean));
        let catalog = tools(tool(native_flow(body), documentation()));
        for (visible, valid) in [
            (json!({"flag":true}), true),
            (json!({"flag":"not boolean"}), false),
        ] {
            let mut inputs = call(json!({"visible":visible}));
            inputs.insert("tools".into(), catalog.clone());
            let result = RuntimeArgumentsNode
                .execute(inputs, &mut ExecutionContext::default())
                .unwrap();
            assert_eq!(result.outputs.contains_key("arguments"), valid);
            assert_eq!(result.outputs.contains_key("error"), !valid);
        }
    }

    #[test]
    fn private_argument_validation_errors_do_not_reveal_argument_names() {
        const PRIVATE_KEY: &str = "private-unknown-argument";
        let interface = prepare_tool_interface("lookup", metadata()).unwrap();
        let inputs = Inputs::from([(
            "call".into(),
            ValueRef::object([
                ("id".into(), "id".into()),
                ("name".into(), "lookup".into()),
                (
                    "arguments".into(),
                    ValueRef::from(json!({(PRIVATE_KEY):"value"})).into_secret(),
                ),
            ]),
        )]);
        let result = ArgumentsNode(interface)
            .execute(inputs, &mut ExecutionContext::default())
            .unwrap();
        let error = &result.outputs["error"];
        assert!(error["error"].as_str().unwrap().contains(PRIVATE_KEY));
        assert!(error.is_secret());
        assert!(
            !serde_json::to_string(&error.redacted())
                .unwrap()
                .contains(PRIVATE_KEY)
        );
        assert!(!result.outputs["plain_content"].contains_secret());
    }

    #[test]
    fn adapter_factories_and_runtime_projection_reject_invalid_config() {
        assert!(definition_factory(documentation()).is_err());
        assert!(definition_factory(json!({})).is_ok());
        assert!(definition_factory(json!({"binding":{}})).is_err());
        assert!(encode_factory(json!({})).is_ok());
        assert!(encode_factory(json!({"extra":true})).is_err());
        assert!(arguments_factory(json!({})).is_ok());
        assert!(arguments_factory(Value::Null).is_err());
        for config in [
            json!({}),
            json!({"name":"lookup","description":"x"}),
            json!({"name":"lookup","description":" ","metadata":metadata()}),
            json!({"name":"lookup","description":"x","metadata":metadata(),"extra":true}),
            json!({"name":"lookup","description":"x","metadata":metadata(),"parameters":{"params.visible":{}}}),
            json!({"name":"lookup","description":"x","metadata":metadata(),"parameters":{"missing":{"description":"x"}}}),
        ] {
            let error = project_definition(config).unwrap_err();
            assert!(std::error::Error::source(&error).is_some());
        }
        for config in [
            json!({"name":[],"description":"x"}),
            json!({"name":"lookup"}),
            json!({"name":"lookup","description":"x","parameters":[]}),
            json!({"name":"lookup","description":"x","parameters":{"params.visible":{}}}),
            json!({"name":"lookup","description":"x","extra":true}),
            json!({"name":"lookup","description":"x","body":{}}),
            binding(metadata()),
        ] {
            assert!(definition_factory(config).is_err());
        }
    }

    #[test]
    fn definition_requires_a_native_flow_and_preserves_typed_input_errors() {
        let node = definition_factory(json!({})).unwrap();
        for inputs in [
            Inputs::new(),
            Inputs::from([("flow".into(), json!({}).into())]),
            Inputs::from([("flow".into(), "<Flow>".into())]),
            Inputs::from([("binding".into(), binding(metadata()).into())]),
            Inputs::from([
                (
                    "flow".into(),
                    FlowValue::new(prepared(metadata())).unwrap().into(),
                ),
                ("extra".into(), true.into()),
            ]),
        ] {
            let error = execute_with(&node, inputs).unwrap_err();
            let source = std::error::Error::source(&error).unwrap();
            assert!(matches!(
                source.downcast_ref::<ToolExecutionError>(),
                Some(ToolExecutionError::PlumbingInput { .. })
            ));
            assert!(std::error::Error::source(source).is_some());
        }
        let valid = definition_inputs(
            tool(native_flow(metadata()), documentation()),
            "lookup".into(),
        );
        for field in ["tool", "name"] {
            let mut missing = valid.clone();
            missing.remove(field);
            assert!(execute_with(&node, missing).is_err());
        }
        for legacy in ["extra", "descriptions"] {
            let mut extra = valid.clone();
            extra.insert(legacy.into(), json!([]).into());
            assert!(execute_with(&node, extra).is_err());
        }
        for (field, value) in [("name", json!(42)), ("tool", json!([]))] {
            let mut invalid = valid.clone();
            invalid.insert(field.into(), value.into());
            assert!(execute_with(&node, invalid).is_err());
        }
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        for kind in [
            "llm.tool",
            "llm.tool_set",
            "llm.tool_calls",
            "llm.tool_call",
        ] {
            assert!(registry.get(kind).is_none());
        }
        for kind in [
            "llm.tool_definition",
            "llm.tool_arguments",
            "llm.tool_result",
        ] {
            assert!(registry.get(kind).is_some());
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
            let interface = prepare_tool_interface("lookup", body).unwrap();
            assert_eq!(interface.plain_content, expected);
            let arguments = ArgumentsNode(interface)
                .execute(
                    call(json!({"visible":"x"})),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(arguments.outputs["plain_content"].as_bool(), Some(expected));
            assert_eq!(
                encode(json!({"outputs":{"content":"text"}}), Some(expected)),
                if expected {
                    "text"
                } else {
                    "{\"content\":\"text\"}"
                }
            );
        }
    }

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
            let interface = prepare_tool_interface("lookup", metadata()).unwrap();
            assert!(
                ArgumentsNode(interface)
                    .execute(
                        Inputs::from([(
                            "call".into(),
                            json!({
                                "id":id, "name":"lookup", "arguments":{"visible":"x"},
                            })
                            .into()
                        )]),
                        &mut ExecutionContext::default()
                    )
                    .is_err()
            );
        }
    }
}
