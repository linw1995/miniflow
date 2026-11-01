use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::ensure;
use std::collections::BTreeSet;

pub const KIND: &str = "builtin.router";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    routes: Vec<String>,
}

struct Router {
    routes: Vec<String>,
}

impl NodePortContract for Router {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("route", ValueType::String, true)],
            outputs: self
                .routes
                .iter()
                .map(String::as_str)
                .chain(["else"])
                .map(|name| PortSpec::owned(name, ValueType::Boolean, false))
                .collect(),
        }
    }
}

impl TaskNode for Router {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let requested = inputs["route"].as_str().expect("validated route string");
        let selected = self
            .routes
            .iter()
            .find(|route| route.as_str() == requested)
            .map_or("else", String::as_str);
        Ok(NodeResult {
            outputs: Outputs::from([(selected.to_owned(), json!(true).into())]),
            skipped: self
                .routes
                .iter()
                .map(String::as_str)
                .chain(["else"])
                .filter(|route| *route != selected)
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        })
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    let mut names = BTreeSet::new();
    ensure!(
        !config.routes.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "router requires at least one route"
        }
    );
    for route in &config.routes {
        ensure!(
            !route.trim().is_empty() && route != "else" && names.insert(route),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: format!("blank, reserved, or duplicate route `{route}`")
            }
        );
    }
    let node = Router {
        routes: config.routes,
    };
    let metadata = NodeMetadata {
        exclusive_outputs: node
            .ports()
            .outputs
            .iter()
            .map(|port| port.name.to_string())
            .collect(),
        ..Default::default()
    };
    PreparedNode::new(node, metadata)
}

inventory::submit! { NodeRegistration { kind: KIND, factory: NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_only_one_gate_without_executing_downstream_work() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let prepared = registry
            .get("builtin.router")
            .unwrap()
            .instantiate(json!({"routes": ["first", "second"]}))
            .unwrap();
        let router = prepared.execution.as_task_node().unwrap();
        for (requested, selected) in [
            ("first", "first"),
            ("second", "second"),
            ("missing", "else"),
        ] {
            let result = router
                .execute(
                    Inputs::from([("route".into(), requested.into())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert_eq!(result.outputs.len(), 1);
            assert_eq!(result.outputs[selected].as_bool(), Some(true));
            assert_eq!(result.skipped.len(), 2);
            assert!(!result.skipped.contains(selected));
        }
    }

    #[test]
    fn validates_routes_and_publishes_exclusive_gates() {
        for config in [
            json!({"routes": []}),
            json!({"routes": [""]}),
            json!({"routes": ["else"]}),
            json!({"routes": ["a", "a"]}),
        ] {
            assert!(factory(config).is_err());
        }
        let node = factory(json!({"routes": ["a", "b"]})).unwrap();
        assert_eq!(node.metadata.exclusive_outputs, vec!["a", "b", "else"]);
    }
}
