use mf_runtime::{
    ContextValue, ExecutionContext, FlowBindings, FlowInvocationBinding, Inputs, NodeBuildError,
    NodeExecutionError, NodeFactory, NodeMetadata, NodePortContract, NodePorts, NodeRegistration,
    NodeResult, Outputs, PortSpec, PreparedNode, PreparedSubgraph, TaskNode, ValueRef, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "builtin.router";
pub const STATE_KIND: &str = "builtin.router_state";
pub const ASSIGN_KIND: &str = "builtin.router_assign";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    routes: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_enables")]
    enables: Option<Vec<String>>,
}

fn deserialize_enables<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(deserializer).map(Some)
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
fn validate_routes(routes: &[String]) -> Result<(), RouterBuildError> {
    let mut seen = BTreeSet::new();
    for route in routes {
        ensure!(
            !route.trim().is_empty() && route != "else" && seen.insert(route),
            RouteSnafu { route }
        );
    }
    Ok(())
}
fn validate(config: &Config) -> Result<(), RouterBuildError> {
    validate_routes(&config.routes)?;
    let seen: BTreeSet<_> = config.routes.iter().collect();
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
    children: BTreeMap<String, crate::call::CallNode>,
}
impl NodePortContract for Router {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("route", ValueType::String, true),
                PortSpec::new("arguments", ValueType::Object, true),
                PortSpec::new("flows", ValueType::Map(Box::new(ValueType::Flow)), false),
            ],
            outputs: vec![PortSpec::new("result", ValueType::Object, true)],
        }
    }
}
impl TaskNode for Router {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        let route = inputs["route"].clone();
        let values = if let Some(id) = context.current_node() {
            let source = format!("%{id}");
            context.node_state(&source, self.config.initial())
        } else {
            self.config.initial()
        };
        let enables = &values["enables"];
        let requested = route.as_str().expect("validated route string");
        let enabled = enables
            .as_array()
            .expect("validated enables array")
            .iter()
            .any(|value| value.as_str() == Some(requested));
        let Some(call) = self.children.get(requested).filter(|_| enabled) else {
            return Ok(Outputs::from([(
                "result".into(),
                ValueRef::from(serde_json::json!({"error":"unknown or disabled route"}))
                    .inherit_secrets([&route, enables]),
            )])
            .into());
        };
        let mut result = call.execute_packed(inputs, context)?;
        for value in result.outputs.values_mut() {
            *value = value.clone().inherit_secrets([&route, enables]);
        }
        Ok(result)
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
        let routes = context.output(&mf_runtime::output_id(&self.config.source, "routes"))?;
        let mut seen = BTreeSet::new();
        ensure!(
            matches!(routes, ContextValue::Value(routes) if
            value.as_array().is_some_and(|values| values.iter().all(|value| {
                value.as_str().is_some_and(|route| {
                    routes.as_array().is_some_and(|known| known.iter().any(|value| value.as_str() == Some(route)))
                        && seen.insert(route)
                })
            }))),
            InvalidEnablesSnafu
        );
        context.stage_node_write(&self.config.source, "enables", value)?;
        Ok(Outputs::from([("done".into(), ValueRef::from(true))]).into())
    }
}

fn bindings(config: &Value) -> FlowBindings {
    if serde_json::from_value::<Config>(config.clone())
        .is_ok_and(|config| validate_routes(&config.routes).is_ok())
    {
        FlowBindings {
            invocation: Some(FlowInvocationBinding {
                input: "flows".into(),
            }),
            ..Default::default()
        }
    } else {
        FlowBindings::default()
    }
}
fn static_bindings(config: &Value) -> BTreeSet<String> {
    serde_json::from_value::<Config>(config.clone())
        .ok()
        .filter(|config| validate_routes(&config.routes).is_ok())
        .map(|config| config.routes.into_iter().collect())
        .unwrap_or_default()
}
fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    prepared_factory(KIND, config, BTreeMap::new())
}
fn prepared_factory(
    id: &str,
    config: Value,
    bodies: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config: Config = mf_runtime::deserialize_config(config)?;
    validate(&config)?;
    ensure!(
        config
            .routes
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .eq(bodies.keys()),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "router children differ from the static route set"
        }
    );
    let children = bodies
        .into_iter()
        .map(|(route, body)| {
            let call = crate::call::CallNode::new(id, &route, route.clone(), body)?;
            Ok((route, call))
        })
        .collect::<Result<_, NodeBuildError>>()?;
    PreparedNode::new(
        Router { config, children },
        NodeMetadata {
            input_bindings: BTreeMap::from([(
                "arguments".into(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
    )
}
inventory::submit! {
    NodeRegistration { kind: KIND, factory: NodeFactory::WithFlowBindings {
        factory: &NodeFactory::BoundSubgraph { bindings: static_bindings, plain: factory, prepared: prepared_factory }, bindings,
    } }
}
inventory::submit! {
    NodeRegistration { kind: STATE_KIND, factory: NodeFactory::Plain(|config| {
        let config = mf_runtime::deserialize_config(config)?; validate(&config)?;
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
    fn router(enables: Option<Value>) -> PreparedNode {
        let mut config = json!({"routes":["first","second"]});
        if let Some(enables) = enables {
            config["enables"] = enables;
        }
        let bodies = ["first", "second"]
            .into_iter()
            .map(|route| {
                (
                    route.into(),
                    PreparedSubgraph::new(
                        vec![],
                        vec![PortSpec::new("selected", ValueType::String, true)],
                        move |_| Ok(Outputs::from([("selected".into(), ValueRef::from(route))])),
                    ),
                )
            })
            .collect();
        prepared_factory("choose", config, bodies).unwrap()
    }
    #[test]
    fn selects_only_one_static_child_with_the_normal_call_scope() {
        let prepared = router(None);
        assert!(prepared.metadata.input_bindings["arguments"].secret);
        assert!(prepared.metadata.stream_outputs.is_empty());
        let node = mf_runtime::FlowNode::new("choose", prepared)
            .into_task()
            .unwrap();
        for route in ["first", "second", "missing"] {
            let mut context = ExecutionContext::default();
            let result = node
                .node
                .execute(
                    Inputs::from([
                        ("route".into(), ValueRef::from(route)),
                        ("arguments".into(), ValueRef::from(json!({}))),
                    ]),
                    &mut context,
                )
                .unwrap();
            if route == "missing" {
                assert!(result.outputs["result"].get("error").is_some());
            } else {
                assert_eq!(
                    result.outputs["result"]["outputs"]["selected"],
                    json!(route)
                );
            }
        }
    }
    #[test]
    fn empty_enables_disables_all_and_native_routers_do_not_share_state() {
        let mut context = ExecutionContext::default();
        for enables in [Some(json!([])), None, Some(json!(["second"])), None] {
            let disabled = enables.is_some();
            let node = mf_runtime::FlowNode::new("choose", router(enables))
                .into_task()
                .unwrap();
            let result = node
                .node
                .execute(
                    Inputs::from([
                        ("route".into(), ValueRef::from("first")),
                        ("arguments".into(), ValueRef::from(json!({}))),
                    ]),
                    &mut context,
                )
                .unwrap();
            assert_eq!(result.outputs["result"].get("error").is_some(), disabled);
        }
        assert!(factory(json!({"routes":[]})).is_ok());
    }
    #[test]
    fn configuration_is_strict_and_children_must_match_routes() {
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
            json!({"routes":[],"enables":null}),
            json!({"routes":["missing"]}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
    }
}
