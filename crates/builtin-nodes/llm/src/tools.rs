use mf_runtime::{GraphInput, ValueRef, ValueSchema, ValueType, WorkflowGraph};
use rig_core::completion::ToolDefinition;
use serde::Serialize;
use serde_json::{Value, json};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

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
    #[snafu(display("tool output `{name}` must connect to one argument-mode Iteration"))]
    Connection { name: String },
    #[snafu(
        display("connected tool metadata is unavailable outside workflow execution"),
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
pub enum ToolArgumentsError {
    #[snafu(display("tool arguments require the declared object shape"))]
    Shape,
    #[snafu(display("tool arguments exceed the size limit"))]
    Limit,
    #[snafu(display("tool argument `{parameter}`: {source}"))]
    Argument {
        parameter: String,
        source: mf_runtime::TypeMismatch,
    },
}
#[derive(Debug, Snafu)]
pub enum ToolEncodingError {
    #[snafu(display("tool data exceeds the size limit"))]
    Size,
    #[snafu(display("tool data could not be encoded"))]
    Json { source: serde_json::Error },
}

struct Parameter {
    name: String,
    input: String,
    descriptor: GraphInput,
}
impl Parameter {
    fn name(&self) -> &str {
        &self.name
    }
    fn required(&self) -> bool {
        self.descriptor.required
    }
    fn constraints(&self) -> &[ValueType] {
        std::slice::from_ref(&self.descriptor.value_type)
    }
    fn schema_constraints(&self) -> impl Iterator<Item = &ValueSchema> {
        self.descriptor.schema.iter()
    }
    fn is_secret(&self) -> bool {
        self.descriptor.secret
    }
}
struct Tool {
    definition: ToolDefinition,
    parameters: Vec<Parameter>,
}
pub struct ConnectedTools {
    tools: BTreeMap<String, Tool>,
}

