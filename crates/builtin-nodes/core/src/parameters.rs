use mf_runtime::{
    ExecutionContext, InputBinding, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, PortSpec,
    PreparedNode, TaskNode, ValueType, deserialize_config,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::{collections::BTreeMap, error::Error};

pub const KIND: &str = "builtin.parameters";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    parameters: BTreeMap<String, Parameter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameter {
    #[serde(rename = "type")]
    value_type: ValueType,
    #[serde(default)]
    env: Option<String>,
    #[serde(default, deserialize_with = "present_default")]
    default: Option<Value>,
    #[serde(default = "default_secret")]
    secret: bool,
}

fn default_secret() -> bool {
    true
}

fn present_default<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Snafu)]
enum ParametersBuildError {
    #[snafu(display("parameters requires at least one parameter"))]
    EmptyParameters,
    #[snafu(display("parameter names must be nonempty"))]
    EmptyName,
    #[snafu(transparent)]
    Binding {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(display("parameter {name:?} has an invalid type: {source}"))]
    TypeDepth {
        name: String,
        source: mf_runtime::TypeDepthError,
    },
}

impl From<ParametersBuildError> for NodeBuildError {
    fn from(source: ParametersBuildError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

struct ParametersNode {
    parameters: BTreeMap<String, ValueType>,
}

impl NodePortContract for ParametersNode {
    fn ports(&self) -> NodePorts {
        let ports: Vec<_> = self
            .parameters
            .iter()
            .map(|(name, value_type)| PortSpec::owned(name, value_type.clone(), true))
            .collect();
        NodePorts {
            inputs: ports.clone(),
            outputs: ports,
        }
    }
}

impl TaskNode for ParametersNode {
    fn execute(
        &self,
        inputs: Inputs,
        _context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(inputs.into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    ensure!(!config.parameters.is_empty(), EmptyParametersSnafu);
    let mut bindings = BTreeMap::new();
    let mut parameters = BTreeMap::new();
    for (name, parameter) in config.parameters {
        ensure!(!name.is_empty(), EmptyNameSnafu);
        parameter
            .value_type
            .check_depth()
            .context(TypeDepthSnafu { name: &name })?;
        let binding = InputBinding {
            env: parameter.env,
            default: parameter.default,
            secret: parameter.secret,
        };
        binding
            .validate(&name, &parameter.value_type)
            .map_err(ParametersBuildError::from)?;
        bindings.insert(name.clone(), binding);
        parameters.insert(name, parameter.value_type);
    }
    PreparedNode::new(
        ParametersNode { parameters },
        NodeMetadata {
            input_bindings: bindings,
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: NodeFactory::Plain(factory) }
}
