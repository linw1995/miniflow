use crate::{FlowNode, Inputs, NodeExecutionError, Outputs, WorkflowRunError, output_id};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct NodeResult {
    pub outputs: Outputs,
    pub skipped: BTreeSet<String>,
}

impl From<Outputs> for NodeResult {
    fn from(outputs: Outputs) -> Self {
        Self {
            outputs,
            skipped: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextValue<'a> {
    Value(&'a Value),
    Skipped,
}

/// Completed output values for one run. Nodes receive an immutable reference.
#[derive(Debug, Default)]
pub struct ExecutionContext {
    // A present None is an explicit skip; an absent key is a missing output.
    outputs: BTreeMap<String, Option<Value>>,
}

impl ExecutionContext {
    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        match self.outputs.get(id) {
            Some(Some(value)) => Ok(ContextValue::Value(value)),
            Some(None) => Ok(ContextValue::Skipped),
            None => Err(NodeExecutionError::ExecutionFailed {
                message: format!("missing context output `{id}`"),
            }),
        }
    }

    fn publish(
        &mut self,
        node: &FlowNode,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if let Some(result) = &result {
            for name in &result.skipped {
                if result.outputs.contains_key(name) {
                    return Err(state_error(
                        id,
                        format!("output `{name}` is both produced and skipped"),
                    ));
                }
                if !node
                    .ports
                    .outputs
                    .iter()
                    .any(|port| port.name == *name && !port.required)
                {
                    return Err(state_error(
                        id,
                        format!("cannot explicitly skip unknown or required output `{name}`"),
                    ));
                }
            }
            for name in result.outputs.keys() {
                if !node.ports.outputs.iter().any(|port| port.name == *name) {
                    return Err(state_error(
                        id,
                        format!("produced undeclared output `{name}`"),
                    ));
                }
            }
        }
        // Validate the complete result before making any values visible.
        match result {
            Some(result) => {
                self.outputs.extend(
                    result
                        .outputs
                        .into_iter()
                        .map(|(port, value)| (output_id(id, &port), Some(value))),
                );
                self.outputs.extend(
                    result
                        .skipped
                        .into_iter()
                        .map(|port| (output_id(id, &port), None)),
                );
            }
            None => self.outputs.extend(
                node.ports
                    .outputs
                    .iter()
                    .map(|port| (output_id(id, &port.name), None)),
            ),
        }
        Ok(())
    }
}

fn state_error(id: &str, message: impl Into<String>) -> WorkflowRunError {
    WorkflowRunError::Context {
        definition_id: id.into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionDependency<'a> {
    pub input: Option<&'a str>,
    pub source_node: &'a str,
    pub source_output: &'a str,
}

/// Executes one step of a validated plan, in its topological order.
pub fn execute_node_in_context(
    node: &FlowNode,
    dependencies: &[ExecutionDependency<'_>],
    ctx: &mut ExecutionContext,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    let mut dependencies = dependencies.to_vec();
    dependencies.sort();
    let mut inputs = Inputs::new();
    let mut skipped = false;
    for dependency in dependencies {
        match ctx
            .output(&output_id(dependency.source_node, dependency.source_output))
            .map_err(|error| {
                state_error(
                    id,
                    format!(
                        "dependency {}: {error}",
                        dependency.input.unwrap_or("<control>")
                    ),
                )
            })? {
            ContextValue::Value(value) => {
                if let Some(input) = dependency.input {
                    inputs.insert(input.to_owned(), value.clone());
                }
            }
            ContextValue::Skipped => skipped = true,
        }
    }
    let result = if skipped {
        None
    } else {
        Some(
            node.node
                .execute_with_context(inputs, ctx)
                .map_err(|source| WorkflowRunError::NodeExecution {
                    definition_id: node.definition_id.clone(),
                    source,
                })?,
        )
    };
    ctx.publish(node, result)
}

pub fn select_context_output(
    ctx: &ExecutionContext,
    name: &str,
    node: &str,
    port: &str,
    optional: bool,
) -> Result<Option<Value>, WorkflowRunError> {
    match ctx
        .output(&output_id(node, port))
        .map_err(|error| state_error(node, format!("workflow output `{name}`: {error}")))?
    {
        ContextValue::Value(value) => Ok(Some(value.clone())),
        ContextValue::Skipped if optional => Ok(None),
        ContextValue::Skipped => Err(state_error(
            node,
            format!(
                "workflow output `{name}` requires skipped output `{}`",
                output_id(node, port)
            ),
        )),
    }
}
