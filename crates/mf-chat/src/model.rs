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

struct Model {
    node: Box<dyn TaskNode>,
}

// Object envelopes preserve absent optional fields without routing missing output ports.

fn call(
    node: &dyn TaskNode,
    request: mfn_llm::LlmInputs,
    context: &mut ExecutionContext,
) -> Result<Reply, ChatError> {
    let result = node.execute(request.into_values()?, context)?;
    Ok(mfn_llm::LlmOutputs::from_values(result.outputs)?.into())
}

impl TypedTaskNode for Model {
    type Input = ModelRequest;
    type Output = ModelResponse;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        Ok(ModelResponse {
            response: call(self.node.as_ref(), input.request, context)?,
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
