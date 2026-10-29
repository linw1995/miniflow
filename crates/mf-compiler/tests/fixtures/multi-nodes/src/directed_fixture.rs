use mf_runtime::*;
use serde_json::Value;

struct Caller {
    alias: String,
    result: String,
    repeat: usize,
    capture_outputs: bool,
    omit_optional: bool,
}
impl TaskNode for Caller {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let interface = context
            .call_targets()
            .iter()
            .find(|call| call.name() == self.alias)
            .unwrap()
            .clone();
        let mut value = ValueRef::from("unselected");
        for _ in 0..self.repeat {
            let arguments = CallArguments {
                outputs: interface
                    .outputs()
                    .iter()
                    .filter(|parameter| !self.omit_optional || parameter.required())
                    .map(|parameter| (parameter.name().into(), "query:".into()))
                    .collect(),
                defaults: interface
                    .defaults()
                    .iter()
                    .map(|parameter| (parameter.name().into(), "default".into()))
                    .collect(),
            };
            let response = context.invoke_call(interface.id(), arguments)?;
            value = match response.outcome {
                CallOutcome::Succeeded(outputs) => {
                    if self.capture_outputs {
                        ValueRef::object(
                            outputs
                                .into_iter()
                                .map(|(name, value)| (name.into(), value)),
                        )
                    } else {
                        outputs[&self.result].clone()
                    }
                }
                CallOutcome::Skipped { .. } => "skipped".into(),
                CallOutcome::Failed { source } => return Err(source.into()),
            };
        }
        Ok(Outputs::from([("value".into(), value)]).into())
    }
}
fn caller(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let call_type = if config["allow_drift"].as_bool() == Some(true)
        && std::env::var_os("MINIFLOW_TEST_CALL_PORT_DRIFT").is_some()
    {
        ValueType::Int64
    } else {
        ValueType::String
    };
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(Caller {
            alias: config["alias"].as_str().unwrap_or("search").into(),
            result: config["result"].as_str().unwrap_or("text").into(),
            repeat: config["repeat"].as_u64().unwrap_or(1) as usize,
            capture_outputs: config["capture_outputs"].as_bool().unwrap_or(false),
            omit_optional: config["omit_optional"].as_bool().unwrap_or(false),
        })),
        NodeMetadata {
            ports: NodePorts {
                inputs: vec![],
                outputs: vec![
                    PortSpec::new("value", ValueType::Any, true),
                    PortSpec::new("query", call_type, false),
                ],
            },
            ..Default::default()
        },
    ))
}
inventory::submit! { NodeRegistration { kind: "test.directed_caller", factory: NodeFactory::Plain(caller) } }

#[derive(NodeValue)]
struct OptionalInputs {
    value: Option<String>,
}
#[derive(NodeValue)]
struct OptionalOutputs {
    value: String,
}
struct OptionalTool;
impl TypedTaskNode for OptionalTool {
    type Input = OptionalInputs;
    type Output = OptionalOutputs;
    fn execute(
        &self,
        input: OptionalInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<OptionalOutputs>, NodeExecutionError> {
        Ok(OptionalOutputs {
            value: input.value.unwrap_or_else(|| "fallback".into()),
        }
        .into())
    }
}
fn optional_tool(_: Value) -> Result<PreparedNode, NodeBuildError> {
    PreparedNode::typed_task(OptionalTool, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "fixture.optional_tool", factory: NodeFactory::Plain(optional_tool) } }
