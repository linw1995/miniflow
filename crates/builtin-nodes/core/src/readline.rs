use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePorts, NodeRegistration, NodeValue, PreparedNode, StdinRequirement,
    StreamNode, TextInput, deserialize_config, encode_typed_result,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::{error::Error, fs::OpenOptions, io, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

struct Readline;

#[derive(NodeValue)]
struct ReadlineInputs {
    path: Option<String>,
}

#[derive(NodeValue)]
struct ReadlineOutputs {
    line: String,
}

#[derive(Debug, Snafu)]
enum ReadlineError {
    #[snafu(display("could not decode readline inputs: {source}"))]
    DecodeInputs {
        source: mf_runtime::InputDecodeError,
    },
    #[snafu(display("could not open text file {path:?}: {source}"))]
    InputFile { path: PathBuf, source: io::Error },
    #[snafu(display("could not inspect text file {path:?}: {source}"))]
    InputMetadata { path: PathBuf, source: io::Error },
    #[snafu(display("readline path {path:?} requires a regular file"))]
    UnsupportedFileType { path: PathBuf },
}

impl From<ReadlineError> for NodeExecutionError {
    fn from(source: ReadlineError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

impl StreamNode for Readline {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let inputs = ReadlineInputs::from_values(inputs).context(DecodeInputsSnafu)?;
        if let Some(path) = inputs.path {
            let path = path.as_str();
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // Reject FIFOs without waiting for a writer, and avoid claiming a terminal.
                options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
            }
            let file = options.open(path).context(InputFileSnafu { path })?;
            let metadata = file.metadata().context(InputMetadataSnafu { path })?;
            // Inspect the opened descriptor so path replacement cannot bypass the contract.
            ensure!(metadata.is_file(), UnsupportedFileTypeSnafu { path });
            let mut input = TextInput::new(file);
            let cancellation = context.cancellation();
            while let Some(line) = input.next_line(&cancellation)? {
                emitter.send(encode_typed_result(ReadlineOutputs { line }.into())?)?;
            }
        } else {
            while let Some(line) = context.stdin_line()? {
                emitter.send(encode_typed_result(ReadlineOutputs { line }.into())?)?;
            }
        }
        Ok(())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let _: Config = deserialize_config(config)?;
    Ok(PreparedNode::stream(
        Readline,
        NodeMetadata {
            stdin: Some(StdinRequirement::UnlessInput("path".into())),
            ..NodeMetadata::new(NodePorts {
                inputs: ReadlineInputs::ports(),
                outputs: ReadlineOutputs::ports(),
            })
        },
    ))
}

inventory::submit! { NodeRegistration { kind: "builtin.readline", factory: NodeFactory::Plain(factory) } }
