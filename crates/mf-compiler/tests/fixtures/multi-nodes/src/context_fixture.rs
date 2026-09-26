use mf_runtime::{
    ContextReference, ContextValue, ExecutionContext, Inputs, Node, NodeBuildError,
    NodeExecutionError, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, ValueType,
};
use serde_json::Value;
use std::io::Write;

struct ContextNode(Value);
fn failure(message: impl Into<String>) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}
impl Node for ContextNode {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(failure("context required"))
    }
    fn ports(&self) -> Option<NodePorts> {
        Some(NodePorts {
            inputs: self.0["inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| PortSpec::owned(v.as_str().unwrap(), ValueType::Any, false))
                .collect(),
            outputs: self.0["ports"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| {
                    let name = v.as_str().unwrap();
                    PortSpec::owned(name, ValueType::Any, self.0["required"] == name)
                })
                .collect(),
        })
    }
    fn context_references(&self) -> Vec<ContextReference> {
        self.0["read"]
            .as_str()
            .map(|v| vec![ContextReference::new(v, "fixture")])
            .unwrap_or_default()
    }
    fn execute_with_context(
        &self,
        _: Inputs,
        ctx: &ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(path) = self.0["trace"].as_str() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(file, "{}", self.0["name"].as_str().unwrap()).unwrap();
        }
        if self.0["fail"] == true {
            return Err(failure("execution sentinel"));
        }
        let mut result = NodeResult::default();
        if let Some(values) = self.0["outputs"].as_object() {
            result.outputs.extend(values.clone());
        }
        if let Some(skips) = self.0["skipped"].as_array() {
            result
                .skipped
                .extend(skips.iter().map(|v| v.as_str().unwrap().to_owned()));
        }
        if let Some(reference) = self.0["read"].as_str() {
            match ctx.output(reference)? {
                ContextValue::Value(value) => {
                    result.outputs.insert("value".into(), value.clone());
                }
                ContextValue::Skipped => {
                    result.skipped.insert("value".into());
                }
            }
        }
        Ok(result)
    }
}
fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let config: serde_json::Map<String, Value> = mf_runtime::deserialize_config(config)?;
    Ok(Box::new(ContextNode(Value::Object(config))))
}
inventory::submit! { NodeRegistration { kind: "fixture.context", inputs: &[], outputs: &[], factory } }
