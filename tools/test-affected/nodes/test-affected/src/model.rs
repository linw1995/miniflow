use mfn_lsp::{AnalysisRequest, SourceRange};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub before: Vec<SourceRange>,
    pub after: Vec<SourceRange>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlanOptions {
    pub configuration: BuildConfiguration,
    pub inventory: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub package: String,
    pub binary: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestPlan {
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
    pub analysis_status: mfn_lsp::Status,
    pub baseline_count: Option<usize>,
    pub selected_count: Option<usize>,
    pub observed_relationships: usize,
    pub analysis_ms: u64,
    pub planning_ms: u64,
}
