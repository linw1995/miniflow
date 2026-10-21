use crate::node::ExecutionFailedSnafu;
use crate::runner::{ContextSnafu, OutputSelectionSnafu, OutputTypeSnafu};
use crate::{
    FlowNode, GeneratedNodeResult, NodeExecutionError, NodeResult, Outputs, ValueRef,
    WorkflowRunError, output_id,
};
use rpds::HashTrieMapSync;
use snafu::{ResultExt, ensure};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextValue<'a> {
    Value(&'a ValueRef),
    Skipped,
}

/// Sorted output keys fixed by the compiler, shared by every invocation of a domain.
#[derive(Clone, Debug)]
pub enum OutputVisibility {
    Borrowed(&'static [&'static str]),
    Owned(Arc<[String]>),
}

impl PartialEq for OutputVisibility {
    fn eq(&self, other: &Self) -> bool {
        self.keys().eq(other.keys())
    }
}

impl Eq for OutputVisibility {}

impl OutputVisibility {
    pub fn from_sorted(keys: Vec<String>) -> Self {
        debug_assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        Self::Owned(keys.into())
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        let (borrowed, owned): (&[&str], &[String]) = match self {
            Self::Borrowed(keys) => (keys, &[]),
            Self::Owned(keys) => (&[], keys),
        };
        borrowed
            .iter()
            .copied()
            .chain(owned.iter().map(String::as_str))
    }

    fn contains(&self, key: &str) -> bool {
        match self {
            Self::Borrowed(keys) => keys.binary_search(&key).is_ok(),
            Self::Owned(keys) => keys
                .binary_search_by(|candidate| candidate.as_str().cmp(key))
                .is_ok(),
        }
    }
}

#[derive(Clone, Debug)]
enum OutputEntry {
    Value(ValueRef),
    Skipped,
    Transferred,
    Missing,
}

/// A shared execution snapshot with isolated writes, distinct from observation history.
#[derive(Clone, Debug, Default)]
pub struct ExecutionOutputs {
    writes: HashTrieMapSync<String, OutputEntry>,
    parent: Option<Arc<Self>>,
    visibility: Option<OutputVisibility>,
}

impl ExecutionOutputs {
    pub fn from_values(values: Outputs) -> Self {
        Self {
            writes: values
                .into_iter()
                .map(|(id, value)| (id, OutputEntry::Value(value)))
                .collect(),
            ..Self::default()
        }
    }

    /// Stores an already validated scope value or external context value.
    pub fn set_value(&mut self, id: String, value: ValueRef) {
        self.writes.insert_mut(id, OutputEntry::Value(value));
    }

    fn entry(&self, id: &str) -> Option<&OutputEntry> {
        if let Some(entry) = self.writes.get(id) {
            return Some(entry);
        }
        let entry = self.parent.as_ref()?.entry(id)?;
        // Generated transfer evidence has always survived domain visibility filtering.
        (matches!(entry, OutputEntry::Transferred)
            || self
                .visibility
                .as_ref()
                .is_none_or(|visibility| visibility.contains(id)))
        .then_some(entry)
    }

    /// Whether a dynamic output is present, including an explicit skip.
    pub fn contains_output(&self, id: &str) -> bool {
        matches!(
            self.entry(id),
            Some(OutputEntry::Value(_) | OutputEntry::Skipped)
        )
    }

    pub fn is_transferred(&self, id: &str) -> bool {
        matches!(self.entry(id), Some(OutputEntry::Transferred))
    }

    pub fn output_available(&self, node: &str, port: &str) -> Result<bool, NodeExecutionError> {
        self.output_available_key(&output_id(node, port))
    }

    /// Reads availability through a precomputed `output_id` key.
    pub fn output_available_key(&self, key: &str) -> Result<bool, NodeExecutionError> {
        if self.is_transferred(key) {
            return Ok(true);
        }
        self.output(key)
            .map(|value| matches!(value, ContextValue::Value(_)))
    }

