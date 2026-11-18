//! Correlation for iteration detail logs, independent of workflow lifecycle delivery.

use crate::{
    ContractError, Count,
    event::{Failure, InputSelection, LoopPathEntry, NodeIdentity, Outcome, SkipCause},
    require,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const SCOPE: &str = "mf.iteration";
pub const CONTEXT_ATTRIBUTE: &str = "mf.iteration.context";
/// Extended correlation is not emitted under the legacy attribute: old readers
/// must not interpret an absolute Flow definition as an item-owned static child.
pub const DETAIL_CONTEXT_ATTRIBUTE: &str = "mf.iteration.context.v2";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationDetailContext {
    pub context: IterationContext,
    /// Runtime scopes relative to the item, distinct from `context.body_path`.
    pub body_scope: Vec<LoopPathEntry>,
}

impl IterationDetailContext {
    pub fn from_json(input: &str) -> Result<Self, ContractError> {
        require(
            input.len() <= 32 * 1024,
            "iteration context exceeds size limit",
        )?;
        let detail: Self = serde_json::from_str(input)?;
        detail.context.validate_for_body_scope(&detail.body_scope)?;
        Ok(detail)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum IterationEvent {
    InvocationFinished,
    ItemStarted,
    ItemFinished {
        outcome: Outcome,
        duration_ns: Option<Count>,
    },
    NodeStarted {
        node: NodeIdentity,
        selected_inputs: Vec<InputSelection>,
    },
    NodeFinished {
        node: NodeIdentity,
        outcome: Outcome,
        duration_ns: Option<Count>,
        produced_ports: Vec<String>,
        skipped_ports: Vec<String>,
        failure: Option<Failure>,
    },
    NodeSkipped {
        node: NodeIdentity,
        causes: Vec<SkipCause>,
        skipped_ports: Vec<String>,
    },
}

impl IterationEvent {
    pub fn node(&self) -> Option<&NodeIdentity> {
        match self {
            Self::NodeStarted { node, .. }
            | Self::NodeFinished { node, .. }
            | Self::NodeSkipped { node, .. } => Some(node),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IterationRecord {
    pub context: IterationContext,
    pub event: IterationEvent,
    pub correlation: Option<(Vec<u8>, Vec<u8>)>,
}

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
        require(
            !context.parent_path.iter().any(LoopPathEntry::is_flow),
            "Flow iteration scopes require the extended detail context",
        )?;
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        self.validate_for_body_scope(&[])
    }

    pub fn validate_for_body_scope(
        &self,
        body_scope: &[LoopPathEntry],
    ) -> Result<(), ContractError> {
        crate::event::validate_path(&self.parent_path)?;
        crate::event::validate_path(body_scope)?;
        require(
            self.invocation.get() > 0 && self.sequence.get() > 0,
            "iteration counters must be positive",
        )?;
        require(
            !self.iteration_path.is_empty()
                && self.iteration_path.len() <= crate::MAX_SCOPE_DEPTH
                && !self.body_path.is_empty()
                && self.body_path.len() <= crate::MAX_SCOPE_DEPTH,
            "invalid iteration scope depth",
        )?;
        require(
            body_scope.iter().any(LoopPathEntry::is_flow)
                || self.body_path.starts_with(&self.iteration_path),
            "iteration body is outside its owner",
        )?;
        require(
            self.body_path
                .iter()
                .chain(&self.iteration_path)
                .all(|id| !id.trim().is_empty() && id.len() <= 1024),
            "invalid iteration scope ID",
        )?;
        require(
            self.parent_path.iter().any(LoopPathEntry::is_flow)
                || self.parent_path.len() < self.iteration_path.len(),
            "invalid iteration parent path",
        )?;
        require(
            self.parent_path.iter().any(LoopPathEntry::is_flow)
                || self.parent_path.iter().map(LoopPathEntry::node_id).eq(self
                    .iteration_path
                    .iter()
                    .take(self.parent_path.len())
                    .map(String::as_str)),
            "iteration parent path disagrees with its scope",
        )?;
        Ok(())
    }

    /// Iteration detail records carry either the explicit runtime scope or the
    /// static child keys relative to the item body.
    pub fn relative_scope<'a>(
        &self,
        body_scope: &'a [LoopPathEntry],
    ) -> Option<Cow<'a, [LoopPathEntry]>> {
        if !body_scope.is_empty() {
            return Some(Cow::Borrowed(body_scope));
        }
        let relative = self
            .body_path
            .strip_prefix(self.iteration_path.as_slice())?;
        Some(Cow::Owned(
            relative
                .iter()
                .map(|id| LoopPathEntry::Call {
                    call_id: id.clone(),
                })
                .collect(),
        ))
    }

    pub fn item(&self) -> ItemIdentity {
        ItemIdentity {
            invocation: self.invocation,
            index: self.index,
        }
    }
}
