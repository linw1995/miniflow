use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    TaskNode, ValueRef, ValueType,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}
struct Keys;
impl NodePortContract for Keys {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("object", ValueType::Object, true)],
            outputs: vec![PortSpec::new(
                "keys",
                ValueType::List(Box::new(ValueType::String)),
                true,
            )],
        }
    }
}
impl TaskNode for Keys {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let object = &inputs["object"];
        let keys = ValueRef::array(
            object
                .as_object()
                .expect("validated object")
                .keys()
                .map(|key| ValueRef::from(key.as_ref())),
        )
        .inherit_secrets([object]);
        Ok(Outputs::from([("keys".into(), keys)]).into())
    }
}
fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let _: Config = mf_runtime::deserialize_config(config)?;
    PreparedNode::new(Keys, NodeMetadata::default())
}
inventory::submit! {
    NodeRegistration { kind: "builtin.keys", factory: NodeFactory::Plain(factory) }
}
