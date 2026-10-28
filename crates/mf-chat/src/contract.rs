//! Application context and results shared by embedded and executable conversation workflows.

use crate::{ChatError, Invocation, error::*, turn};
use mf_runtime::{NodeValue, NodeValues, WorkflowArguments, WorkflowInputSchema, WorkflowManifest};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
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

/// Application-owned call facts enriched with event-derived performance measurements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct CallUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<mfn_llm::UsageAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<crate::CallTiming>,
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<mfn_llm::UsageNode>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<BTreeMap<String, i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<mfn_llm::CallUsage> for CallUsage {
    fn from(facts: mfn_llm::CallUsage) -> Self {
        let mfn_llm::CallUsage {
            attribution,
            started_at,
            call_id,
            node,
            model,
            usage,
            error,
        } = facts;
        Self {
            attribution,
            started_at,
            call_id,
            node,
            model,
            usage,
            error,
            timing: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ConversationResult {
    pub status: String,
    #[value(default)]
    #[serde(default)]
    pub text: String,
    pub usages: Vec<CallUsage>,
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
    pub fn from_json(bytes: &[u8]) -> Result<Self, ChatError> {
        ensure!(
            bytes.len() <= recovery::MAX_RECOVERY_BYTES,
            RecoveryTooLargeSnafu {
                limit: recovery::MAX_RECOVERY_BYTES
            }
        );
        let values = mf_runtime::decode_unique_json(bytes).context(JsonSnafu)?;
        let result = Self::decode(values)?;
        Ok(result)
    }

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
    pub answer_stream: bool,
    pub streaming: bool,
    inputs: BTreeMap<String, String>,
    owns_title: bool,
}

impl ChatContract {
    pub fn from_manifest(manifest: &WorkflowManifest) -> Result<Self, ChatError> {
        manifest.validate()?;
        ensure!(
            manifest.input_schema.stdin.is_empty(),
            InvalidSnafu {
                field: "chat executable",
                reason: "requires workflow execution without stdin"
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
        let streaming = manifest.description()?.is_streaming();
        let answer_stream = manifest
            .definition
            .outputs
            .iter()
            .any(|output| output.name == "answer");
        ensure!(
            !answer_stream || streaming,
            InvalidSnafu {
                field: "answer stream",
                reason: "requires native streaming execution"
            }
        );
        Ok(Self {
            parameters: configuration,
            answer_stream,
            streaming,
            inputs,
            owns_title: manifest
                .definition
                .outputs
                .iter()
                .any(|output| output.name == "title"),
        })
    }

    /// Reuses resolved provider inputs only when the workflow leaves titles to the host.
    pub fn builtin_title_parameters(
        &self,
        parameters: &WorkflowArguments,
    ) -> Option<WorkflowArguments> {
        if self.owns_title {
            return None;
        }
        let mut arguments = WorkflowArguments::default();
        for name in ["model", "base_url", "api_key"] {
            let mut candidates = self
                .parameters
                .inputs
                .iter()
                .filter_map(|(node, ports)| ports.get(name).map(|port| (node, port)));
            let (node, port) = candidates.next()?;
            if candidates.next().is_some() || port.value_type != mf_runtime::ValueType::String {
                return None;
            }
            let value = parameters.0.get(&mf_runtime::output_id(node, name))?;
            if value.as_str()?.trim().is_empty() {
                return None;
            }
            arguments
                .0
                .insert(mf_runtime::output_id("input", name), value.clone());
        }
        Some(arguments)
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

/// Decodes a public answer fragment without treating it as authoritative conversation state.
pub fn decode_answer_delta(
    value: &mf_runtime::ValueRef,
) -> Result<mfn_llm::TextDelta, mf_runtime::InputDecodeError> {
    <mfn_llm::TextDelta as mf_runtime::InputField>::decode_field("answer", Some(value.clone()))
}

/// Validates native workflow output records against the conversation protocol.
pub struct ConversationOutputDecoder {
    answer: bool,
    result: Option<ConversationResult>,
}

impl ConversationOutputDecoder {
    pub fn new(contract: &ChatContract) -> Self {
        Self {
            answer: contract.answer_stream,
            result: None,
        }
    }

    pub fn accept(
        &mut self,
        mut values: NodeValues,
    ) -> Result<Option<mfn_llm::TextDelta>, ChatError> {
        let delta = values
            .remove("answer")
            .map(|value| -> Result<_, ChatError> {
                ensure!(
                    self.answer,
                    InvalidSnafu {
                        field: "answer stream",
                        reason: "was not declared by the workflow"
                    }
                );
                let delta = decode_answer_delta(&value)?;
                ensure!(
                    !delta.text.is_empty() && !delta.model_call_id.trim().is_empty(),
                    InvalidSnafu {
                        field: "answer stream",
                        reason: "requires nonempty text and a model call identity"
                    }
                );
                Ok(delta)
            })
            .transpose()?;
        let ports = ConversationResult::ports();
        values.retain(|name, _| ports.iter().any(|port| port.name == name.as_str()));
        if !values.is_empty() {
            ensure!(
                self.result.is_none(),
                InvalidSnafu {
                    field: "workflow result",
                    reason: "must be produced only once"
                }
            );
            self.result = Some(ConversationResult::decode(values)?);
        }
        Ok(delta)
    }

    pub fn result(&self) -> Option<&ConversationResult> {
        self.result.as_ref()
    }

    pub fn finish(self) -> Result<ConversationResult, ChatError> {
        self.result.context(CaptureSnafu {
            message: "workflow ended without a terminal conversation result",
        })
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use serde_json::json;

    fn decoder(answer: bool) -> ConversationOutputDecoder {
        ConversationOutputDecoder::new(&ChatContract {
            owns_title: false,
            parameters: Default::default(),
            answer_stream: answer,
            streaming: true,
            inputs: Default::default(),
        })
    }

    fn values(value: serde_json::Value) -> NodeValues {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn native_results_remain_authoritative_when_other_domains_finish_later() {
        let mut decoder = decoder(true);
        decoder
            .accept(values(
                json!({"status":"succeeded","text":"canonical","usages":[]}),
            ))
            .unwrap();
        assert!(
            decoder
                .accept(values(json!({"debug":42})))
                .unwrap()
                .is_none()
        );
        let delta = decoder
            .accept(values(
                json!({"answer":{"text":"prefix","model_call_id":"model"}}),
            ))
            .unwrap()
            .unwrap();
        assert_eq!(delta.text, "prefix");
        assert_eq!(decoder.finish().unwrap().text, "canonical");
    }

    #[test]
    fn native_conversation_records_reject_unadvertised_or_invalid_deltas() {
        assert!(
            decoder(false)
                .accept(values(
                    json!({"answer":{"text":"prefix","model_call_id":"model"}})
                ))
                .is_err()
        );
        assert!(matches!(
            decoder(true).accept(values(json!({"answer":"wrong type"}))),
            Err(ChatError::Decode { .. })
        ));
        for delta in [
            json!({"text":"","model_call_id":"model"}),
            json!({"text":"prefix","model_call_id":""}),
        ] {
            assert!(matches!(
                decoder(true).accept(values(json!({"answer":delta}))),
                Err(ChatError::Invalid { .. })
            ));
        }
    }

    #[test]
    fn native_conversation_records_require_one_complete_terminal_result() {
        assert!(matches!(
            decoder(true).finish(),
            Err(ChatError::Capture { .. })
        ));
        let mut decoder = decoder(true);
        let result = json!({"status":"succeeded","text":"canonical","usages":[]});
        decoder.accept(values(result.clone())).unwrap();
        assert!(matches!(
            decoder.accept(values(result)),
            Err(ChatError::Invalid { .. })
        ));
        assert_eq!(decoder.result().unwrap().text, "canonical");
    }
}
