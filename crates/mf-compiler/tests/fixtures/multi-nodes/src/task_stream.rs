use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, TypedEmitter, TypedNodeResult, TypedTaskStreamNode,
};
use snafu::{ResultExt, Snafu};
use std::path::PathBuf;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("could not write fixture marker `{}`: {source}", path.display()))]
    WriteMarker {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("timed out waiting for fixture delivery gate `{}`", path.display()))]
    DeliveryGateTimeout { path: PathBuf },
    #[snafu(display("fixture iteration item {count} failed"))]
    ItemFailed { count: usize },
}

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

struct Count {
    wait_path: Option<String>,
    marker: Option<String>,
}

impl TypedTaskStreamNode for Count {
    type Input = Request;
    type Output = Reply;
    type Emission = Increment;

    fn execute(
        &self,
        input: Request,
        context: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Increment>,
    ) -> Result<TypedNodeResult<Reply>, NodeExecutionError> {
        if let Some(path) = &self.marker {
            std::fs::write(path, "executed")
                .context(WriteMarkerSnafu { path })
                .boxed()?;
        }
        if emitter.is_connected() {
            for delta in 0..input.count {
                emitter.send(Increment { delta })?;
                if delta == 0
                    && let Some(path) = &self.wait_path
                {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !std::path::Path::new(path).exists() {
                        if let Some(error) = context.cancellation().failure() {
                            return Err(error.into());
                        }
                        if std::time::Instant::now() >= deadline {
                            DeliveryGateTimeoutSnafu { path }.fail::<()>().boxed()?;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            }
        }
        Ok(Reply { total: input.count }.into())
    }
}

fn prepare(config: serde_json::Value) -> Result<PreparedNode, NodeBuildError> {
    let config: serde_json::Map<String, serde_json::Value> =
        mf_runtime::deserialize_config(config)?;
    let wait_path = config
        .get("wait_path")
        .map(|value| mf_runtime::deserialize_config(value.clone()))
        .transpose()?;
    let marker = config
        .get("marker")
        .map(|value| mf_runtime::deserialize_config(value.clone()))
        .transpose()?;
    let mut prepared =
        PreparedNode::typed_task_stream(Count { wait_path, marker }, NodeMetadata::default())?;
    // Exercise rejection of providers that violate preparation stability.
    if std::env::var_os("MF_FIXTURE_STREAM_CONTRACT_CHANGE").is_some() {
        prepared.metadata.stream_outputs[0].value_type = mf_runtime::ValueType::String;
    }
    Ok(prepared)
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
            .context(WriteMarkerSnafu { path: &self.0 })
            .boxed()?;
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IterationProbe {
    #[serde(default)]
    wait_first: Option<PathBuf>,
    #[serde(default)]
    marker_dir: Option<PathBuf>,
    #[serde(default)]
    fail_count: Option<usize>,
}

impl TypedTaskStreamNode for IterationProbe {
    type Input = Request;
    type Output = Reply;
    type Emission = Increment;

    fn execute(
        &self,
        input: Request,
        context: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Increment>,
    ) -> Result<TypedNodeResult<Reply>, NodeExecutionError> {
        if let Some(directory) = &self.marker_dir {
            let path = directory.join(input.count.to_string());
            std::fs::write(&path, "started")
                .context(WriteMarkerSnafu { path })
                .boxed()?;
        }
        if emitter.is_connected() {
            if input.count == 1
                && let Some(path) = &self.wait_first
            {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !path.exists() {
                    if let Some(error) = context.cancellation().failure() {
                        return Err(error.into());
                    }
                    if std::time::Instant::now() >= deadline {
                        DeliveryGateTimeoutSnafu { path }.fail::<()>().boxed()?;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            emitter.send(Increment { delta: input.count })?;
        }
        if self.fail_count == Some(input.count) {
            ItemFailedSnafu { count: input.count }
                .fail::<()>()
                .boxed()?;
        }
        Ok(Reply { total: input.count }.into())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.iteration_stream",
        factory: NodeFactory::Plain(|config| {
            let node = mf_runtime::deserialize_config(config)?;
            let mut prepared = PreparedNode::typed_task_stream::<IterationProbe>(node, NodeMetadata::default())?;
            if std::env::var_os("MF_FIXTURE_STREAM_CONTRACT_CHANGE").is_some() {
                prepared.metadata.stream_outputs[0].value_type = mf_runtime::ValueType::String;
            }
            Ok(prepared)
        }),
    }
}
