use mf_runtime::{
    ExecutionContext, InputResource, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePorts, NodeRegistration, Outputs, PortSpec, PreparedNode, StreamNode,
    ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    item_type: ValueType,
}

struct StdinSource {
    item_type: ValueType,
}

fn execution_error(source: mf_runtime::StreamError) -> NodeExecutionError {
    NodeExecutionError::PluginFailed {
        source: Box::new(source),
    }
}

impl StreamNode for StdinSource {
    fn execute(
        &mut self,
        _: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        while let Some(value) = context
            .stdin_next(&self.item_type)
            .map_err(execution_error)?
        {
            emitter.send(Outputs::from([("item".into(), value)]).into())?;
        }
        Ok(())
    }
}

fn stdin_source(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let Config { item_type } = deserialize_config(config)?;
    let metadata = NodeMetadata {
        ports: NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("item", item_type.clone(), true)],
        },
        resources: vec![InputResource::Stdin],
        ..Default::default()
    };
    Ok(PreparedNode::stream(StdinSource { item_type }, metadata))
}

inventory::submit! { NodeRegistration {
    kind: "builtin.stdin",
    factory: NodeFactory::Plain(stdin_source),
} }
inventory::submit! { NodeRegistration {
    kind: "builtin.channel",
    factory: NodeFactory::Plain(|config| {
        let Config { item_type } = deserialize_config(config)?;
        Ok(mf_runtime::channel_source(item_type))
    }),
} }
