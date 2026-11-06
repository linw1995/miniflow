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

struct SubgraphRouter {
    id: String,
    subgraphs: mf_runtime::PreparedFlowArguments,
}

impl NodePortContract for SubgraphRouter {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("route", ValueType::String, true),
                PortSpec::new("arguments", ValueType::Object, true),
            ],
            outputs: vec![PortSpec::new("result", ValueType::Object, true)],
        }
    }
}

impl TaskNode for SubgraphRouter {
    fn execute(
        &self,
        mut inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        use snafu::OptionExt;
        let route = inputs.remove("route").expect("validated route");
        let name = route.as_str().expect("validated route string");
        let body =
            self.subgraphs
                .get(name)
                .with_context(|| mf_runtime::NodeExecutionFailedSnafu {
                    message: format!("unknown passed router subgraph `{name}`"),
                })?;
        // This receiver intentionally exposes only a terminal result envelope.
        // Drive the selected Flow to completion, discarding incremental emissions.
        super::call::execute_passed_flow(
            &self.id,
            name,
            body,
            inputs,
            context,
            &mut mf_runtime::DiscardSink,
        )
    }
}

fn subgraph_factory(
    id: &str,
    config: Value,
    subgraphs: mf_runtime::PreparedFlowArguments,
) -> Result<PreparedNode, NodeBuildError> {
    let config: mf_runtime::RouterSubgraphConfig = deserialize_config(config)?;
    ensure!(
        config.arguments_object && config.capture_errors,
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "subgraph routers require arguments_object and capture_errors"
        }
    );
    ensure!(
        config
            .subgraphs
            .iter()
            .all(|(branch, reference)| !branch.trim().is_empty()
                && branch != "else"
                && !reference.parameter.trim().is_empty()),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "subgraph router branch and parameter names must be nonblank; `else` is reserved"
        }
    );
    ensure!(
        config
            .subgraphs
            .keys()
            .map(String::as_str)
            .eq(subgraphs.names()),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "prepared router subgraphs must exactly match its passed parameter mapping"
        }
    );
    let metadata = super::call::passed_flow_metadata(&subgraphs)?;
    PreparedNode::new(
        SubgraphRouter {
            id: id.into(),
            subgraphs,
        },
        metadata,
    )
}

inventory::submit! { NodeRegistration { kind: KIND, factory: NodeFactory::FlowArguments { plain: factory, prepared: subgraph_factory } } }

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
    fn prepared_router_validates_capture_contract_and_parameter_names() {
        for config in [
            json!({"subgraphs":{}, "arguments_object":true, "capture_errors":false}),
            json!({"subgraphs":{}, "arguments_object":false, "capture_errors":true}),
        ] {
            let error = subgraph_factory("router", config, Default::default())
                .err()
                .unwrap();
            assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
            assert!(
                error
                    .to_string()
                    .contains("subgraph routers require arguments_object and capture_errors")
            );
        }
        for (branch, parameter) in [(" ", "target"), ("else", "target"), ("tool", " ")] {
            let error = subgraph_factory(
                "router",
                json!({"subgraphs":{(branch):{"parameter":parameter}}, "arguments_object":true, "capture_errors":true}),
                Default::default(),
            ).err().unwrap();
            assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
            assert!(
                error
                    .to_string()
                    .contains("branch and parameter names must be nonblank; `else` is reserved")
            );
        }
    }

    #[test]
    fn prepared_router_rejects_extra_missing_and_stdin_bodies() {
        use mf_runtime::{
            PortSpec, PreparedFlowArguments, PreparedSubgraph, StdinRequirement,
            WorkflowInputSchema,
        };
        let config = json!({"subgraphs":{"a":{"parameter":"target"}}, "arguments_object":true,"capture_errors":true});
        let body = PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()));
        assert!(
            subgraph_factory("router", config.clone(), PreparedFlowArguments::default()).is_err()
        );
        assert!(
            subgraph_factory(
                "router",
                config.clone(),
                PreparedFlowArguments::new(
                    [
                        ("a".into(), body.clone()),
                        ("unauthorized".into(), body.clone())
                    ]
                    .into()
                )
            )
            .is_err()
        );
        let mut schema = WorkflowInputSchema::default();
        schema.stdin.insert("read".into(), StdinRequirement::Always);
        assert!(
            subgraph_factory(
                "router",
                config.clone(),
                PreparedFlowArguments::new([("a".into(), body.with_input_schema(schema))].into())
            )
            .is_err()
        );
        let body = PreparedSubgraph::streaming(
            vec![],
            vec![],
            vec![PortSpec::new("value", ValueType::String, true)],
            |_, sink| {
                assert!(!sink.is_connected());
                sink.send(NodeResult {
                    outputs: Outputs::from([("value".into(), "incremental".into())]),
                    ..Default::default()
                })
                .unwrap();
                Ok(Outputs::from([("final".into(), "complete".into())]))
            },
        );
        let router = subgraph_factory(
            "router",
            config,
            PreparedFlowArguments::new([("a".into(), body)].into()),
        )
        .unwrap();
        assert!(router.metadata.stream_outputs.is_empty());
        let result = router
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([
                    ("route".into(), "a".into()),
                    ("arguments".into(), json!({}).into()),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(
            result.outputs["result"]["outputs"]["final"],
            json!("complete")
        );
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        assert!(
            registry
                .get(KIND)
                .unwrap()
                .instantiate(json!({"subgraphs":{},"arguments_object":true,"capture_errors":true}))
                .is_err()
        );
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
