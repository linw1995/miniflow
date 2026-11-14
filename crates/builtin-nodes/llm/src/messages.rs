//! Publish explicitly selected, current-invocation messages from an Agent Flow.

use crate::ChatMessage;
use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeFactory, NodeMetadata, NodeRegistration, NodeValue,
    PreparedNode, Shared, TypedEmitter, TypedNodeResult, TypedTaskStreamNode,
};
use snafu::{ResultExt, Snafu, ensure};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, NodeValue)]
pub struct MessageBatch {
    pub offset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_call_id: Option<String>,
    pub messages: Vec<ChatMessage>,
}
#[derive(NodeValue)]
struct MessageInputs {
    batch: Shared<MessageBatch>,
}
#[derive(NodeValue)]
struct MessageEmission {
    event: Shared<MessageBatch>,
}
#[derive(NodeValue)]
struct MessageOutputs {
    done: bool,
}

#[derive(Debug, Snafu)]
#[snafu(display("message events require a nonnegative offset and nonblank model identity"))]
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
        let batch = input.batch.decode().boxed()?;
        ensure!(
            batch.offset >= 0
                && batch
                    .model_call_id
                    .as_ref()
                    .is_none_or(|id| !id.trim().is_empty()),
            InvalidMessageEventSnafu
        );
        // Forward the checked value, not decoded owned strings: re-encoding would discard sensitivity labels.
        emitter.send(MessageEmission { event: input.batch })?;
        Ok(MessageOutputs { done: true }.into())
    }
}
inventory::submit! { NodeRegistration { kind: "llm.message_events", factory: NodeFactory::Plain(|_| PreparedNode::typed_task_stream(PublishMessages, NodeMetadata::default())) } }
