//! Self-contained branch results and their normalized display/context updates.

use crate::{CallUsage, ChatError, error::*, turn};
use mf_runtime::{
    InputDecodeError, InputField, InputValue, NodeValue, NodeValues, OutputEncodeError,
    OutputValue, PortSpec, TypeMismatch, ValueRef, ValueType,
};
use mfn_llm::{ChatMessage, TranscriptMode, validate_transcript};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageUpdate {
    Append {
        messages: Vec<ChatMessage>,
    },
    ReplaceContext {
        context_messages: Vec<ChatMessage>,
        display_messages: Vec<ChatMessage>,
    },
}

impl MessageUpdate {
    pub fn output_name(&self) -> &'static str {
        match self {
            Self::Append { .. } => "append",
            Self::ReplaceContext { .. } => "replace_context",
        }
    }

    pub fn display_messages(&self) -> &[ChatMessage] {
        match self {
            Self::Append { messages } => messages,
            Self::ReplaceContext {
                display_messages, ..
            } => display_messages,
        }
    }

    pub fn candidate_context(&self, request: &[ChatMessage]) -> Vec<ChatMessage> {
        match self {
            Self::Append { messages } => request.iter().chain(messages).cloned().collect(),
            Self::ReplaceContext {
                context_messages, ..
            } => context_messages.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, mf_runtime::NodeEnum)]
#[serde(rename_all = "snake_case")]
pub enum ResultAlternatives {
    #[value(rename = "append")]
    Append,
    #[value(rename = "replace_context")]
    ReplaceContext,
    #[value(rename = "both")]
    Both,
}

impl ResultAlternatives {
    pub fn allows(self, update: &MessageUpdate) -> bool {
        matches!(
            (self, update),
            (Self::Both, _)
                | (Self::Append, MessageUpdate::Append { .. })
                | (Self::ReplaceContext, MessageUpdate::ReplaceContext { .. })
        )
    }

