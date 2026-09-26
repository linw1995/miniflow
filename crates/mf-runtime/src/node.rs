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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedPortSpec {
    pub name: String,
    pub value_type: ValueType,
    pub required: bool,
}

impl OwnedPortSpec {
    pub fn new(name: impl Into<String>, value_type: ValueType, required: bool) -> Self {
        Self {
            name: name.into(),
            value_type,
            required,
        }
    }
}

impl From<PortSpec> for OwnedPortSpec {
    fn from(port: PortSpec) -> Self {
        Self::new(port.name, port.value_type, port.required)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodePorts {
    pub inputs: Vec<OwnedPortSpec>,
    pub outputs: Vec<OwnedPortSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReference {
    pub output: String,
    pub label: String,
}

impl ContextReference {
    pub fn new(output: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            label: label.into(),
        }
    }
}

pub fn output_id(node: &str, port: &str) -> String {
    format!("{node}.{port}")
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

    fn ports(&self) -> Option<NodePorts> {
        None
    }

    fn context_references(&self) -> Vec<ContextReference> {
        Vec::new()
    }
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

    pub fn effective_ports(&self, node: &dyn Node) -> NodePorts {
        node.ports().unwrap_or_else(|| NodePorts {
            inputs: self.inputs.iter().copied().map(Into::into).collect(),
            outputs: self.outputs.iter().copied().map(Into::into).collect(),
        })
    }
}

inventory::collect!(NodeRegistration);
