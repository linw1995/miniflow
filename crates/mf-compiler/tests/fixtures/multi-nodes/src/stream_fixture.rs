use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs, PortSpec, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use std::{io::Write, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    trace: PathBuf,
}
struct Echo(Config);

fn record(path: &PathBuf, event: &str) -> std::io::Result<()> {
    writeln!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?,
        "{event}"
    )
}

impl mf_runtime::TaskNode for Echo {
    fn execute(
        &self,
        mut inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        record(&self.0.trace, "execute").map_err(|source| NodeExecutionError::PluginFailed {
            source: Box::new(source),
        })?;
        println!("execution diagnostic");
        Ok((Outputs::from([("value".into(), inputs.remove("input").unwrap())])).into())
    }
}

fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: Config = mf_runtime::deserialize_config(config)?;
    record(&config.trace, "prepare").map_err(|source| NodeBuildError::FactoryFailed {
        source: Box::new(source),
    })?;
    println!("factory diagnostic");
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Echo(config))),
        mf_runtime::NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Any, true)],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        },
    ))
}

inventory::submit! { NodeRegistration { kind: "fixture.stream_echo", factory: mf_runtime::NodeFactory::Plain(factory) } }
