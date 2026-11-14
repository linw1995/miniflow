//! Publish immutable message facts, independently of a Flow's final authoritative context.

use crate::ChatMessage;
use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeFactory, NodeMetadata, NodeRegistration, NodeValue,
    PreparedNode, Shared, TypedEmitter, TypedNodeResult, TypedTaskStreamNode, ValueRef,
};
use snafu::{ResultExt, Snafu, ensure};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct MessageEvent {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_call_id: Option<String>,
    pub messages: Vec<ChatMessage>,
}
#[derive(NodeValue)]
struct MessageInputs {
    messages: Shared<Vec<ChatMessage>>,
    model_call_id: Option<String>,
}
#[derive(NodeValue)]
struct MessageEmission {
    event: Shared<MessageEvent>,
}
#[derive(NodeValue)]
struct MessageOutputs {
    done: bool,
}

#[derive(Debug, Snafu)]
#[snafu(display("message events require messages and a nonblank optional model identity"))]
struct InvalidMessageEvent;
impl From<InvalidMessageEvent> for NodeExecutionError {
    fn from(source: InvalidMessageEvent) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}
struct PublishMessages;
impl TypedTaskStreamNode for PublishMessages {
    type Input = MessageInputs;
    type Output = MessageOutputs;
    type Emission = MessageEmission;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Self::Emission>,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let messages = input.messages.decode().boxed()?;
        ensure!(
            !messages.is_empty()
                && input
                    .model_call_id
                    .as_ref()
                    .is_none_or(|id| !id.trim().is_empty()),
            InvalidMessageEventSnafu
        );
        let id = uuid::Uuid::new_v4().to_string();
        let mut fields = vec![
            ("id".into(), ValueRef::from(id)),
            // Preserve native sensitivity; do not decode/re-encode business data.
            ("messages".into(), input.messages.into_value()),
        ];
        if let Some(model) = input.model_call_id {
            fields.push(("model_call_id".into(), model.into()));
        }
        emitter.send(MessageEmission {
            event: Shared::new(ValueRef::object(fields)).boxed()?,
        })?;
        Ok(MessageOutputs { done: true }.into())
    }
}
inventory::submit! { NodeRegistration { kind: "llm.message_events", factory: NodeFactory::Plain(|_| PreparedNode::typed_task_stream(PublishMessages, NodeMetadata::default())) } }
