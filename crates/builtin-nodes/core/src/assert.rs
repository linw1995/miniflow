use crate::condition::Condition;
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, PreparedNode, TaskNode, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};

pub const KIND: &str = "builtin.assert";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssertNode {
    condition: Condition,
    message: String,
}

#[derive(Debug, Snafu)]
#[snafu(display("{message}"))]
struct AssertionError {
    message: String,
}

impl From<AssertionError> for NodeExecutionError {
    fn from(source: AssertionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl NodePortContract for AssertNode {
    fn ports(&self) -> NodePorts {
        NodePorts::default()
    }
}

impl TaskNode for AssertNode {
    fn execute(
        &self,
        _inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(error) = context.cancellation().failure() {
            return Err(error.into());
        }
        ensure!(
            self.condition.evaluate(context)?,
            AssertionSnafu {
                message: &self.message
            }
        );
        Ok(NodeResult::default())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let node: AssertNode = deserialize_config(config)?;
    node.condition.validate()?;
    let metadata = NodeMetadata {
        context_references: vec![node.condition.reference("condition")],
        ..Default::default()
    };
    PreparedNode::new(node, metadata)
}

inventory::submit! { NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{ContextReference, NodeRegistry, StreamError};
    use serde_json::json;
    use std::error::Error;

    fn config() -> Value {
        json!({
            "condition": {"source":{"output":"source.value","path":""}, "operator":"eq", "value":true},
            "message":"current identity is inconsistent"
        })
    }

    #[test]
    fn registration_validates_conditions_and_declares_context_references() {
        let registry = NodeRegistry::from_inventory().unwrap();
        let registration = registry.get(KIND).unwrap();
        for (field, value) in [
            ("message", json!(false)),
            ("extra", json!(true)),
            ("condition", json!(true)),
            (
                "condition",
                json!({"source":{"output":"source.value","path":"bad~"},"operator":"exists"}),
            ),
            (
                "condition",
                json!({"source":{"output":"source.value","path":""},"operator":"gt","value":"1"}),
            ),
        ] {
            let mut candidate = config();
            candidate[field] = value;
            assert!(registration.instantiate(candidate).is_err());
        }
        for field in ["condition", "message"] {
            let mut candidate = config();
            candidate.as_object_mut().unwrap().remove(field);
            assert!(registration.instantiate(candidate).is_err());
        }
        let prepared = registration.instantiate(config()).unwrap();
        assert!(prepared.metadata.ports.inputs.is_empty());
        assert!(prepared.metadata.ports.outputs.is_empty());
        assert_eq!(
            prepared.metadata.context_references,
            vec![ContextReference::new("source.value", "condition")]
        );
    }

    #[test]
    fn cancellation_precedes_condition_evaluation() {
        let node: AssertNode = serde_json::from_value(config()).unwrap();
        let mut context = ExecutionContext::default();
        context.cancellation().cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "cancel assertion",
            }
            .build(),
        );
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
