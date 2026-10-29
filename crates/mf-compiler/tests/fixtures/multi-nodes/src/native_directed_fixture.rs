use mf_runtime::*;
use serde_json::{Value, json};
use std::time::Duration;

struct Source(Vec<String>);
impl NodePortContract for Source {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("done", ValueType::Boolean, true)],
        }
    }
    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("item", ValueType::String, true)]
    }
}
impl TaskStreamNode for Source {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        for item in &self.0 {
            emitter.send(Outputs::from([("item".into(), item.clone().into())]).into())?;
        }
        Ok(Outputs::from([("done".into(), true.into())]).into())
    }
}
struct Config {
    delay: u64,
    skip: bool,
    empty: bool,
    secret: bool,
}
impl TaskNode for Config {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        std::thread::sleep(Duration::from_millis(self.delay));
        if self.empty {
            return Ok(NodeResult::default());
        }
        if self.skip {
            Ok(NodeResult {
                skipped: ["token".into()].into(),
                ..Default::default()
            })
        } else {
            let token = ValueRef::from("config");
            let token = if self.secret {
                token.into_secret()
            } else {
                token
            };
            Ok(Outputs::from([("token".into(), token)]).into())
        }
    }
}
struct Caller;
impl NodePortContract for Caller {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("message", ValueType::String, true)],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        }
    }
    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("increment", ValueType::Any, true)]
    }
}
impl TaskStreamNode for Caller {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        let interface = context
            .call_targets()
            .iter()
            .find(|call| call.name() == "search")
            .unwrap()
            .clone();
        let mut last = Value::Null;
        for _ in 0..2 {
            let arguments = CallArguments {
                outputs: interface
                    .outputs()
                    .iter()
                    .map(|parameter| (parameter.name().into(), inputs["message"].clone()))
                    .collect(),
                defaults: interface
                    .defaults()
                    .iter()
                    .map(|parameter| (parameter.name().into(), inputs["message"].clone()))
                    .collect(),
            };
            let response = context.invoke_call(interface.id(), arguments)?;
            last = match response.outcome {
                CallOutcome::Succeeded(outputs) => {
                    json!({"text":outputs["text"], "invocation":response.id.invocation})
                }
                CallOutcome::Skipped { .. } => {
                    json!({"skipped":true,"invocation":response.id.invocation})
                }
                CallOutcome::Failed { source } => return Err(source.into()),
            };
            emitter.send(Outputs::from([("increment".into(), last.clone().into())]).into())?;
        }
        Ok(Outputs::from([("value".into(), last.into())]).into())
    }
}
fn source(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let values = config
        .get("values")
        .cloned()
        .unwrap_or_else(|| json!(["one", "two", "three", "four"]));
    let values = deserialize_config(values)?;
    PreparedNode::task_stream(Source(values), NodeMetadata::default())
}
fn config(value: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(Config {
            delay: value["delay_ms"].as_u64().unwrap_or(30),
            skip: value["skip"].as_bool().unwrap_or(false),
            empty: value["empty"].as_bool().unwrap_or(false),
            secret: value["secret"].as_bool().unwrap_or(false),
        })),
        NodeMetadata::new(NodePorts {
            inputs: vec![],
            outputs: if value["empty"].as_bool() == Some(true) {
                vec![]
            } else {
                vec![PortSpec::new("token", ValueType::String, false)]
            },
        }),
    ))
}
fn caller(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let call_type = if config["allow_drift"].as_bool() == Some(true)
        && std::env::var_os("MINIFLOW_TEST_CALL_PORT_DRIFT").is_some()
    {
        ValueType::Int64
    } else {
        ValueType::String
    };
    let mut prepared = PreparedNode::task_stream(Caller, NodeMetadata::default())?;
    prepared
        .metadata
        .ports
        .outputs
        .push(PortSpec::new("query", call_type, false));
    Ok(prepared)
}
inventory::submit! { NodeRegistration { kind:"fixture.directed_source", factory:NodeFactory::Plain(source) } }
inventory::submit! { NodeRegistration { kind:"fixture.directed_config", factory:NodeFactory::Plain(config) } }
inventory::submit! { NodeRegistration { kind:"fixture.directed_stream_caller", factory:NodeFactory::Plain(caller) } }

struct Tool;
impl NodePortContract for Tool {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("query", ValueType::String, true),
                PortSpec::new("token", ValueType::String, true),
            ],
            outputs: vec![PortSpec::new("text", ValueType::String, true)],
        }
    }
    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("progress", ValueType::String, true)]
    }
}
impl TaskStreamNode for Tool {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        assert!(!emitter.is_connected());
        emitter.send(Outputs::from([("progress".into(), "working".into())]).into())?;
        Ok(Outputs::from([(
            "text".into(),
            format!(
                "{}{}",
                inputs["query"].as_str().unwrap(),
                inputs["token"].as_str().unwrap()
            )
            .into(),
        )])
        .into())
    }
}
inventory::submit! { NodeRegistration { kind:"fixture.directed_stream_tool", factory:NodeFactory::Plain(|_| PreparedNode::task_stream(Tool, NodeMetadata::default())) } }
