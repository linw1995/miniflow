//! Application context and results shared by embedded and executable conversation workflows.

pub mod transcript;
pub use transcript::{
    ConversationResult, MessageUpdate, ResultAlternatives, TerminalResultDecoder,
};

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
pub struct ModelContextRevision {
    pub revision: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ConversationContext {
    pub title: TitleContext,
    pub model_context: ModelContextRevision,
}

#[derive(Clone, Debug, NodeValue)]
pub struct ConversationInputs {
    pub messages: Vec<mfn_llm::ChatMessage>,
    pub context: Option<ConversationContext>,
}

impl ConversationInputs {
    pub fn prompt(&self) -> Result<&str, ChatError> {
        match self.messages.last() {
            Some(mfn_llm::ChatMessage::User { content, .. }) if !content.is_empty() => Ok(content),
            _ => InvalidSnafu {
                field: "messages",
                reason: "must end with the current nonempty user message",
            }
            .fail(),
        }
    }

    pub fn validate(&self) -> Result<(), ChatError> {
        self.prompt()?;
        mfn_llm::validate_transcript(&self.messages, mfn_llm::TranscriptMode::Complete)
            .context(TranscriptSnafu { view: "request" })?;
        Ok(())
    }
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
pub struct Completion {
    pub invocation: Invocation,
    pub result: ConversationResult,
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
    pub activity_streams: std::collections::BTreeSet<String>,
    pub streaming: bool,
    inputs: BTreeMap<String, String>,
    pub alternatives: ResultAlternatives,
}

impl ChatContract {
    pub fn has_previews(&self) -> bool {
        self.answer_stream || !self.activity_streams.is_empty()
    }

    pub fn preview_streams(&self) -> impl Iterator<Item = &str> {
        self.answer_stream
            .then_some("answer")
            .into_iter()
            .chain(self.activity_streams.iter().map(String::as_str))
    }

    /// Returns the qualified workflow ports bound to conversation inputs.
    pub fn input_targets(&self) -> impl Iterator<Item = &str> {
        self.inputs.values().map(String::as_str)
    }

    /// Recovers conversation inputs from the qualified arguments of a registered Flow.
    pub fn conversation_inputs(
        &self,
        arguments: &WorkflowArguments,
    ) -> Result<ConversationInputs, ChatError> {
        let values = self
            .inputs
            .iter()
            .filter_map(|(name, target)| {
                arguments
                    .0
                    .get(target)
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect();
        Ok(ConversationInputs::from_values(values)?)
    }

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
                    reason: "requires an unambiguous messages input and optional context; update and recompile the chatflow"
                }
            );
            if let Some((node, port)) = candidates.first() {
                ensure!(
                    port.value_type == expected.value_type && (!expected.required || port.required),
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
        let outputs = &manifest.graph.definition.outputs;
        let append = outputs.iter().find(|output| output.name == "append");
        let replace = outputs
            .iter()
            .find(|output| output.name == "replace_context");
        let alternatives =
            match (append, replace) {
                (Some(append), Some(replace)) => {
                    ensure!(
                        append.optional && replace.optional,
                        InvalidSnafu {
                            field: "chat outputs",
                            reason: "both result alternatives must be optional selections"
                        }
                    );
                    ResultAlternatives::Both
                }
                (Some(_), None) => ResultAlternatives::Append,
                (None, Some(_)) => ResultAlternatives::ReplaceContext,
                (None, None) => return InvalidSnafu {
                    field: "chat outputs",
                    reason: "requires append or replace_context; update and recompile the chatflow",
                }
                .fail(),
            };
        let streaming = manifest.description()?.is_streaming();
        let answer_stream = manifest
            .graph
            .definition
            .outputs
            .iter()
            .any(|output| output.name == "answer");
        let activity_streams = outputs
            .iter()
            .filter(|output| {
                matches!(
                    output.name.as_ref(),
                    "tool_call" | "tool_started" | "tool_finished"
                )
            })
            .map(|output| output.name.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            (!answer_stream && activity_streams.is_empty()) || streaming,
            InvalidSnafu {
                field: "answer stream",
                reason: "requires native streaming execution"
            }
        );
        Ok(Self {
            parameters: configuration,
            answer_stream,
            activity_streams,
            streaming,
            inputs,
            alternatives,
        })
    }

    /// Reuses unambiguous public provider inputs; returned title facts decide actual composition.
    pub fn builtin_title_parameters(
        &self,
        parameters: &WorkflowArguments,
    ) -> Option<WorkflowArguments> {
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
            let text = value.as_str()?;
            if text.trim().is_empty() && !(name == "api_key" && text.is_empty()) {
                return None;
            }
            arguments
                .0
                .insert(mf_runtime::output_id("params", name), value.clone());
        }
        Some(arguments)
    }

