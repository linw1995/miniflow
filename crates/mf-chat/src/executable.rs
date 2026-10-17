//! Parent-owned chat invocation state and executable contracts.

use crate::{ChatError, Completion, Invocation, ModelIdentity, error::*, turn};
use mf_runtime::{NodeValue, WorkflowArguments, WorkflowInputSchema, WorkflowManifest};
use sha2::{Digest, Sha256};
use snafu::{ResultExt, ensure};
use std::collections::BTreeMap;

mod recovery;
pub use recovery::{CompletionState, ConversationExit, MAX_RECOVERY_BYTES, ReconciledCompletion};

#[derive(Clone, Debug)]
pub struct ChatContract {
    pub config_id: String,
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
        for expected in mfn_llm::LlmInputs::ports() {
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
                    reason: "requires an unambiguous prompt and optional system and history inputs"
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
        let config_id =
            Sha256::digest(serde_json::to_vec(&manifest.definition).context(JsonSnafu)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        Ok(Self {
            config_id,
            parameters: configuration,
            inputs,
        })
    }

    pub fn model_identity(
        &self,
        profile: &str,
        parameters: &WorkflowArguments,
    ) -> Result<ModelIdentity, ChatError> {
        self.parameters.validate(parameters)?;
        let public: BTreeMap<_, _> = parameters
            .0
            .iter()
            .filter(|(_, value)| !value.contains_secret())
            .collect();
        let config_id =
            Sha256::digest(serde_json::to_vec(&(&self.config_id, public)).context(JsonSnafu)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        Ok(ModelIdentity {
            profile: profile.into(),
            model: None,
            config_id,
        })
    }

    pub fn arguments(
        &self,
        mut input: mfn_llm::LlmInputs,
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
            Err(error) => {
                let mut source: Option<&dyn std::error::Error> = Some(&error);
                while let Some(cause) = source {
                    if matches!(cause.downcast_ref::<ChatError>(), Some(ChatError::TurnMissing { id }) if id == &invocation.turn_id)
                    {
                        return Ok(InvocationState::NotBegun);
                    }
                    source = cause.source();
                }
                return Err(error);
            }
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
                reply: turn::saved_reply(&call)?,
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
        let original: turn::TurnRequestOutputs = crate::operation::execute(
            self.path(),
            "mf_chat.turn.request",
            turn::RecordInputs {
                database_path: String::new(),
                id: turn_id.into(),
            },
        )?;
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
