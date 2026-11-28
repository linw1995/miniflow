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

pub const DEFAULT_COMPACTION_CONTEXT_WINDOW: i64 = 128_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct CompactionConfig {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    pub reserve_tokens: i64,
    pub keep_recent_tokens: i64,
    pub custom_instructions: String,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            context_window: None,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            custom_instructions: String::new(),
        }
    }
}

impl CompactionConfig {
    pub fn validate(&self) -> Result<(), ChatError> {
        ensure!(
            self.context_window.is_none_or(|value| value >= 0)
                && self.reserve_tokens >= 0
                && self.keep_recent_tokens >= 0,
            InvalidSnafu {
                field: "automatic_compaction",
                reason: "context_window, reserve_tokens, and keep_recent_tokens must be non-negative"
            }
        );
        Ok(())
    }

    pub fn resolved_context(&self, discovered_context_window: Option<i64>) -> CompactionContext {
        CompactionContext {
            enabled: self.enabled,
            context_window: self
                .context_window
                .or(discovered_context_window)
                .unwrap_or(DEFAULT_COMPACTION_CONTEXT_WINDOW),
            reserve_tokens: self.reserve_tokens,
            keep_recent_tokens: self.keep_recent_tokens,
            custom_instructions: self.custom_instructions.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct CompactionContext {
    pub enabled: bool,
    pub context_window: i64,
    pub reserve_tokens: i64,
    pub keep_recent_tokens: i64,
    pub custom_instructions: String,
}

impl Default for CompactionContext {
    fn default() -> Self {
        Self::from(&CompactionConfig::default())
    }
}

impl From<&CompactionConfig> for CompactionContext {
    fn from(config: &CompactionConfig) -> Self {
        config.resolved_context(None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, NodeValue)]
pub struct ConversationContext {
    pub title: TitleContext,
    pub model_context: ModelContextRevision,
    #[serde(default)]
    pub compaction: CompactionContext,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderParameterPorts {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ChatContract {
    pub parameters: WorkflowInputSchema,
    /// The user workflow declares an unambiguous Agent reply callback.
    pub supports_steering: bool,
    pub answer_stream: bool,
    pub tool_call_delta_stream: bool,
    pub tool_output_stream: bool,
    pub streaming: bool,
    inputs: BTreeMap<String, String>,
    pub alternatives: ResultAlternatives,
}

impl ChatContract {
    pub fn has_previews(&self) -> bool {
        self.answer_stream || self.tool_call_delta_stream || self.tool_output_stream
    }

    pub fn preview_streams(&self) -> impl Iterator<Item = &'static str> {
        [
            self.answer_stream.then_some("answer"),
            self.tool_call_delta_stream.then_some("tool_call_delta"),
            self.tool_output_stream.then_some("tool_output"),
        ]
        .into_iter()
        .flatten()
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
            manifest.descriptor.input_schema.stdin.is_empty(),
            InvalidSnafu {
                field: "chat executable",
                reason: "requires workflow execution without stdin"
            }
        );
        let mut configuration = manifest.descriptor.input_schema.clone();
        let mut inputs = BTreeMap::new();
        for expected in crate::ConversationInputs::ports() {
            let candidates: Vec<_> = manifest
                .descriptor
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
        let outputs = &manifest.definition.outputs;
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
        let answer_stream = outputs.iter().any(|output| output.name == "answer");
        let tool_call_delta_stream = outputs
            .iter()
            .any(|output| output.name == "tool_call_delta");
        let tool_output_stream = outputs.iter().any(|output| output.name == "tool_output");
        ensure!(
            (!answer_stream && !tool_call_delta_stream && !tool_output_stream) || streaming,
            InvalidSnafu {
                field: "conversation preview",
                reason: "requires native streaming execution"
            }
        );
        Ok(Self {
            parameters: configuration,
            answer_stream,
            tool_call_delta_stream,
            tool_output_stream,
            streaming,
            supports_steering: manifest
                .descriptor
                .callbacks
                .contains_key(crate::steering::CALLBACK_NAME),
            inputs,
            alternatives,
        })
    }

    /// Locates unambiguous startup parameter ports for provider endpoint and model selection.
    pub fn provider_parameter_ports(&self) -> Option<ProviderParameterPorts> {
        for (model_name, base_url_name, api_key_name) in [
            ("model", "base_url", "api_key"),
            ("llm_model", "llm_base_url", "llm_api_key"),
        ] {
            let Ok(Some(model)) = self.unique_string_parameter(model_name) else {
                continue;
            };
            let Ok(Some(base_url)) = self.unique_string_parameter(base_url_name) else {
                continue;
            };
            let Ok(api_key) = self.unique_string_parameter(api_key_name) else {
                continue;
            };
            return Some(ProviderParameterPorts {
                model,
                base_url,
                api_key,
            });
        }
        None
    }

    fn unique_string_parameter(&self, name: &str) -> Result<Option<String>, ()> {
        let mut candidates = self
            .parameters
            .inputs
            .iter()
            .filter_map(|(node, ports)| ports.get(name).map(|port| (node, port)));
        let Some((node, port)) = candidates.next() else {
            return Ok(None);
        };
        if candidates.next().is_some() || port.value_type != mf_runtime::ValueType::String {
            return Err(());
        }
        Ok(Some(mf_runtime::output_id(node, name)))
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

    /// Reuses unambiguous public provider inputs for host-managed compaction when the Flow does
    /// not declare its own `replace_context` output.
    pub fn builtin_compaction_parameters(
        &self,
        parameters: &WorkflowArguments,
    ) -> Option<WorkflowArguments> {
        if self.alternatives != ResultAlternatives::Append {
            return None;
        }
        self.builtin_title_parameters(parameters)
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
    tool_call_delta: bool,
    tool_output: bool,
    terminal: TerminalResultDecoder,
}

impl ConversationOutputDecoder {
    pub fn new(contract: &ChatContract) -> Self {
        Self {
            answer: contract.answer_stream,
            tool_call_delta: contract.tool_call_delta_stream,
            tool_output: contract.tool_output_stream,
            terminal: TerminalResultDecoder::new(contract.alternatives),
        }
    }

    pub fn accept(
        &mut self,
        mut values: NodeValues,
    ) -> Result<Option<crate::PreviewDelta>, ChatError> {
        let answer = values.remove("answer");
        let tool_call_delta = values.remove("tool_call_delta");
        let tool_output = values.remove("tool_output");
        // Count terminal alternatives even when a mixed record carries an invalid preview.
        self.terminal.accept(values)?;
        let delta = match (answer, tool_call_delta, tool_output) {
            (None, None, None) => return Ok(None),
            (None, None, Some(value)) => {
                ensure!(
                    self.tool_output,
                    InvalidSnafu {
                        field: "tool_output stream",
                        reason: "was not declared by the workflow"
                    }
                );
                crate::PreviewDelta::ToolOutput {
                    delta: <crate::ToolOutputUpdate as mf_runtime::InputField>::decode_field(
                        "tool_output",
                        Some(value),
                    )?,
                }
            }
            (Some(value), None, None) => {
                ensure!(
                    self.answer,
                    InvalidSnafu {
                        field: "answer stream",
                        reason: "was not declared by the workflow"
                    }
                );
                crate::PreviewDelta::from(decode_answer_delta(&value)?)
            }
            (None, Some(value), None) => {
                ensure!(
                    self.tool_call_delta,
                    InvalidSnafu {
                        field: "tool_call_delta stream",
                        reason: "was not declared by the workflow"
                    }
                );
                let delta = <crate::ToolCallDelta as mf_runtime::InputField>::decode_field(
                    "tool_call_delta",
                    Some(value),
                )?;
                crate::PreviewDelta::from(delta)
            }
            _ => {
                return InvalidSnafu {
                    field: "conversation preview",
                    reason: "requires one preview event per record",
                }
                .fail();
            }
        };
        delta.validate()?;
        Ok(Some(delta))
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
            tool_call_delta_stream: false,
            tool_output_stream: false,
            streaming: true,
            supports_steering: false,
            inputs: Default::default(),
        })
    }

    fn values(value: serde_json::Value) -> NodeValues {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn tool_call_previews_require_a_declared_stream_and_safe_event_shapes() {
        let fragment = json!({"model_call_id":"model", "index":0, "arguments":"{"});
        let record = values(json!({"tool_call_delta":fragment}));
        assert!(decoder(false).accept(record.clone()).is_err());
        let mut declared = decoder(true);
        declared.tool_call_delta = true;
        let preview = declared.accept(record).unwrap().unwrap();
        assert!(
            matches!(preview, crate::PreviewDelta::ToolCall { delta } if delta.arguments == "{")
        );
        let hint = declared
            .accept(values(json!({"tool_call_delta":{"model_call_id":"model","index":0,"arguments":"","id":"hint"}})))
            .unwrap()
            .unwrap();
        assert!(
            matches!(hint, crate::PreviewDelta::ToolCall { delta } if delta.arguments.is_empty() && delta.id.as_deref() == Some("hint"))
        );
        for event in [
            json!({"model_call_id":" ","index":0,"arguments":"{"}),
            json!({"model_call_id":"model","index":0,"arguments":""}),
            json!({"model_call_id":"model","index":0,"arguments":"","id":" "}),
        ] {
            assert!(matches!(
                declared.accept(values(json!({"tool_call_delta":event}))),
                Err(ChatError::Invalid { .. })
            ));
        }
        for event in [
            json!({"kind":"not a delta"}),
            json!({"model_call_id":"model","index":-1,"arguments":"{"}),
            json!({"model_call_id":"model","index":0,"arguments":{}}),
            json!({"model_call_id":"model","index":0,"arguments":"{","unexpected":true}),
        ] {
            assert!(matches!(
                declared.accept(values(json!({"tool_call_delta":event}))),
                Err(ChatError::Decode { .. })
            ));
        }
        assert!(declared.accept(values(json!({"answer":{"text":"text","model_call_id":"model"},"tool_call_delta":fragment}))).is_err());
        let terminal = json!({"append":{"status":"succeeded","messages":[{"role":"assistant","content":"canonical"}],"usages":[]}});
        declared.accept(values(terminal.clone())).unwrap();
        let mut conflict = terminal;
        conflict["tool_call_delta"] = json!(null);
        assert!(declared.accept(values(conflict)).is_err());
        assert!(declared.result().is_none());
        assert!(declared.finish().is_err());
    }

    #[test]
    fn tool_output_requires_an_advertised_bounded_snapshot_and_actual_call_identities() {
        let update = json!({"context":{"model_call_id":"m","tool_call_id":"t","name":"bash"}, "value":"latest"});
        let record = |u| values(json!({"tool_output":u}));
        let mut decoder = decoder(false);
        assert!(decoder.accept(record(update.clone())).is_err());
        decoder.tool_output = true;
        assert!(
            matches!(decoder.accept(record(update.clone())).unwrap(), Some(crate::PreviewDelta::ToolOutput { delta }) if delta.value == "latest")
        );
        for field in ["model_call_id", "tool_call_id", "name"] {
            let mut invalid = update.clone();
            invalid["context"][field] = " ".into();
            assert!(matches!(
                decoder.accept(record(invalid)),
                Err(ChatError::Invalid { .. })
            ));
        }
        for invalid in [String::new(), "a".repeat(16 * 1024 + 1)] {
            let mut value = update.clone();
            value["value"] = invalid.into();
            assert!(decoder.accept(record(value)).is_err());
        }
        assert!(matches!(
            decoder.accept(record(json!({"value":"no identity"}))),
            Err(ChatError::Decode { .. })
        ));
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

    #[test]
    fn provider_parameter_ports_recognizes_standard_and_llm_prefixed_triples() {
        let contract_with_inputs = |inputs: serde_json::Value| ChatContract {
            parameters: serde_json::from_value(json!({ "inputs": inputs, "stdin": {} })).unwrap(),
            answer_stream: false,
            tool_call_delta_stream: false,
            tool_output_stream: false,
            streaming: false,
            supports_steering: false,
            inputs: BTreeMap::new(),
            alternatives: ResultAlternatives::Append,
        };

        let standard = contract_with_inputs(json!({
            "params": {
                "model": {"type": "string", "required": true},
                "base_url": {"type": "string", "required": true},
                "api_key": {"type": "string", "required": false}
            }
        }));
        assert_eq!(
            standard.provider_parameter_ports(),
            Some(ProviderParameterPorts {
                model: "params.model".into(),
                base_url: "params.base_url".into(),
                api_key: Some("params.api_key".into()),
            })
        );

        let keyless_prefixed = contract_with_inputs(json!({
            "params": {
                "llm_model": {"type": "string", "required": true},
                "llm_base_url": {"type": "string", "required": true}
            }
        }));
        assert_eq!(
            keyless_prefixed.provider_parameter_ports(),
            Some(ProviderParameterPorts {
                model: "params.llm_model".into(),
                base_url: "params.llm_base_url".into(),
                api_key: None,
            })
        );

        let ambiguous = contract_with_inputs(json!({
            "a": {"model": {"type": "string", "required": true}},
            "b": {
                "model": {"type": "string", "required": true},
                "base_url": {"type": "string", "required": true}
            }
        }));
        assert_eq!(ambiguous.provider_parameter_ports(), None);

        let missing_base_url = contract_with_inputs(json!({
            "params": {"model": {"type": "string", "required": true}}
        }));
        assert_eq!(missing_base_url.provider_parameter_ports(), None);

        let ambiguous_api_key = contract_with_inputs(json!({
            "params": {
                "model": {"type": "string", "required": true},
                "base_url": {"type": "string", "required": true},
                "api_key": {"type": "string", "required": false}
            },
            "other": {
                "api_key": {"type": "string", "required": false}
            }
        }));
        assert_eq!(ambiguous_api_key.provider_parameter_ports(), None);
    }
}
