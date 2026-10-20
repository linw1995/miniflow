//! Input-driven language-server analysis for repository developer flows.

mod analysis;
mod error;
pub mod model;
mod protocol;

pub use analysis::{analyze, byte_offset, validate_config};
pub use error::Error;
pub use model::*;

use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, OutputDerivation, PreparedNode, TypedNodeResult, TypedStreamNode, ValueRef,
    deserialize_config,
};
use serde_json::Value;
use snafu::ResultExt;

pub const KIND: &str = "lsp.analyze";

#[derive(NodeValue)]
pub struct AnalyzeInput {
    pub request: ValueRef,
    pub context: ValueRef,
}

#[derive(NodeValue)]
pub struct AnalyzeOutput {
    pub outcome: ValueRef,
    pub context: ValueRef,
}

pub struct AnalyzeNode {
    config: ServerConfig,
}

impl AnalyzeNode {
    pub fn new(config: ServerConfig) -> Result<Self, Error> {
        validate_config(&config)?;
        Ok(Self { config })
    }
}

impl TypedStreamNode for AnalyzeNode {
    type Input = AnalyzeInput;
    type Output = AnalyzeOutput;

    fn execute(
        &mut self,
        input: Self::Input,
        context: &mut ExecutionContext,
        emit: &mut dyn FnMut(TypedNodeResult<Self::Output>) -> Result<(), NodeExecutionError>,
    ) -> Result<(), NodeExecutionError> {
        let request =
            serde_json::from_value(serde_json::to_value(&input.request).context(error::JsonSnafu)?)
                .context(error::JsonSnafu)?;
        let outcome = analyze(&self.config, request, context.cancellation())?;
        emit(
            AnalyzeOutput {
                outcome: ValueRef::from(serde_json::to_value(outcome).context(error::JsonSnafu)?)
                    .inherit_secrets([&input.request]),
                context: input.context,
            }
            .into(),
        )
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let node = AnalyzeNode::new(deserialize_config(config)?)?;
    PreparedNode::typed_stream(
        node,
        NodeMetadata {
            output_derivations: vec![OutputDerivation::forward_input("context", "context")],
            ..Default::default()
        },
    )
}

inventory::submit! { NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) } }
