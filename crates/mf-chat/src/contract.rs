//! Application context and results shared by embedded and executable conversation workflows.

use crate::{ChatError, Invocation, error::*, turn};
use mf_runtime::{NodeValue, NodeValues, WorkflowArguments, WorkflowInputSchema, WorkflowManifest};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct TitleContext {
    pub source: String,
    pub revision: i64,
    pub attempted: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ConversationContext {
    pub title: TitleContext,
}

#[derive(Clone, Debug, NodeValue)]
pub struct ConversationInputs {
    pub prompt: String,
    pub system: Option<String>,
    pub history: Option<Vec<mfn_llm::ChatMessage>>,
    pub context: Option<ConversationContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ConversationResult {
    pub status: String,
    #[value(default)]
    #[serde(default)]
    pub text: String,
    pub usages: Vec<mfn_llm::CallUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<mfn_llm::MessageMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct Completion {
    pub invocation: Invocation,
    pub result: ConversationResult,
}

impl Default for ConversationResult {
    fn default() -> Self {
        Self {
            status: "succeeded".into(),
            text: String::new(),
            usages: Vec::new(),
            metadata: None,
            finish_reason: None,
            error: None,
            title: None,
        }
    }
}

impl ConversationResult {
    pub fn decode(mut values: NodeValues) -> Result<Self, ChatError> {
        let ports = Self::ports();
        values.retain(|name, _| ports.iter().any(|port| port.name == *name));
        let result = Self::from_values(values)?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), ChatError> {
        turn::validate_usages(&self.usages)?;
        match self.status.as_str() {
            "succeeded" => ensure!(
                !self.text.is_empty() && self.error.as_ref().is_none_or(String::is_empty),
                InvalidSnafu {
                    field: "workflow result",
                    reason: "success requires nonempty text and no error"
                }
            ),
            "failed" => ensure!(
                self.title.is_none()
                    && self
                        .error
                        .as_ref()
                        .is_some_and(|error| !error.trim().is_empty()),
                InvalidSnafu {
                    field: "workflow result",
                    reason: "failure requires an error summary and cannot update the title"
                }
            ),
            _ => {
                return InvalidSnafu {
                    field: "workflow result",
                    reason: "requires succeeded or failed status",
                }
                .fail();
            }
        }
        Ok(())
    }
}

mod recovery;
pub use recovery::{
    CompletionState, ConversationExit, MAX_RECOVERY_BYTES, ReconciledCompletion,
    UncommittedCompletion,
};

#[derive(Clone, Debug)]
pub struct ChatContract {
    pub parameters: WorkflowInputSchema,
    inputs: BTreeMap<String, String>,
}

impl ChatContract {
    pub fn from_manifest(manifest: &WorkflowManifest) -> Result<Self, ChatError> {
        manifest.validate()?;
        ensure!(
            !manifest.description()?.is_streaming() && manifest.input_schema.stdin.is_empty(),
            InvalidSnafu {
                field: "chat executable",
                reason: "requires finite JSON execution without stdin"
            }
        );
        let mut configuration = manifest.input_schema.clone();
        let mut inputs = BTreeMap::new();
        for expected in crate::ConversationInputs::ports() {
            let candidates: Vec<_> = manifest
                .input_schema
                .inputs
                .iter()
                .filter_map(|(node, ports)| {
                    ports.get(expected.name.as_ref()).map(|port| (node, port))
                })
                .collect();
            ensure!(
                candidates.len() <= 1 && (!expected.required || candidates.len() == 1),
                InvalidSnafu {
                    field: "chat inputs",
                    reason: "requires an unambiguous prompt and optional system, history, and context inputs"
                }
            );
            if let Some((node, port)) = candidates.first() {
                ensure!(
                    port.value_type == expected.value_type,
                    InvalidSnafu {
                        field: "chat inputs",
                        reason: "conversation input types must match the chat contract"
                    }
                );
                configuration
                    .inputs
                    .get_mut(*node)
                    .unwrap()
                    .remove(expected.name.as_ref());
                inputs.insert(
                    expected.name.to_string(),
                    mf_runtime::output_id(node, &expected.name),
                );
            }
        }
        for name in ["status", "text", "usages"] {
            ensure!(
                manifest
                    .definition
                    .outputs
                    .iter()
                    .any(|output| output.name == name && (name == "text" || !output.optional)),
                InvalidSnafu {
                    field: "chat outputs",
                    reason: "requires status and usages outputs, with text for successful results"
                }
            );
        }
        Ok(Self {
            parameters: configuration,
            inputs,
        })
    }

    pub fn arguments(
        &self,
        mut input: crate::ConversationInputs,
        parameters: &WorkflowArguments,
    ) -> Result<WorkflowArguments, ChatError> {
        self.parameters.validate(parameters)?;
        input.system.get_or_insert_default();
        input.history.get_or_insert_default();
        let mut arguments = parameters.clone();
        for (name, value) in input.into_values()? {
            if let Some(target) = self.inputs.get(&name) {
                arguments.0.insert(target.clone(), value);
            }
        }
        Ok(arguments)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnExit {
    Failed,
    Cancelled,
    Interrupted,
}
impl TurnExit {
    pub fn state(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug)]
pub enum InvocationState {
    NotBegun,
    Running,
    Committed(Completion),
    Finished(crate::Turn),
}

impl crate::ChatDatabase {
    pub fn inspect_invocation(
        &self,
        invocation: &Invocation,
    ) -> Result<InvocationState, ChatError> {
        let turn = match self.turn(&invocation.turn_id) {
            Ok(turn) => turn,
            Err(ChatError::TurnMissing { id }) if id == invocation.turn_id => {
                return Ok(InvocationState::NotBegun);
            }
            Err(error) => return Err(error),
        };
        let call = self.call(&invocation.call_id)?;
        ensure!(
            turn.session_id == invocation.session_id
                && call.session_id == invocation.session_id
                && call.turn_id.as_deref() == Some(invocation.turn_id.as_str())
                && call.purpose == "conversation"
                && call.state == turn.state,
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        match turn.state.as_str() {
            "running" => Ok(InvocationState::Running),
            "succeeded" => Ok(InvocationState::Committed(Completion {
                invocation: invocation.clone(),
                result: turn::saved_result(&call)?,
            })),
            "failed" | "cancelled" | "interrupted" => Ok(InvocationState::Finished(turn)),
            _ => ConflictSnafu {
                id: &invocation.call_id,
            }
            .fail(),
        }
    }

    pub fn finalize_invocation(
        &self,
        invocation: Invocation,
        outcome: TurnExit,
        summary: &str,
    ) -> Result<InvocationState, ChatError> {
        match self.inspect_invocation(&invocation)? {
            InvocationState::Running => {
                self.fail_turn(invocation.clone(), outcome.state(), summary)?;
                self.inspect_invocation(&invocation)
            }
            state => Ok(state),
        }
    }

    pub fn retry_prompt(&self, turn_id: &str) -> Result<(crate::Turn, String), ChatError> {
        let original: turn::TurnRequestOutputs = turn::get_request(turn::RecordInputs {
            database_path: self
                .path()
                .to_str()
                .context(PathSnafu { path: self.path() })?
                .into(),
            id: turn_id.into(),
        })?;
        ensure!(
            matches!(
                original.turn.state.as_str(),
                "failed" | "cancelled" | "interrupted"
            ),
            InvalidSnafu {
                field: "retry_of",
                reason: "must identify an unsuccessful turn"
            }
        );
        Ok((original.turn, original.prompt))
    }
}
