//! Recognize assignments to router-local `enables` without adding node kinds.
use crate::{WorkflowCompileError, WorkflowDefinition};
use serde::Deserialize;
use snafu::{OptionExt, ResultExt};

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
) -> Option<mf_runtime::RouterWrite> {
    if !target.starts_with('%') || target.starts_with("%root.") || target.starts_with("%loop.") {
        return None;
    }
    let (router, field) = target.strip_prefix('%')?.split_once('.')?;
    if field != "enables" {
        return None;
    }
    let node = definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == router && node.kind == ROUTER)?;
    Some(mf_runtime::RouterWrite {
        router: node.id.clone(),
    })
}

pub fn write_values(
    definition: &WorkflowDefinition,
    write: &mf_runtime::RouterWrite,
) -> Result<(Vec<String>, Vec<String>), WorkflowCompileError> {
    let node = definition
        .nodes
        .iter()
        .find(|node| node.id == write.router)
        .with_context(|| crate::compiler::InvalidGraphSnafu {
            definition_id: write.router.clone(),
            message: "router assignment references an unknown target node",
        })?;
    let config: RouterConfig = serde_json::from_value(node.config.clone()).context(
        crate::compiler::NodeConfigurationSnafu {
            definition_id: node.id.clone(),
        },
    )?;
    let enables = config.enables.unwrap_or_else(|| config.routes.clone());
    Ok((config.routes, enables))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition() -> WorkflowDefinition {
        WorkflowDefinition::from_json(
            &json!({"version":"2026-11-13","nodes":[
                {"id":"execute","kind":"builtin.router","config":{"routes":["a"],"enables":[]}}
            ]})
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn only_router_enables_targets_are_state_writes() {
        let definition = definition();
        assert!(write_target(&definition, "variable").is_none());
        assert!(write_target(&definition, "%plain").is_none());
        assert!(write_target(&definition, "%execute.routes").is_none());
        let write = write_target(&definition, "%execute.enables").unwrap();
        assert_eq!(write.router.as_str(), "execute");
    }
}
