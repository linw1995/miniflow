//! Callable interface metadata carried by a prepared body.
//!
//! A Flow owns its own properties: startup inputs, terminal and incremental
//! outputs, and stdin requirements are derived from the prepared body instead of
//! being re-declared in a tool record. Exposing them as plain JSON lets a caller
//! consume them with ordinary value nodes, while `builtin.constant` keeps the
//! native Flow itself opaque.
//!
//! The projection is deliberately flat and index-free so a CEL caller only needs
//! single-level field access plus map membership:
//!
//! ```json
//! {
//!   "callable": true,
//!   "startup": "calculate",
//!   "parameters": [{"name":"left","type":"double","schema":{...},"required":true,
//!                   "secret":false,"has_env":false,"has_default":false,"runtime_only":false}],
//!   "allowed": {"left": {"type": "number"}},
//!   "required": ["left"],
//!   "plain": false,
//!   "outputs": [{"name":"sum","type":"int","schema":{...},"required":true}],
//!   "streams": [],
//!   "stdin": {}
//! }
//! ```

use crate::{PortSpec, StdinRequirement, ValueType, WorkflowInputSchema};
use serde_json::{Map, Value, json};

/// JSON Schema for a native value type. Only JSON-compatible shapes are
/// projected; callable inputs stay unconstrained because they cannot cross a
/// JSON boundary.
pub fn value_schema(value_type: &ValueType) -> Value {
    match value_type {
        ValueType::Any | ValueType::Flow => json!({}),
        ValueType::Null => json!({"type": "null"}),
        ValueType::Boolean => json!({"type": "boolean"}),
        ValueType::Number | ValueType::Float32 | ValueType::Float64 => json!({"type": "number"}),
        ValueType::Int64 | ValueType::Uint64 | ValueType::Usize => json!({"type": "integer"}),
        ValueType::String => json!({"type": "string"}),
        ValueType::Array => json!({"type": "array"}),
        ValueType::Object => json!({"type": "object"}),
        ValueType::List(inner) => json!({"type": "array", "items": value_schema(inner)}),
        ValueType::Map(inner) => {
            json!({"type": "object", "additionalProperties": value_schema(inner)})
        }
        ValueType::Nullable(inner) => json!({"anyOf": [value_schema(inner), {"type": "null"}]}),
    }
}

fn declared_type(value_type: &ValueType) -> Value {
    serde_json::to_value(value_type).expect("native value types serialize as JSON descriptors")
}

fn ports(ports: &[PortSpec]) -> Vec<Value> {
    ports
        .iter()
        .map(|port| {
            json!({
                "name": port.name,
                "type": declared_type(&port.value_type),
                "schema": value_schema(&port.value_type),
                "required": port.required,
            })
        })
        .collect()
}

fn stdin(inputs: &WorkflowInputSchema) -> Value {
    let mut owners = Map::new();
    for (node, requirement) in &inputs.stdin {
        owners.insert(
            node.clone(),
            match requirement {
                StdinRequirement::Always => json!("always"),
                StdinRequirement::UnlessInput(input) => {
                    json!({"unless_input": crate::output_id(node, input)})
                }
            },
        );
    }
    Value::Object(owners)
}

