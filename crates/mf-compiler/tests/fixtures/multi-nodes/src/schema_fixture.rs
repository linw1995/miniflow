use mf_runtime::*;
use serde_json::{Value, json};

#[derive(NodeEnum)]
enum Mode {
    #[value(rename = "fast")]
    Fast,
    #[value(rename = "thorough")]
    Thorough,
}
#[derive(NodeValue)]
struct Payload {
    #[value(description = "Search text")]
    query: String,
    mode: Mode,
    limit: Option<u64>,
}
#[derive(NodeValue)]
struct ChangedPayload {
    query: String,
    mode: Mode,
    limit: Option<u64>,
    filter: String,
}
struct Credential(String);
impl InputValue for Credential {
    fn value_type() -> ValueType {
        ValueType::String
    }
    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        <String as InputValue>::decode(value).map(Self)
    }
}
impl InputField for Credential {
    const REQUIRED: bool = true;
    fn value_type() -> ValueType {
        ValueType::String
    }
    fn decode_field(port: &str, value: Option<ValueRef>) -> Result<Self, InputDecodeError> {
        decode_required_input(port, value)
    }
}
#[derive(NodeInputs)]
struct ToolInputs {
    payload: Payload,
    token: Credential,
}
#[derive(NodeInputs)]
struct ChangedInputs {
    payload: ChangedPayload,
    token: Credential,
}
#[derive(NodeOutputs)]
struct ToolOutputs {
    value: String,
}
struct Tool;
impl TypedTaskNode for Tool {
    type Input = ToolInputs;
    type Output = ToolOutputs;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let _ = input.token.0;
        Ok(ToolOutputs {
            value: input.payload.query,
        }
        .into())
    }
}
struct ChangedTool;
impl TypedTaskNode for ChangedTool {
    type Input = ChangedInputs;
    type Output = ToolOutputs;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        let _ = input.token.0;
        Ok(ToolOutputs {
            value: input.payload.query,
        }
        .into())
    }
}
struct Caller;
impl TaskNode for Caller {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let interface = context
            .call_targets()
            .iter()
            .find(|call| call.name() == "lookup")
            .unwrap()
            .clone();
        let arguments = CallArguments {
            outputs: Outputs::from([(
                "request".into(),
                json!({"query":"find","mode":"fast"}).into(),
            )]),
            ..Default::default()
        };
        interface.validate_decoded_arguments(&arguments)?;
        match context.invoke_call(interface.id(), arguments)?.outcome {
            CallOutcome::Succeeded(outputs) => Ok(outputs.into()),
            CallOutcome::Failed { source } => Err(source.into()),
            CallOutcome::Skipped { .. } => panic!("unexpected skip"),
        }
    }
}
fn tool(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let metadata = NodeMetadata {
        input_bindings: [(
            "token".into(),
            InputBinding {
                secret: true,
                ..Default::default()
            },
        )]
        .into(),
        ..Default::default()
    };
    if config["allow_drift"].as_bool() == Some(true)
        && std::env::var_os("MINIFLOW_TEST_SCHEMA_DRIFT").is_some()
    {
        PreparedNode::typed_task(ChangedTool, metadata)
    } else {
        PreparedNode::typed_task(Tool, metadata)
    }
}
fn caller(_: Value) -> Result<PreparedNode, NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(Caller)),
        NodeMetadata {
            ports: NodePorts {
                inputs: vec![],
                outputs: vec![
                    PortSpec::new("value", ValueType::String, true),
                    PortSpec::new("request", ValueType::Any, false),
                ],
            },
            ..Default::default()
        },
    ))
}
inventory::submit! { NodeRegistration { kind:"fixture.schema_tool", factory:NodeFactory::Plain(tool) } }
inventory::submit! { NodeRegistration { kind:"fixture.schema_caller", factory:NodeFactory::Plain(caller) } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_contract_changes_leave_coarse_ports_equal() {
        let first = PreparedNode::typed_task(Tool, NodeMetadata::default())
            .unwrap()
            .metadata;
        let second = PreparedNode::typed_task(ChangedTool, NodeMetadata::default())
            .unwrap()
            .metadata;
        assert_eq!(first.ports, second.ports);
        assert_ne!(first.input_schemas, second.input_schemas);
        assert!(verify_generated_metadata(&second, &serde_json::to_value(first).unwrap()).is_err());
    }
}
