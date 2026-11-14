//! Lower node-local router state independently of callable and container state.
use crate::{NodeDefinition, WorkflowCompileError, WorkflowDefinition};
use serde_json::json;
use snafu::ensure;
use std::collections::BTreeMap;

const ROUTER: &str = "builtin.router";
const SOURCE: &str = "builtin.router_state";
const ASSIGN: &str = "builtin.router_assign";

pub fn lower(
    definition: &mut WorkflowDefinition,
    lowered_sources: bool,
) -> Result<(), WorkflowCompileError> {
    let routers: BTreeMap<_, _> = definition
        .nodes
        .iter()
        .filter(|node| node.kind == ROUTER)
        .map(|node| (format!("%{}", node.id), node.config.clone()))
        .collect();
    for node in &definition.nodes {
        ensure!(
            lowered_sources || (node.kind != SOURCE && node.kind != ASSIGN),
            crate::compiler::InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "node-local state instructions are implicit",
            }
        );
    }
    for (source, config) in &routers {
        if let Some(existing) = definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == source)
        {
            ensure!(
                lowered_sources && existing.kind == SOURCE,
                crate::compiler::InvalidGraphSnafu {
                    definition_id: existing.id.clone(),
                    message: "router state sources are implicit",
                }
            );
            existing.config = config.clone();
        } else {
            definition.nodes.push(NodeDefinition {
                id: source.clone().into(),
                kind: SOURCE.into(),
                config: config.clone(),
            });
        }
        let owner = source.strip_prefix('%').expect("synthetic source");
        let edge = mf_runtime::OrderEdgeDefinition {
            from_node: source.clone().into(),
            to_node: owner.into(),
        };
        if !definition.order_edges.contains(&edge) {
            definition.order_edges.push(edge);
        }
    }
    for node in &mut definition.nodes {
        if node.kind != mf_runtime::ASSIGN_KIND {
            continue;
        }
        let Some(target) = node
            .config
            .get("variable")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if !target.starts_with('%') || target.starts_with("%root.") || target.starts_with("%loop.")
        {
            continue;
        }
        let source = target.strip_suffix(".enables");
        ensure!(
            source.is_some_and(|source| routers.contains_key(source)),
            crate::compiler::InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "assignment requires a writable node-local enables field",
            }
        );
        let source = source.expect("validated writable field");
        ensure!(
            node.config
                .as_object()
                .is_some_and(|fields| fields.len() == 1),
            crate::compiler::InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "assignment config only accepts variable",
            }
        );
        let source = source.to_owned();
        node.config = json!({"source":source});
        node.kind = ASSIGN.into();
        definition
            .control_edges
            .push(mf_runtime::ControlEdgeDefinition {
                from_node: source.into(),
                from_output: "routes".into(),
                to_node: node.id.clone(),
            });
    }
    Ok(())
}