/// Projects a prepared body's interface without executing it.
///
/// A body has a callable interface only when it starts from exactly one node and
/// exposes neither incremental outputs nor stdin. Such a body publishes no
/// `allowed` parameters: a dynamic caller cannot bind them unambiguously.
pub fn interface_properties(
    inputs: &WorkflowInputSchema,
    outputs: &[PortSpec],
    streams: &[PortSpec],
) -> Value {
    let startups: Vec<&String> = inputs.inputs.keys().collect();
    let callable = startups.len() == 1 && streams.is_empty() && inputs.stdin.is_empty();
    let mut parameters = Vec::new();
    let mut allowed = Map::new();
    let mut required = Vec::new();
    if let Some(startup) = callable.then(|| startups[0]) {
        for (name, input) in &inputs.inputs[startup] {
            let has_env = input.binding.env.is_some();
            let has_default = input.binding.default.is_some();
            parameters.push(json!({
                "name": name,
                "type": declared_type(&input.value_type),
                "schema": value_schema(&input.value_type),
                "required": input.required,
                "secret": input.binding.secret,
                "has_env": has_env,
                "has_default": has_default,
                // Callable values cannot cross a JSON or model boundary.
                "runtime_only": input.value_type.is_runtime_only(),
            }));
            // Secret inputs are resolved from their own environment or default and
            // cannot be provided by a dynamic caller.
            if input.binding.secret {
                continue;
            }
            allowed.insert(name.clone(), value_schema(&input.value_type));
            if input.required && !has_env && !has_default {
                required.push(Value::String(name.clone()));
            }
        }
    }
    let plain = callable
        && matches!(outputs, [output] if output.required && output.value_type == ValueType::String && output.name == "content");
    json!({
        "callable": callable,
        "startup": callable.then(|| startups[0].to_string()),
        "parameters": parameters,
        "allowed": Value::Object(allowed),
        "required": required,
        "plain": plain,
        "outputs": ports(outputs),
        "streams": ports(streams),
        "stdin": stdin(inputs),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputBinding, WorkflowInput};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn schema() -> WorkflowInputSchema {
        WorkflowInputSchema {
            inputs: BTreeMap::from([(
                "calculate".into(),
                BTreeMap::from([
                    (
                        "left".into(),
                        WorkflowInput {
                            value_type: ValueType::Float64,
                            required: true,
                            binding: InputBinding::default(),
                        },
                    ),
                    (
                        "token".into(),
                        WorkflowInput {
                            value_type: ValueType::String,
                            required: false,
                            binding: InputBinding {
                                secret: true,
                                env: Some("TOKEN".into()),
                                ..InputBinding::default()
                            },
                        },
                    ),
                    (
                        "history".into(),
                        WorkflowInput {
                            value_type: ValueType::List(Box::new(ValueType::Object)),
                            required: false,
                            binding: InputBinding {
                                default: Some(json!([])),
                                ..InputBinding::default()
                            },
                        },
                    ),
                ]),
            )]),
            stdin: BTreeMap::new(),
        }
    }

    #[test]
    fn projects_a_flat_callable_interface() {
        let properties = interface_properties(
            &schema(),
            &[PortSpec::new("content", ValueType::String, true)],
            &[],
        );
        assert_eq!(properties["callable"], json!(true));
        assert_eq!(properties["startup"], json!("calculate"));
        assert_eq!(
            properties["parameters"],
            json!([
                {
                    "name": "history", "type": {"list":"object"},
                    "schema": {"type":"array", "items": {"type":"object"}},
                    "required": false, "secret": false,
                    "has_env": false, "has_default": true, "runtime_only": false,
                },
                {
                    "name": "left", "type": "double", "schema": {"type":"number"},
                    "required": true, "secret": false,
                    "has_env": false, "has_default": false, "runtime_only": false,
                },
                {
                    "name": "token", "type": "string", "schema": {"type":"string"},
                    "required": false, "secret": true,
                    "has_env": true, "has_default": false, "runtime_only": false,
                },
            ])
        );
        assert_eq!(
            properties["allowed"],
            json!({
                "history": {"type":"array", "items": {"type":"object"}},
                "left": {"type":"number"},
            })
        );
        assert_eq!(properties["required"], json!(["left"]));
        assert_eq!(
            properties["plain"],
            json!(true),
            "a sole required string `content` output is plain text"
        );
        assert_eq!(
            properties["outputs"],
            json!([{
                "name":"content", "type":"string", "schema":{"type":"string"}, "required":true,
            }])
        );
        assert_eq!(properties["streams"], json!([]));
        assert_eq!(properties["stdin"], json!({}));
    }

    #[test]
    fn plain_text_shapes_and_open_boundaries_are_reported() {
        let schema = schema();
        assert_eq!(
            interface_properties(
                &schema,
                &[
                    PortSpec::new("content", ValueType::String, true),
                    PortSpec::new("extra", ValueType::String, true),
                ],
                &[],
            )["plain"],
            json!(false),
            "plain text requires exactly one terminal output"
        );
        assert_eq!(
            interface_properties(
                &schema,
                &[PortSpec::new("content", ValueType::String, false)],
                &[],
            )["plain"],
            json!(false),
            "an optional output is not plain text"
        );
        let single = WorkflowInputSchema {
            inputs: BTreeMap::from([(
                "read".into(),
                BTreeMap::from([
                    (
                        "path".into(),
                        WorkflowInput {
                            value_type: ValueType::String,
                            required: true,
                            binding: InputBinding::default(),
                        },
                    ),
                    (
                        "policy".into(),
                        WorkflowInput {
                            value_type: ValueType::Map(Box::new(ValueType::Flow)),
                            required: true,
                            binding: InputBinding::default(),
                        },
                    ),
                ]),
            )]),
            stdin: BTreeMap::from([("read".into(), StdinRequirement::Always)]),
        };
        let properties = interface_properties(&single, &[], &[]);
        assert_eq!(properties["callable"], json!(false));
        assert_eq!(properties["startup"], Value::Null);
        assert_eq!(properties["parameters"], json!([]));
        assert_eq!(properties["stdin"], json!({"read": "always"}));

        let runtime_only = WorkflowInputSchema {
            inputs: BTreeMap::from([(
                "call".into(),
                BTreeMap::from([(
                    "tools".into(),
                    WorkflowInput {
                        value_type: ValueType::Map(Box::new(ValueType::Flow)),
                        required: true,
                        binding: InputBinding::default(),
                    },
                )]),
            )]),
            stdin: BTreeMap::new(),
        };
        let properties = interface_properties(
            &runtime_only,
            &[PortSpec::new("content", ValueType::String, true)],
            &[],
        );
        assert_eq!(properties["callable"], json!(true));
        assert_eq!(properties["plain"], json!(true));
        assert_eq!(properties["parameters"][0]["runtime_only"], json!(true));
        assert_eq!(properties["parameters"][0]["type"], json!({"map":"flow"}));
        assert_eq!(
            properties["allowed"],
            json!({"tools": {"type":"object", "additionalProperties": {}}})
        );
    }

    #[test]
    fn conditional_stdin_reports_its_qualified_trigger() {
        let schema = WorkflowInputSchema {
            inputs: schema().inputs,
            stdin: BTreeMap::from([("read".into(), StdinRequirement::UnlessInput("skip".into()))]),
        };
        let properties = interface_properties(&schema, &[], &[]);
        assert_eq!(
            properties["stdin"],
            json!({"read": {"unless_input": "read.skip"}})
        );
    }

    #[test]
    fn streams_make_a_body_non_callable() {
        let properties = interface_properties(
            &schema(),
            &[PortSpec::new("sum", ValueType::Int64, true)],
            &[PortSpec::new("chunk", ValueType::String, false)],
        );
        assert_eq!(properties["callable"], json!(false));
        assert_eq!(properties["startup"], Value::Null);
        assert_eq!(
            properties["streams"],
            json!([{
                "name":"chunk", "type":"string", "schema":{"type":"string"}, "required":false,
            }])
        );
    }

    #[test]
    fn schema_projection_covers_every_json_shape() {
        for (value_type, expected) in [
            (ValueType::Any, json!({})),
            (ValueType::Null, json!({"type":"null"})),
            (ValueType::Boolean, json!({"type":"boolean"})),
            (ValueType::Number, json!({"type":"number"})),
            (ValueType::Int64, json!({"type":"integer"})),
            (ValueType::String, json!({"type":"string"})),
            (ValueType::Array, json!({"type":"array"})),
            (ValueType::Object, json!({"type":"object"})),
            (
                ValueType::Map(Box::new(ValueType::String)),
                json!({"type":"object", "additionalProperties": {"type":"string"}}),
            ),
            (
                ValueType::Nullable(Box::new(ValueType::Int64)),
                json!({"anyOf": [{"type":"integer"}, {"type":"null"}]}),
            ),
        ] {
            assert_eq!(value_schema(&value_type), expected, "{value_type:?}");
        }
    }
}
