//! Explicit saved-plan protocol. Native compiler graphs do not implement serialization.

use crate::{CompiledWorkflow, DefinitionId, WorkflowDefinition};
use mf_runtime::{CompiledGraph, GraphRelations};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDocument {
    pub graph: GraphDocument,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDocument {
    pub definition: WorkflowDefinition,
    pub execution_order: Vec<DefinitionId>,
    pub relations: GraphRelations,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub children: BTreeMap<DefinitionId, BTreeMap<String, GraphDocument>>,
}

impl From<&CompiledWorkflow> for PlanDocument {
    fn from(plan: &CompiledWorkflow) -> Self {
        Self {
            graph: (&plan.graph).into(),
        }
    }
}
impl From<PlanDocument> for CompiledWorkflow {
    fn from(document: PlanDocument) -> Self {
        Self {
            graph: document.graph.into(),
        }
    }
}
impl From<&CompiledGraph> for GraphDocument {
    fn from(graph: &CompiledGraph) -> Self {
        Self {
            definition: graph.definition.clone(),
            execution_order: graph.execution_order.clone(),
            relations: graph.relations.clone(),
            children: graph
                .children
                .iter()
                .map(|(id, bindings)| {
                    (
                        id.clone(),
                        bindings
                            .iter()
                            .map(|(name, graph)| (name.clone(), graph.into()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}
impl From<GraphDocument> for CompiledGraph {
    fn from(document: GraphDocument) -> Self {
        Self {
            definition: document.definition,
            execution_order: document.execution_order,
            relations: document.relations,
            children: document
                .children
                .into_iter()
                .map(|(id, bindings)| {
                    (
                        id,
                        bindings
                            .into_iter()
                            .map(|(name, graph)| (name, graph.into()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
}
