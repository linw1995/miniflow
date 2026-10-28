use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DefinitionId(Cow<'static, str>);

impl DefinitionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into().into())
    }

    pub const fn from_static(id: &'static str) -> Self {
        Self(Cow::Borrowed(id))
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

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDefinition {
    pub version: WorkflowDefinitionVersion,
    pub execution: Option<crate::StreamExecution>,
    pub dependencies: BTreeMap<String, NodeDependency>,
    pub flows: BTreeMap<String, FlowSource>,
    pub nodes: Vec<NodeDefinition>,
    pub edges: Vec<EdgeDefinition>,
    pub control_edges: Vec<ControlEdgeDefinition>,
    pub outputs: Vec<WorkflowOutputDefinition>,
    pub order_edges: Vec<OrderEdgeDefinition>,
    pub clusters: Vec<ClusterDefinition>,
}

mod wire;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "WorkflowDefinition", deny_unknown_fields)]
struct DefinitionFields {
    pub version: WorkflowDefinitionVersion,
    #[serde(
        default,
        deserialize_with = "deserialize_execution",
        skip_serializing_if = "Option::is_none"
    )]
    pub execution: Option<crate::StreamExecution>,
    #[serde(deserialize_with = "deserialize_dependencies")]
    pub dependencies: BTreeMap<String, NodeDependency>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flows: BTreeMap<String, FlowSource>,
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_edges: Vec<ControlEdgeDefinition>,
    #[serde(default)]
    pub outputs: Vec<WorkflowOutputDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_edges: Vec<OrderEdgeDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clusters: Vec<ClusterDefinition>,
}

fn deserialize_execution<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::StreamExecution>, D::Error> {
    crate::StreamExecution::deserialize(deserializer).map(Some)
}

impl WorkflowDefinition {
    pub fn has_order_edges(&self) -> bool {
        !self.order_edges.is_empty() || self.nodes.iter().any(NodeDefinition::has_order_edges)
    }

    pub fn from_json(input: &str) -> Result<Self, DefinitionParseError> {
        let value: Value = serde_json::from_str(input).context(JsonParseSnafu)?;
        if value.get("version").and_then(Value::as_str) == Some("2026-09-24") {
            return LegacyVersionSnafu.fail();
        }
        let definition: Self = serde_json::from_str(input).context(JsonParseSnafu)?;
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
    #[serde(rename = "2026-10-19")]
    V2026_10_19,
    #[serde(rename = "2026-10-23")]
    V2026_10_23,
    #[serde(rename = "2026-10-25")]
    V2026_10_25,
    #[serde(rename = "2026-10-28")]
    V2026_10_28,
}

impl WorkflowDefinitionVersion {
    pub const CURRENT: Self = Self::V2026_10_28;

    pub fn supports_child_streams(self) -> bool {
        matches!(self, Self::V2026_10_28)
    }

    pub fn supports_nested_flows(self) -> bool {
        matches!(self, Self::V2026_10_25 | Self::V2026_10_28)
    }

    pub fn supports_startup_inputs(self) -> bool {
        matches!(
            self,
            Self::V2026_10_03
                | Self::V2026_10_19
                | Self::V2026_10_23
                | Self::V2026_10_25
                | Self::V2026_10_28
        )
    }

    pub fn supports_node_success_edges(self) -> bool {
        matches!(
            self,
            Self::V2026_10_19 | Self::V2026_10_23 | Self::V2026_10_25 | Self::V2026_10_28
        )
    }

    pub fn supports_clusters(self) -> bool {
        matches!(
            self,
            Self::V2026_10_23 | Self::V2026_10_25 | Self::V2026_10_28
        )
    }
}

pub const CALL_KIND: &str = "workflow.call";
pub const MAX_CALL_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FlowSource {
    Path { path: PathBuf },
    Builtin { builtin: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallConfig {
    pub flow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<Box<WorkflowDefinition>>,
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

impl NodeDefinition {
    pub fn has_order_edges(&self) -> bool {
        self.loop_definition
            .as_ref()
            .is_some_and(|definition| definition.body.has_order_edges())
            || (self.kind == crate::ITERATION_KIND
                && self.config["body"]["order_edges"]
                    .as_array()
                    .is_some_and(|edges| !edges.is_empty()))
    }
}

impl LoopBodyDefinition {
    pub fn has_order_edges(&self) -> bool {
        !self.order_edges.is_empty() || self.nodes.iter().any(NodeDefinition::has_order_edges)
    }
}

pub const LOOP_KIND: &str = "workflow.loop";
pub const LOOP_ASSIGN_KIND: &str = "workflow.loop_assign";
pub const EXIT_LOOP_KIND: &str = "workflow.exit_loop";
pub const LOOP_SOURCE_ID: &str = "%loop";
pub const MAX_LOOP_ITERATIONS: u16 = mf_telemetry::MAX_LOOP_ITERATIONS;
pub const MAX_LOOP_DEPTH: usize = mf_telemetry::MAX_LOOP_DEPTH;
pub const MAX_SCHEDULED_STEPS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopDefinition {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stream_outputs: BTreeMap<String, OutputReferenceDefinition>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_edges: Vec<OrderEdgeDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clusters: Vec<ClusterDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDefinition {
    pub label: String,
    pub annotation: String,
    pub nodes: Vec<DefinitionId>,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub exclusive: bool,
    pub from_node: DefinitionId,
    pub from_output: String,
    pub to_node: DefinitionId,
    pub to_input: String,
}

#[derive(Debug, Snafu)]
#[snafu(display("invalid exclusive input `{node}`.`{input}`: {reason}"))]
pub struct ExclusiveInputError {
    pub node: DefinitionId,
    pub input: String,
    pub reason: &'static str,
}

pub fn validate_exclusive_inputs(edges: &[EdgeDefinition]) -> Result<(), ExclusiveInputError> {
    let mut groups: BTreeMap<_, std::collections::BTreeSet<_>> = BTreeMap::new();
    for edge in edges.iter().filter(|edge| edge.exclusive) {
        let sources = groups.entry((&edge.to_node, &edge.to_input)).or_default();
        snafu::ensure!(
            sources.insert((&edge.from_node, &edge.from_output)),
            ExclusiveInputSnafu {
                node: edge.to_node.clone(),
                input: edge.to_input.clone(),
                reason: "duplicate candidate source",
            }
        );
    }
    for edge in edges.iter().filter(|edge| !edge.exclusive) {
        snafu::ensure!(
            !groups.contains_key(&(&edge.to_node, &edge.to_input)),
            ExclusiveInputSnafu {
                node: edge.to_node.clone(),
                input: edge.to_input.clone(),
                reason: "cannot mix ordinary and exclusive edges for one input",
            }
        );
    }
    for ((node, input), sources) in groups {
        snafu::ensure!(
            sources.len() >= 2,
            ExclusiveInputSnafu {
                node: node.clone(),
                input: input.clone(),
                reason: "at least two sources are required",
            }
        );
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEdgeDefinition {
    pub from_node: DefinitionId,
    pub from_output: Cow<'static, str>,
    pub to_node: DefinitionId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderEdgeDefinition {
    pub from_node: DefinitionId,
    pub to_node: DefinitionId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputReferenceDefinition {
    pub node: DefinitionId,
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum InputBindingDefinition {
    From {
        from: OutputReferenceDefinition,
    },
    Exclusive {
        exclusive: Vec<OutputReferenceDefinition>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputDefinition {
    pub name: Cow<'static, str>,
    pub node: DefinitionId,
    pub port: Cow<'static, str>,
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
