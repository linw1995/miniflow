//! Correlation for iteration detail logs, independent of workflow lifecycle delivery.

use crate::{ContractError, Count, event::LoopPathEntry, require};
use serde::{Deserialize, Serialize};

pub const SCOPE: &str = "mf.iteration";
pub const CONTEXT_ATTRIBUTE: &str = "mf.iteration.context";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemIdentity {
    pub invocation: Count,
    pub index: Count,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationContext {
    pub invocation: Count,
    pub iteration_path: Vec<String>,
    pub body_path: Vec<String>,
    pub parent_path: Vec<LoopPathEntry>,
    #[serde(default)]
    pub parent_stream_invocation: Option<u64>,
    pub index: Count,
    pub sequence: Count,
    pub elapsed_ns: Count,
}

impl IterationContext {
    pub fn from_json(input: &str) -> Result<Self, ContractError> {
        require(
            input.len() <= 32 * 1024,
            "iteration context exceeds size limit",
        )?;
        let context: Self = serde_json::from_str(input)?;
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        require(
            self.invocation.get() > 0 && self.sequence.get() > 0,
            "iteration counters must be positive",
        )?;
        require(
            !self.iteration_path.is_empty() && self.body_path.len() <= crate::MAX_SCOPE_DEPTH,
            "invalid iteration scope depth",
        )?;
        require(
            self.body_path.starts_with(&self.iteration_path),
            "iteration body is outside its owner",
        )?;
        require(
            self.body_path
                .iter()
                .all(|id| !id.trim().is_empty() && id.len() <= 1024),
            "invalid iteration scope ID",
        )?;
        require(
            self.parent_path.len() < self.iteration_path.len(),
            "invalid iteration parent path",
        )?;
        require(
            self.parent_path.iter().map(LoopPathEntry::node_id).eq(self
                .iteration_path
                .iter()
                .take(self.parent_path.len())
                .map(String::as_str)),
            "iteration parent path disagrees with its scope",
        )?;
        Ok(())
    }

    pub fn item(&self) -> ItemIdentity {
        ItemIdentity {
            invocation: self.invocation,
            index: self.index,
        }
    }
}
