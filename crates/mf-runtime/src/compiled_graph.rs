use crate::{DefinitionId, WorkflowDefinition};
use mf_telemetry::{ContractError, identity::WorkflowId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Provider-free compiled facts for one scope and its actual executable children.
///
/// Child bindings are authoritative; provider configuration remains opaque and is
/// never inspected to discover additional scopes when reading a compiled graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledGraph {
    pub definition: WorkflowDefinition,
    pub execution_order: Vec<DefinitionId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub children: BTreeMap<DefinitionId, BTreeMap<String, CompiledGraph>>,
}

impl CompiledGraph {
    /// Identifies all authoritative compiled scopes and their execution orders.
    /// Child-free graphs retain their historical definition/order identity.
    pub fn workflow_id(&self) -> Result<WorkflowId, ContractError> {
        let order = self
            .execution_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if self.children.is_empty() {
            return WorkflowId::from_definition(&self.definition, &order);
        }
        // Serialize fallible definitions before constructing the JSON envelope:
        // json! on them directly would panic instead of preserving typed errors.
        let definition = serde_json::to_value(&self.definition)?;
        let children = serde_json::to_value(&self.children)?;
        WorkflowId::from_definition(
            &serde_json::json!({"definition": definition, "children": children}),
            &order,
        )
    }
}
