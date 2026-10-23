use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, TypedEmitter, TypedNodeResult, TypedTaskStreamNode,
};

#[derive(NodeValue)]
struct Request {
    count: usize,
}

#[derive(NodeValue)]
struct Reply {
    total: usize,
}

#[derive(NodeValue)]
struct Increment {
    delta: usize,
}

struct Count;

impl TypedTaskStreamNode for Count {
    type Input = Request;
    type Output = Reply;
    type Emission = Increment;

    fn execute(
        &self,
        input: Request,
        _: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Increment>,
    ) -> Result<TypedNodeResult<Reply>, NodeExecutionError> {
        if emitter.is_connected() {
            for delta in 0..input.count {
                emitter.send(Increment { delta })?;
            }
        }
        Ok(Reply { total: input.count }.into())
    }
}

fn prepare(config: serde_json::Value) -> Result<PreparedNode, NodeBuildError> {
    let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
    PreparedNode::typed_task_stream(Count, NodeMetadata::default())
}

inventory::submit! {
    NodeRegistration { kind: "fixture.task_stream", factory: NodeFactory::Plain(prepare) }
}

#[derive(NodeValue)]
struct JoinInput {
    delta: usize,
    total: usize,
}

struct Join;

impl mf_runtime::TypedTaskNode for Join {
    type Input = JoinInput;
    type Output = Reply;

    fn execute(
        &self,
        input: JoinInput,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Reply>, NodeExecutionError> {
        Ok(Reply {
            total: input.delta + input.total,
        }
        .into())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_join",
        factory: NodeFactory::Plain(|_| PreparedNode::typed_task(Join, NodeMetadata::default())),
    }
}
