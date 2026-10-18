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

pub fn failure_usages(
    source: &(dyn std::error::Error + 'static),
) -> Option<Vec<mfn_llm::CallUsage>> {
    let mut current = Some(source);
    while let Some(source) = current {
        if let Some(ChatError::ModelFailure { usages, .. }) = source.downcast_ref::<ChatError>() {
            return Some(usages.clone());
        }
        current = source.source();
    }
    None
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
        let response = mfn_llm::LlmOutputs::from_values(result.outputs).map_err(ChatError::from)?;
        if let Some(message) = response.error {
            return Err(ModelFailureSnafu {
                message,
                usages: response.usages,
            }
            .build()
            .into());
        }
        Ok(ModelResponse {
            response: response.into(),
        }
        .into())
    }
}

pub fn prepare(
    config: serde_json::Value,
) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let prepared = mfn_llm::prepare_llm(config)?;
    let metadata = mf_runtime::NodeMetadata {
        context_references: prepared.metadata.context_references,
        ..Default::default()
    };
    let node = prepared.execution.into_task_node().context(InvalidSnafu {
        field: "LLM node",
        reason: "must prepare a task",
    })?;
    mf_runtime::PreparedNode::typed_task(Model { node }, metadata)
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "mf_chat.llm", factory: mf_runtime::NodeFactory::Plain(prepare),
    }
}
