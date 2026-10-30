use mf_runtime::{GraphInput, NodeValue, ValueSchema, ValueType, WorkflowGraph};
use serde::Serialize;
use serde_json::{Value, json};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, io::Write};

pub const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;
pub const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_BATCH_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_TOOL_CALLS: usize = 128;

#[derive(Debug, Snafu)]
pub enum ToolSchemaError {
    #[snafu(display(
        "tool alias `{name}` requires 1 to 64 ASCII letters, digits, underscores, or hyphens"
    ))]
    Alias { name: String },
    #[snafu(display("tool `{tool}` parameter `{parameter}`: {reason}"))]
    Unsupported {
        tool: String,
        parameter: String,
        reason: &'static str,
    },
    #[snafu(display("LLM tool declarations exceed the supported size or count"))]
    DeclarationsLimit,
    #[snafu(display("dispatch branch `{name}` requires connected gates and parameter ports"))]
    Connection { name: String },
    #[snafu(display("tool workflow requires one public dispatch selector and arguments input"))]
    Entry,
    #[snafu(display("tool workflow metadata is missing at `{path}`"))]
    Source { path: String },
    #[snafu(display("could not encode tool declarations: {source}"))]
    Encode { source: mf_runtime::TypeMismatch },
    #[snafu(
        display("tool metadata is unavailable outside workflow execution"),
        visibility(pub)
    )]
    Unavailable,
}
impl From<ToolSchemaError> for mf_runtime::NodeExecutionError {
    fn from(source: ToolSchemaError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum ToolEncodingError {
    #[snafu(display("tool data exceeds the size limit"))]
    Size,
    #[snafu(display("tool data could not be encoded"))]
    Json { source: serde_json::Error },
}

#[derive(Clone, Debug, NodeValue)]
pub struct DispatchBinding {
    pub selector: String,
    pub arguments: String,
}

struct Parameter {
    name: String,
    descriptors: Vec<GraphInput>,
}
impl Parameter {
    fn name(&self) -> &str {
        &self.name
    }
    // A selected route publishes every explicitly connected argument value.
    fn required(&self) -> bool {
        true
    }
    fn constraints(&self) -> impl Iterator<Item = &ValueType> {
        self.descriptors.iter().map(|input| &input.value_type)
    }
    fn schema_constraints(&self) -> impl Iterator<Item = &ValueSchema> {
        self.descriptors
            .iter()
            .filter_map(|input| input.schema.as_ref())
    }
    fn is_secret(&self) -> bool {
        self.descriptors.iter().any(|input| input.secret)
    }
}

pub struct ToolCatalog {
    pub tools: Vec<crate::ToolDefinition>,
    pub binding: DispatchBinding,
}

fn input_location<'graph>(
    mut graph: &'graph WorkflowGraph,
    mut input: &'graph GraphInput,
) -> (&'graph WorkflowGraph, &'graph GraphInput) {
    while let Some(child) = graph
        .nodes
        .get(&input.node)
        .and_then(|node| node.subgraph.as_deref())
        && let Some(origin) = child.inputs.get(&input.port)
    {
        graph = child;
        input = origin;
    }
    (graph, input)
}

impl ToolCatalog {
    pub fn from_path(
        graph: &WorkflowGraph,
        source: &[mf_runtime::DefinitionId],
        descriptions: &BTreeMap<String, String>,
    ) -> Result<Self, ToolSchemaError> {
        let graph = if source.is_empty() {
            graph
        } else {
            graph
                .locate(source)
                .and_then(|(_, node)| node.subgraph.as_deref())
                .with_context(|| SourceSnafu {
                    path: source
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("/"),
                })?
        };
        Self::new(graph, descriptions)
    }
    pub fn new(
        graph: &WorkflowGraph,
        descriptions: &BTreeMap<String, String>,
    ) -> Result<Self, ToolSchemaError> {
        let entries: Vec<_> = graph
            .inputs
            .iter()
            .filter_map(|(name, input)| {
                let (scope, input) = input_location(graph, input);
                if input.port != "selector" {
                    return None;
                }
                let Some(ValueSchema::Enum(branches)) = input.schema.as_ref() else {
                    return None;
                };
                let arguments = graph.inputs.iter().find(|(_, candidate)| {
                    let (candidate_scope, candidate) = input_location(graph, candidate);
                    std::ptr::eq(scope, candidate_scope)
                        && candidate.node == input.node
                        && candidate.port == "arguments"
                })?;
                let node = scope.nodes.get(&input.node)?;
                (node.exclusive_outputs == *branches).then_some((
                    name,
                    input,
                    arguments.0,
                    node,
                    branches,
                    scope,
                ))
            })
            .collect();
        ensure!(entries.len() == 1, EntrySnafu);
        let (selector, entry, arguments, node, branches, graph) = entries[0];
        ensure!(branches.len() <= MAX_TOOL_CALLS, DeclarationsLimitSnafu);
        let mut tools = Vec::new();
        for name in branches {
            ensure!(crate::call::valid_name(name), AliasSnafu { name });
            ensure!(
                graph
                    .controls
                    .iter()
                    .any(|edge| edge.from_node == entry.node && edge.from_output == *name),
                ConnectionSnafu { name }
            );
            let prefix = format!("{name}.");
            let mut parameters = Vec::new();
            for output in node
                .ports
                .outputs
                .iter()
                .filter(|port| port.name.starts_with(&prefix))
            {
                let field = output.name.strip_prefix(&prefix).unwrap();
                let mut descriptors = Vec::new();
                for edge in graph
                    .connections
                    .iter()
                    .filter(|edge| edge.from_node == entry.node && edge.from_output == output.name)
                {
                    let target = graph
                        .nodes
                        .get(&edge.to_node)
                        .context(ConnectionSnafu { name })?;
                    let port = target
                        .ports
                        .inputs
                        .iter()
                        .find(|port| port.name == edge.to_input)
                        .context(ConnectionSnafu { name })?;
                    descriptors.push(GraphInput {
                        node: edge.to_node.clone(),
                        port: edge.to_input.clone(),
                        value_type: port.value_type.clone(),
                        required: true,
                        schema: target.input_schemas.get(&edge.to_input).cloned(),
                        secret: target.secret_inputs.contains(&edge.to_input),
                    });
                }
                ensure!(!descriptors.is_empty(), ConnectionSnafu { name });
                parameters.push(Parameter {
                    name: field.into(),
                    descriptors,
                });
            }
            parameters.sort_by(|left, right| left.name.cmp(&right.name));
            tools.push(crate::ToolDefinition {
                name: name.clone(),
                description: descriptions.get(name).cloned().unwrap_or_default(),
                input_schema: parameters_schema(name, &parameters)?.into(),
            });
        }
        let encoded = mf_runtime::OutputValue::encode(tools.clone()).context(EncodeSnafu)?;
        ensure!(
            bounded_json(&encoded, MAX_TOOL_SCHEMA_BYTES).is_ok(),
            DeclarationsLimitSnafu
        );
        Ok(Self {
            tools,
            binding: DispatchBinding {
                selector: selector.clone(),
                arguments: arguments.clone(),
            },
        })
    }
}

struct LimitedBytes {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
}
impl Write for LimitedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.overflow = true;
            return Err(std::io::Error::other("tool data size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn bounded_json<T: Serialize + ?Sized>(
    value: &T,
    limit: usize,
) -> Result<String, ToolEncodingError> {
    let mut output = LimitedBytes {
        bytes: Vec::new(),
        limit,
        overflow: false,
    };
    let result = serde_json::to_writer(&mut output, value);
    ensure!(!output.overflow, SizeSnafu);
    result.context(JsonSnafu)?;
    Ok(String::from_utf8(output.bytes).expect("JSON serialization produces UTF-8"))
}

fn parameters_schema(tool: &str, parameters: &[Parameter]) -> Result<Value, ToolSchemaError> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in parameters {
        let mut constraints = Vec::new();
        for value_type in parameter.constraints() {
            constraints.push(lower_type(value_type, tool, parameter.name())?);
        }
        for schema in parameter.schema_constraints() {
            constraints.push(lower_schema(schema, tool, parameter.name())?);
        }
        constraints.retain(|value| value.as_object().is_none_or(|object| !object.is_empty()));
        constraints.dedup();
        let mut schema = match constraints.len() {
            0 => json!({}),
            1 => constraints.pop().unwrap(),
            _ => json!({"allOf":constraints}),
        };
        if parameter.is_secret() {
            schema["writeOnly"] = true.into();
        }
        properties.insert(parameter.name().into(), schema);
        if parameter.required() {
            required.push(parameter.name());
        }
    }
    Ok(
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}

fn lower_type(value: &ValueType, tool: &str, parameter: &str) -> Result<Value, ToolSchemaError> {
    Ok(match value {
        ValueType::Any => json!({}),
        ValueType::Null => json!({"type":"null"}),
        ValueType::Boolean => json!({"type":"boolean"}),
        ValueType::String => json!({"type":"string"}),
        ValueType::Number => json!({"type":"number"}),
        ValueType::Int64 => {
            json!({"type":"integer","minimum":i64::MIN,"maximum":i64::MAX,"$comment":"Runtime also rejects negative zero."})
        }
        ValueType::Uint64 => {
            json!({"type":"integer","minimum":0,"maximum":u64::MAX,"$comment":"Runtime also rejects negative zero."})
        }
        ValueType::Usize => {
            json!({"type":"integer","minimum":0,"maximum":usize::MAX,"$comment":"Runtime also rejects negative zero."})
        }
        ValueType::Float64 => lossless_f64(),
        ValueType::Float32 => {
            return UnsupportedSnafu {
                tool,
                parameter,
                reason: "lossless binary32 values require an explicit supported projection",
            }
            .fail();
        }
        ValueType::Array => json!({"type":"array","items":{}}),
        ValueType::Object => json!({"type":"object","additionalProperties":true}),
        ValueType::List(inner) => json!({"type":"array","items":lower_type(inner,tool,parameter)?}),
        ValueType::Map(inner) => {
            json!({"type":"object","additionalProperties":lower_type(inner,tool,parameter)?})
        }
        ValueType::Nullable(inner) => {
            json!({"anyOf":[{"type":"null"},lower_type(inner,tool,parameter)?]})
        }
    })
}

fn lossless_f64() -> Value {
    // Nonintegral JSON numbers already use binary64. Stored integers must preserve significand bits.
    let mut alternatives = vec![
        json!({"not":{"type":"integer"}}),
        json!({"minimum":-(1_i64 << 53),"maximum":1_u64 << 53}),
        json!({"exclusiveMaximum":i64::MIN}),
        json!({"exclusiveMinimum":u64::MAX}),
    ];
    for exponent in 53..64 {
        let upper = if exponent == 63 {
            u64::MAX
        } else {
            1_u64 << (exponent + 1)
        };
        alternatives.push(json!({"minimum":1_u64 << exponent,"maximum":upper,"multipleOf":1_u64 << (exponent - 52)}));
        if exponent < 63 {
            let lower = if exponent == 62 {
                i64::MIN
            } else {
                -(1_i64 << (exponent + 1))
            };
            alternatives.push(json!({"minimum":lower,"maximum":-(1_i64 << exponent),"multipleOf":1_u64 << (exponent - 52)}));
        }
    }
    json!({"type":"number","anyOf":alternatives})
}

fn object(
    fields: &BTreeMap<String, mf_runtime::SchemaField>,
    tool: &str,
    parameter: &str,
) -> Result<Value, ToolSchemaError> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, field) in fields {
        properties.insert(name.clone(), lower_schema(&field.schema, tool, parameter)?);
        if field.required {
            required.push(name);
        }
    }
    Ok(
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}
fn lower_schema(
    schema: &ValueSchema,
    tool: &str,
    parameter: &str,
) -> Result<Value, ToolSchemaError> {
    Ok(match schema {
        ValueSchema::Type(value) => lower_type(value, tool, parameter)?,
        ValueSchema::Object(fields) => object(fields, tool, parameter)?,
        ValueSchema::List(inner) => {
            json!({"type":"array","items":lower_schema(inner,tool,parameter)?})
        }
        ValueSchema::Map(inner) => {
            json!({"type":"object","additionalProperties":lower_schema(inner,tool,parameter)?})
        }
        ValueSchema::Nullable(inner) => {
            json!({"anyOf":[{"type":"null"},lower_schema(inner,tool,parameter)?]})
        }
        ValueSchema::Enum(values) => {
            ensure!(
                !values.is_empty(),
                UnsupportedSnafu {
                    tool,
                    parameter,
                    reason: "empty enum contract"
                }
            );
            json!({"type":"string","enum":values})
        }
        ValueSchema::Tagged { tag, variants } => {
            ensure!(
                !variants.is_empty(),
                UnsupportedSnafu {
                    tool,
                    parameter,
                    reason: "empty tagged contract"
                }
            );
            let variants = variants
                .iter()
                .map(|(name, fields)| {
                    let mut value = object(fields, tool, parameter)?;
                    value["properties"]
                        .as_object_mut()
                        .unwrap()
                        .insert(tag.clone(), json!({"const":name,"type":"string"}));
                    value["required"]
                        .as_array_mut()
                        .unwrap()
                        .push(tag.clone().into());
                    Ok(value)
                })
                .collect::<Result<Vec<_>, ToolSchemaError>>()?;
            json!({"oneOf":variants})
        }
        ValueSchema::Described {
            schema,
            description,
        } => {
            let mut value = lower_schema(schema, tool, parameter)?;
            value
                .as_object_mut()
                .unwrap()
                .insert("description".into(), description.clone().into());
            value
        }
        ValueSchema::Opaque { .. } => {
            return UnsupportedSnafu {
                tool,
                parameter,
                reason: "opaque, recursive, or incomplete codec schema",
            }
            .fail();
        }
    })
}

pub fn validate_wire_arguments(reply: &Value) -> Result<(), crate::LlmExecutionError> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for choice in reply
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for call in choice
            .pointer("/message/tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            count += 1;
            ensure!(
                count <= MAX_TOOL_CALLS,
                crate::ToolLimitSnafu {
                    resource: "batch count"
                }
            );
            if let Some(raw) = call.pointer("/function/arguments").and_then(Value::as_str) {
                ensure!(
                    raw.len() <= MAX_TOOL_ARGUMENT_BYTES,
                    crate::ToolLimitSnafu {
                        resource: "argument bytes"
                    }
                );
                bytes = bytes.saturating_add(raw.len());
                ensure!(
                    bytes <= MAX_TOOL_BATCH_ARGUMENT_BYTES,
                    crate::ToolLimitSnafu {
                        resource: "batch argument bytes"
                    }
                );
                let _: Value = mf_runtime::decode_unique_json(raw.as_bytes())
                    .context(crate::ToolArgumentsJsonSnafu)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{
        ControlEdgeDefinition, DefinitionId, EdgeDefinition, GraphNode, NodePorts, PortSpec,
    };
    use std::collections::BTreeSet;
    fn node() -> GraphNode {
        GraphNode {
            ports: NodePorts::default(),
            stream_outputs: vec![],
            input_schemas: BTreeMap::new(),
            secret_inputs: BTreeSet::new(),
            exclusive_outputs: vec![],
            subgraph: None,
        }
    }
    fn graph() -> WorkflowGraph {
        let mut dispatch = node();
        dispatch.ports.inputs = vec![
            PortSpec::new("selector", ValueType::String, true),
            PortSpec::new("arguments", ValueType::Object, true),
        ];
        dispatch.ports.outputs = vec![
            PortSpec::new("multiply", ValueType::Boolean, false),
            PortSpec::new("multiply.amount", ValueType::Any, false),
        ];
        dispatch.exclusive_outputs = vec!["multiply".into()];
        dispatch.input_schemas.insert(
            "selector".into(),
            ValueSchema::Enum(vec!["multiply".into()]),
        );
        let mut calculate = node();
        calculate.ports.inputs = vec![
            PortSpec::new("amount", ValueType::Int64, true),
            PortSpec::new("factor", ValueType::Int64, true),
        ];
        WorkflowGraph {
            inputs: BTreeMap::from([
                (
                    "router.selector".into(),
                    GraphInput {
                        node: "router".into(),
                        port: "selector".into(),
                        value_type: ValueType::String,
                        required: true,
                        schema: Some(ValueSchema::Enum(vec!["multiply".into()])),
                        secret: false,
                    },
                ),
                (
                    "router.arguments".into(),
                    GraphInput {
                        node: "router".into(),
                        port: "arguments".into(),
                        value_type: ValueType::Object,
                        required: true,
                        schema: None,
                        secret: false,
                    },
                ),
            ]),
            nodes: BTreeMap::from([
                ("router".into(), dispatch),
                ("calculate".into(), calculate),
                ("config".into(), node()),
            ]),
            connections: vec![
                EdgeDefinition {
                    exclusive: false,
                    from_node: "router".into(),
                    from_output: "multiply.amount".into(),
                    to_node: "calculate".into(),
                    to_input: "amount".into(),
                },
                EdgeDefinition {
                    exclusive: false,
                    from_node: "config".into(),
                    from_output: "factor".into(),
                    to_node: "calculate".into(),
                    to_input: "factor".into(),
                },
            ],
            controls: vec![ControlEdgeDefinition {
                from_node: "router".into(),
                from_output: "multiply".into(),
                to_node: "calculate".into(),
            }],
        }
    }
    #[test]
    fn dispatch_catalog_uses_linked_ports_and_excludes_other_upstreams() {
        let catalog = ToolCatalog::new(&graph(), &BTreeMap::new()).unwrap();
        assert_eq!(catalog.binding.selector, "router.selector");
        assert_eq!(catalog.binding.arguments, "router.arguments");
        let schema = &catalog.tools[0].input_schema;
        assert!(schema["properties"].get("amount").is_some());
        assert!(schema["properties"].get("factor").is_none());
        assert_eq!(schema["required"], json!(["amount"]));
    }
    #[test]
    fn reusable_router_calls_keep_their_public_input_names() {
        let child = graph();
        let inputs = child
            .inputs
            .iter()
            .map(|(name, input)| {
                (
                    format!("wrapper.{name}"),
                    GraphInput {
                        node: "wrapper".into(),
                        port: name.clone(),
                        ..input.clone()
                    },
                )
            })
            .collect();
        let mut wrapper = node();
        wrapper.subgraph = Some(std::sync::Arc::new(child));
        let graph = WorkflowGraph {
            inputs,
            nodes: BTreeMap::from([("wrapper".into(), wrapper)]),
            ..Default::default()
        };
        let catalog = ToolCatalog::new(&graph, &BTreeMap::new()).unwrap();
        assert_eq!(catalog.binding.selector, "wrapper.router.selector");
        assert_eq!(catalog.binding.arguments, "wrapper.router.arguments");
        assert_eq!(catalog.tools[0].name, "multiply");
    }

    #[test]
    fn receiver_sensitivity_marks_model_call_arguments_before_publication() {
        let mut graph = graph();
        graph
            .nodes
            .get_mut(&DefinitionId::from("calculate"))
            .unwrap()
            .secret_inputs
            .insert("amount".into());
        let catalog = ToolCatalog::new(&graph, &BTreeMap::new()).unwrap();
        assert_eq!(
            catalog.tools[0].input_schema["properties"]["amount"]["writeOnly"],
            json!(true)
        );
        let mut call = crate::ToolCall {
            id: "one".into(),
            name: "multiply".into(),
            arguments: json!({"amount":1234}).into(),
        };
        call.mark_arguments(&catalog.tools);
        assert!(call.arguments["amount"].is_secret());
    }
    #[test]
    fn unbound_routes_and_ambiguous_entries_are_rejected_before_model_io() {
        let mut graph = graph();
        graph
            .connections
            .retain(|edge| edge.from_node.as_str() != "router");
        assert!(ToolCatalog::new(&graph, &BTreeMap::new()).is_err());
        let mut graph = super::tests::graph();
        for port in ["selector", "arguments"] {
            let mut input = graph.inputs[&format!("router.{port}")].clone();
            input.node = "another".into();
            graph.inputs.insert(format!("another.{port}"), input);
        }
        graph.nodes.insert(
            "another".into(),
            graph.nodes[&DefinitionId::from("router")].clone(),
        );
        assert!(matches!(
            ToolCatalog::new(&graph, &BTreeMap::new()),
            Err(ToolSchemaError::Entry)
        ));
    }
    #[test]
    fn wire_argument_budgets_and_duplicate_json_keys_remain_checked() {
        let wire = |arguments: Vec<String>| json!({"choices":[{"message":{"tool_calls":arguments.into_iter().map(|arguments|json!({"function":{"arguments":arguments}})).collect::<Vec<_>>()}}]});
        assert!(validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS])).is_ok());
        assert!(validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS + 1])).is_err());
        assert!(validate_wire_arguments(&wire(vec!["{\"key\":1,\"key\":2}".into()])).is_err());
    }
    #[test]
    fn schema_projection_supports_nested_decoded_types_and_rejects_incomplete_codecs() {
        use mf_runtime::SchemaField;
        let text = ValueSchema::Type(ValueType::String);
        let fields = BTreeMap::from([
            (
                "label".into(),
                SchemaField {
                    schema: text.clone(),
                    required: true,
                },
            ),
            (
                "note".into(),
                SchemaField {
                    schema: text.clone(),
                    required: false,
                },
            ),
        ]);
        let schemas = vec![
            ValueSchema::Type(ValueType::Any),
            ValueSchema::Type(ValueType::Null),
            ValueSchema::Type(ValueType::Boolean),
            text.clone(),
            ValueSchema::Type(ValueType::Number),
            ValueSchema::Type(ValueType::Int64),
            ValueSchema::Type(ValueType::Uint64),
            ValueSchema::Type(ValueType::Usize),
            ValueSchema::Type(ValueType::Float64),
            ValueSchema::Type(ValueType::Array),
            ValueSchema::Type(ValueType::Object),
            text.clone().list(),
            text.clone().map(),
            text.clone().nullable(),
            ValueSchema::Object(fields.clone()),
            ValueSchema::List(Box::new(ValueSchema::Object(fields.clone()))),
            ValueSchema::Map(Box::new(ValueSchema::Object(fields.clone()))),
            ValueSchema::Nullable(Box::new(ValueSchema::Object(fields.clone()))),
            ValueSchema::Enum(vec!["ready".into()]),
            ValueSchema::Tagged {
                tag: "kind".into(),
                variants: BTreeMap::from([("named".into(), fields)]),
            },
            text.described("A public label"),
        ];
        for schema in schemas {
            let mut graph = graph();
            let target = graph
                .nodes
                .get_mut(&DefinitionId::from("calculate"))
                .unwrap();
            target.ports.inputs[0].value_type = schema.value_type();
            target.input_schemas.insert("amount".into(), schema.clone());
            let catalog = ToolCatalog::new(&graph, &BTreeMap::new()).unwrap();
            assert_eq!(
                catalog.tools[0].input_schema["additionalProperties"],
                json!(false)
            );
            assert_eq!(catalog.tools[0].input_schema["required"], json!(["amount"]));
            let encoded = serde_json::to_string(&catalog.tools[0].input_schema).unwrap();
            match schema {
                ValueSchema::Object(_) => {
                    assert!(encoded.contains("label") && encoded.contains("note"))
                }
                ValueSchema::Tagged { .. } => {
                    assert!(encoded.contains("oneOf") && encoded.contains("named"))
                }
                ValueSchema::Described { .. } => assert!(encoded.contains("A public label")),
                ValueSchema::Type(ValueType::Float64) => assert!(encoded.contains("multipleOf")),
                _ => {}
            }
        }
        for schema in [
            ValueSchema::Type(ValueType::Float32),
            ValueSchema::Enum(vec![]),
            ValueSchema::Tagged {
                tag: "kind".into(),
                variants: BTreeMap::new(),
            },
            ValueSchema::opaque(ValueType::Object),
        ] {
            let mut graph = graph();
            let target = graph
                .nodes
                .get_mut(&DefinitionId::from("calculate"))
                .unwrap();
            target.ports.inputs[0].value_type = schema.value_type();
            target.input_schemas.insert("amount".into(), schema);
            assert!(matches!(
                ToolCatalog::new(&graph, &BTreeMap::new()),
                Err(ToolSchemaError::Unsupported { .. })
            ));
        }
    }
}
