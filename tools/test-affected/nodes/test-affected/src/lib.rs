//! Git facts and conservative test plans for repository development.

mod classify;
mod error;
pub mod git;
pub mod model;
mod plan;

pub use error::Error;
pub use model::*;
pub use plan::make_plan;

use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, ValueRef, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt;
use std::path::Path;

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    pub comparison_mode: ComparisonMode,
    pub initialization_options: Value,
}

#[derive(NodeValue)]
pub struct GitInput {
    pub repository: String,
    pub base: String,
    pub head: Option<String>,
    pub artifacts: Option<String>,
    pub options: Option<ValueRef>,
}

pub struct GitNode {
    pub config: GitConfig,
}

impl TypedTaskNode for GitNode {
    type Input = GitInput;
    type Output = mfn_lsp::AnalyzeInput;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let comparison = git::compare_with_mode(
            Path::new(&input.repository),
            &input.base,
            input.head.as_deref().unwrap_or("HEAD"),
            input.artifacts.as_deref().map(Path::new),
            self.config.initialization_options.clone(),
            self.config.comparison_mode,
        )?;
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let request = serde_json::to_value(&comparison.request)
            .context(error::JsonSnafu {
                action: "encode analysis request",
            })?
            .into();
        let comparison = serde_json::to_value(comparison)
            .context(error::JsonSnafu {
                action: "encode Git comparison",
            })?
            .into();
        Ok(mfn_lsp::AnalyzeInput {
            request,
            context: ValueRef::object([
                ("comparison".into(), comparison),
                (
                    "options".into(),
                    input
                        .options
                        .unwrap_or_else(|| serde_json::json!({}).into()),
                ),
            ]),
        }
        .into())
    }
}

#[derive(NodeValue)]
pub struct PlanOutput {
    pub plan: ValueRef,
}

pub struct PlanNode;

impl TypedTaskNode for PlanNode {
    type Input = mfn_lsp::AnalyzeOutput;
    type Output = PlanOutput;

    fn execute(
        &self,
        input: Self::Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let comparison = serde_json::from_value(
            serde_json::to_value(&input.context["comparison"]).context(error::JsonSnafu {
                action: "decode comparison value",
            })?,
        )
        .context(error::JsonSnafu {
            action: "decode Git comparison",
        })?;
        let options = serde_json::from_value(
            serde_json::to_value(&input.context["options"]).context(error::JsonSnafu {
                action: "decode plan option value",
            })?,
        )
        .context(error::JsonSnafu {
            action: "decode plan options",
        })?;
        let outcome = serde_json::from_value(serde_json::to_value(&input.outcome).context(
            error::JsonSnafu {
                action: "decode analysis value",
            },
        )?)
        .context(error::JsonSnafu {
            action: "decode analysis outcome",
        })?;
        let plan = make_plan(&comparison, &outcome, &options)?;
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        Ok(PlanOutput {
            plan: ValueRef::from(serde_json::to_value(plan).context(error::JsonSnafu {
                action: "encode test plan",
            })?)
            .inherit_secrets([&input.context, &input.outcome]),
        }
        .into())
    }
}

fn git_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::typed_task(
        GitNode {
            config: deserialize_config(config)?,
        },
        NodeMetadata::default(),
    )
}
fn plan_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(PlanNode, NodeMetadata::default())
}

inventory::submit! { NodeRegistration { kind: "git.diff", factory: mf_runtime::NodeFactory::Plain(git_factory) } }
inventory::submit! { NodeRegistration { kind: "test_affected.plan", factory: mf_runtime::NodeFactory::Plain(plan_factory) } }
