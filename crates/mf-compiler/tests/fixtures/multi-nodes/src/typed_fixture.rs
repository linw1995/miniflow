use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, OutputDerivation,
    Outputs, PortSpec, TaskNode, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};

struct IntegerSource;

impl TaskNode for IntegerSource {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), json!(7).into())])).into())
    }
}

fn integer_source(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let _: serde_json::Map<String, Value> = deserialize_config(config)?;
    let node = IntegerSource;
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        },
        output_derivations: Vec::new(),
        resources: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.integer_source", factory: mf_runtime::NodeFactory::Plain(integer_source) }
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

impl TaskNode for TypedSource {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), self.0.value.clone().into())])).into())
    }
}
impl TypedSource {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::owned(
                "value",
                self.0.value_type.value_type(),
                true,
            )],
        }
    }
}

fn typed_source(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = TypedSource(deserialize_config(config)?);
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: Vec::new(),
        resources: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.typed_source", factory: mf_runtime::NodeFactory::Plain(typed_source) }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EchoConfig {
    #[serde(rename = "type")]
    value_type: TypeChoice,
}

struct TypedEcho(EchoConfig);

impl TaskNode for TypedEcho {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let value = inputs
            .get("input")
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "missing typed fixture input".into(),
            })?;
        Ok((Outputs::from([("value".into(), value.clone())])).into())
    }
}
impl TypedEcho {
    fn ports(&self) -> NodePorts {
        NodePorts {
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
        }
    }
}

fn typed_echo(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = TypedEcho(deserialize_config(config)?);
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: Vec::new(),
        resources: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.typed_echo", factory: mf_runtime::NodeFactory::Plain(typed_echo) }
}

struct DishonestForward;

impl TaskNode for DishonestForward {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), json!("wrong").into())])).into())
    }
}
impl DishonestForward {
    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "input")]
    }
}

fn dishonest_forward(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let _: serde_json::Map<String, Value> = deserialize_config(config)?;
    let node = DishonestForward;
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Any, true)],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        },
        output_derivations: node.output_derivations(),
        resources: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.dishonest_forward", factory: mf_runtime::NodeFactory::Plain(dishonest_forward) }
}
