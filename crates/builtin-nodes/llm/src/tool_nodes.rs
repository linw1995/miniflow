use crate::{CallOutcome, ToolCall, ToolResult, tools};
use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract, NodePorts,
    NodeRegistration, NodeResult, NodeValue, PortSpec, PreparedNode, TaskNode, ValueRef, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Snafu)]
pub enum ToolResultsError {
    #[snafu(display("tool results require unique nonblank call IDs"))]
    Identity,
    #[snafu(display("tool `{tool}` produced {actual} results for {expected} requests"))]
    Count {
        tool: String,
        actual: usize,
        expected: usize,
    },
    #[snafu(transparent)]
    Encoding { source: tools::ToolEncodingError },
}
impl From<ToolResultsError> for NodeExecutionError {
    fn from(source: ToolResultsError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(NodeValue)]
struct ResultInputs {
    outcome: CallOutcome,
    tool_calls: Vec<ToolCall>,
}
#[derive(NodeValue)]
struct ResultOutputs {
    tool_results: Vec<ToolResult>,
}
struct ResultsNode {
    inputs: Vec<String>,
}
impl NodePortContract for ResultsNode {
    fn ports(&self) -> NodePorts {
        let mut ports = NodePorts::from_types::<ResultInputs, ResultOutputs>();
        ports.inputs.extend(
            self.inputs
                .iter()
                .map(|name| PortSpec::owned(name, ValueType::List(Box::new(ValueType::Any)), true)),
        );
        ports
    }
}
impl TaskNode for ResultsNode {
    fn execute(
        &self,
        mut inputs: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut values = BTreeMap::new();
        for name in &self.inputs {
            let list: Vec<ValueRef> =
                mf_runtime::decode_required_input(name, inputs.remove(name)).boxed()?;
            values.insert(name.as_str(), VecDeque::from(list));
        }
        let input = mf_runtime::decode_node_inputs::<ResultInputs>(inputs)?;
        if !matches!(input.outcome, CallOutcome::ToolCalls) {
            return mf_runtime::encode_typed_result(
                ResultOutputs {
                    tool_results: Vec::new(),
                }
                .into(),
            );
        }
        let mut ids = BTreeSet::new();
        ensure!(
            input
                .tool_calls
                .iter()
                .all(|call| !call.id.trim().is_empty() && ids.insert(&call.id)),
            IdentitySnafu
        );
        for (tool, results) in &values {
            let expected = input
                .tool_calls
                .iter()
                .filter(|call| call.name == *tool)
                .count();
            ensure!(
                results.len() == expected,
                CountSnafu {
                    tool: *tool,
                    actual: results.len(),
                    expected
                }
            );
        }
        let mut results = Vec::new();
        for call in &input.tool_calls {
            let content = if let Some(values) = values.get_mut(call.name.as_str()) {
                let value = values
                    .pop_front()
                    .expect("validated result count")
                    .redacted();
                tools::bounded_json(&value, tools::MAX_TOOL_RESULT_BYTES)
                    .map_err(ToolResultsError::from)?
            } else {
                "{\"error\":\"no workflow result is connected for this tool\"}".into()
            };
            results.push(ToolResult {
                tool_call_id: call.id.clone(),
                content,
            });
        }
        mf_runtime::encode_typed_result(
            ResultOutputs {
                tool_results: results,
            }
            .into(),
        )
    }
}
pub fn prepare_tool_results(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        inputs: Vec<String>,
    }
    let config: Config = mf_runtime::deserialize_config(config)?;
    let mut names = BTreeSet::new();
    for name in &config.inputs {
        ensure!(
            crate::call::valid_name(name)
                && !matches!(name.as_str(), "outcome" | "tool_calls")
                && names.insert(name),
            mf_runtime::NodeInvalidSubgraphSnafu {
                message: "tool result inputs must have unique non-reserved tool names"
            }
        );
    }
    PreparedNode::new(
        ResultsNode {
            inputs: config.inputs,
        },
        NodeMetadata::default(),
    )
}
inventory::submit! { NodeRegistration { kind: "builtin.tool_results", factory: mf_runtime::NodeFactory::Plain(prepare_tool_results) } }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn execute(calls: Value, values: Value) -> Result<NodeResult, NodeExecutionError> {
        let node = prepare_tool_results(json!({"inputs":["multiply"]})).unwrap();
        node.execution.as_task_node().unwrap().execute(
            Inputs::from([
                ("outcome".into(), "tool_calls".into()),
                ("tool_calls".into(), calls.into()),
                ("multiply".into(), values.into()),
            ]),
            &mut mf_runtime::ExecutionContext::default(),
        )
    }
    #[test]
    fn ordinary_results_correlate_repeated_tools_in_model_response_order() {
        let result = execute(
            json!([
                {"id":"first","name":"multiply","arguments":{}},
                {"id":"other","name":"external","arguments":{}},
                {"id":"second","name":"multiply","arguments":{}}
            ]),
            json!([{"value":2},{"value":4}]),
        )
        .unwrap();
        let value = serde_json::to_value(&result.outputs["tool_results"]).unwrap();
        assert_eq!(value[0]["tool_call_id"], "first");
        assert_eq!(value[2]["tool_call_id"], "second");
        assert_eq!(
            serde_json::from_str::<Value>(value[2]["content"].as_str().unwrap()).unwrap(),
            json!({"value":4})
        );
    }
    #[test]
    fn result_count_mismatches_and_duplicate_ids_are_rejected() {
        let calls = json!([{"id":"first","name":"multiply","arguments":{}}]);
        assert!(execute(calls.clone(), json!([])).is_err());
        assert!(execute(calls, json!([1, 2])).is_err());
        assert!(execute(json!([{"id":"same","name":"multiply","arguments":{}},{"id":"same","name":"multiply","arguments":{}}]),json!([1,2])).is_err());
    }
    #[test]
    fn serialization_redacts_sensitive_values_from_connected_results() {
        let node = prepare_tool_results(json!({"inputs":["multiply"]})).unwrap();
        let result = node
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([
                    ("outcome".into(), "tool_calls".into()),
                    (
                        "tool_calls".into(),
                        json!([{"id":"first","name":"multiply","arguments":{}}]).into(),
                    ),
                    (
                        "multiply".into(),
                        ValueRef::array(vec![ValueRef::from("private-value").into_secret()]),
                    ),
                ]),
                &mut mf_runtime::ExecutionContext::default(),
            )
            .unwrap();
        assert!(
            !serde_json::to_string(&result.outputs)
                .unwrap()
                .contains("private-value")
        );
    }
}
