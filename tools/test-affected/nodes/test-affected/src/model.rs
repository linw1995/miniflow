use mfn_lsp::{AnalysisRequest, SourceRange};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub before: Vec<SourceRange>,
    pub after: Vec<SourceRange>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ComparisonMode {
    #[default]
    MergeBase,
    Snapshot,
    Worktree,
    Commit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    #[serde(default)]
    pub comparison_mode: ComparisonMode,
    pub repository: PathBuf,
    pub base: String,
    pub head: String,
    pub base_root: PathBuf,
    pub head_root: PathBuf,
    pub changes: Vec<Change>,
    pub request: AnalysisRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfiguration {
    pub target: Option<String>,
    pub all_features: bool,
    pub features: Vec<String>,
    pub no_default_features: bool,
}

impl Default for BuildConfiguration {
    fn default() -> Self {
        Self {
            target: None,
            all_features: true,
            features: Vec::new(),
            no_default_features: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlanOptions {
    pub configuration: BuildConfiguration,
    pub inventory: Option<PathBuf>,
    pub exact_tests: bool,
    #[serde(skip)]
    pub coverage_graph: Option<CoverageGraphInput>,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            configuration: BuildConfiguration::default(),
            inventory: None,
            exact_tests: true,
            coverage_graph: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonRequest {
    pub mode: ComparisonMode,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default = "default_head")]
    pub head: String,
    #[serde(default = "default_analysis")]
    pub analyze: bool,
    #[serde(default)]
    pub require_checkout: bool,
    #[serde(default)]
    pub initialization_options: Option<Value>,
    #[serde(default)]
    pub options: PlanOptions,
    #[serde(default)]
    pub validation: Value,
    #[serde(default)]
    pub execution: Value,
}
fn default_head() -> String {
    "HEAD".into()
}
fn default_analysis() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub package: String,
    pub binary: String,
    pub name: String,
}

/// Coverage graph supplied to planning by the acquisition side of the Flow.
/// Unavailability is a recorded degradation, never an error.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum CoverageGraphInput {
    Resolved(GraphSource),
    Unavailable { reason: String },
}

/// One downloaded coverage graph artifact: the SQLite file plus its published
/// identity. Planning re-validates every field against the graph itself.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphSource {
    pub path: PathBuf,
    pub commit: String,
    pub run_id: String,
    pub attempt: u32,
    /// Nextest profile under which the selected tests will execute; matched
    /// against the producing run's execution signature.
    pub profile: String,
}

/// One file widened from exact graph selectors back to package scope.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphWidening {
    pub file: String,
    pub package: String,
    pub reason: String,
}

/// Planning outcome for the coverage graph: provenance when a compatible
/// graph participated, otherwise the recorded degradation reason.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GraphPlan {
    pub commit: Option<String>,
    pub run_id: Option<String>,
    pub attempt: Option<u32>,
    pub schema_version: Option<i64>,
    pub reason: Option<String>,
    pub widened: Vec<GraphWidening>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestPlan {
    #[serde(default)]
    pub comparison_mode: ComparisonMode,
    pub repository: PathBuf,
    pub base: String,
    pub head: String,
    pub snapshot: PathBuf,
    pub tree: String,
    pub configuration: BuildConfiguration,
    pub toolchain: String,
    pub build_environment: BTreeMap<String, String>,
    pub baseline_packages: Vec<String>,
    pub packages: Vec<String>,
    pub tests: Vec<TestCase>,
    pub reasons: BTreeMap<String, Vec<String>>,
    pub fallback_reasons: Vec<String>,
    #[serde(default)]
    pub ignored_files: Vec<String>,
    pub analysis_status: mfn_lsp::Status,
    pub baseline_count: Option<usize>,
    pub selected_count: Option<usize>,
    pub observed_relationships: usize,
    pub analysis_ms: u64,
    pub planning_ms: u64,
    #[serde(default)]
    pub coverage_graph: Option<GraphPlan>,
}