    pub fn arguments(
        &self,
        input: crate::ConversationInputs,
        parameters: &WorkflowArguments,
    ) -> Result<WorkflowArguments, ChatError> {
        self.parameters.validate(parameters)?;
        input.validate()?;
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
        if call.terminal_result.is_some() {
            let result = turn::saved_result(&call)?;
            ensure!(result.status == turn.state, ConflictSnafu { id: &call.id });
            return Ok(InvocationState::Committed(Completion {
                invocation: invocation.clone(),
                result,
            }));
        }
        match turn.state.as_str() {
            "running" => Ok(InvocationState::Running),
            "succeeded" => Ok(InvocationState::Finished(turn)),
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
    activities: std::collections::BTreeSet<String>,
    terminal: TerminalResultDecoder,
}

impl ConversationOutputDecoder {
    pub fn new(contract: &ChatContract) -> Self {
        Self {
            answer: contract.answer_stream,
            activities: contract.activity_streams.clone(),
            terminal: TerminalResultDecoder::new(contract.alternatives),
        }
    }

    pub fn accept(
        &mut self,
        mut values: NodeValues,
    ) -> Result<Option<crate::PreviewDelta>, ChatError> {
        let answer = values.remove("answer");
        let activities: Vec<_> = ["tool_call", "tool_started", "tool_finished"]
            .into_iter()
            .filter_map(|name| values.remove(name).map(|value| (name, value)))
            .collect();
        // Count terminal alternatives even when a mixed record carries an invalid preview.
        self.terminal.accept(values)?;
        ensure!(
            activities.len() + usize::from(answer.is_some()) <= 1,
            InvalidSnafu {
                field: "conversation preview",
                reason: "requires one preview event per record"
            }
        );
        if let Some((name, value)) = activities.into_iter().next() {
            ensure!(
                self.activities.contains(name),
                InvalidSnafu {
                    field: "tool activity stream",
                    reason: "was not declared by the workflow"
                }
            );
            let activity =
                <crate::ToolActivity as mf_runtime::InputField>::decode_field(name, Some(value))?;
            ensure!(
                activity.stream_name() == name,
                InvalidSnafu {
                    field: "tool activity stream",
                    reason: "event kind does not match the stream"
                }
            );
            let delta = crate::PreviewDelta::from(activity);
            delta.validate()?;
            return Ok(Some(delta));
        }
        let delta = answer
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
                Ok(delta.into())
            })
            .transpose()?;
        Ok(delta)
    }

    pub fn result(&self) -> Option<&ConversationResult> {
        self.terminal.result()
    }

    pub fn finish(self) -> Result<ConversationResult, ChatError> {
        self.terminal.finish()
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use serde_json::json;

    fn decoder(answer: bool) -> ConversationOutputDecoder {
        ConversationOutputDecoder::new(&ChatContract {
            alternatives: ResultAlternatives::Both,
            parameters: Default::default(),
            answer_stream: answer,
            activity_streams: Default::default(),
            streaming: true,
            inputs: Default::default(),
        })
    }

    fn values(value: serde_json::Value) -> NodeValues {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn tool_previews_require_declared_matching_streams_and_valid_identities() {
        for (name, event) in [
            (
                "tool_call",
                json!({"kind":"call", "model_call_id":"model", "part":0, "arguments":"{"}),
            ),
            (
                "tool_started",
                json!({"kind":"running", "tool_call_id":"id", "name":"lookup"}),
            ),
            (
                "tool_finished",
                json!({"kind":"result", "tool_call_id":"id", "failed":false, "content":""}),
            ),
        ] {
            let record = values(json!({name:event}));
            assert!(decoder(false).accept(record.clone()).is_err());
            let mut declared = decoder(false);
            declared.activities.insert(name.into());
            let preview = declared.accept(record).unwrap().unwrap();
            assert!(matches!(preview, crate::PreviewDelta::Tool { .. }));
        }
        let mut declared = decoder(false);
        declared
            .activities
            .extend(["tool_started".into(), "tool_finished".into()]);
        for record in [
            json!({"tool_started":{"kind":"result","tool_call_id":"id","failed":false,"content":"x"}}),
            json!({"tool_started":{"kind":"running","tool_call_id":"","name":"lookup"}}),
            json!({"tool_finished":{"kind":"result","tool_call_id":" ","failed":false,"content":"x"}}),
            json!({"tool_finished":{"kind":"unknown"}}),
            json!({"answer":{"text":"text","model_call_id":"model"},"tool_finished":{"kind":"result","tool_call_id":"id","failed":false,"content":"x"}}),
        ] {
            assert!(declared.accept(values(record)).is_err());
        }
        let mut conflict = decoder(false);
        conflict.accept(values(json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"first"}],"usages":[]}}))).unwrap();
        assert!(conflict.accept(values(json!({"tool_finished":null,"append":{"status":"succeeded","messages":[{"role":"assistant","content":"other"}],"usages":[]}}))).is_err());
        assert!(conflict.finish().is_err());
    }

    #[test]
    fn native_results_remain_authoritative_when_other_domains_finish_later() {
        let mut decoder = decoder(true);
        decoder
            .accept(values(
                json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"canonical"}],"usages":[]}}),
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
        assert!(matches!(delta, crate::PreviewDelta::Text { delta } if delta.text == "prefix"));
        assert_eq!(decoder.finish().unwrap().final_text(), Some("canonical"));
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
    fn invalid_previews_cannot_hide_a_conflicting_terminal_result() {
        let first = json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"canonical"}],"usages":[]}});
        for preview in [
            json!(null),
            json!("invalid"),
            json!({"text":"","model_call_id":"model"}),
        ] {
            let mut decoder = decoder(true);
            decoder.accept(values(first.clone())).unwrap();
            let mut second = first.clone();
            second["answer"] = preview;
            assert!(decoder.accept(values(second)).is_err());
            assert!(decoder.result().is_none());
            assert!(decoder.finish().is_err());
        }
    }

    #[test]
    fn native_conversation_records_require_one_complete_terminal_result() {
        assert!(matches!(
            decoder(true).finish(),
            Err(ChatError::Capture { .. })
        ));
        let mut decoder = decoder(true);
        let result = json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"canonical"}],"usages":[]}});
        decoder.accept(values(result.clone())).unwrap();
        assert!(matches!(
            decoder.accept(values(result)),
            Err(ChatError::Invalid { .. })
        ));
        assert!(decoder.result().is_none());
        assert!(decoder.finish().is_err());
    }
}