    pub fn validate(self, result: &ConversationResult) -> Result<(), ChatError> {
        ensure!(
            self.allows(&result.update),
            InvalidSnafu {
                field: "workflow result",
                reason: "selected an undeclared result alternative"
            }
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationResult {
    pub status: String,
    pub update: MessageUpdate,
    pub usages: Vec<CallUsage>,
    pub error: Option<String>,
    pub title: Option<String>,
}

#[derive(NodeValue)]
struct AppendResult {
    status: String,
    messages: Vec<ChatMessage>,
    usages: Vec<CallUsage>,
    error: Option<String>,
    title: Option<String>,
}

#[derive(NodeValue)]
struct ReplacementResult {
    status: String,
    context_messages: Vec<ChatMessage>,
    display_messages: Vec<ChatMessage>,
    usages: Vec<CallUsage>,
    error: Option<String>,
    title: Option<String>,
}

impl Default for ConversationResult {
    fn default() -> Self {
        Self::append(Vec::new())
    }
}

impl ConversationResult {
    pub fn append(messages: Vec<ChatMessage>) -> Self {
        Self {
            status: "succeeded".into(),
            update: MessageUpdate::Append { messages },
            usages: Vec::new(),
            error: None,
            title: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::append(vec![ChatMessage::Assistant {
            content: text.into(),
            tool_calls: None,
            metadata: None,
        }])
    }

    pub fn display_messages(&self) -> &[ChatMessage] {
        self.update.display_messages()
    }

    pub fn final_text(&self) -> Option<&str> {
        match self.display_messages().last() {
            Some(ChatMessage::Assistant { content, .. }) => Some(content),
            _ => None,
        }
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, ChatError> {
        ensure!(
            bytes.len() <= super::MAX_RECOVERY_BYTES,
            RecoveryTooLargeSnafu {
                limit: super::MAX_RECOVERY_BYTES
            }
        );
        Self::decode(mf_runtime::decode_unique_json(bytes).context(JsonSnafu)?)
    }

    pub fn decode(mut values: NodeValues) -> Result<Self, ChatError> {
        ensure!(
            values.contains_key("append") != values.contains_key("replace_context"),
            InvalidSnafu {
                field: "workflow result",
                reason: "requires exactly one append or replace_context result"
            }
        );
        values.retain(|name, _| matches!(name.as_str(), "append" | "replace_context"));
        let result = Self::from_values(values)?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), ChatError> {
        turn::validate_usages(&self.usages)?;
        let mode = match self.status.as_str() {
            "succeeded" => {
                ensure!(
                    self.error.as_ref().is_none_or(String::is_empty)
                        && matches!(self.display_messages().last(),
                            Some(ChatMessage::Assistant { content, tool_calls, .. })
                                if !content.is_empty() && tool_calls.as_ref().is_none_or(Vec::is_empty)),
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "success requires a final nonempty assistant answer without calls or an error"
                    }
                );
                TranscriptMode::Complete
            }
            "failed" => {
                ensure!(
                    self.title.is_none()
                        && self
                            .error
                            .as_ref()
                            .is_some_and(|error| !error.trim().is_empty()),
                    InvalidSnafu {
                        field: "workflow result",
                        reason: "failure requires an error and cannot update the title"
                    }
                );
                TranscriptMode::Partial
            }
            _ => {
                return InvalidSnafu {
                    field: "workflow result",
                    reason: "requires succeeded or failed status",
                }
                .fail();
            }
        };
        ensure!(
            self.display_messages().iter().all(|message| matches!(
                message,
                ChatMessage::Assistant { .. } | ChatMessage::ToolResult { .. }
            )),
            InvalidSnafu {
                field: "display messages",
                reason: "must contain only newly generated assistant and tool messages"
            }
        );
        validate_transcript(self.display_messages(), mode)
            .context(TranscriptSnafu { view: "display" })?;
        if let MessageUpdate::ReplaceContext {
            context_messages, ..
        } = &self.update
        {
            validate_transcript(context_messages, TranscriptMode::Complete).context(
                TranscriptSnafu {
                    view: "model context",
                },
            )?;
        }
        Ok(())
    }

    pub fn validate_against(&self, request: &[ChatMessage]) -> Result<(), ChatError> {
        self.validate()?;
        if let MessageUpdate::Append { .. } = &self.update {
            let mode = if self.status == "succeeded" {
                TranscriptMode::Complete
            } else {
                TranscriptMode::Partial
            };
            validate_transcript(&self.update.candidate_context(request), mode).context(
                TranscriptSnafu {
                    view: "append context",
                },
            )?;
        }
        Ok(())
    }
}

impl NodeValue for ConversationResult {
    fn ports() -> Vec<PortSpec> {
        ["append", "replace_context"]
            .map(|name| PortSpec::new(name, ValueType::Object, false))
            .into()
    }

    fn from_values(mut values: NodeValues) -> Result<Self, InputDecodeError> {
        let result = if values.contains_key("append") {
            let result: AppendResult = mf_runtime::decode_input(&mut values, "append")?;
            Self {
                status: result.status,
                update: MessageUpdate::Append {
                    messages: result.messages,
                },
                usages: result.usages,
                error: result.error,
                title: result.title,
            }
        } else {
            let result: ReplacementResult =
                mf_runtime::decode_input(&mut values, "replace_context")?;
            Self {
                status: result.status,
                update: MessageUpdate::ReplaceContext {
                    context_messages: result.context_messages,
                    display_messages: result.display_messages,
                },
                usages: result.usages,
                error: result.error,
                title: result.title,
            }
        };
        mf_runtime::reject_unknown_inputs(values)?;
        Ok(result)
    }

    fn into_values(self) -> Result<NodeValues, OutputEncodeError> {
        let Self {
            status,
            update,
            usages,
            error,
            title,
        } = self;
        let mut values = NodeValues::new();
        match update {
            MessageUpdate::Append { messages } => {
                mf_runtime::encode_output(
                    &mut values,
                    "append",
                    AppendResult {
                        status,
                        messages,
                        usages,
                        error,
                        title,
                    },
                )?;
            }
            MessageUpdate::ReplaceContext {
                context_messages,
                display_messages,
            } => {
                mf_runtime::encode_output(
                    &mut values,
                    "replace_context",
                    ReplacementResult {
                        status,
                        context_messages,
                        display_messages,
                        usages,
                        error,
                        title,
                    },
                )?;
            }
        }
        Ok(values)
    }
}

impl mf_runtime::NodeInputs for ConversationResult {
    fn ports() -> Vec<PortSpec> {
        <Self as NodeValue>::ports()
    }
    fn from_inputs(values: NodeValues) -> Result<Self, InputDecodeError> {
        Self::from_values(values)
    }
}

impl mf_runtime::NodeOutputs for ConversationResult {
    fn ports() -> Vec<PortSpec> {
        <Self as NodeValue>::ports()
    }
    fn into_outputs(self) -> Result<NodeValues, OutputEncodeError> {
        self.into_values()
    }
}

impl InputValue for ConversationResult {
    fn value_type() -> ValueType {
        ValueType::Object
    }
    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        mf_runtime::decode_node_value(value)
    }
}

impl InputField for ConversationResult {
    const REQUIRED: bool = true;
    fn value_type() -> ValueType {
        ValueType::Object
    }
    fn decode_field(port: &str, value: Option<ValueRef>) -> Result<Self, InputDecodeError> {
        mf_runtime::decode_required_input(port, value)
    }
}

impl OutputValue for ConversationResult {
    fn value_type() -> ValueType {
        ValueType::Object
    }
    fn encode(self) -> Result<ValueRef, TypeMismatch> {
        mf_runtime::encode_node_value(self)
    }
}

impl Serialize for ConversationResult {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.clone()
            .into_values()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConversationResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_values(NodeValues::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Accumulates one terminal result while invalidating every ambiguous recovery candidate.
pub struct TerminalResultDecoder {
    alternatives: ResultAlternatives,
    result: Option<ConversationResult>,
    invalid: bool,
}

impl TerminalResultDecoder {
    pub fn new(alternatives: ResultAlternatives) -> Self {
        Self {
            alternatives,
            result: None,
            invalid: false,
        }
    }

    pub fn accept(&mut self, values: NodeValues) -> Result<(), ChatError> {
        if !values
            .keys()
            .any(|name| matches!(name.as_str(), "append" | "replace_context"))
        {
            return Ok(());
        }
        if self.invalid || self.result.is_some() {
            self.result = None;
            self.invalid = true;
            return InvalidSnafu {
                field: "workflow result",
                reason: "must be produced exactly once across the invocation",
            }
            .fail();
        }
        match ConversationResult::decode(values).and_then(|result| {
            self.alternatives.validate(&result)?;
            Ok(result)
        }) {
            Ok(result) => self.result = Some(result),
            Err(error) => {
                self.invalid = true;
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn result(&self) -> Option<&ConversationResult> {
        self.result.as_ref()
    }

    pub fn finish(self) -> Result<ConversationResult, ChatError> {
        ensure!(
            !self.invalid,
            InvalidSnafu {
                field: "workflow result",
                reason: "result selection is invalid or ambiguous"
            }
        );
        self.result.context(CaptureSnafu {
            message: "workflow ended without a terminal conversation result",
        })
    }
}
