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

#[derive(NodeValue)]
struct Empty {}

struct Feed(Vec<usize>);

impl mf_runtime::TypedStreamNode for Feed {
    type Input = Empty;
    type Output = Request;

    fn execute(
        &mut self,
        _: Empty,
        _: &mut ExecutionContext,
        emit: &mut dyn FnMut(TypedNodeResult<Request>) -> Result<(), NodeExecutionError>,
    ) -> Result<(), NodeExecutionError> {
        for count in &self.0 {
            emit(Request { count: *count }.into())?;
        }
        Ok(())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_feed",
        factory: NodeFactory::Plain(|config| {
            let counts: Vec<usize> = mf_runtime::deserialize_config(config["counts"].clone())?;
            PreparedNode::typed_stream(Feed(counts), NodeMetadata::default())
        }),
    }
}

struct Finish(String);

impl mf_runtime::TypedTaskNode for Finish {
    type Input = Reply;
    type Output = Empty;

    fn execute(
        &self,
        input: Reply,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Empty>, NodeExecutionError> {
        std::fs::write(&self.0, input.total.to_string())
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
        Ok(Empty {}.into())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_finish",
        factory: NodeFactory::Plain(|config| {
            let path: String = mf_runtime::deserialize_config(config["path"].clone())?;
            PreparedNode::typed_task(Finish(path), NodeMetadata::default())
        }),
    }
}

struct InvalidTerminal;

struct Panic;

impl TypedTaskStreamNode for Panic {
    type Input = Request;
    type Output = Reply;
    type Emission = Increment;

    fn execute(
        &self,
        _: Request,
        _: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Increment>,
    ) -> Result<TypedNodeResult<Reply>, NodeExecutionError> {
        emitter.send(Increment { delta: 0 })?;
        panic!("combined panic sentinel");
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_panic",
        factory: NodeFactory::Plain(|_| PreparedNode::typed_task_stream(Panic, NodeMetadata::default())),
    }
}

impl mf_runtime::NodePortContract for InvalidTerminal {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts::from_types::<Request, Reply>()
    }

    fn stream_outputs(&self) -> Vec<mf_runtime::PortSpec> {
        <Increment as mf_runtime::NodeValue>::ports()
    }
}

impl mf_runtime::TaskStreamNode for InvalidTerminal {
    fn execute(
        &self,
        _: mf_runtime::Inputs,
        _: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        emitter.send(mf_runtime::encode_typed_result(
            Increment { delta: 0 }.into(),
        )?)?;
        Ok(mf_runtime::Outputs::from([("total".into(), "invalid".into())]).into())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_invalid_terminal",
        factory: NodeFactory::Plain(|_| PreparedNode::task_stream(InvalidTerminal, NodeMetadata::default())),
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.task_stream_join",
        factory: NodeFactory::Plain(|_| PreparedNode::typed_task(Join, NodeMetadata::default())),
    }
}
