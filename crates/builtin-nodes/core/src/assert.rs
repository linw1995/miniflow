use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, OutputDerivation, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueType, deserialize_config,
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
            outputs: vec![PortSpec::new("passed", ValueType::Boolean, true)],
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
            .ok_or_else(|| InvalidConditionSnafu.build())?;
        let passed = condition
            .as_bool()
            .ok_or_else(|| InvalidConditionSnafu.build())?;
        ensure!(
            passed,
            FailedSnafu {
                message: &self.message
            }
        );
        Ok(Outputs::from([("passed".into(), condition.clone())]).into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let node: AssertNode = deserialize_config(config)?;
    PreparedNode::new(
        node,
        NodeMetadata {
            output_derivations: vec![OutputDerivation::forward_input("passed", "condition")],
            ..Default::default()
        },
    )
}

inventory::submit! { NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::ValueRef;
    use serde_json::json;

    #[test]
    fn asserts_without_losing_secrets_or_disclosing_failed_values() {
        let node = AssertNode {
            message: "current identity is inconsistent".into(),
        };
        let condition = ValueRef::from(true).into_secret();
        let result = node
            .execute(
                Inputs::from([("condition".into(), condition.clone())]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert!(result.outputs["passed"].ptr_eq(&condition));
        assert!(result.outputs["passed"].is_secret());
        for value in [json!(false), json!(null), json!("sensitive value")] {
            let error = node
                .execute(
                    Inputs::from([("condition".into(), ValueRef::from(value).into_secret())]),
                    &mut ExecutionContext::default(),
                )
                .unwrap_err();
            assert!(!error.to_string().contains("sensitive value"));
            assert!(std::error::Error::source(&error).is_some());
        }
        let mut context = ExecutionContext::default();
        context
            .cancellation()
            .cancel(mf_runtime::StreamError::Execution {
                message: "cancel assertion".into(),
            });
        let error = node
            .execute(
                Inputs::from([("condition".into(), condition)]),
                &mut context,
            )
            .unwrap_err();
        assert!(error.to_string().contains("cancel assertion"));
    }
}
