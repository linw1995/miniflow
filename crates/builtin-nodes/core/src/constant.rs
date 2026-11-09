use mf_runtime::{
    FlowValue, NodeBuildError, NodeConfig, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, OutputDerivation, PreparedNode, PreparedSubgraph, TypedNodeResult,
    TypedTaskNode, ValueRef, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub const KIND: &str = "builtin.constant";

pub fn kind() -> &'static str {
    KIND
}

#[derive(Deserialize)]
struct ConstantConfig {
    value: Value,
}

#[derive(Deserialize)]
enum FlowType {
    #[serde(rename = "flow")]
    Flow,
}

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct FlowConfig {
    #[serde(rename = "type")]
    value_type: FlowType,
    value: PreparedSubgraph,
}

fn is_flow(config: &Value) -> bool {
    config.get("type").and_then(Value::as_str) == Some("flow")
}

fn flow_parameters(config: &Value) -> BTreeMap<String, Value> {
    if is_flow(config) {
        FlowConfig::flow_parameters(config)
    } else {
        BTreeMap::new()
    }
}

struct ConstantNode {
    value: ValueRef,
}

#[derive(NodeValue)]
struct ConstantInputs {}

#[derive(NodeValue)]
struct ConstantOutputs {
    value: ValueRef,
}

impl TypedTaskNode for ConstantNode {
    type Input = ConstantInputs;
    type Output = ConstantOutputs;

    fn execute(
        &self,
        _inputs: ConstantInputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<ConstantOutputs>, NodeExecutionError> {
        Ok(ConstantOutputs {
            value: self.value.clone(),
        }
        .into())
    }
}

fn constant_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    if is_flow(&config) {
        return mf_runtime::NodeInvalidSubgraphSnafu {
            message: "a flow constant requires a prepared `value` binding",
        }
        .fail();
    }
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value.into(),
    };
    let metadata = NodeMetadata {
        output_derivations: vec![OutputDerivation::literal("value", node.value.clone())],
        ..Default::default()
    };
    PreparedNode::typed_task(node, metadata)
}

fn flow_factory(
    _id: &str,
    config: Value,
    flows: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let FlowConfig {
        value_type: FlowType::Flow,
        value,
    } = FlowConfig::from_config(config, flows)?;
    let node = ConstantNode {
        value: FlowValue::new(value)?.into(),
    };
    // Native capabilities are never literal evidence: only their known type may
    // be serialized into compiler metadata. Keep the ordinary typed task adapter.
    PreparedNode::typed_task(
        node,
        NodeMetadata {
            output_derivations: vec![OutputDerivation::known_type("value", ValueType::Flow)],
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph {
            flow_parameters,
            plain: constant_factory,
            prepared: flow_factory,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{ExecutionContext, Inputs, PortSpec};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn flow_constant_publishes_a_native_value_without_executing_its_body() {
        let executions = Arc::new(AtomicUsize::new(0));
        let effects = executions.clone();
        let body = PreparedSubgraph::new(Vec::new(), Vec::new(), move |_| {
            effects.fetch_add(1, Ordering::SeqCst);
            Ok(Default::default())
        });
        let prepared = flow_factory(
            "definition",
            json!({"type":"flow", "value":{"opaque":true}}),
            BTreeMap::from([("value".into(), body)]),
        )
        .unwrap();
        assert!(matches!(
            prepared.execution,
            mf_runtime::NodeExecution::Task(_)
        ));
        assert!(prepared.metadata.stream_outputs.is_empty());
        assert_eq!(
            prepared.metadata.ports.outputs,
            [PortSpec::new("value", ValueType::Any, true)]
        );
        assert_eq!(
            prepared.metadata.output_derivations,
            [OutputDerivation::known_type("value", ValueType::Flow)]
        );
        serde_json::to_value(&prepared.metadata).unwrap();
        let task = prepared.execution.as_task_node().unwrap();
        let mut context = ExecutionContext::default();
        let first = task.execute(Inputs::new(), &mut context).unwrap();
        let second = task.execute(Inputs::new(), &mut context).unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(first.outputs["value"].ptr_eq(&second.outputs["value"]));
        let flow = first.outputs["value"].as_flow().unwrap();
        assert_eq!(flow, second.outputs["value"].as_flow().unwrap());
        flow.invoke("call", Default::default(), &mut context)
            .unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn only_an_explicit_top_level_flow_type_authorizes_the_flat_value_field() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        let sources = [
            json!({"path":"./B.json"}),
            json!({"version":"2026-10-28", "dependencies":{}, "nodes":[]}),
        ];
        for source in sources {
            let config = json!({"type":"flow", "value":source});
            assert_eq!(
                registration.flow_parameters(&config),
                BTreeMap::from([("value".into(), source.clone())])
            );
            assert!(registration.instantiate(config).is_err());

            for value in [source, json!({"type":"flow", "value":{"path":"./B.json"}})] {
                let ordinary = json!({"value":value});
                assert!(registration.flow_parameters(&ordinary).is_empty());
                let prepared = registration.instantiate(ordinary).unwrap();
                assert_eq!(
                    prepared.metadata.output_derivations,
                    [OutputDerivation::literal("value", value.clone())]
                );
                let result = prepared
                    .execution
                    .as_task_node()
                    .unwrap()
                    .execute(Inputs::new(), &mut ExecutionContext::default())
                    .unwrap();
                assert_eq!(result.outputs["value"], value);
            }
        }
    }

    #[test]
    fn flow_constant_uses_the_exact_prepared_value_and_strict_configuration() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        let config = json!({"type":"flow", "value":{"path":"./B.json"}});
        let body = || PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Default::default()));
        for flows in [
            BTreeMap::new(),
            BTreeMap::from([("body".into(), body())]),
            BTreeMap::from([("value".into(), body()), ("extra".into(), body())]),
        ] {
            assert!(
                registration
                    .instantiate_subgraphs("constant", config.clone(), flows)
                    .is_err()
            );
        }
        for invalid in [
            json!({"type":"flow", "value":null}),
            json!({"type":"flow", "value":{}, "unexpected":true}),
        ] {
            assert!(
                flow_factory(
                    "constant",
                    invalid,
                    BTreeMap::from([("value".into(), body())])
                )
                .is_err()
            );
        }
        registration
            .instantiate_subgraphs(
                "constant",
                config,
                BTreeMap::from([("value".into(), body())]),
            )
            .unwrap();
    }
}
