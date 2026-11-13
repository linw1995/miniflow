use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
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
    #[serde(
        default,
        deserialize_with = "deserialize_dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
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
        !self.order_edges.is_empty()
    }

    pub fn from_json(input: &str) -> Result<Self, DefinitionParseError> {
        Self::parse_json(input, None)
    }

    /// Parse a receiver-authorized child, inheriting the original root version only
    /// when this definition omits `version`. Node configuration remains opaque.
    pub fn from_value_with_version(
        mut value: Value,
        inherited: WorkflowDefinitionVersion,
    ) -> Result<Self, serde_json::Error> {
        if let Some(scope) = value.as_object_mut()
            && !scope.contains_key("version")
        {
            scope.insert("version".into(), serde_json::to_value(inherited)?);
        }
        serde_json::from_value(value)
    }

    /// Parse an authorized child source while retaining duplicate-key rejection
    /// and the same typed JSON and legacy-version errors as the root parser.
    pub fn from_json_with_version(
        input: &str,
        inherited: WorkflowDefinitionVersion,
    ) -> Result<Self, DefinitionParseError> {
        Self::parse_json(input, Some(inherited))
    }

    fn parse_json(
        input: &str,
        inherited: Option<WorkflowDefinitionVersion>,
    ) -> Result<Self, DefinitionParseError> {
        let value = serde_json::from_str::<crate::workflow_inputs::UniqueValue>(input)
            .context(JsonParseSnafu)?
            .0;
        if value.get("version").and_then(Value::as_str) == Some("2026-09-24") {
            return LegacyVersionSnafu.fail();
        }
        match inherited {
            Some(version) => Self::from_value_with_version(value, version),
            None => serde_json::from_value(value),
        }
        .context(JsonParseSnafu)
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
    #[serde(rename = "2026-11-11")]
    V2026_11_11,
}

impl WorkflowDefinitionVersion {
    pub const CURRENT: Self = Self::V2026_11_11;

    pub fn supports_child_streams(self) -> bool {
        matches!(self, Self::V2026_10_28 | Self::V2026_11_11)
    }

    pub fn supports_nested_flows(self) -> bool {
        matches!(
            self,
            Self::V2026_10_25 | Self::V2026_10_28 | Self::V2026_11_11
        )
    }

    pub fn supports_startup_inputs(self) -> bool {
        matches!(
            self,
            Self::V2026_10_03
                | Self::V2026_10_19
                | Self::V2026_10_23
                | Self::V2026_10_25
                | Self::V2026_10_28
                | Self::V2026_11_11
        )
    }

    pub fn supports_node_success_edges(self) -> bool {
        matches!(
            self,
            Self::V2026_10_19
                | Self::V2026_10_23
                | Self::V2026_10_25
                | Self::V2026_10_28
                | Self::V2026_11_11
        )
    }

    pub fn supports_clusters(self) -> bool {
        matches!(
            self,
            Self::V2026_10_23 | Self::V2026_10_25 | Self::V2026_10_28 | Self::V2026_11_11
        )
    }

    pub fn supports_cluster_collapsed(self) -> bool {
        matches!(self, Self::V2026_11_11)
    }

