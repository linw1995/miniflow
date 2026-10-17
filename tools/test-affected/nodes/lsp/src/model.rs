use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub path: String,
    pub ranges: Vec<SourceRange>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub id: String,
    pub root: PathBuf,
    pub documents: Vec<Document>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRequest {
    pub snapshots: Vec<Snapshot>,
    #[serde(default)]
    pub initialization_options: Value,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Standard,
    RustAnalyzer,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub profile: Profile,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default = "default_deadline")]
    pub deadline_ms: u64,
    #[serde(default = "default_requests")]
    pub max_requests: usize,
    #[serde(default = "default_bytes")]
    pub max_bytes: usize,
}

fn default_deadline() -> u64 {
    120_000
}
fn default_requests() -> usize {
    2048
}
fn default_bytes() -> usize {
    16 * 1024 * 1024
}

impl ServerConfig {
    pub fn accepts(&self, document: &Document) -> bool {
        self.extensions.is_empty()
            || std::path::Path::new(&document.path)
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| self.extensions.iter().any(|e| e == s))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Analyzed,
    Incomplete,
    NotRequested,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    Unsupported,
    AnalyzerFailure,
    Limit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Gap {
    pub snapshot: String,
    pub operation: String,
    pub kind: GapKind,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SymbolEvidence {
    pub name: String,
    pub kind: u64,
    pub range: SourceRange,
    pub references: Vec<Value>,
    pub incoming_calls: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DocumentEvidence {
    pub path: String,
    pub symbols: Vec<SymbolEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SnapshotEvidence {
    pub snapshot: String,
    pub server_info: Value,
    pub position_encoding: String,
    pub progress_notifications: usize,
    pub documents: Vec<DocumentEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnalysisOutcome {
    pub request: AnalysisRequest,
    pub server: ServerConfig,
    pub status: Status,
    pub evidence: Vec<SnapshotEvidence>,
    pub gaps: Vec<Gap>,
    pub elapsed_ms: u64,
}