impl ConnectedTools {
    pub fn new(
        graph: &WorkflowGraph,
        source: &str,
        outputs: &[String],
        descriptions: &BTreeMap<String, String>,
    ) -> Result<Self, ToolSchemaError> {
        ensure!(outputs.len() <= MAX_TOOL_CALLS, DeclarationsLimitSnafu);
        let mut tools = BTreeMap::new();
        for name in outputs {
            ensure!(crate::call::valid_name(name), AliasSnafu { name });
            let candidates: Vec<_> = graph
                .connections
                .iter()
                .filter(|edge| {
                    edge.from_node.as_str() == source
                        && edge.from_output == *name
                        && edge.to_input == "items"
                })
                .collect();
            ensure!(candidates.len() == 1, ConnectionSnafu { name });
            let target = &candidates[0].to_node;
            let target_node = graph.nodes.get(target).context(ConnectionSnafu { name })?;
            let body = target_node
                .subgraph
                .as_deref()
                .context(ConnectionSnafu { name })?;
            ensure!(
                body.nodes.contains_key(&mf_runtime::DefinitionId::from(
                    mf_runtime::ITERATION_INPUT_ID
                )) && matches!(
                    target_node.input_schemas.get("items"),
                    Some(ValueSchema::List(_))
                ),
                ConnectionSnafu { name }
            );
            let external: BTreeSet<_> = graph
                .connections
                .iter()
                .filter(|edge| edge.to_node == *target && edge.to_input != "items")
                .map(|edge| edge.to_input.as_str())
                .collect();
            let inputs: Vec<_> = body
                .inputs
                .iter()
                .filter(|(input, _)| !external.contains(input.as_str()))
                .collect();
            ensure!(inputs.len() <= 256, DeclarationsLimitSnafu);
            let mut counts = BTreeMap::new();
            for (input, _) in &inputs {
                let origin = body.input_origin(input).context(ConnectionSnafu { name })?;
                *counts.entry(origin.port.clone()).or_insert(0usize) += 1;
            }
            let mut parameters = Vec::new();
            for (input, descriptor) in inputs {
                let origin = body.input_origin(input).context(ConnectionSnafu { name })?;
                let alias = if counts[&origin.port] == 1 {
                    origin.port.clone()
                } else {
                    input.clone()
                };
                parameters.push(Parameter {
                    name: alias,
                    input: input.clone(),
                    descriptor: descriptor.clone(),
                });
            }
            parameters.sort_by(|a, b| a.name.cmp(&b.name));
            let definition = ToolDefinition {
                name: name.clone(),
                description: descriptions.get(name).cloned().unwrap_or_default(),
                parameters: parameters_schema(name, &parameters)?,
            };
            tools.insert(
                name.clone(),
                Tool {
                    definition,
                    parameters,
                },
            );
        }
        let definitions: Vec<_> = tools.values().map(|tool| &tool.definition).collect();
        ensure!(
            bounded_json(&definitions, MAX_TOOL_SCHEMA_BYTES).is_ok(),
            DeclarationsLimitSnafu
        );
        Ok(Self { tools })
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }
    pub fn batches(
        &self,
        calls: &[crate::ToolCall],
    ) -> Result<BTreeMap<String, ValueRef>, ToolArgumentsError> {
        let mut groups: BTreeMap<String, Vec<ValueRef>> = self
            .tools
            .keys()
            .map(|name| (name.clone(), Vec::new()))
            .collect();
        for call in calls {
            let Some(tool) = self.tools.get(&call.name) else {
                continue;
            };
            ensure!(
                bounded_json(&call.arguments, MAX_TOOL_ARGUMENT_BYTES).is_ok(),
                LimitSnafu
            );
            let object = call.arguments.as_object().context(ShapeSnafu)?;
            ensure!(
                object.keys().all(|name| tool
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == name.as_ref())),
                ShapeSnafu
            );
            let mut arguments = Vec::new();
            for parameter in &tool.parameters {
                let Some(value) = object.get(parameter.name.as_str()) else {
                    ensure!(!parameter.required(), ShapeSnafu);
                    continue;
                };
                parameter
                    .descriptor
                    .value_type
                    .validate_shared(value)
                    .context(ArgumentSnafu {
                        parameter: &parameter.name,
                    })?;
                if let Some(schema) = &parameter.descriptor.schema {
                    schema.validate_shared(value).context(ArgumentSnafu {
                        parameter: &parameter.name,
                    })?;
                }
                let value = if parameter.is_secret() {
                    value.clone().into_secret()
                } else {
                    value.clone()
                };
                arguments.push((parameter.input.clone().into(), value));
            }
            groups
                .get_mut(&call.name)
                .unwrap()
                .push(ValueRef::object(arguments));
        }
        Ok(groups
            .into_iter()
            .map(|(name, values)| (name, ValueRef::array(values)))
            .collect())
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
    use mf_runtime::{DefinitionId, EdgeDefinition, GraphNode, NodePorts};
    use std::sync::Arc;
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
        let body = WorkflowGraph {
            inputs: ["amount", "factor"]
                .into_iter()
                .map(|port| {
                    (
                        format!("calculate.{port}"),
                        GraphInput {
                            node: "calculate".into(),
                            port: port.into(),
                            value_type: ValueType::Int64,
                            required: true,
                            schema: None,
                            secret: false,
                        },
                    )
                })
                .collect(),
            nodes: BTreeMap::from([("calculate".into(), node())]),
            ..Default::default()
        };
        let mut batch = node();
        batch.subgraph = Some(Arc::new(body));
        Arc::make_mut(batch.subgraph.as_mut().unwrap())
            .nodes
            .insert(mf_runtime::ITERATION_INPUT_ID.into(), node());
        batch
            .input_schemas
            .insert("items".into(), ValueSchema::Object(BTreeMap::new()).list());
        WorkflowGraph {
            nodes: BTreeMap::from([("batch".into(), batch)]),
            connections: vec![
                EdgeDefinition {
                    exclusive: false,
                    from_node: "model".into(),
                    from_output: "multiply".into(),
                    to_node: "batch".into(),
                    to_input: "items".into(),
                },
                EdgeDefinition {
                    exclusive: false,
                    from_node: "config".into(),
                    from_output: "value".into(),
                    to_node: "batch".into(),
                    to_input: "calculate.factor".into(),
                },
            ],
            ..Default::default()
        }
    }
    fn catalog(graph: &WorkflowGraph) -> ConnectedTools {
        ConnectedTools::new(graph, "model", &["multiply".into()], &BTreeMap::new()).unwrap()
    }
    fn call(id: &str, args: Value) -> crate::ToolCall {
        crate::ToolCall {
            id: id.into(),
            name: "multiply".into(),
            arguments: args.into(),
        }
    }
    #[test]
    fn real_connections_hide_shared_inputs_and_generate_normal_workflow_argument_records() {
        let tools = catalog(&graph());
        let definitions = tools.definitions();
        let properties = &definitions[0].parameters["properties"];
        assert!(properties.get("amount").is_some() && properties.get("factor").is_none());
        let batches = tools
            .batches(&[
                call("first", json!({"amount":2})),
                call("second", json!({"amount":4})),
            ])
            .unwrap();
        assert_eq!(
            serde_json::to_value(&batches["multiply"]).unwrap(),
            json!([{"calculate.amount":2},{"calculate.amount":4}])
        );
    }
    #[test]
    fn optional_parameters_and_sensitivity_are_preserved_in_the_data_outputs() {
        let mut graph = graph();
        let body = Arc::make_mut(
            graph
                .nodes
                .get_mut(&DefinitionId::from("batch"))
                .unwrap()
                .subgraph
                .as_mut()
                .unwrap(),
        );
        let input = body.inputs.get_mut("calculate.amount").unwrap();
        input.required = false;
        input.secret = true;
        let tools = catalog(&graph);
        assert_eq!(
            tools.definitions()[0].parameters["properties"]["amount"]["writeOnly"],
            true
        );
        assert!(
            tools.definitions()[0].parameters["required"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let batches = tools
            .batches(&[
                call("first", json!({})),
                call("second", json!({"amount":2})),
            ])
            .unwrap();
        assert!(
            batches["multiply"].as_array().unwrap()[1]
                .as_object()
                .unwrap()["calculate.amount"]
                .is_secret()
        );
    }
    #[test]
    fn unknown_missing_and_invalid_arguments_do_not_publish_partial_batches() {
        let tools = catalog(&graph());
        for args in [
            json!({}),
            json!({"amount":null}),
            json!({"amount":"bad"}),
            json!({"amount":2,"factor":99}),
        ] {
            assert!(
                tools
                    .batches(&[call("valid", json!({"amount":1})), call("invalid", args)])
                    .is_err()
            );
        }
    }
    #[test]
    fn missing_or_ambiguous_normal_connections_fail_before_requesting_a_model() {
        let mut graph = graph();
        graph.connections.clear();
        assert!(
            ConnectedTools::new(&graph, "model", &["multiply".into()], &BTreeMap::new()).is_err()
        );
        graph.connections = vec![
            EdgeDefinition {
                exclusive: false,
                from_node: "model".into(),
                from_output: "multiply".into(),
                to_node: "batch".into(),
                to_input: "items".into()
            };
            2
        ];
        assert!(
            ConnectedTools::new(&graph, "model", &["multiply".into()], &BTreeMap::new()).is_err()
        );
    }
    #[test]
    fn wire_argument_budgets_and_duplicate_json_keys_remain_checked() {
        let wire = |arguments: Vec<String>| json!({"choices":[{"message":{"tool_calls":arguments.into_iter().map(|arguments|json!({"function":{"arguments":arguments}})).collect::<Vec<_>>()}}]});
        assert!(validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS])).is_ok());
        assert!(validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS + 1])).is_err());
        assert!(validate_wire_arguments(&wire(vec!["{\"key\":1,\"key\":2}".into()])).is_err());
    }
    #[test]
    fn reflected_scalar_and_container_contracts_validate_model_arguments() {
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
        let cases = vec![
            (
                ValueSchema::Type(ValueType::Any),
                json!({"free": [null, true]}),
                None,
            ),
            (
                ValueSchema::Type(ValueType::Null),
                Value::Null,
                Some(json!(false)),
            ),
            (
                ValueSchema::Type(ValueType::Boolean),
                json!(true),
                Some(json!(0)),
            ),
            (text.clone(), json!("text"), Some(json!(1))),
            (
                ValueSchema::Type(ValueType::Number),
                json!(1.25),
                Some(json!("1.25")),
            ),
            (
                ValueSchema::Type(ValueType::Int64),
                json!(i64::MIN),
                Some(json!(u64::MAX)),
            ),
            (
                ValueSchema::Type(ValueType::Uint64),
                json!(u64::MAX),
                Some(json!(-1)),
            ),
            (
                ValueSchema::Type(ValueType::Usize),
                json!(usize::MAX),
                Some(json!(-1)),
            ),
            (
                ValueSchema::Type(ValueType::Float64),
                json!(1.5),
                Some(json!(9_007_199_254_740_993_u64)),
            ),
            (
                ValueSchema::Type(ValueType::Array),
                json!([1, "two"]),
                Some(json!({})),
            ),
            (
                ValueSchema::Type(ValueType::Object),
                json!({"free": 1}),
                Some(json!([])),
            ),
            (text.clone().list(), json!(["one"]), Some(json!([1]))),
            (
                text.clone().map(),
                json!({"key": "value"}),
                Some(json!({"key": 1})),
            ),
            (text.clone().nullable(), Value::Null, Some(json!(false))),
            (
                ValueSchema::Object(fields.clone()),
                json!({"label": "ok"}),
                Some(json!({"note": "missing label"})),
            ),
            (
                ValueSchema::List(Box::new(ValueSchema::Object(fields.clone()))),
                json!([{"label": "ok"}]),
                Some(json!([{"label": 1}])),
            ),
            (
                ValueSchema::Map(Box::new(ValueSchema::Object(fields.clone()))),
                json!({"key": {"label": "ok"}}),
                Some(json!({"key": {}})),
            ),
            (
                ValueSchema::Nullable(Box::new(ValueSchema::Object(fields.clone()))),
                Value::Null,
                Some(json!({})),
            ),
            (
                ValueSchema::Enum(vec!["ready".into(), "done".into()]),
                json!("ready"),
                Some(json!("unknown")),
            ),
            (
                ValueSchema::Tagged {
                    tag: "kind".into(),
                    variants: BTreeMap::from([("named".into(), fields)]),
                },
                json!({"kind": "named", "label": "ok"}),
                Some(json!({"kind": "unknown"})),
            ),
            (
                text.described("A public label"),
                json!("ok"),
                Some(json!(false)),
            ),
        ];
        for (schema, valid, invalid) in cases {
            let mut graph = graph();
            let body = Arc::make_mut(
                graph
                    .nodes
                    .get_mut(&DefinitionId::from("batch"))
                    .unwrap()
                    .subgraph
                    .as_mut()
                    .unwrap(),
            );
            let input = body.inputs.get_mut("calculate.amount").unwrap();
            input.value_type = schema.value_type();
            input.schema = Some(schema.clone());
            let tools = catalog(&graph);
            let definition = &tools.definitions()[0];
            assert_eq!(definition.parameters["additionalProperties"], false);
            assert_eq!(definition.parameters["required"], json!(["amount"]));
            assert!(
                tools
                    .batches(&[call("valid", json!({"amount": valid}))])
                    .is_ok(),
                "{schema:?}"
            );
            if let Some(invalid) = invalid {
                assert!(
                    tools
                        .batches(&[call("invalid", json!({"amount": invalid}))])
                        .is_err(),
                    "{schema:?}"
                );
            }
            let encoded = serde_json::to_string(&definition.parameters).unwrap();
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
    }

    #[test]
    fn incomplete_and_unrepresentable_contracts_report_the_tool_parameter() {
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
            let body = Arc::make_mut(
                graph
                    .nodes
                    .get_mut(&DefinitionId::from("batch"))
                    .unwrap()
                    .subgraph
                    .as_mut()
                    .unwrap(),
            );
            let input = body.inputs.get_mut("calculate.amount").unwrap();
            input.value_type = schema.value_type();
            input.schema = Some(schema);
            let error =
                ConnectedTools::new(&graph, "model", &["multiply".into()], &BTreeMap::new())
                    .err()
                    .unwrap();
            assert!(
                matches!(error, ToolSchemaError::Unsupported { ref tool, ref parameter, .. } if tool == "multiply" && parameter == "amount")
            );
        }
    }

    #[test]
    fn connected_batches_bound_payloads_and_leave_external_tools_to_the_host() {
        let mut graph = graph();
        let body = Arc::make_mut(
            graph
                .nodes
                .get_mut(&DefinitionId::from("batch"))
                .unwrap()
                .subgraph
                .as_mut()
                .unwrap(),
        );
        body.inputs.get_mut("calculate.amount").unwrap().value_type = ValueType::Any;
        let tools = catalog(&graph);
        let oversized = call(
            "large",
            json!({"amount": "x".repeat(MAX_TOOL_ARGUMENT_BYTES)}),
        );
        assert!(matches!(
            tools.batches(&[oversized]),
            Err(ToolArgumentsError::Limit)
        ));
        assert!(matches!(
            tools.batches(&[call("scalar", json!(1))]),
            Err(ToolArgumentsError::Shape)
        ));
        let mut external = call("host", json!({}));
        external.name = "external".into();
        let batches = tools.batches(&[external]).unwrap();
        assert!(batches["multiply"].as_array().unwrap().is_empty());
        for outputs in [
            vec!["invalid name".into()],
            vec!["multiply".into(); MAX_TOOL_CALLS + 1],
        ] {
            assert!(ConnectedTools::new(&graph, "model", &outputs, &BTreeMap::new()).is_err());
        }
        let descriptions = BTreeMap::from([("multiply".into(), "x".repeat(MAX_TOOL_SCHEMA_BYTES))]);
        assert!(matches!(
            ConnectedTools::new(&graph, "model", &["multiply".into()], &descriptions),
            Err(ToolSchemaError::DeclarationsLimit)
        ));
    }
}
