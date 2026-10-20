use crate::node::ExecutionFailedSnafu;
use crate::runner::{ContextSnafu, OutputSelectionSnafu, OutputTypeSnafu};
use crate::{
    FlowNode, GeneratedNodeResult, NodeExecutionError, NodeResult, Outputs, ValueRef,
    WorkflowRunError, output_id,
};
use snafu::{ResultExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextValue<'a> {
    Value(&'a ValueRef),
    Skipped,
}

/// Output state for one execution context, including generated typed presence.
#[derive(Clone, Debug, Default)]
pub struct ExecutionOutputs {
    // A present None is an explicit skip; an absent key is a missing output.
    outputs: BTreeMap<String, Option<ValueRef>>,
    typed_outputs: BTreeSet<String>,
}

impl ExecutionOutputs {
    pub fn from_values(values: Outputs) -> Self {
        Self {
            outputs: values
                .into_iter()
                .map(|(id, value)| (id, Some(value)))
                .collect(),
            typed_outputs: BTreeSet::new(),
        }
    }

    /// Stores an already validated scope value or external context value.
    pub fn set_value(&mut self, id: String, value: ValueRef) {
        self.typed_outputs.remove(&id);
        self.outputs.insert(id, Some(value));
    }

    /// Whether a dynamic output is present, including an explicit skip.
    pub fn contains_output(&self, id: &str) -> bool {
        self.outputs.contains_key(id)
    }

    pub fn is_transferred(&self, id: &str) -> bool {
        self.typed_outputs.contains(id)
    }

    pub fn output_available(&self, node: &str, port: &str) -> Result<bool, NodeExecutionError> {
        let key = output_id(node, port);
        if self.is_transferred(&key) {
            return Ok(true);
        }
        self.output(&key)
            .map(|value| matches!(value, ContextValue::Value(_)))
    }

    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        match self.outputs.get(id) {
            Some(Some(value)) => Ok(ContextValue::Value(value)),
            Some(None) => Ok(ContextValue::Skipped),
            None => ExecutionFailedSnafu {
                message: format!("missing context output `{id}`"),
            }
            .fail(),
        }
    }

    /// Copies only visible dynamic entries while retaining generated transfer evidence.
    pub fn fork_visible(&self, visible: &BTreeSet<String>) -> Self {
        Self {
            outputs: visible
                .iter()
                .filter_map(|id| {
                    self.outputs
                        .get(id)
                        .map(|value| (id.clone(), value.clone()))
                })
                .collect(),
            typed_outputs: self.typed_outputs.clone(),
        }
    }

    pub fn merge_nodes<'a, N: 'a>(
        &mut self,
        source: &Self,
        nodes: impl IntoIterator<Item = &'a FlowNode<N>>,
    ) {
        for node in nodes {
            for port in &node.metadata.ports.outputs {
                let id = output_id(node.definition_id.as_str(), &port.name);
                if source.typed_outputs.contains(&id) {
                    self.outputs.remove(&id);
                    self.typed_outputs.insert(id);
                } else if let Some(value) = source.outputs.get(&id) {
                    self.typed_outputs.remove(&id);
                    self.outputs.insert(id, value.clone());
                }
            }
        }
    }

    pub fn select_output(
        &self,
        name: &str,
        node: &str,
        port: &str,
        optional: bool,
    ) -> Result<Option<ValueRef>, WorkflowRunError> {
        match self
            .output(&output_id(node, port))
            .context(OutputSelectionSnafu {
                definition_id: node,
                output: name,
            })? {
            ContextValue::Value(value) => Ok(Some(value.clone())),
            ContextValue::Skipped if optional => Ok(None),
            ContextValue::Skipped => ContextSnafu {
                definition_id: node,
                message: format!(
                    "workflow output `{name}` requires skipped output `{}`",
                    output_id(node, port)
                ),
            }
            .fail(),
        }
    }

    /// Validates the complete result without publishing any values.
    pub fn validate_result<N>(
        node: &FlowNode<N>,
        result: &NodeResult,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        validate_skipped_outputs(node, &result.skipped, |name| {
            result.outputs.contains_key(name)
        })?;
        for (name, value) in &result.outputs {
            let Some(port) = node
                .metadata
                .ports
                .outputs
                .iter()
                .find(|port| port.name == *name)
            else {
                return ContextSnafu {
                    definition_id: id,
                    message: format!("produced undeclared output `{name}`"),
                }
                .fail();
            };
            port.value_type
                .validate_shared(value)
                .with_context(|_| OutputTypeSnafu {
                    definition_id: id,
                    output: name,
                })?;
        }
        Ok(())
    }

    pub fn publish_generated<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<GeneratedNodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let (produced, skipped) = match result {
            Some(GeneratedNodeResult::Typed {
                produced, skipped, ..
            }) => (produced, skipped),
            Some(GeneratedNodeResult::Dynamic(result)) => return self.publish(node, Some(result)),
            None => return self.publish(node, None),
        };
        validate_skipped_outputs(node, &skipped, |name| produced.contains(&name))?;
        for name in &produced {
            ensure!(
                node.metadata
                    .ports
                    .outputs
                    .iter()
                    .any(|port| port.name == *name),
                ContextSnafu {
                    definition_id: node.definition_id.as_str(),
                    message: format!("produced undeclared output `{name}`"),
                }
            );
        }
        for name in &produced {
            let key = output_id(node.definition_id.as_str(), name);
            self.outputs.remove(&key);
            self.typed_outputs.insert(key);
        }
        for name in skipped {
            let key = output_id(node.definition_id.as_str(), &name);
            self.typed_outputs.remove(&key);
            self.outputs.insert(key, None);
        }
        Ok(())
    }

    pub fn publish<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        let id = node.definition_id.as_str();
        if let Some(result) = &result {
            Self::validate_result(node, result)?;
        }
        // Validate the complete result before making any values visible.
        for port in &node.metadata.ports.outputs {
            self.typed_outputs.remove(&output_id(id, &port.name));
        }
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
                node.metadata
                    .ports
                    .outputs
                    .iter()
                    .map(|port| (output_id(id, &port.name), None)),
            ),
        }
        Ok(())
    }
}

fn validate_skipped_outputs<N>(
    node: &FlowNode<N>,
    skipped: &BTreeSet<String>,
    produced: impl Fn(&str) -> bool,
) -> Result<(), WorkflowRunError> {
    let id = node.definition_id.as_str();
    for name in skipped {
        ensure!(
            !produced(name),
            ContextSnafu {
                definition_id: id,
                message: format!("output `{name}` is both produced and skipped"),
            }
        );
        ensure!(
            node.metadata
                .ports
                .outputs
                .iter()
                .any(|port| port.name == *name && !port.required),
            ContextSnafu {
                definition_id: id,
                message: format!("cannot explicitly skip unknown or required output `{name}`"),
            }
        );
    }
    Ok(())
}
