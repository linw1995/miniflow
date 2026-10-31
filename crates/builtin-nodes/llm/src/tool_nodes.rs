use crate::{
    CallOutcome, ToolCall, ToolDefinition, ToolResult,
    tools::{self, ToolCatalog},
};
use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeMetadata, NodeRegistration, NodeValue, PreparedNode,
    ValueRef,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogConfig {}
#[derive(NodeValue)]
struct CatalogInputs {
    #[value(default)]
    descriptions: BTreeMap<String, String>,
    #[value(default)]
    bindings: BTreeMap<String, BTreeMap<String, ValueRef>>,
}
#[derive(NodeValue)]
struct CatalogOutputs {
    tools: Vec<ToolDefinition>,
}
struct CatalogNode {
    interfaces: BTreeMap<String, mf_runtime::SubgraphInterface>,
}
impl mf_runtime::TypedTaskNode for CatalogNode {
    type Input = CatalogInputs;
    type Output = CatalogOutputs;
    fn execute(
        &self,
        inputs: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
        let catalog = ToolCatalog::new(&self.interfaces, &inputs.descriptions, &inputs.bindings)?;
        Ok(CatalogOutputs {
            tools: catalog.tools,
        }
        .into())
    }
}
fn prepare_catalog(
    _: &str,
    config: Value,
    _: Value,
    mut parameters: mf_runtime::PreparedSubgraphs,
) -> Result<PreparedNode, NodeBuildError> {
    let _: CatalogConfig = mf_runtime::deserialize_config(config)?;
    let interfaces = mf_runtime::take_subgraph_interfaces(&mut parameters, "tools")?;
    ensure!(
        parameters.is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "unexpected catalog subgraph parameter"
        }
    );
    let mut metadata = NodeMetadata::default();
    if interfaces
        .values()
        .any(|interface| interface.inputs.values().any(|input| input.secret))
    {
        metadata.input_bindings.insert(
            "bindings".into(),
            mf_runtime::InputBinding {
                secret: true,
                ..Default::default()
            },
        );
    }
    PreparedNode::typed_task(CatalogNode { interfaces }, metadata)
}
inventory::submit! { NodeRegistration { kind: "builtin.tool_catalog", factory: mf_runtime::NodeFactory::Subgraphs(prepare_catalog) } }

