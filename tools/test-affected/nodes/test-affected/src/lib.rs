//! Git facts and conservative test plans for repository development.

mod classify;
mod error;
mod evidence;
pub mod git;
mod git_evidence;
mod github;
pub mod model;
mod plan;
#[cfg(unix)]
mod process;

pub use error::Error;
pub use model::*;
pub use plan::make_plan;

use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration,
    NodeValue, PreparedNode, TypedNodeResult, TypedTaskNode, ValueRef, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, ensure};
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
    pub base: Option<String>,
    pub head: Option<String>,
    pub request: Option<ValueRef>,
    pub artifacts: Option<mf_runtime::Nullable<String>>,
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
        let sensitivity = input
            .request
            .iter()
            .chain(input.options.iter())
            .cloned()
            .collect::<Vec<_>>();
        let normalized = if let Some(value) = input.request {
            ensure!(
                input.base.is_none() && input.head.is_none() && input.options.is_none(),
                error::InvalidSnafu {
                    message: "request cannot be combined with legacy base/head/options inputs"
                }
            );
            serde_json::from_value::<ComparisonRequest>(serde_json::to_value(value).context(
                error::JsonSnafu {
                    action: "encode comparison request",
                },
            )?)
            .context(error::JsonSnafu {
                action: "decode comparison request",
            })?
        } else {
            ComparisonRequest {
                mode: self.config.comparison_mode,
                base: input.base,
                head: input.head.unwrap_or_else(|| "HEAD".into()),
                analyze: true,
                require_checkout: false,
                initialization_options: None,
                options: input
                    .options
                    .map(|value| {
                        serde_json::from_value(serde_json::to_value(value).context(
                            error::JsonSnafu {
                                action: "encode planning options",
                            },
                        )?)
                        .context(error::JsonSnafu {
                            action: "decode planning options",
                        })
                    })
                    .transpose()?
                    .unwrap_or_default(),
                validation: Value::Null,
            }
        };
        let comparison = git::compare_request(
            Path::new(&input.repository),
            &normalized,
            match &input.artifacts {
                Some(mf_runtime::Nullable::Value(path)) => Some(Path::new(path)),
                _ => None,
            },
            normalized
                .initialization_options
                .clone()
                .unwrap_or_else(|| self.config.initialization_options.clone()),
        )?;
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let request = ValueRef::from(serde_json::to_value(&comparison.request).context(
            error::JsonSnafu {
                action: "encode analysis request",
            },
        )?)
        .inherit_secrets(sensitivity.iter());
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
                    serde_json::to_value(normalized.options)
                        .context(error::JsonSnafu {
                            action: "encode planning options",
                        })?
                        .into(),
                ),
                ("validation".into(), normalized.validation.into()),
            ])
            .inherit_secrets(sensitivity.iter()),
        }
        .into())
    }
}

#[derive(NodeValue)]
pub struct PlanOutput {
    pub plan: ValueRef,
    pub validation: ValueRef,
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
            validation: input
                .context
                .as_object()
                .and_then(|value| value.get("validation"))
                .cloned()
                .unwrap_or_else(|| Value::Null.into())
                .inherit_secrets([&input.context]),
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
