use serde::de::DeserializeOwned;
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

pub type Inputs = BTreeMap<String, Value>;
pub type Outputs = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Any,
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

impl ValueType {
    pub fn is_assignable_to(self, input: Self) -> bool {
        self == input || input == Self::Any
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Any => "any",
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Number => "number",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortSpec {
    pub name: &'static str,
    pub value_type: ValueType,
    pub required: bool,
}

impl PortSpec {
    pub const fn new(name: &'static str, value_type: ValueType, required: bool) -> Self {
        Self {
            name,
            value_type,
            required,
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum NodeBuildError {
    #[snafu(display("invalid node configuration: {source}"))]
    InvalidConfiguration { source: serde_json::Error },
    #[snafu(display("node factory failed: {source}"))]
    FactoryFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum NodeExecutionError {
    #[snafu(display("node execution failed: {message}"))]
    ExecutionFailed { message: String },
    #[snafu(display("node plugin failed: {source}"))]
    PluginFailed {
        source: Box<dyn Error + Send + Sync + 'static>,
    },
}

pub fn deserialize_config<T>(value: Value) -> Result<T, NodeBuildError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).context(InvalidConfigurationSnafu)
}

pub trait Node: Send + Sync {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError>;
}

pub type NodeFactory = fn(Value) -> Result<Box<dyn Node>, NodeBuildError>;

#[derive(Clone, Copy, Debug)]
pub struct NodeRegistration {
    pub kind: &'static str,
    pub inputs: &'static [PortSpec],
    pub outputs: &'static [PortSpec],
    pub factory: NodeFactory,
}

impl NodeRegistration {
    pub fn instantiate(&self, config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
        (self.factory)(config)
    }
}

inventory::collect!(NodeRegistration);
