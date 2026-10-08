use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeRegistration, NodeValue, TypedConstructor,
    TypedNodeResult, TypedTaskHandle, TypedTaskNode,
};

#[derive(NodeValue)]
#[value(typed)]
pub struct Text {
    text: String,
}

struct Echo {
    fail: bool,
}

impl TypedTaskNode for Echo {
    type Input = Text;
    type Output = Text;

    fn execute(
        &self,
        input: Text,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<Text>, NodeExecutionError> {
        if self.fail {
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
    let config: serde_json::Map<String, serde_json::Value> =
        mf_runtime::deserialize_config(config)?;
    TypedTaskHandle::new(
        Echo {
            fail: config
                .get("fail")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        },
        mf_runtime::NodePorts::default(),
        TypedConstructor {
            package: "fixture-multi-nodes",
            path: &["generated_fixture", "text"],
        },
        std::env::var_os("MF_FIXTURE_TYPED_GENERATION_DRIFT").is_none(),
    )
}

inventory::submit! {
    NodeRegistration { kind: "fixture.typed_text", factory: mf_runtime::NodeFactory::Plain(|config| {
        let bad_constructor = config.get("bad_constructor").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let bad_names = config.get("bad_names").and_then(serde_json::Value::as_bool).unwrap_or(false);
        let mut prepared = text(config)?.prepared();
        if bad_constructor { prepared.metadata.typed_generation.as_mut().unwrap().constructor.path = &["generated_fixture", "items"]; }
        if bad_names { prepared.metadata.typed_generation.as_mut().unwrap().constructor.path = &["generated_fixture", "renamed"]; }
        Ok(prepared)
    }) }
}

#[derive(NodeValue)]
#[value(typed)]
pub struct Items {
    items: Vec<i64>,
}
struct ItemsEcho;
impl TypedTaskNode for ItemsEcho {
    type Input = Items;
    type Output = Items;
    fn execute(
        &self,
        input: Items,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<Items>, NodeExecutionError> {
        Ok(input.into())
    }
}
pub fn items(
    config: serde_json::Value,
) -> Result<TypedTaskHandle<impl TypedTaskNode<Input = Items, Output = Items>>, NodeBuildError> {
    let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
    TypedTaskHandle::new(
        ItemsEcho,
        mf_runtime::NodePorts::default(),
        TypedConstructor {
            package: "fixture-multi-nodes",
            path: &["generated_fixture", "items"],
        },
        true,
    )
}
inventory::submit! {
    NodeRegistration { kind: "fixture.typed_items", factory: mf_runtime::NodeFactory::Plain(|config| Ok(items(config)?.prepared())) }
}

#[derive(NodeValue)]
#[value(typed)]
pub struct Renamed {
    #[value(rename = "other")]
    text: String,
}
struct RenamedEcho;
impl TypedTaskNode for RenamedEcho {
    type Input = Renamed;
    type Output = Renamed;
    fn execute(
        &self,
        input: Renamed,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<Renamed>, NodeExecutionError> {
        Ok(input.into())
    }
}
pub fn renamed(
    config: serde_json::Value,
) -> Result<TypedTaskHandle<impl TypedTaskNode<Input = Renamed, Output = Renamed>>, NodeBuildError>
{
    let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
    TypedTaskHandle::new(
        RenamedEcho,
        mf_runtime::NodePorts::default(),
        TypedConstructor {
            package: "fixture-multi-nodes",
            path: &["generated_fixture", "renamed"],
        },
        true,
    )
}