    pub fn supports_loop_inputs(self) -> bool {
        matches!(self, Self::V2026_11_11)
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
    pub body: Option<Box<WorkflowDefinition>>,
}

impl CallConfig {
    /// Identify the strict edge-carried callable contract without interpreting
    /// sources or validating the receiving node's business rules. Static selector
    /// and body fields, including null fields, cannot be mixed into this mode.
    pub fn is_dynamic(config: &Value) -> bool {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InputContract {
            #[serde(rename = "inputs")]
            _inputs: Vec<crate::PortSpec>,
            #[serde(rename = "outputs")]
            _outputs: Vec<crate::PortSpec>,
            #[serde(default, rename = "stream_outputs")]
            _stream_outputs: Vec<crate::PortSpec>,
        }

        serde_json::from_value::<InputContract>(config.clone()).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDefinition {
    pub id: DefinitionId,
    pub kind: String,
    #[serde(default = "empty_object")]
    pub config: Value,
}

/// Errors reading or replacing declared executable configuration fields.
#[derive(Debug, Snafu)]
pub enum FlowParameterError {
    #[snafu(display("invalid Flow parameters: {message}"), visibility(pub(super)))]
    Invalid { message: &'static str },
    #[snafu(
        display("invalid Flow parameter `{parameter}`: {source}"),
        visibility(pub(super))
    )]
    Json {
        parameter: String,
        source: serde_json::Error,
    },
}

impl NodeDefinition {
    /// Writes a normalized child into its declared configuration field.
    pub fn set_node_flow(
        &mut self,
        name: &str,
        body: WorkflowDefinition,
    ) -> Result<(), FlowParameterError> {
        ensure!(
            !name.trim().is_empty(),
            InvalidSnafu {
                message: "Flow parameter field names must be nonblank"
            }
        );
        let config = self.config.as_object_mut().ok_or_else(|| {
            InvalidSnafu {
                message: "Flow parameters require an object configuration",
            }
            .build()
        })?;
        let value = serde_json::to_value(body).context(JsonSnafu { parameter: name })?;
        config.insert(name.into(), value);
        Ok(())
    }

    /// Reads only the declared, present, non-null executable configuration fields.
    /// Source resolution must normalize these fields to direct workflow definitions first.
    /// Every other field remains opaque provider data.
    pub fn flow_parameters(
        &self,
        parameters: &[&str],
    ) -> Result<BTreeMap<String, WorkflowDefinition>, FlowParameterError> {
        let mut flows = BTreeMap::new();
        for &name in parameters {
            ensure!(
                !name.trim().is_empty(),
                InvalidSnafu {
                    message: "Flow parameter field names must be nonblank"
                }
            );
            if let Some(value) = self.config.get(name).filter(|value| !value.is_null()) {
                let body =
                    serde_json::from_value(value.clone()).context(JsonSnafu { parameter: name })?;
                flows.insert(name.to_owned(), body);
            }
        }
        Ok(flows)
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, crate::ValueType>,
    pub max_iterations: u16,
    pub variables: Vec<LoopVariableDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<LoopConditionDefinition>,
    pub body: Box<WorkflowDefinition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopVariableDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterDefinition {
    pub label: String,
    pub annotation: String,
    pub nodes: Vec<DefinitionId>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
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

#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
pub enum NodeDependencyError {
    #[snafu(display("package must be a nonempty Cargo package name"))]
    InvalidPackage,
    #[snafu(display("select exactly one source: version, git with rev, or path"))]
    InvalidSources,
    #[snafu(display("git must not be empty"))]
    EmptyGit,
    #[snafu(display("rev must be a full Git commit hash"))]
    InvalidRevision,
    #[snafu(display("rev requires git"))]
    RevisionWithoutGit,
    #[snafu(display("version must not be empty"))]
    EmptyVersion,
    #[snafu(display("path must not be empty"))]
    EmptyPath,
    #[snafu(display("features must not contain empty names"))]
    EmptyFeature,
}

impl NodeDependency {
    pub fn validate(&self) -> Result<(), NodeDependencyError> {
        ensure!(
            !self.package.is_empty()
                && self
                    .package
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            InvalidPackageSnafu
        );
        let sources = usize::from(self.version.is_some())
            + usize::from(self.git.is_some())
            + usize::from(self.path.is_some());
        ensure!(sources == 1, InvalidSourcesSnafu);
        if let Some(git) = &self.git {
            ensure!(!git.trim().is_empty(), EmptyGitSnafu);
            let valid = self.rev.as_ref().is_some_and(|rev| {
                (rev.len() == 40 || rev.len() == 64) && rev.bytes().all(|c| c.is_ascii_hexdigit())
            });
            ensure!(valid, InvalidRevisionSnafu);
        } else {
            ensure!(self.rev.is_none(), RevisionWithoutGitSnafu);
        }
        ensure!(
            !self.version.as_ref().is_some_and(|v| v.trim().is_empty()),
            EmptyVersionSnafu
        );
        ensure!(
            !self.path.as_ref().is_some_and(|p| p.as_os_str().is_empty()),
            EmptyPathSnafu
        );
        ensure!(
            !self.features.iter().any(|f| f.trim().is_empty()),
            EmptyFeatureSnafu
        );
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
