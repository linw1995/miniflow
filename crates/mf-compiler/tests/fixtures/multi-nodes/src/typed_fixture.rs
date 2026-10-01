use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration,
    OutputDerivation, Outputs, PortSpec, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};

struct IntegerSource;

impl Node for IntegerSource {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".into(), json!(7).into())]))
    }
}

fn integer_source(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: serde_json::Map<String, Value> = deserialize_config(config)?;
    Ok(Box::new(IntegerSource))
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.integer_source",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Int64, true)],
        factory: integer_source,
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TypeChoice {
    Any,
    Number,
    Int64,
    Float64,
    Boolean,
    String,
    Array,
    Object,
    ListNumber,
    ListInt64,
    ListString,
    ListMapInt64,
    MapInt64,
}

impl TypeChoice {
    fn value_type(self) -> ValueType {
        match self {
            Self::Any => ValueType::Any,
            Self::Number => ValueType::Number,
            Self::Int64 => ValueType::Int64,
            Self::Float64 => ValueType::Float64,
            Self::Boolean => ValueType::Boolean,
            Self::String => ValueType::String,
            Self::Array => ValueType::Array,
            Self::Object => ValueType::Object,
            Self::ListNumber => ValueType::List(Box::new(ValueType::Number)),
            Self::ListInt64 => ValueType::List(Box::new(ValueType::Int64)),
            Self::ListString => ValueType::List(Box::new(ValueType::String)),
            Self::ListMapInt64 => {
                ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
            }
            Self::MapInt64 => ValueType::Map(Box::new(ValueType::Int64)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceConfig {
    #[serde(rename = "type")]
    value_type: TypeChoice,
    value: Value,
}

struct TypedSource(SourceConfig);

impl Node for TypedSource {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([(
            "value".into(),
            self.0.value.clone().into(),
        )]))
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::owned(
                "value",
                self.0.value_type.value_type(),
                true,
            )],
        })
    }
}

fn typed_source(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(TypedSource(deserialize_config(config)?)))
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.typed_source",
        inputs: &[],
        outputs: &[],
        factory: typed_source,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EchoConfig {
    #[serde(rename = "type")]
    value_type: TypeChoice,
}

struct TypedEcho(EchoConfig);

impl Node for TypedEcho {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        let value = inputs
            .get("input")
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "missing typed fixture input".into(),
            })?;
        Ok(Outputs::from([("value".into(), value.clone())]))
    }

    fn ports(&self) -> Option<NodePorts> {
        Some(NodePorts {
            inputs: vec![PortSpec::owned(
                "input",
                self.0.value_type.value_type(),
                true,
            )],
            outputs: vec![PortSpec::owned(
                "value",
                self.0.value_type.value_type(),
                true,
            )],
        })
    }
}

fn typed_echo(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(TypedEcho(deserialize_config(config)?)))
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.typed_echo",
        inputs: &[],
        outputs: &[],
        factory: typed_echo,
    }
}

struct DishonestForward;

impl Node for DishonestForward {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".into(), json!("wrong").into())]))
    }

    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "input")]
    }
}

fn dishonest_forward(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: serde_json::Map<String, Value> = deserialize_config(config)?;
    Ok(Box::new(DishonestForward))
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.dishonest_forward",
        inputs: &[PortSpec::new("input", ValueType::Any, true)],
        outputs: &[PortSpec::new("value", ValueType::Any, true)],
        factory: dishonest_forward,
    }
}
