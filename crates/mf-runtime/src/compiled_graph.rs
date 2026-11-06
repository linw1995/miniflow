use crate::{DefinitionId, NodeDefinition, ScopePathEntry, ValueType, WorkflowDefinition};
use mf_telemetry::{ContractError, identity::WorkflowId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Concrete execution relationships for one compiled graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphRelations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeRelations>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub streams: BTreeMap<String, crate::OutputReferenceDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Explicit source, state, and instruction relationships within a child scope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeRelations {
    pub input: DefinitionId,
    /// Typed frame values; the shared source supplies its intrinsic `index` separately.
    pub values: BTreeMap<String, ValueType>,
    /// Source variable to receiving owner output. The current frame protocol
    /// preserves variable names; nonidentity mappings are not executable.
    pub state_outputs: BTreeMap<String, String>,
    /// Write instruction node to scope variable.
    pub writes: BTreeMap<DefinitionId, String>,
    pub exits: BTreeSet<DefinitionId>,
}

/// The execution role of a node explicitly named by scope relationships.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeInstruction<'a> {
    Input,
    Write(&'a str),
    Exit,
}

impl GraphRelations {
    pub fn instruction(&self, id: &DefinitionId) -> Option<ScopeInstruction<'_>> {
        let scope = self.scope.as_ref()?;
        if id == &scope.input {
            Some(ScopeInstruction::Input)
        } else if let Some(variable) = scope.writes.get(id) {
            Some(ScopeInstruction::Write(variable))
        } else if scope.exits.contains(id) {
            Some(ScopeInstruction::Exit)
        } else {
            None
        }
    }

    pub fn is_callable(&self) -> bool {
        self.scope.is_none()
    }
}

impl ScopeRelations {
    pub fn is_stateful(&self) -> bool {
        !self.state_outputs.is_empty()
    }
}

/// Provider-free compiled facts for one scope and its actual executable children.
///
/// Child bindings are authoritative; provider configuration remains opaque and is
/// never inspected to discover additional scopes when reading a compiled graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledGraph {
    pub definition: WorkflowDefinition,
    pub execution_order: Vec<DefinitionId>,
    pub relations: GraphRelations,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub children: BTreeMap<DefinitionId, BTreeMap<String, CompiledGraph>>,
}

impl CompiledGraph {
    /// Observation identity for this child binding, independent of provider kind.
    pub fn child_scope_key(&self, owner: &NodeDefinition, binding: &str) -> String {
        self.relations
            .key
            .clone()
            .unwrap_or_else(|| ScopePathEntry::binding_key(owner.id.as_str(), binding))
    }

    /// Identifies all authoritative compiled relationships and child scopes.
    /// Plain child-free graphs retain their historical definition/order identity.
    pub fn workflow_id(&self) -> Result<WorkflowId, ContractError> {
        let order = self
            .execution_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if self.children.is_empty() && self.relations == GraphRelations::default() {
            return WorkflowId::from_definition(&self.definition, &order);
        }
        WorkflowId::from_definition(self, &order)
    }
}