    pub fn output(&self, id: &str) -> Result<ContextValue<'_>, NodeExecutionError> {
        match self.entry(id) {
            Some(OutputEntry::Value(value)) => Ok(ContextValue::Value(value)),
            Some(OutputEntry::Skipped) => Ok(ContextValue::Skipped),
            _ => ExecutionFailedSnafu {
                message: format!("missing context output `{id}`"),
            }
            .fail(),
        }
    }

    /// Freezes the current state without visiting outputs or copying their keys.
    /// Visibility applies only to inherited entries; local writes remain readable.
    pub fn fork_snapshot(&self, visibility: Option<OutputVisibility>) -> Self {
        Self {
            parent: Some(Arc::new(self.clone())),
            visibility,
            ..Self::default()
        }
    }

    pub fn merge_nodes<'a, N: 'a>(
        &mut self,
        source: &Self,
        nodes: impl IntoIterator<Item = &'a FlowNode<N>>,
    ) {
        for node in nodes {
            for id in node.output_keys() {
                if let Some(entry) = source.entry(id)
                    && !matches!(entry, OutputEntry::Missing)
                {
                    self.writes.insert_mut(id.clone(), entry.clone());
                }
            }
        }
    }

    /// Selects through a precomputed `output_id` key.
    pub fn select_output_key(
        &self,
        name: &str,
        node: &str,
        key: &str,
        optional: bool,
    ) -> Result<Option<ValueRef>, WorkflowRunError> {
        match self.output(key).context(OutputSelectionSnafu {
            definition_id: node,
            output: name,
        })? {
            ContextValue::Value(value) => Ok(Some(value.clone())),
            ContextValue::Skipped if optional => Ok(None),
            ContextValue::Skipped => ContextSnafu {
                definition_id: node,
                message: format!("workflow output `{name}` requires skipped output `{key}`"),
            }
            .fail(),
        }
    }

    pub fn select_output(
        &self,
        name: &str,
        node: &str,
        port: &str,
        optional: bool,
    ) -> Result<Option<ValueRef>, WorkflowRunError> {
        self.select_output_key(name, node, &output_id(node, port), optional)
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
        // Validate every produced name before writing, so a rejected result
        // leaves existing dynamic and typed outputs untouched.
        let mut transferred = Vec::with_capacity(produced.len());
        for name in &produced {
            let Some(key) = node.output_key(name) else {
                return ContextSnafu {
                    definition_id: node.definition_id.as_str(),
                    message: format!("produced undeclared output `{name}`"),
                }
                .fail();
            };
            transferred.push(key.to_owned());
        }
        for key in transferred {
            self.writes.insert_mut(key, OutputEntry::Transferred);
        }
        for name in skipped {
            if let Some(key) = node.output_key(&name) {
                self.writes.insert_mut(key.to_owned(), OutputEntry::Skipped);
            }
        }
        Ok(())
    }

    pub fn publish<N>(
        &mut self,
        node: &FlowNode<N>,
        result: Option<NodeResult>,
    ) -> Result<(), WorkflowRunError> {
        if let Some(result) = &result {
            Self::validate_result(node, result)?;
        }
        match result {
            Some(result) => {
                // Produced and skipped entries replace typed evidence directly. Only omitted
                // ports need tombstones to prevent inherited evidence from resurfacing.
                for (port, key) in node.metadata.ports.outputs.iter().zip(node.output_keys()) {
                    let name: &str = port.name.as_ref();
                    if result.outputs.contains_key(name) || result.skipped.contains(name) {
                        continue;
                    }
                    if self.is_transferred(key) {
                        self.writes.insert_mut(key.clone(), OutputEntry::Missing);
                    }
                }
                for (name, value) in result.outputs {
                    let key = node.output_key(&name).expect("validated declared output");
                    self.set_value(key.to_owned(), value);
                }
                for name in result.skipped {
                    if let Some(key) = node.output_key(&name) {
                        self.writes.insert_mut(key.to_owned(), OutputEntry::Skipped);
                    }
                }
            }
            None => {
                for key in node.output_keys() {
                    self.writes.insert_mut(key.clone(), OutputEntry::Skipped);
                }
            }
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
