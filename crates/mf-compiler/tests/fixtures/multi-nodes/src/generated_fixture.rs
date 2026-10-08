use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeRegistration, NodeValue, TypedConstructor,
    TypedNodeResult, TypedTaskHandle, TypedTaskNode,
};

#[derive(NodeValue)]
#[value(typed)]
pub struct Text {
    text: String,
}

struct Echo;

impl TypedTaskNode for Echo {
    type Input = Text;
    type Output = Text;

    fn execute(
        &self,
        input: Text,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<Text>, NodeExecutionError> {
        if cfg!(feature = "fail-execution") {
            return mf_runtime::NodeExecutionFailedSnafu {
                message: "typed execution sentinel",
            }
            .fail();
        }
        Ok(input.into())
    }
}

pub fn text(
    config: serde_json::Value,
) -> Result<TypedTaskHandle<impl TypedTaskNode<Input = Text, Output = Text>>, NodeBuildError> {
    let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
    TypedTaskHandle::new(
        Echo,
        mf_runtime::NodePorts::default(),
        TypedConstructor {
            package: env!("CARGO_PKG_NAME"),
            path: &["generated_fixture", "text"],
        },
        true,
    )
}

inventory::submit! {
    NodeRegistration { kind: "fixture.typed_text", factory: mf_runtime::NodeFactory::Plain(|config| Ok(text(config)?.prepared())) }
}
