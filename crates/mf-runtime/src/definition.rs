use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

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
    #[serde(
        default,
        deserialize_with = "deserialize_execution",
        skip_serializing_if = "Option::is_none"
    )]
    pub execution: Option<crate::StreamExecution>,
    #[serde(deserialize_with = "deserialize_dependencies")]
    pub dependencies: BTreeMap<String, NodeDependency>,
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_edges: Vec<ControlEdgeDefinition>,
    #[serde(default)]
    pub outputs: Vec<WorkflowOutputDefinition>,
}

fn deserialize_execution<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::StreamExecution>, D::Error> {
    crate::StreamExecution::deserialize(deserializer).map(Some)
}

impl WorkflowDefinition {
    pub fn validate_execution(&self) -> Result<(), crate::StreamBuildError> {
        if let Some(execution) = &self.execution {
            ensure!(
                self.version == WorkflowDefinitionVersion::V2026_10_03,
                crate::stream_plan::InvalidPlanSnafu {
                    message: "stream execution requires workflow schema 2026-10-03",
                }
            );
            execution.limits.validate()?;
        }
        Ok(())
    }

    pub fn from_json(input: &str) -> Result<Self, DefinitionParseError> {
        let value: Value = serde_json::from_str(input).context(JsonParseSnafu)?;
        if value.get("version").and_then(Value::as_str) == Some("2026-09-24") {
            return LegacyVersionSnafu.fail();
        }
        let definition: Self = serde_json::from_str(input).context(JsonParseSnafu)?;
        definition
            .validate_execution()
            .map_err(<serde_json::Error as serde::de::Error>::custom)
            .context(JsonParseSnafu)?;
        Ok(definition)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowDefinitionVersion {
    #[serde(rename = "2026-09-26")]
    V2026_09_26,
    #[serde(rename = "2026-09-29")]
    V2026_09_29,
    #[serde(rename = "2026-10-02")]
    V2026_10_02,
    #[serde(rename = "2026-10-03")]
    V2026_10_03,
}

impl WorkflowDefinitionVersion {
    pub const CURRENT: Self = Self::V2026_10_03;

    pub fn supports_startup_inputs(self) -> bool {
        self == Self::V2026_10_03
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDefinition {
    pub id: DefinitionId,
    pub kind: String,
    #[serde(default = "empty_object")]
    pub config: Value,
    #[serde(rename = "loop", default, skip_serializing_if = "Option::is_none")]
    pub loop_definition: Option<Box<LoopDefinition>>,
}

pub const LOOP_KIND: &str = "workflow.loop";
pub const LOOP_ASSIGN_KIND: &str = "workflow.loop_assign";
pub const EXIT_LOOP_KIND: &str = "workflow.exit_loop";
pub const LOOP_SOURCE_ID: &str = "%loop";
pub const MAX_LOOP_ITERATIONS: u16 = mf_telemetry::MAX_LOOP_ITERATIONS;
pub const MAX_LOOP_DEPTH: usize = mf_telemetry::MAX_LOOP_DEPTH;
pub const MAX_SCHEDULED_STEPS: usize = mf_telemetry::MAX_LOOP_SCHEDULED_STEPS as usize;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopDefinition {
    pub max_iterations: u16,
    pub variables: Vec<LoopVariableDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<LoopConditionDefinition>,
    pub body: LoopBodyDefinition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopVariableDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopBodyDefinition {
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_edges: Vec<ControlEdgeDefinition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopComparisonOperator {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopConditionDefinition {
    pub variable: String,
    pub operator: LoopComparisonOperator,
    pub value: Value,
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEdgeDefinition {
    pub from_node: DefinitionId,
    pub from_output: String,
    pub to_node: DefinitionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputDefinition {
    pub name: String,
    pub node: DefinitionId,
    pub port: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Snafu)]
pub enum DefinitionParseError {
    #[snafu(display(
        "workflow schema 2026-09-24 is no longer supported; use 2026-09-26 and declare the packages providing node kinds in dependencies"
    ))]
    LegacyVersion,
    #[snafu(display("invalid workflow definition JSON: {source}"))]
    JsonParse { source: serde_json::Error },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDependency {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(rename = "default-features", default = "enabled")]
    pub default_features: bool,
}

fn enabled() -> bool {
    true
}

impl NodeDependency {
    pub fn validate(&self) -> Result<(), String> {
        if self.package.is_empty()
            || !self
                .package
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("package must be a nonempty Cargo package name".into());
        }
        let sources = usize::from(self.version.is_some())
            + usize::from(self.git.is_some())
            + usize::from(self.path.is_some());
        if sources != 1 {
            return Err("select exactly one source: version, git with rev, or path".into());
        }
        if let Some(git) = &self.git {
            if git.trim().is_empty() {
                return Err("git must not be empty".into());
            }
            let valid = self.rev.as_ref().is_some_and(|rev| {
                (rev.len() == 40 || rev.len() == 64) && rev.bytes().all(|c| c.is_ascii_hexdigit())
            });
            if !valid {
                return Err("rev must be a full Git commit hash".into());
            }
        } else if self.rev.is_some() {
            return Err("rev requires git".into());
        }
        if self.version.as_ref().is_some_and(|v| v.trim().is_empty()) {
            return Err("version must not be empty".into());
        }
        if self.path.as_ref().is_some_and(|p| p.as_os_str().is_empty()) {
            return Err("path must not be empty".into());
        }
        if self.features.iter().any(|f| f.trim().is_empty()) {
            return Err("features must not contain empty names".into());
        }
        Ok(())
    }
}

fn deserialize_dependencies<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, NodeDependency>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let values = BTreeMap::<String, Value>::deserialize(deserializer)?;
    values
        .into_iter()
        .map(|(alias, value)| {
            if alias.trim().is_empty() {
                return Err(D::Error::custom("dependencies alias must not be blank"));
            }
            let dependency: NodeDependency = serde_json::from_value(value)
                .map_err(|error| D::Error::custom(format!("dependencies.{alias}: {error}")))?;
            dependency
                .validate()
                .map_err(|error| D::Error::custom(format!("dependencies.{alias}: {error}")))?;
            Ok((alias, dependency))
        })
        .collect()
}