#[derive(Debug, Snafu)]
pub enum AgentCallsError {
    #[snafu(display("agent tool calls require unique nonblank call IDs"))]
    CallIdentity,
    #[snafu(display("agent max_tool_calls requires an integer from 1 to 128"))]
    Budget,
    #[snafu(display("agent tool call count exceeds max_tool_calls"))]
    Limit,
}
impl From<AgentCallsError> for NodeExecutionError {
    fn from(source: AgentCallsError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}
#[derive(NodeValue)]
struct AgentCallsInputs {
    outcome: CallOutcome,
    tool_calls: Vec<ToolCall>,
    #[value(default)]
    shared: BTreeMap<String, BTreeMap<String, ValueRef>>,
    max_tool_calls: usize,
}
#[derive(NodeValue)]
struct AgentCallsOutputs {
    records: Vec<ValueRef>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallsConfig {
    node: String,
}
struct AgentCalls {
    node: String,
}
impl mf_runtime::TypedTaskNode for AgentCalls {
    type Input = AgentCallsInputs;
    type Output = AgentCallsOutputs;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
        ensure!(
            (1..=tools::MAX_TOOL_CALLS).contains(&input.max_tool_calls),
            BudgetSnafu
        );
        if !matches!(input.outcome, CallOutcome::ToolCalls) {
            return Ok(AgentCallsOutputs { records: vec![] }.into());
        }
        ensure!(input.tool_calls.len() <= input.max_tool_calls, LimitSnafu);
        let mut ids = BTreeSet::new();
        ensure!(
            input
                .tool_calls
                .iter()
                .all(|call| !call.id.trim().is_empty() && ids.insert(&call.id)),
            CallIdentitySnafu
        );
        let shared = ValueRef::object(input.shared.into_iter().map(|(name, values)| {
            (
                name.into(),
                ValueRef::object(values.into_iter().map(|(name, value)| (name.into(), value))),
            )
        }));
        let records = input
            .tool_calls
            .into_iter()
            .map(|call| {
                ValueRef::object([
                    (format!("{}.selector", self.node).into(), call.name.into()),
                    (format!("{}.arguments", self.node).into(), call.arguments),
                    (format!("{}.bindings", self.node).into(), shared.clone()),
                ])
            })
            .collect();
        Ok(AgentCallsOutputs { records }.into())
    }
}
fn prepare_calls(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config: CallsConfig = mf_runtime::deserialize_config(config)?;
    ensure!(
        !config.node.trim().is_empty(),
        mf_runtime::NodeInvalidSubgraphSnafu {
            message: "agent call node prefix must not be blank"
        }
    );
    PreparedNode::typed_task(AgentCalls { node: config.node }, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "builtin.agent_calls", factory: mf_runtime::NodeFactory::Plain(prepare_calls) } }

#[derive(Debug, Snafu)]
pub enum ToolResultsError {
    #[snafu(display("tool results require unique nonblank call IDs"))]
    Identity,
    #[snafu(display("tool workflow produced {actual} results for {expected} requests"))]
    Count { actual: usize, expected: usize },
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
    results: Vec<ValueRef>,
}
#[derive(NodeValue)]
struct ResultOutputs {
    tool_results: Vec<ToolResult>,
}
struct ResultsNode;
fn encode_results(input: ResultInputs) -> Result<Vec<ToolResult>, ToolResultsError> {
    if !matches!(input.outcome, CallOutcome::ToolCalls) {
        return Ok(vec![]);
    }
    let mut ids = BTreeSet::new();
    ensure!(
        input
            .tool_calls
            .iter()
            .all(|call| !call.id.trim().is_empty() && ids.insert(&call.id)),
        IdentitySnafu
    );
    ensure!(
        input.results.len() == input.tool_calls.len(),
        CountSnafu {
            actual: input.results.len(),
            expected: input.tool_calls.len()
        }
    );
    input
        .tool_calls
        .into_iter()
        .zip(input.results)
        .map(|(call, value)| {
            Ok(ToolResult {
                tool_call_id: call.id,
                content: tools::bounded_json(&value.redacted(), tools::MAX_TOOL_RESULT_BYTES)?,
            })
        })
        .collect()
}
impl mf_runtime::TypedTaskNode for ResultsNode {
    type Input = ResultInputs;
    type Output = ResultOutputs;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(ResultOutputs {
            tool_results: encode_results(input)?,
        }
        .into())
    }
}
pub fn prepare_tool_results(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = mf_runtime::deserialize_config(config)?;
    PreparedNode::typed_task(ResultsNode, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind:"builtin.tool_results",factory:mf_runtime::NodeFactory::Plain(prepare_tool_results) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::Inputs;
    use serde_json::json;
    fn results(calls: Value, values: Value) -> Result<Vec<ToolResult>, ToolResultsError> {
        encode_results(
            ResultInputs::from_values(Inputs::from([
                ("outcome".into(), "tool_calls".into()),
                ("tool_calls".into(), calls.into()),
                ("results".into(), values.into()),
            ]))
            .unwrap(),
        )
    }
    #[test]
    fn unified_results_preserve_model_order_across_different_and_repeated_tools() {
        let calls = json!([{"id":"a","name":"multiply","arguments":{}},{"id":"b","name":"add","arguments":{}},{"id":"c","name":"multiply","arguments":{}}]);
        let values = results(calls, json!([{"value":2},{"value":3},{"value":4}])).unwrap();
        assert_eq!(
            values
                .iter()
                .map(|value| value.tool_call_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(
            serde_json::from_str::<Value>(&values[2].content).unwrap(),
            json!({"value":4})
        );
        assert!(
            results(
                json!([{"id":"a","name":"multiply","arguments":{}}]),
                json!([])
            )
            .is_err()
        );
        assert!(results(json!([{"id":"a","name":"multiply","arguments":{}},{"id":"a","name":"multiply","arguments":{}}]),json!([1,2])).is_err());
    }
    #[test]
    fn result_encoding_redacts_private_values() {
        let result = encode_results(ResultInputs {
            outcome: CallOutcome::ToolCalls,
            tool_calls: vec![ToolCall {
                id: "one".into(),
                name: "multiply".into(),
                arguments: json!({}).into(),
            }],
            results: vec![ValueRef::from("private-value").into_secret()],
        })
        .unwrap();
        assert!(!result[0].content.contains("private-value"));
    }
    #[test]
    fn call_records_keep_selector_arguments_and_caller_bindings_separate() {
        let node = prepare_calls(json!({"node":"invoke"})).unwrap();
        let secret = ValueRef::from("private").into_secret();
        let calls = ValueRef::array(vec![ValueRef::object([
            ("id".into(), "one".into()),
            ("name".into(), "multiply".into()),
            (
                "arguments".into(),
                ValueRef::object([("amount".into(), secret)]),
            ),
        ])]);
        let input = Inputs::from([
            ("outcome".into(), "tool_calls".into()),
            ("tool_calls".into(), calls),
            ("shared".into(), json!({"multiply":{"factor":2}}).into()),
            ("max_tool_calls".into(), 128u64.into()),
        ]);
        let output = node
            .execution
            .as_task_node()
            .unwrap()
            .execute(input, &mut mf_runtime::ExecutionContext::default())
            .unwrap();
        let record = &output.outputs["records"].as_array().unwrap()[0];
        assert_eq!(record["invoke.selector"], json!("multiply"));
        assert_eq!(record["invoke.bindings"]["multiply"]["factor"], json!(2));
        assert!(record["invoke.arguments"]["amount"].is_secret());
    }
}
