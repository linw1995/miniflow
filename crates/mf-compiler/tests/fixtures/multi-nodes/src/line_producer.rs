use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, Outputs, PortSpec, PreparedNode, StreamNode, ValueType,
};
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

struct ReadLines;

fn io_error(source: std::io::Error) -> NodeExecutionError {
    NodeExecutionError::PluginFailed {
        source: Box::new(source),
    }
}

impl StreamNode for ReadLines {
    fn execute(
        &mut self,
        inputs: Inputs,
        _context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let path = inputs
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "input `path` must be a string".into(),
            })?;
        let file = File::open(path).map_err(io_error)?;
        for line in BufReader::new(file).lines() {
            emitter
                .send(Outputs::from([("line".into(), line.map_err(io_error)?.into())]).into())?;
        }
        Ok(())
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.read_lines",
        factory: NodeFactory::Plain(|config| {
            let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
            Ok(PreparedNode::stream(ReadLines, NodePorts {
                inputs: vec![PortSpec::new("path", ValueType::String, true)],
                outputs: vec![PortSpec::new("line", ValueType::String, true)],
            }))
        }),
    }
}
