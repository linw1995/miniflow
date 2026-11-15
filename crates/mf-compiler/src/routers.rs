//! Recognize assignments to router-local `enables` without adding node kinds.
use crate::{WorkflowCompileError, WorkflowDefinition};
use serde::Deserialize;
use snafu::ResultExt;

const ROUTER: &str = "builtin.router";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouterConfig {
    routes: Vec<String>,
    #[serde(default)]
    enables: Option<Vec<String>>,
}

pub fn write_target(
    definition: &WorkflowDefinition,
    target: &str,
) -> Result<Option<mf_runtime::RouterWrite>, WorkflowCompileError> {
    if !target.starts_with('%') || target.starts_with("%root.") || target.starts_with("%loop.") {
        return Ok(None);
    }
    let Some((router, field)) = target
        .strip_prefix('%')
        .and_then(|target| target.split_once('.'))
    else {
        return Ok(None);
    };
    if field != "enables" {
        return Ok(None);
    }
    let Some(node) = definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == router && node.kind == ROUTER)
    else {
        return Ok(None);
    };
    let config: RouterConfig = serde_json::from_value(node.config.clone()).context(
        crate::compiler::NodeConfigurationSnafu {
            definition_id: node.id.clone(),
        },
    )?;
    Ok(Some(mf_runtime::RouterWrite {
        router: node.id.clone(),
        enables: config.enables.unwrap_or_else(|| config.routes.clone()),
        routes: config.routes,
    }))
}
