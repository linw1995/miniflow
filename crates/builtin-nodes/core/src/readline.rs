use mf_runtime::{
    Emitter, ExecutionContext, InputResource, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeMetadata, NodePorts, NodeRegistration, Outputs, PortSpec, PreparedNode,
    StreamError, StreamNode, TextInput, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu};
use std::{error::Error, fs::File, io, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

struct Readline;

#[derive(Debug, Snafu)]
enum ReadlineError {
    #[snafu(display("readline path input must be a string"))]
    InvalidPath,
    #[snafu(display("could not open text file {path:?}: {source}"))]
    InputFile { path: PathBuf, source: io::Error },
    #[snafu(transparent)]
    Stream { source: StreamError },
}

impl StreamNode for Readline {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let mut execute = || -> Result<(), Box<dyn Error + Send + Sync>> {
            if let Some(path) = inputs.get("path") {
                let path = path.as_str().context(InvalidPathSnafu)?;
                let file = File::open(path).context(InputFileSnafu {
                    path: PathBuf::from(path),
                })?;
                let mut input = TextInput::new(file);
                let cancellation = context.cancellation();
                while let Some(line) = input.next_line(&cancellation)? {
                    emitter.send(Outputs::from([("line".into(), line.into())]).into())?;
                }
            } else {
                while let Some(line) = context.stdin_line()? {
                    emitter.send(Outputs::from([("line".into(), line.into())]).into())?;
                }
            }
            Ok(())
        };
        execute().context(mf_runtime::NodePluginFailedSnafu)
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let _: Config = deserialize_config(config)?;
    Ok(PreparedNode::stream(
        Readline,
        NodeMetadata {
            ports: NodePorts {
                inputs: vec![PortSpec::new("path", ValueType::String, false)],
                outputs: vec![PortSpec::new("line", ValueType::String, true)],
            },
            resources: vec![InputResource::StdinIfMissing("path".into())],
            ..Default::default()
        },
    ))
}

inventory::submit! { NodeRegistration { kind: "builtin.readline", factory: NodeFactory::Plain(factory) } }
