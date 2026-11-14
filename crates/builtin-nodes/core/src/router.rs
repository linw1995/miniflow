use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueRef, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};
use std::collections::BTreeSet;

pub const KIND: &str = "builtin.router";
pub const STATE_KIND: &str = "builtin.router_state";
pub const ASSIGN_KIND: &str = "builtin.router_assign";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    routes: Vec<String>,
    #[serde(default)]
    enables: Option<Vec<String>>,
}

#[derive(Debug, Snafu)]
pub enum RouterBuildError {
    #[snafu(display("Blank, reserved, or duplicate router route `{route}`"))]
    Route { route: String },
    #[snafu(display("Router enables contains a duplicate or unknown route `{route}`"))]
    Enable { route: String },
}

impl From<RouterBuildError> for NodeBuildError {
    fn from(source: RouterBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

fn validate(config: &Config) -> Result<(), RouterBuildError> {
    let mut seen = BTreeSet::new();
    for route in &config.routes {
        ensure!(
            !route.trim().is_empty() && route != "else" && seen.insert(route),
            RouteSnafu { route }
        );
    }
    if let Some(enables) = &config.enables {
        let mut enabled = BTreeSet::new();
        for route in enables {
            ensure!(
                seen.contains(route) && enabled.insert(route),
                EnableSnafu { route }
            );
        }
    }
    Ok(())
}

impl Config {
    fn initial(&self) -> Outputs {
        Outputs::from([
            (
                "routes".into(),
                ValueRef::from(serde_json::json!(self.routes)),
            ),
            (
                "enables".into(),
                ValueRef::from(serde_json::json!(
                    self.enables.as_ref().unwrap_or(&self.routes)
                )),
            ),
        ])
    }
}

struct Router {
    config: Config,
}

impl NodePortContract for Router {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("route", ValueType::String, true)],
            outputs: self
                .config
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
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let route = &inputs["route"];
        let source = format!("%{}", context.current_node().unwrap_or(KIND));
        let values = context.node_state(&source, self.config.initial());
        let enables = &values["enables"];
        let selected = enables
            .as_array()
            .expect("validated enables array")
            .iter()
            .filter_map(ValueRef::as_str)
            .find(|candidate| Some(*candidate) == route.as_str())
            .unwrap_or("else");
        Ok(NodeResult {
            outputs: Outputs::from([(
                selected.into(),
                ValueRef::from(true).inherit_secrets([route, enables]),
            )]),
            skipped: self
                .config
                .routes
                .iter()
                .map(String::as_str)
                .chain(["else"])
                .filter(|candidate| *candidate != selected)
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        })
    }
}

struct StateSource {
    config: Config,
}

impl NodePortContract for StateSource {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: Vec::new(),
            outputs: ["routes", "enables"]
                .map(|name| PortSpec::new(name, ValueType::List(Box::new(ValueType::String)), true))
                .into(),
        }
    }
}

impl TaskNode for StateSource {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let id = context
            .current_node()
            .expect("state source has an ID")
            .to_owned();
        Ok(context.node_state(&id, self.config.initial()).into())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignmentConfig {
    source: String,
    routes: Vec<String>,
}

struct Assignment {
    config: AssignmentConfig,
}

#[derive(Debug, Snafu)]
pub enum RouterStateError {
    #[snafu(display("Router enables must be a string array without duplicates or unknown routes"))]
    InvalidEnables,
}

impl From<RouterStateError> for NodeExecutionError {
    fn from(source: RouterStateError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl NodePortContract for Assignment {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new(
                "value",
                ValueType::List(Box::new(ValueType::String)),
                true,
            )],
            outputs: vec![PortSpec::new("done", ValueType::Boolean, true)],
        }
    }
}

impl TaskNode for Assignment {
    fn supports_success_dependencies(&self) -> bool {
        false
    }

    fn execute(
        &self,
        mut inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value = inputs.remove("value").expect("validated assignment value");
        let mut seen = BTreeSet::new();
        ensure!(
            value
                .as_array()
                .is_some_and(|values| values.iter().all(|value| {
                    value.as_str().is_some_and(|route| {
                        self.config.routes.iter().any(|known| known == route) && seen.insert(route)
                    })
                })),
            InvalidEnablesSnafu
        );
        context.stage_node_write(&self.config.source, "enables", value)?;
        Ok(Outputs::from([("done".into(), ValueRef::from(true))]).into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = mf_runtime::deserialize_config(config)?;
    validate(&config)?;
    let exclusive_outputs = if config.routes.is_empty() {
        Vec::new()
    } else {
        config
            .routes
            .iter()
            .map(Into::into)
            .chain(["else".into()])
            .collect()
    };
    PreparedNode::new(
        Router { config },
        NodeMetadata {
            exclusive_outputs,
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: NodeFactory::Plain(factory) }
}
inventory::submit! {
    NodeRegistration { kind: STATE_KIND, factory: NodeFactory::Plain(|config| {
        let config = mf_runtime::deserialize_config(config)?;
        validate(&config)?;
        PreparedNode::new(StateSource { config }, NodeMetadata::default())
    }) }
}
inventory::submit! {
    NodeRegistration { kind: ASSIGN_KIND, factory: NodeFactory::Plain(|config| {
        PreparedNode::new(Assignment { config: mf_runtime::deserialize_config(config)? }, NodeMetadata::default())
    }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn router_is_a_gate_with_a_fixed_exclusive_port_set() {
        let prepared = factory(json!({"routes":["first","second"]})).unwrap();
        assert_eq!(prepared.metadata.ports.inputs.len(), 1);
        assert_eq!(
            prepared.metadata.exclusive_outputs,
            ["first", "second", "else"]
        );
        let node = mf_runtime::FlowNode::new("choose", prepared)
            .into_task()
            .unwrap();
        for requested in ["first", "second", "missing"] {
            let mut context = ExecutionContext::default();
            let result = context
                .with_node("choose".into(), |context| {
                    node.node.execute(
                        Inputs::from([("route".into(), ValueRef::from(requested))]),
                        context,
                    )
                })
                .unwrap();
            let selected = if requested == "missing" {
                "else"
            } else {
                requested
            };
            assert_eq!(
                result
                    .outputs
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [selected]
            );
            assert_eq!(result.skipped.len(), 2);
        }
    }

    #[test]
    fn empty_enables_closes_all_routes_and_omission_enables_all() {
        for enables in [None, Some(json!([])), Some(json!(["second"]))] {
            let mut config = json!({"routes":["first","second"]});
            if let Some(enables) = enables {
                config["enables"] = enables;
            }
            let prepared = factory(config.clone()).unwrap();
            let node = mf_runtime::FlowNode::new("choose", prepared)
                .into_task()
                .unwrap();
            let result = node
                .node
                .execute(
                    Inputs::from([("route".into(), ValueRef::from("first"))]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            assert!(
                result
                    .outputs
                    .contains_key(if config.get("enables").is_none() {
                        "first"
                    } else {
                        "else"
                    })
            );
        }
        assert!(factory(json!({"routes":[]})).is_ok());
    }

    #[test]
    fn invalid_routes_and_legacy_execute_configuration_are_rejected() {
        for config in [
            json!({}),
            json!({"routes":[""]}),
            json!({"routes":["else"]}),
            json!({"routes":["a","a"]}),
            json!({"routes":["a"],"enables":["b"]}),
            json!({"routes":["a"],"enables":["a","a"]}),
            json!({"routes":[],"mode":"execute"}),
            json!({"routes":[],"flow_field":"run"}),
            json!({"routes":[],"flow_candidates":{}}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
    }
}
