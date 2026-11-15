use mf_runtime::{
    ExecutionContext, FlowBindings, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec,
    PreparedNode, PreparedSubgraph, TaskNode, ValueRef, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "builtin.router";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    routes: Vec<String>,
}

#[derive(Debug, Snafu)]
pub enum RouterBuildError {
    #[snafu(display("Blank, reserved, or duplicate router route `{route}`"))]
    Route { route: String },
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

struct Router {
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
        let Some(call) = self
            .children
            .get(route.as_str().expect("validated route string"))
        else {
            return Ok(Outputs::from([(
                "result".into(),
                ValueRef::from(serde_json::json!({"error":"unknown route"}))
                    .inherit_secrets([&route]),
            )])
            .into());
        };
        let mut result = call.execute_packed(inputs, context)?;
        for value in result.outputs.values_mut() {
            *value = value.clone().inherit_secrets([&route]);
        }
        Ok(result)
    }
}

fn bindings(config: &Value) -> FlowBindings {
    if serde_json::from_value::<Config>(config.clone())
        .is_ok_and(|config| validate_routes(&config.routes).is_ok())
    {
        FlowBindings {
            invocation: Some("flows".into()),
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
    validate_routes(&config.routes)?;
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
        Router { children },
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
        factory: &NodeFactory::BoundSubgraph {
            bindings: static_bindings, plain: factory, prepared: prepared_factory,
        },
        bindings,
    } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn router() -> PreparedNode {
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
        prepared_factory("choose", json!({"routes":["first","second"]}), bodies).unwrap()
    }

    #[test]
    fn selects_one_static_child_with_the_normal_call_scope() {
        let prepared = router();
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
    fn cancellation_prevents_selection_before_any_child_is_invoked() {
        let node = mf_runtime::FlowNode::new("choose", router())
            .into_task()
            .unwrap();
        let mut context = ExecutionContext::default();
        context
            .cancellation()
            .cancel(mf_runtime::StreamError::InputFailure {
                message: "cancelled".into(),
            });
        assert!(node.node.execute(Inputs::new(), &mut context).is_err());
    }

    #[test]
    fn configuration_is_strict_and_children_must_match_routes() {
        for config in [
            json!({}),
            json!({"routes":[""]}),
            json!({"routes":["else"]}),
            json!({"routes":["a","a"]}),
            json!({"routes":[],"enables":[]}),
            json!({"routes":[],"mode":"execute"}),
            json!({"routes":[],"flow_field":"run"}),
            json!({"routes":[],"flow_candidates":{}}),
            json!({"routes":["missing"]}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
    }
}
