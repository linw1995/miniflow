use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, PortSpec, PreparedNode, TaskNode, ValueType,
    deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};

pub const KIND: &str = "builtin.assert";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssertNode {
    message: String,
}

#[derive(Debug, Snafu)]
enum AssertionError {
    #[snafu(display("assert requires one boolean condition"))]
    InvalidCondition,
    #[snafu(display("{message}"))]
    Failed { message: String },
}

impl From<AssertionError> for NodeExecutionError {
    fn from(source: AssertionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl NodePortContract for AssertNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("condition", ValueType::Boolean, true)],
            outputs: vec![],
        }
    }
}

impl TaskNode for AssertNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        ensure!(inputs.len() == 1, InvalidConditionSnafu);
        let condition = inputs
            .get("condition")
            .and_then(|value| value.as_bool())
            .ok_or_else(|| InvalidConditionSnafu.build())?;
        ensure!(
            condition,
            FailedSnafu {
                message: &self.message
            }
        );
        Ok(NodeResult::default())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let node: AssertNode = deserialize_config(config)?;
    PreparedNode::new(node, NodeMetadata::default())
}

inventory::submit! { NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeRegistry, StreamError, ValueRef};
    use serde_json::json;
    use std::error::Error;

    #[test]
    fn registration_requires_a_static_message_and_boolean_condition() {
        let registry = NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        for config in [
            json!({}),
            json!({"message": false}),
            json!({"message": "failed", "extra": true}),
        ] {
            assert!(registration.instantiate(config).is_err());
        }
        let prepared = registration
            .instantiate(json!({"message": "failed"}))
            .unwrap();
        assert_eq!(
            prepared.metadata.ports.inputs,
            vec![PortSpec::new("condition", ValueType::Boolean, true)]
        );
        assert!(prepared.metadata.ports.outputs.is_empty());
    }

    #[test]
    fn checks_conditions_without_disclosing_input_values() {
        let node = AssertNode {
            message: "current identity is inconsistent".into(),
        };
        let inputs = |value: Value| {
            Inputs::from([("condition".into(), ValueRef::from(value).into_secret())])
        };
        let result = node
            .execute(inputs(json!(true)), &mut ExecutionContext::default())
            .unwrap();
        assert!(result.outputs.is_empty());
        assert!(result.skipped.is_empty());

        let error = node
            .execute(inputs(json!(false)), &mut ExecutionContext::default())
            .unwrap_err();
        assert!(
            matches!(error.source().unwrap().downcast_ref::<AssertionError>(), Some(AssertionError::Failed { message }) if message == &node.message)
        );
        for input in [
            Inputs::new(),
            inputs(json!(null)),
            inputs(json!("sensitive value")),
            inputs(json!(1)),
            inputs(json!([])),
            inputs(json!({})),
            Inputs::from([("unexpected".into(), true.into())]),
            Inputs::from([
                ("condition".into(), true.into()),
                ("extra".into(), true.into()),
            ]),
        ] {
            let error = node
                .execute(input, &mut ExecutionContext::default())
                .unwrap_err();
            assert!(matches!(
                error.source().unwrap().downcast_ref::<AssertionError>(),
                Some(AssertionError::InvalidCondition)
            ));
            assert!(!format!("{error:?}").contains("sensitive value"));
        }
    }

    #[test]
    fn cancellation_precedes_condition_validation() {
        let node = AssertNode {
            message: "assertion failed".into(),
        };
        let mut context = ExecutionContext::default();
        context.cancellation().cancel(StreamError::Execution {
            message: "cancel assertion".into(),
        });
        let error = node.execute(Inputs::new(), &mut context).unwrap_err();
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<StreamError>()
                .is_some()
        );
        assert!(error.to_string().contains("cancel assertion"));
    }
}
