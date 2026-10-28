use mf_runtime::{
    CallArguments, CallBindingId, CallInterface, CallParameter, ExecutionContext, ValueRef,
    ValueSchema, ValueType,
};
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
    #[snafu(display("LLM tools require executable call bindings, not discovery-only metadata"))]
    Unavailable,
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
}

#[derive(Debug, Snafu)]
pub enum ToolArgumentsError {
    #[snafu(display("unknown tool name"))]
    UnknownTool,
    #[snafu(display("tool arguments require the declared object shape"))]
    Shape,
    #[snafu(display("tool arguments exceed the size limit"))]
    Limit,
    #[snafu(transparent)]
    Contract {
        source: mf_runtime::CallContractError,
    },
}

#[derive(Debug, Snafu)]
pub enum ToolEncodingError {
    #[snafu(display("tool data exceeds the size limit"))]
    Size,
    #[snafu(display("tool data could not be encoded"))]
    Json { source: serde_json::Error },
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

struct Tool {
    interface: CallInterface,
    namespaced: bool,
    definition: ToolDefinition,
}
pub struct ToolCatalog {
    tools: BTreeMap<String, Tool>,
}
impl ToolCatalog {
    pub fn new(context: &ExecutionContext) -> Result<Self, ToolSchemaError> {
        let interfaces = context.call_targets();
        ensure!(
            interfaces.is_empty() || context.can_invoke_calls(),
            UnavailableSnafu
        );
        ensure!(interfaces.len() <= MAX_TOOL_CALLS, DeclarationsLimitSnafu);
        let mut tools = BTreeMap::new();
        let mut schema_bytes = 2usize;
        for interface in interfaces {
            let name = interface.name();
            ensure!(
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
                AliasSnafu { name }
            );
            ensure!(
                interface.outputs().len() + interface.defaults().len() <= 256,
                DeclarationsLimitSnafu
            );
            let output_names: BTreeSet<_> = interface
                .outputs()
                .iter()
                .map(CallParameter::name)
                .collect();
            let namespaced = interface
                .defaults()
                .iter()
                .any(|parameter| output_names.contains(parameter.name()));
            let parameters = if namespaced {
                let outputs = parameters(name, interface.outputs())?;
                let defaults = parameters(name, interface.defaults())?;
                let required: Vec<_> = [
                    ("outputs", interface.outputs()),
                    ("defaults", interface.defaults()),
                ]
                .into_iter()
                .filter(|(_, fields)| fields.iter().any(CallParameter::required))
                .map(|(name, _)| name)
                .collect();
                json!({"type":"object","properties":{"outputs":outputs,"defaults":defaults},"required":required,"additionalProperties":false})
            } else {
                let all: Vec<_> = interface
                    .outputs()
                    .iter()
                    .chain(interface.defaults())
                    .cloned()
                    .collect();
                parameters(name, &all)?
            };
            let definition = ToolDefinition {
                name: name.into(),
                description: interface
                    .description()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(name)
                    .into(),
                parameters,
            };
            let encoded = bounded_json(
                &definition,
                MAX_TOOL_SCHEMA_BYTES.saturating_sub(schema_bytes),
            )
            .ok()
            .context(DeclarationsLimitSnafu)?;
            schema_bytes += encoded.len() + 1;
            tools.insert(
                name.into(),
                Tool {
                    interface: interface.clone(),
                    namespaced,
                    definition,
                },
            );
        }
        let catalog = Self { tools };
        ensure!(
            bounded_json(&catalog.definitions(), MAX_TOOL_SCHEMA_BYTES).is_ok(),
            DeclarationsLimitSnafu
        );
        Ok(catalog)
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }
    pub fn arguments(
        &self,
        name: &str,
        value: &Value,
    ) -> Result<(CallBindingId, CallArguments), ToolArgumentsError> {
        ensure!(
            bounded_json(value, MAX_TOOL_ARGUMENT_BYTES).is_ok(),
            LimitSnafu
        );
        let tool = self.tools.get(name).context(UnknownToolSnafu)?;
        let object = value.as_object().context(ShapeSnafu)?;
        let mut arguments = CallArguments::default();
        let convert = |name: &str, value: &Value, parameters: &[CallParameter]| {
            let value = ValueRef::from(value.clone());
            if parameters
                .iter()
                .any(|parameter| parameter.name() == name && parameter.is_secret())
            {
                value.into_secret()
            } else {
                value
            }
        };
        if tool.namespaced {
            ensure!(
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "outputs" | "defaults")),
                ShapeSnafu
            );
            for (namespace, parameters, values) in [
                ("outputs", tool.interface.outputs(), &mut arguments.outputs),
                (
                    "defaults",
                    tool.interface.defaults(),
                    &mut arguments.defaults,
                ),
            ] {
                if let Some(object) = object.get(namespace) {
                    for (name, value) in object.as_object().context(ShapeSnafu)? {
                        values.insert(name.clone(), convert(name, value, parameters));
                    }
                }
            }
        } else {
            for (name, value) in object {
                if tool
                    .interface
                    .outputs()
                    .iter()
                    .any(|parameter| parameter.name() == name)
                {
                    arguments
                        .outputs
                        .insert(name.clone(), convert(name, value, tool.interface.outputs()));
                } else {
                    arguments.defaults.insert(
                        name.clone(),
                        convert(name, value, tool.interface.defaults()),
                    );
                }
            }
        }
        tool.interface.validate_decoded_arguments(&arguments)?;
        Ok((tool.interface.id(), arguments))
    }

    pub fn history_arguments(&self, name: &str, value: &Value) -> ValueRef {
        let Some(tool) = self.tools.get(name) else {
            return value.clone().into();
        };
        let Some(object) = value.as_object() else {
            return value.clone().into();
        };
        let mark = |object: &serde_json::Map<String, Value>, parameters: &[CallParameter]| {
            ValueRef::object(object.iter().map(|(name, value)| {
                let value = ValueRef::from(value.clone());
                let value = if parameters
                    .iter()
                    .any(|parameter| parameter.name() == name && parameter.is_secret())
                {
                    value.into_secret()
                } else {
                    value
                };
                (name.as_str().into(), value)
            }))
        };
        if tool.namespaced {
            ValueRef::object(object.iter().map(|(name, value)| {
                let parameters = match name.as_str() {
                    "outputs" => tool.interface.outputs(),
                    "defaults" => tool.interface.defaults(),
                    _ => &[],
                };
                let value = if let Some(object) = value.as_object() {
                    mark(object, parameters)
                } else if parameters.iter().any(CallParameter::is_secret) {
                    ValueRef::from(value.clone()).into_secret()
                } else {
                    value.clone().into()
                };
                (name.as_str().into(), value)
            }))
        } else {
            let parameters: Vec<_> = tool
                .interface
                .outputs()
                .iter()
                .chain(tool.interface.defaults())
                .cloned()
                .collect();
            mark(object, &parameters)
        }
    }
}

