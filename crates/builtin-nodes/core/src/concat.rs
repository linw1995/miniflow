use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeValue, TypedConstructor, TypedTaskHandle,
    TypedTaskNode, ValueRef,
};
use serde_json::Value;

pub const KIND: &str = "builtin.concat";

#[derive(NodeValue)]
#[value(typed)]
pub struct ConcatInputs {
    pub left: Vec<ValueRef>,
    pub right: Vec<ValueRef>,
}

#[derive(NodeValue)]
#[value(typed)]
pub struct ConcatOutputs {
    pub value: Vec<ValueRef>,
}

struct ConcatNode;

impl TypedTaskNode for ConcatNode {
    type Input = ConcatInputs;
    type Output = ConcatOutputs;

    fn execute(
        &self,
        mut inputs: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
        inputs.left.append(&mut inputs.right);
        Ok(ConcatOutputs { value: inputs.left }.into())
    }
}

pub fn prepare_concat(
    _: Value,
) -> Result<
    TypedTaskHandle<impl TypedTaskNode<Input = ConcatInputs, Output = ConcatOutputs>>,
    NodeBuildError,
> {
    TypedTaskHandle::new(
        ConcatNode,
        mf_runtime::NodeMetadata::default(),
        TypedConstructor {
            package: env!("CARGO_PKG_NAME"),
            path: &["prepare_concat"],
        },
        true,
    )
}

inventory::submit! {
    mf_runtime::NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(|config| {
        Ok(prepare_concat(config)?.prepared())
    }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concatenation_preserves_order_aliases_and_secret_values() {
        let value = ValueRef::from(serde_json::json!({"nested":[1,"two"]})).into_secret();
        let node = prepare_concat(serde_json::json!({})).unwrap().prepared();
        let output = node
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                mf_runtime::NodeValues::from([
                    ("left".into(), ValueRef::array([value.clone()])),
                    ("right".into(), ValueRef::array([value.clone(), 3.into()])),
                ]),
                &mut Default::default(),
            )
            .unwrap()
            .outputs;
        let values = output["value"].as_array().unwrap();
        assert_eq!(values.len(), 3);
        assert!(values[0].ptr_eq(&value));
        assert!(values[1].contains_secret());
        assert_eq!(values[2], serde_json::json!(3));
    }
}
