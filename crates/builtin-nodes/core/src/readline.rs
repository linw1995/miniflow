use mf_runtime::{
    Emitter, ExecutionContext, InputResource, Inputs, NodeBuildError, NodeExecutionError,
    NodeFactory, NodeMetadata, NodePorts, NodeRegistration, Outputs, PortSpec, PreparedNode,
    StreamNode, TextInput, ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt;
use std::fs::File;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

struct Readline;

impl StreamNode for Readline {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        if let Some(path) = inputs.get("path") {
            let path = path.as_str().expect("validated string input");
            let file = File::open(path)
                .context(mf_runtime::StreamInputFileSnafu {
                    path: std::path::PathBuf::from(path),
                })
                .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> { Box::new(source) })
                .context(mf_runtime::NodePluginFailedSnafu)?;
            let mut input = TextInput::new(file);
            let cancellation = context.cancellation();
            while let Some(line) = input
                .next_line(&cancellation)
                .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> { Box::new(source) })
                .context(mf_runtime::NodePluginFailedSnafu)?
            {
                emitter.send(Outputs::from([("line".into(), line.into())]).into())?;
            }
        } else {
            while let Some(line) = context
                .stdin_line()
                .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> { Box::new(source) })
                .context(mf_runtime::NodePluginFailedSnafu)?
            {
                emitter.send(Outputs::from([("line".into(), line.into())]).into())?;
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
