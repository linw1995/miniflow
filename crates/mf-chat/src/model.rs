use crate::{ChatError, Reply, error::*};
use mf_runtime::{ExecutionContext, NodeValue, TaskNode, TypedNodeResult, TypedTaskNode};
use snafu::OptionExt;

#[derive(Clone, Debug, NodeValue)]
pub struct ModelRequest {
    pub request: mfn_llm::LlmInputs,
}

#[derive(Clone, Debug, NodeValue)]
pub struct ModelResponse {
    pub response: Reply,
}

// Object envelopes preserve absent optional fields without routing missing output ports.
struct Model {
    node: Box<dyn TaskNode>,
}

impl TypedTaskNode for Model {
    type Input = ModelRequest;
    type Output = ModelResponse;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        let result = self.node.execute(
            input.request.into_values().map_err(ChatError::from)?,
            context,
        )?;
        Ok(ModelResponse {
            response: mfn_llm::LlmOutputs::from_values(result.outputs)
                .map_err(ChatError::from)?
                .into(),
        }
        .into())
    }
}

pub fn prepare(
    config: serde_json::Value,
) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let node = mfn_llm::prepare_llm(config)?
        .execution
        .into_task_node()
        .context(InvalidSnafu {
            field: "LLM node",
            reason: "must prepare a task",
        })?;
    mf_runtime::PreparedNode::typed_task(Model { node }, mf_runtime::NodeMetadata::default())
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.llm", factory: mf_runtime::NodeFactory::Plain(prepare),
    }
}
