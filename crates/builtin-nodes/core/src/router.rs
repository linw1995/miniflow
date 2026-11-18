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
    #[serde(default, deserialize_with = "deserialize_enables")]
    enables: Option<Vec<String>>,
    #[serde(default)]
    observe: Option<String>,
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
    #[snafu(display("Router observation category must be a nonblank label of at most 256 bytes"))]
    Observation,
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
    ensure!(
        config
            .observe
            .as_deref()
            .is_none_or(mf_telemetry::events::CallExecution::valid_label),
        ObservationSnafu
    );
    let known: BTreeSet<_> = config.routes.iter().collect();
    if let Some(enables) = &config.enables {
        let mut enabled = BTreeSet::new();
        for route in enables {
            ensure!(
                known.contains(route) && enabled.insert(route),
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
        let mut inputs = vec![
            PortSpec::new("route", ValueType::String, true),
            PortSpec::new("arguments", ValueType::Object, true),
            PortSpec::new("flows", ValueType::Map(Box::new(ValueType::Flow)), false),
        ];
        if self.config.observe.is_some() {
            inputs.push(PortSpec::new("observation_id", ValueType::String, false));
        }
        NodePorts {
            inputs,
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
        let configured = inputs
            .get("flows")
            .and_then(|flows| flows.get(requested))
            .and_then(ValueRef::as_flow)
            .cloned();
        if inputs.contains_key("flows") && configured.is_none() {
            return Ok(Outputs::from([(
                "result".into(),
                ValueRef::from(serde_json::json!({"error":"unknown or disabled route"}))
                    .inherit_secrets([&route, enables]),
            )])
            .into());
        }
        // Metadata is opt-in and never taken from the packed arguments or result.
        let observation_id = inputs
            .get("observation_id")
            .filter(|id| !id.is_secret() && !route.is_secret() && !enables.contains_secret())
            .and_then(ValueRef::as_str)
            .filter(|id| mf_telemetry::events::CallExecution::valid_label(id))
            .map(str::to_owned);
        let observation = self
            .config
            .observe
            .as_deref()
            .zip(observation_id.as_deref());
        let mut result = call.execute_packed(inputs, context, configured.as_ref(), observation)?;
        for value in result.outputs.values_mut() {
            *value = value.clone().inherit_secrets([&route, enables]);
        }
        Ok(result)
    }
}

fn bindings(config: &Value) -> FlowBindings {
    if serde_json::from_value::<Config>(config.clone())
        .is_ok_and(|config| validate_routes(&config.routes).is_ok())
    {
        FlowBindings::invoker("flows")
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

    fn execute(
        node: &mf_runtime::TaskFlowNode,
        route: &str,
        context: &mut ExecutionContext,
    ) -> NodeResult {
        node.node
            .execute(
                Inputs::from([
                    ("route".into(), ValueRef::from(route)),
                    ("arguments".into(), ValueRef::from(json!({}))),
                ]),
                context,
            )
            .unwrap()
    }

    #[test]
    fn selects_one_enabled_static_child_with_the_normal_call_scope() {
        let prepared = router(None);
        assert!(prepared.metadata.input_bindings["arguments"].secret);
        assert!(prepared.metadata.stream_outputs.is_empty());
        let node = mf_runtime::FlowNode::new("choose", prepared)
            .into_task()
            .unwrap();
        for (route, selected) in [
            ("first", "first"),
            ("second", "second"),
            ("missing", "missing"),
        ] {
            let result = execute(&node, route, &mut ExecutionContext::default());
            if selected == "missing" {
                assert!(result.outputs["result"].get("error").is_some());
            } else {
                assert_eq!(
                    result.outputs["result"]["outputs"]["selected"],
                    json!(selected)
                );
            }
        }
        for enables in [json!([]), json!(["second"])] {
            let node = mf_runtime::FlowNode::new("choose", router(Some(enables)))
                .into_task()
                .unwrap();
            assert!(
                execute(&node, "first", &mut ExecutionContext::default()).outputs["result"]
                    .get("error")
                    .is_some()
            );
        }
    }

    #[test]
    fn runtime_flow_maps_require_the_selected_capability_but_cannot_replace_static_bodies() {
        let node = mf_runtime::FlowNode::new("choose", router(None))
            .into_task()
            .unwrap();
        let selected = mf_runtime::FlowValue::new(PreparedSubgraph::new(
            vec![],
            vec![PortSpec::new("selected", ValueType::String, true)],
            |_| panic!("a runtime map cannot replace the frozen body"),
        ))
        .unwrap();
        for (flows, expected) in [
            (ValueRef::object([]), false),
            (ValueRef::object([("first".into(), selected.into())]), true),
        ] {
            let output = node
                .node
                .execute(
                    Inputs::from([
                        ("route".into(), "first".into()),
                        ("arguments".into(), ValueRef::object([])),
                        ("flows".into(), flows),
                    ]),
                    &mut ExecutionContext::default(),
                )
                .unwrap();
            if expected {
                assert_eq!(
                    output.outputs["result"]["outputs"]["selected"],
                    json!("first")
                );
            } else {
                assert_eq!(
                    output.outputs["result"]["error"],
                    json!("unknown or disabled route")
                );
            }
        }
    }

    #[test]
    fn cancellation_prevents_selection_before_any_child_is_invoked() {
        let node = mf_runtime::FlowNode::new("choose", router(None))
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
    fn observes_the_selected_body_not_the_router_envelope() {
        use mf_telemetry::events::{
            CALL_EXECUTION_EVENT, CallExecution, CallExecutionPhase, collect,
        };
        use std::sync::{Arc, Mutex};

        for (mode, expected) in [
            ("success", Some(CallExecutionPhase::Succeeded)),
            ("business_failure", Some(CallExecutionPhase::Failed)),
            ("cancelled", Some(CallExecutionPhase::Interrupted)),
            ("disabled", None),
            ("secret", None),
            ("missing", None),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let _record = collect(move |event| {
                assert_eq!(event.name, CALL_EXECUTION_EVENT);
                assert!(!event.body.to_string().contains("private payload"));
                observed
                    .lock()
                    .unwrap()
                    .push(serde_json::from_value::<CallExecution>(event.body.clone()).unwrap());
            });
            let during = events.clone();
            let body = PreparedSubgraph::new(vec![], vec![], move |state| {
                assert_ne!(mode, "disabled");
                assert_eq!(
                    during.lock().unwrap().first().map(|call| call.phase),
                    expected.map(|_| CallExecutionPhase::Running)
                );
                if mode == "business_failure" {
                    return Err(mf_runtime::WorkflowRunError::NodeExecution {
                        definition_id: "body".into(),
                        source: mf_runtime::NodeExecutionFailedSnafu {
                            message: "private payload",
                        }
                        .build(),
                    });
                }
                if mode == "cancelled" {
                    state.cancellation().cancel(
                        mf_runtime::StreamExecutionSnafu {
                            message: "cancelled",
                        }
                        .build(),
                    );
                }
                Ok(Outputs::new())
            });
            let mut config = json!({"routes":["chosen"],"observe":"tool"});
            if mode == "disabled" {
                config["enables"] = json!([]);
            }
            let prepared =
                prepared_factory("choose", config, BTreeMap::from([("chosen".into(), body)]))
                    .unwrap();
            let mut inputs = Inputs::from([
                ("route".into(), ValueRef::from("chosen")),
                ("arguments".into(), ValueRef::object([])),
            ]);
            if mode != "missing" {
                let id = if mode == "secret" {
                    ValueRef::from("private payload").into_secret()
                } else {
                    ValueRef::from("wire-id")
                };
                inputs.insert("observation_id".into(), id);
            }
            let node = mf_runtime::FlowNode::new("choose", prepared)
                .into_task()
                .unwrap();
            let result = node.node.execute(inputs, &mut ExecutionContext::default());
            assert_eq!(result.is_err(), mode == "cancelled");
            if mode == "business_failure" {
                assert!(result.unwrap().outputs["result"].get("error").is_some());
            }
            let events = events.lock().unwrap();
            if let Some(phase) = expected {
                let [start, end] = events.as_slice() else {
                    panic!("expected a measured call: {events:?}");
                };
                assert_eq!(start.correlation_id, "wire-id");
                assert_eq!(start.name, "chosen");
                assert_eq!(start.attempt, end.attempt);
                assert_eq!(end.phase, phase);
            } else {
                assert!(events.is_empty());
            }
        }
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
            json!({"routes":[],"enables":null}),
            json!({"routes":[],"mode":"execute"}),
            json!({"routes":[],"observe":" "}),
            json!({"routes":[],"flow_field":"run"}),
            json!({"routes":["missing"]}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
    }
}