fn parameters(tool: &str, parameters: &[CallParameter]) -> Result<Value, ToolSchemaError> {
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
        let schema = match constraints.len() {
            0 => json!({}),
            1 => constraints.pop().unwrap(),
            _ => json!({"allOf":constraints}),
        };
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

pub fn tool_result(outputs: mf_runtime::Outputs) -> Result<String, ToolEncodingError> {
    let value = ValueRef::object(
        outputs
            .into_iter()
            .map(|(name, value)| (name.into(), value)),
    )
    .redacted();
    bounded_json(&value, MAX_TOOL_RESULT_BYTES)
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
    use mf_runtime::{SchemaField, SchemaOpacity};

    #[test]
    fn schemas_preserve_closed_shapes_presence_null_and_descriptions() {
        let fields = BTreeMap::from([
            (
                "required".into(),
                SchemaField {
                    schema: ValueSchema::Type(ValueType::String),
                    required: true,
                },
            ),
            (
                "optional".into(),
                SchemaField {
                    schema: ValueSchema::Nullable(Box::new(ValueSchema::Enum(vec![
                        "yes".into(),
                        "no".into(),
                    ]))),
                    required: false,
                },
            ),
        ]);
        let schema = ValueSchema::Described {
            description: "A structured argument".into(),
            schema: Box::new(ValueSchema::Tagged {
                tag: "kind".into(),
                variants: BTreeMap::from([("record".into(), fields)]),
            }),
        };
        let lowered = lower_schema(&schema, "tool", "arg").unwrap();
        assert_eq!(lowered["description"], "A structured argument");
        let record = &lowered["oneOf"][0];
        assert_eq!(record["additionalProperties"], false);
        assert_eq!(record["required"], json!(["required", "kind"]));
        assert_eq!(
            record["properties"]["kind"],
            json!({"const":"record","type":"string"})
        );
        assert_eq!(
            record["properties"]["optional"]["anyOf"],
            json!([{"type":"null"},{"type":"string","enum":["yes","no"]}])
        );
    }

    #[test]
    fn unknown_and_inexact_schemas_fail_closed_and_encoding_never_truncates() {
        for reason in [
            SchemaOpacity::Codec,
            SchemaOpacity::Recursive,
            SchemaOpacity::Limit,
        ] {
            let schema = ValueSchema::Opaque {
                value_type: ValueType::Object,
                reason,
            };
            assert!(lower_schema(&schema, "tool", "arg").is_err());
        }
        assert!(lower_type(&ValueType::Float32, "tool", "arg").is_err());
        assert_eq!(bounded_json(&"abcd", 6).unwrap(), "\"abcd\"");
        assert!(matches!(
            bounded_json(&"abcd", 5),
            Err(ToolEncodingError::Size)
        ));
        let result = tool_result(mf_runtime::Outputs::from([(
            "data".into(),
            ValueRef::object([(
                "token".into(),
                ValueRef::from("private-value").into_secret(),
            )]),
        )]))
        .unwrap();
        assert!(!result.contains("private-value"));
        assert!(result.contains("[REDACTED]"));
    }
    #[test]
    fn primitive_and_collection_schemas_keep_numeric_bounds_and_nested_constraints() {
        for (kind, expected) in [
            (ValueType::Null, "null"),
            (ValueType::Boolean, "boolean"),
            (ValueType::Number, "number"),
            (ValueType::String, "string"),
            (ValueType::Array, "array"),
            (ValueType::Object, "object"),
            (ValueType::Int64, "integer"),
            (ValueType::Uint64, "integer"),
            (ValueType::Usize, "integer"),
        ] {
            assert_eq!(lower_type(&kind, "tool", "arg").unwrap()["type"], expected);
        }
        assert_eq!(
            lower_type(&ValueType::Int64, "tool", "arg").unwrap()["minimum"],
            json!(i64::MIN)
        );
        assert_eq!(
            lower_type(&ValueType::Uint64, "tool", "arg").unwrap()["maximum"],
            json!(u64::MAX)
        );
        let exact = lower_type(&ValueType::Float64, "tool", "arg").unwrap();
        assert_eq!(exact["type"], "number");
        assert_eq!(exact["anyOf"][1]["maximum"], json!(1_u64 << 53));
        assert_eq!(exact["anyOf"][4]["multipleOf"], 2);
        let nested = ValueType::Nullable(Box::new(ValueType::List(Box::new(ValueType::Map(
            Box::new(ValueType::Boolean),
        )))));
        assert_eq!(
            lower_type(&nested, "tool", "arg").unwrap(),
            json!({"anyOf":[{"type":"null"},{"type":"array","items":{"type":"object","additionalProperties":{"type":"boolean"}}}]})
        );
        assert_eq!(
            lower_type(&ValueType::Any, "tool", "arg").unwrap(),
            json!({})
        );
        let schema = ValueSchema::List(Box::new(ValueSchema::Map(Box::new(ValueSchema::Enum(
            vec!["one".into()],
        )))));
        assert_eq!(
            lower_schema(&schema, "tool", "arg").unwrap()["items"]["additionalProperties"]["enum"],
            json!(["one"])
        );
        assert!(lower_schema(&ValueSchema::Enum(vec![]), "tool", "arg").is_err());
        assert!(
            lower_schema(
                &ValueSchema::Tagged {
                    tag: "kind".into(),
                    variants: BTreeMap::new()
                },
                "tool",
                "arg"
            )
            .is_err()
        );
    }

    #[test]
    fn public_secrets_remain_marked_in_both_argument_namespaces_and_history() {
        use mf_runtime::{
            CallInputSource, NodeId, PortSpec, ResolvedCallBinding, ResolvedCallInput,
        };
        for namespaced in [false, true] {
            let mut inputs = vec![ResolvedCallInput {
                port: PortSpec::new("token", ValueType::String, true),
                source: CallInputSource::Output(PortSpec::new("key", ValueType::String, true)),
                secret: true,
                schema: None,
            }];
            if namespaced {
                inputs.push(ResolvedCallInput {
                    port: PortSpec::new("key", ValueType::String, true),
                    source: CallInputSource::Default,
                    secret: true,
                    schema: None,
                });
            }
            let interface = ResolvedCallBinding {
                id: CallBindingId::new(0),
                name: "search".into(),
                description: None,
                caller: NodeId::new(0),
                target: NodeId::new(1),
                inputs,
                dependencies: vec![],
                success_dependencies: vec![],
                prerequisites: vec![],
            }
            .interface();
            let catalog = ToolCatalog {
                tools: BTreeMap::from([(
                    "search".into(),
                    Tool {
                        interface,
                        namespaced,
                        definition: ToolDefinition {
                            name: "search".into(),
                            description: String::new(),
                            parameters: json!({}),
                        },
                    },
                )]),
            };
            let value = if namespaced {
                json!({"outputs":{"key":"first-secret"},"defaults":{"key":"second-secret"}})
            } else {
                json!({"key":"first-secret"})
            };
            let (_, arguments) = catalog.arguments("search", &value).unwrap();
            assert!(arguments.outputs["key"].is_secret());
            if namespaced {
                assert!(arguments.defaults["key"].is_secret());
            }
            let history =
                serde_json::to_value(catalog.history_arguments("search", &value).redacted())
                    .unwrap();
            assert!(!history.to_string().contains("first-secret"));
            assert!(!history.to_string().contains("second-secret"));
            let bad = if namespaced {
                json!({"outputs":"invalid-secret"})
            } else {
                json!({"key":false})
            };
            assert!(catalog.arguments("search", &bad).is_err());
            assert!(
                !serde_json::to_string(&catalog.history_arguments("search", &bad).redacted())
                    .unwrap()
                    .contains("invalid-secret")
            );
            assert!(catalog.arguments("search", &json!(null)).is_err());
            assert_eq!(
                catalog.history_arguments("search", &json!(null)),
                ValueRef::null()
            );
        }
    }
    #[test]
    fn wire_arguments_enforce_individual_batch_and_count_limits_before_sdk_decoding() {
        let wire = |arguments: Vec<String>| json!({"choices":[{"message":{"tool_calls":arguments.into_iter().map(|arguments| json!({"function":{"arguments":arguments}})).collect::<Vec<_>>()}}]});
        assert!(validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS])).is_ok());
        let excessive_count =
            validate_wire_arguments(&wire(vec!["{}".into(); MAX_TOOL_CALLS + 1])).unwrap_err();
        assert!(excessive_count.to_string().contains("batch count"));
        let oversized = serde_json::to_string(&"x".repeat(MAX_TOOL_ARGUMENT_BYTES)).unwrap();
        assert!(
            validate_wire_arguments(&wire(vec![oversized]))
                .unwrap_err()
                .to_string()
                .contains("argument bytes")
        );
        let bounded = serde_json::to_string(&"x".repeat(MAX_TOOL_ARGUMENT_BYTES - 2)).unwrap();
        assert!(validate_wire_arguments(&wire(vec![bounded.clone(); 4])).is_ok());
        assert!(
            validate_wire_arguments(&wire(vec![bounded; 5]))
                .unwrap_err()
                .to_string()
                .contains("batch argument bytes")
        );
        assert!(validate_wire_arguments(&wire(vec!["{\"key\":1,\"key\":2}".into()])).is_err());
    }
}
