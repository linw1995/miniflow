use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeInputs, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, TaskNode,
    ValueRef, ValueSchema, ValueType,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "builtin.dispatch";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    branches: BTreeMap<String, Vec<String>>,
}

#[derive(NodeInputs)]
struct DispatchInputs {
    selector: String,
    arguments: BTreeMap<String, ValueRef>,
}

#[derive(Debug, Snafu)]
pub enum DispatchBuildError {
    #[snafu(display("dispatch requires at least one branch"))]
    Empty,
    #[snafu(display("invalid dispatch branch `{branch}`"))]
    Branch { branch: String },
    #[snafu(display("dispatch branch `{branch}` has an empty or duplicate parameter"))]
    Parameter { branch: String },
}

impl From<DispatchBuildError> for NodeBuildError {
    fn from(source: DispatchBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum DispatchExecutionError {
    #[snafu(display("unknown dispatch branch `{branch}`"))]
    UnknownBranch { branch: String },
    #[snafu(display("dispatch branch `{branch}` requires parameter `{parameter}`"))]
    Missing { branch: String, parameter: String },
    #[snafu(display("dispatch branch `{branch}` does not declare parameter `{parameter}`"))]
    Unknown { branch: String, parameter: String },
}

impl From<DispatchExecutionError> for NodeExecutionError {
    fn from(source: DispatchExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

struct Dispatch {
    branches: BTreeMap<String, Vec<String>>,
}

impl NodePortContract for Dispatch {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: DispatchInputs::ports(),
            outputs: self
                .branches
                .iter()
                .flat_map(|(branch, parameters)| {
                    std::iter::once(PortSpec::owned(branch, ValueType::Boolean, false)).chain(
                        parameters.iter().map(move |parameter| {
                            PortSpec::owned(format!("{branch}.{parameter}"), ValueType::Any, false)
                        }),
                    )
                })
                .collect(),
        }
    }
}

impl TaskNode for Dispatch {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let DispatchInputs {
            selector,
            arguments,
        } = mf_runtime::decode_node_inputs(inputs)?;
        let parameters = self
            .branches
            .get(&selector)
            .context(UnknownBranchSnafu { branch: &selector })?;
        for parameter in arguments.keys() {
            ensure!(
                parameters.contains(parameter),
                UnknownSnafu {
                    branch: &selector,
                    parameter
                }
            );
        }
        let mut outputs = Outputs::from([(selector.clone(), true.into())]);
        for parameter in parameters {
            let value = arguments.get(parameter).context(MissingSnafu {
                branch: &selector,
                parameter,
            })?;
            outputs.insert(format!("{selector}.{parameter}"), value.clone());
        }
        let skipped = self
            .ports()
            .outputs
            .into_iter()
            .filter(|port| !outputs.contains_key(port.name.as_ref()))
            .map(|port| port.name.into_owned())
            .collect();
        Ok(NodeResult {
            outputs,
            skipped,
            ..Default::default()
        })
    }
}

fn factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: Config = mf_runtime::deserialize_config(config)?;
    ensure!(!config.branches.is_empty(), EmptySnafu);
    for (branch, parameters) in &config.branches {
        let mut bytes = branch.bytes();
        ensure!(
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            BranchSnafu { branch }
        );
        let mut names = BTreeSet::new();
        ensure!(
            parameters
                .iter()
                .all(|name| !name.is_empty() && names.insert(name)),
            ParameterSnafu { branch }
        );
    }
    let names: Vec<_> = config.branches.keys().cloned().collect();
    let mut input_schemas = DispatchInputs::input_schemas();
    input_schemas.insert("selector".into(), ValueSchema::Enum(names.clone()));
    mf_runtime::PreparedNode::new(
        Dispatch {
            branches: config.branches,
        },
        NodeMetadata {
            input_schemas,
            exclusive_outputs: names,
            ..Default::default()
        },
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routes_one_branch_through_normal_outputs_and_preserves_secrets() {
        let prepared =
            factory(json!({"branches":{"multiply":["amount"],"add":["a","b"],"ping":[]}})).unwrap();
        let result = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([
                    ("selector".into(), "multiply".into()),
                    (
                        "arguments".into(),
                        ValueRef::object([("amount".into(), ValueRef::from(1234).into_secret())]),
                    ),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(result.outputs["multiply"], json!(true));
        assert!(result.outputs["multiply.amount"].is_secret());
        assert_eq!(result.outputs.len(), 2);
        assert_eq!(
            result.skipped.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from(["add".into(), "add.a".into(), "add.b".into(), "ping".into()])
        );
        assert_eq!(
            prepared.metadata.exclusive_outputs,
            ["add", "multiply", "ping"]
        );
        let ping = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([
                    ("selector".into(), "ping".into()),
                    ("arguments".into(), json!({}).into()),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(ping.outputs.len(), 1);
        assert_eq!(ping.outputs["ping"], json!(true));
    }

    #[test]
    fn malformed_routes_and_arguments_fail_before_publishing_values() {
        for config in [
            json!({"branches":{}}),
            json!({"branches":{"bad.name":[]}}),
            json!({"branches":{"ok":[""]}}),
            json!({"branches":{"ok":["a","a"]}}),
        ] {
            assert!(factory(config).is_err());
        }
        let prepared = factory(json!({"branches":{"multiply":["amount"]}})).unwrap();
        for (selector, arguments) in [
            ("missing", json!({})),
            ("multiply", json!({})),
            ("multiply", json!({"amount":1,"extra":2})),
        ] {
            assert!(
                prepared
                    .execution
                    .as_task_node()
                    .unwrap()
                    .execute(
                        Inputs::from([
                            ("selector".into(), selector.into()),
                            ("arguments".into(), arguments.into())
                        ]),
                        &mut ExecutionContext::default(),
                    )
                    .is_err()
            );
        }
    }
}
