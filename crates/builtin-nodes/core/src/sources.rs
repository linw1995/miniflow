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

struct Source {
    item_type: ValueType,
    resource: InputResource,
}

fn execution_error(source: mf_runtime::StreamError) -> NodeExecutionError {
    NodeExecutionError::PluginFailed {
        source: Box::new(source),
    }
}

impl StreamNode for Source {
    fn execute(
        &mut self,
        _: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        loop {
            let value = match self.resource {
                InputResource::Stdin => context.stdin_next(&self.item_type),
                InputResource::Channel => context.channel_next(),
            }
            .map_err(execution_error)?;
            let Some(value) = value else {
                return Ok(());
            };
            emitter.send(Outputs::from([("item".into(), value)]).into())?;
            if self.resource == InputResource::Channel {
                context.channel_published().map_err(execution_error)?;
            }
        }
    }
}

fn factory(config: Value, resource: InputResource) -> Result<PreparedNode, NodeBuildError> {
    let Config { item_type } = deserialize_config(config)?;
    if resource == InputResource::Channel {
        return Ok(mf_runtime::channel_source(item_type));
    }
    let metadata = NodeMetadata {
        ports: NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("item", item_type.clone(), true)],
        },
        resources: vec![resource],
        ..Default::default()
    };
    Ok(PreparedNode::stream(
        Source {
            item_type,
            resource,
        },
        metadata,
    ))
}

inventory::submit! { NodeRegistration {
    kind: "builtin.stdin",
    factory: NodeFactory::Plain(|config| factory(config, InputResource::Stdin)),
} }
inventory::submit! { NodeRegistration {
    kind: "builtin.channel",
    factory: NodeFactory::Plain(|config| factory(config, InputResource::Channel)),
} }
