use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::fmt;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DefinitionId(String);

impl DefinitionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DefinitionId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for DefinitionId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

impl fmt::Display for DefinitionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    pub version: WorkflowDefinitionVersion,
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default)]
    pub outputs: Vec<WorkflowOutputDefinition>,
}

impl WorkflowDefinition {
    pub fn from_json(input: &str) -> Result<Self, DefinitionParseError> {
        serde_json::from_str(input).context(JsonParseSnafu)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDefinitionVersion {
    #[serde(rename = "2026-09-24")]
    V2026_09_24,
}

impl WorkflowDefinitionVersion {
    pub const CURRENT: Self = Self::V2026_09_24;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDefinition {
    pub id: DefinitionId,
    pub kind: String,
    #[serde(default = "empty_object")]
    pub config: Value,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeDefinition {
    pub from_node: DefinitionId,
    pub from_output: String,
    pub to_node: DefinitionId,
    pub to_input: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputDefinition {
    pub name: String,
    pub node: DefinitionId,
    pub port: String,
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum DefinitionParseError {
    #[snafu(display("invalid workflow definition JSON: {source}"))]
    JsonParse { source: serde_json::Error },
}
