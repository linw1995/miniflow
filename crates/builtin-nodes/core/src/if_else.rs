use crate::condition::{Condition, ConditionExecutionError, ConditionValidationError};
use mf_runtime::{
    ContextReference, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, TaskNode,
    ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{ResultExt, Snafu};
use std::{collections::BTreeSet, error::Error};

pub const KIND: &str = "builtin.if_else";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    branches: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Branch {
    id: String,
    condition: Condition,
}
#[derive(Debug, Snafu)]
enum BranchBuildError {
    #[snafu(display("{message}"))]
    InvalidConfig { message: String },
    #[snafu(display("branch `{branch}`: {source}"))]
    Condition {
        branch: String,
        source: ConditionValidationError,
    },
    #[snafu(display("branch `{branch}`: {source}"))]
    Configuration {
        branch: String,
        source: NodeBuildError,
    },
}

impl From<BranchBuildError> for NodeBuildError {
    fn from(source: BranchBuildError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("branch `{branch}` {source}"))]
struct BranchExecutionError {
    branch: String,
    source: ConditionExecutionError,
}

impl From<BranchExecutionError> for NodeExecutionError {
    fn from(source: BranchExecutionError) -> Self {
        Box::<dyn Error + Send + Sync>::from(source).into()
    }
}

fn invalid(message: impl Into<String>) -> NodeBuildError {
    InvalidConfigSnafu {
        message: message.into(),
    }
    .build()
    .into()
}

struct IfElse {
    branches: Vec<Branch>,
}
impl TaskNode for IfElse {
    fn execute(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let mut selected = "else";
        for branch in &self.branches {
            let matched = branch
                .condition
                .evaluate(ctx)
                .context(BranchExecutionSnafu { branch: &branch.id })?;
            if matched {
                selected = &branch.id;
                break;
            }
        }
        Ok(NodeResult {
            outputs: Outputs::from([(selected.to_owned(), Value::Bool(true).into())]),
            skipped: self
                .branches
                .iter()
                .map(|branch| branch.id.as_str())
                .chain(["else"])
                .filter(|name| *name != selected)
                .map(str::to_owned)
                .collect(),
            ..NodeResult::default()
        })
    }
}
impl NodePortContract for IfElse {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: self
                .branches
                .iter()
                .map(|branch| branch.id.as_str())
                .chain(["else"])
                .map(|name| PortSpec::owned(name, ValueType::Boolean, false))
                .collect(),
        }
    }
}
impl IfElse {
    fn context_references(&self) -> Vec<ContextReference> {
        self.branches
            .iter()
            .map(|branch| branch.condition.reference(&branch.id))
            .collect()
    }
}
fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: Config = deserialize_config(config)?;
    if config.branches.is_empty() {
        return Err(invalid("branches must contain at least one condition"));
    }
    let mut ids = BTreeSet::new();
    let mut branches = Vec::with_capacity(config.branches.len());
    for (index, value) in config.branches.into_iter().enumerate() {
        let label = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("index {index}"));
        let branch: Branch =
            deserialize_config(value).context(ConfigurationSnafu { branch: label })?;
        let mut bytes = branch.id.bytes();
        let valid = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if !valid || branch.id == "else" || !ids.insert(branch.id.clone()) {
            return Err(invalid(format!(
                "invalid, reserved, or duplicate branch ID `{}`",
                branch.id
            )));
        }
        branch
            .condition
            .validate()
            .context(ConditionSnafu { branch: &branch.id })?;
        branches.push(branch);
    }
    let node = IfElse { branches };
    let metadata = mf_runtime::NodeMetadata {
        exclusive_outputs: node
            .ports()
            .outputs
            .iter()
            .map(|port| port.name.to_string())
            .collect(),
        context_references: node.context_references(),
        ..Default::default()
    };
    mf_runtime::PreparedNode::new(node, metadata)
}
inventory::submit! { NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_branches_preserve_the_configuration_error_chain() {
        let error = factory(serde_json::json!({"branches": [{"id": "first"}]}))
            .err()
            .unwrap();
        assert!(error.to_string().contains("branch `first`"));
        let mut current: &(dyn Error + 'static) = &error;
        loop {
            if let Some(source) = current.downcast_ref::<serde_json::Error>() {
                assert!(source.is_data());
                break;
            }
            current = current
                .source()
                .expect("typed JSON source must remain in the chain");
        }
    }
    fn condition(operator: &str, value: Value) -> Value {
        json!({"source":{"output":"source.value","path":""},"operator":operator,"value":value})
    }
    fn config() -> Value {
        json!({"branches":[{"id":"accepted","condition":condition("eq", json!(true))}]})
    }
    #[test]
    fn requires_nonempty_unique_branches_and_valid_predicates() {
        assert!(factory(json!({})).is_err());
        assert!(factory(json!({"branches":[]})).is_err());
        for id in ["", "else", "1bad", "bad.name", "bad name"] {
            let mut value = config();
            value["branches"][0]["id"] = json!(id);
            assert!(factory(value).is_err(), "{id}");
        }
        let mut value = config();
        let duplicate = value["branches"][0].clone();
        value["branches"].as_array_mut().unwrap().push(duplicate);
        assert!(factory(value).is_err());
        for field in ["extra", "condition"] {
            let mut value = config();
            value["branches"][0][field] = json!(true);
            assert!(factory(value).is_err());
        }
        for predicate in [
            condition("unknown", json!(1)),
            condition("gt", json!("1")),
            condition("eq", json!([])),
            condition("ne", json!({})),
            condition("exists", json!(null)),
            json!({"source":{"output":"source.value","path":""},"operator":"eq"}),
            json!({"source":{"output":"","path":""},"operator":"exists"}),
        ] {
            let mut value = config();
            value["branches"][0]["condition"] = predicate;
            assert!(factory(value).is_err());
        }
        for path in ["amount", "/bad~", "/bad~2", "/bad~01~x"] {
            let mut value = config();
            value["branches"][0]["condition"]["source"]["path"] = json!(path);
            assert!(factory(value).is_err());
        }
        let node = factory(config()).unwrap();
        assert!(node.metadata.ports.inputs.is_empty());
        assert_eq!(node.metadata.ports.outputs.len(), 2);
    }
}
