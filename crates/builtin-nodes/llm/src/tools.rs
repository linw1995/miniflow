use mf_runtime::{GraphInput, Inputs, SubgraphInterface, ValueSchema, ValueType};
use serde::Serialize;
use serde_json::{Value, json};
use snafu::{ResultExt, Snafu, ensure};
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
    #[snafu(display("tool bindings name an unknown tool `{tool}`"))]
    UnknownBinding { tool: String },
    #[snafu(display("invalid bindings for tool `{tool}`: {source}"))]
    Binding {
        tool: String,
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("could not encode tool declarations: {source}"))]
    Encode { source: mf_runtime::TypeMismatch },
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

struct Parameter {
    name: String,
    descriptors: Vec<GraphInput>,
}
impl Parameter {
    fn name(&self) -> &str {
        &self.name
    }
    fn required(&self) -> bool {
        self.descriptors.iter().any(|input| input.required)
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
}

impl ToolCatalog {
    pub fn new(
        interfaces: &BTreeMap<String, SubgraphInterface>,
        descriptions: &BTreeMap<String, String>,
        bindings: &BTreeMap<String, Inputs>,
    ) -> Result<Self, ToolSchemaError> {
        ensure!(interfaces.len() <= MAX_TOOL_CALLS, DeclarationsLimitSnafu);
        for tool in bindings.keys() {
            ensure!(interfaces.contains_key(tool), UnknownBindingSnafu { tool });
        }
        let mut tools = Vec::new();
        for (name, interface) in interfaces {
            ensure!(crate::call::valid_name(name), AliasSnafu { name });
            if let Some(bound) = bindings.get(name) {
                interface
                    .validate_supplied(bound)
                    .context(BindingSnafu { tool: name })?;
            }
            let parameters = interface
                .inputs
                .iter()
                .filter(|(field, _)| {
                    bindings
                        .get(name)
                        .is_none_or(|bound| !bound.contains_key(*field))
                })
                .map(|(field, input)| Parameter {
                    name: field.clone(),
                    descriptors: vec![input.clone()],
                })
                .collect::<Vec<_>>();
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
        Ok(Self { tools })
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
    fn interfaces() -> BTreeMap<String, SubgraphInterface> {
        BTreeMap::from([(
            "multiply".into(),
            SubgraphInterface {
                inputs: BTreeMap::from([(
                    "amount".into(),
                    GraphInput {
                        node: "calculate".into(),
                        port: "amount".into(),
                        value_type: ValueType::Int64,
                        required: true,
                        schema: Some(ValueSchema::Type(ValueType::Int64)),
                        secret: false,
                    },
                )]),
                outputs: vec![],
                stream_outputs: vec![],
            },
        )])
    }

    #[test]
    fn catalog_exposes_only_unbound_public_inputs() {
        let mut interfaces = interfaces();
        let mut factor = interfaces["multiply"].inputs["amount"].clone();
        factor.port = "factor".into();
        interfaces
            .get_mut("multiply")
            .unwrap()
            .inputs
            .insert("factor".into(), factor);
        let bound = BTreeMap::from([(
            "multiply".into(),
            Inputs::from([("factor".into(), 2.into())]),
        )]);
        let catalog = ToolCatalog::new(&interfaces, &BTreeMap::new(), &bound).unwrap();
        assert_eq!(catalog.tools[0].input_schema["required"], json!(["amount"]));
        assert!(
            catalog.tools[0].input_schema["properties"]
                .get("factor")
                .is_none()
        );
        assert!(
            ToolCatalog::new(
                &interfaces,
                &BTreeMap::new(),
                &BTreeMap::from([("unknown".into(), Inputs::new())])
            )
            .is_err()
        );
        assert!(
            ToolCatalog::new(
                &interfaces,
                &BTreeMap::new(),
                &BTreeMap::from([(
                    "multiply".into(),
                    Inputs::from([("factor".into(), "bad".into())])
                )])
            )
            .is_err()
        );
    }

    #[test]
    fn optional_fields_and_sensitivity_come_from_the_subgraph_interface() {
        let mut interfaces = interfaces();
        let amount = interfaces
            .get_mut("multiply")
            .unwrap()
            .inputs
            .get_mut("amount")
            .unwrap();
        amount.required = false;
        amount.secret = true;
        let catalog = ToolCatalog::new(&interfaces, &BTreeMap::new(), &BTreeMap::new()).unwrap();
        assert_eq!(catalog.tools[0].input_schema["required"], json!([]));
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
    fn declarations_reject_invalid_names_and_enforce_size_and_count_budgets() {
        let interface = interfaces().remove("multiply").unwrap();
        for name in ["", "invalid.name"] {
            assert!(
                ToolCatalog::new(
                    &BTreeMap::from([(name.into(), interface.clone())]),
                    &BTreeMap::new(),
                    &BTreeMap::new()
                )
                .is_err()
            );
        }
        let many = (0..=MAX_TOOL_CALLS)
            .map(|i| (format!("tool_{i}"), interface.clone()))
            .collect();
        assert!(ToolCatalog::new(&many, &BTreeMap::new(), &BTreeMap::new()).is_err());
        assert!(
            ToolCatalog::new(
                &interfaces(),
                &BTreeMap::from([("multiply".into(), "x".repeat(MAX_TOOL_SCHEMA_BYTES))]),
                &BTreeMap::new()
            )
            .is_err()
        );
        assert!(bounded_json(&"text", 2).is_err());
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
            let mut interfaces = interfaces();
            let target = interfaces
                .get_mut("multiply")
                .unwrap()
                .inputs
                .get_mut("amount")
                .unwrap();
            target.value_type = schema.value_type();
            target.schema = Some(schema.clone());
            let catalog =
                ToolCatalog::new(&interfaces, &BTreeMap::new(), &BTreeMap::new()).unwrap();
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
            let mut interfaces = interfaces();
            let target = interfaces
                .get_mut("multiply")
                .unwrap()
                .inputs
                .get_mut("amount")
                .unwrap();
            target.value_type = schema.value_type();
            target.schema = Some(schema);
            assert!(matches!(
                ToolCatalog::new(&interfaces, &BTreeMap::new(), &BTreeMap::new()),
                Err(ToolSchemaError::Unsupported { .. })
            ));
        }
    }
}
