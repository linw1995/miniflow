use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, Outputs, PortSpec, PreparedNode, StreamNode, ValueType,
};
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ReadLines {
    marker: Option<std::path::PathBuf>,
    delay_ms: u64,
}

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
        if let Some(marker) = &self.marker {
            std::fs::write(marker, b"executed").map_err(io_error)?;
        }
        let path = inputs
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| NodeExecutionError::ExecutionFailed {
                message: "input `path` must be a string".into(),
            })?;
        let file = File::open(path).map_err(io_error)?;
        for line in BufReader::new(file).lines() {
            if self.delay_ms != 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
            }
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
            let node: ReadLines = mf_runtime::deserialize_config(config)?;
            Ok(PreparedNode::stream(node, NodePorts {
                inputs: vec![PortSpec::new("path", ValueType::String, true)],
                outputs: vec![PortSpec::new("line", ValueType::String, true)],
            }))
        }),
    }
}
