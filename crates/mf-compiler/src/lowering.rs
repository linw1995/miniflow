//! Translate authoring syntax into concrete scope relationships once, before
//! discovery or compilation consumes the graph. Compiled-artifact readers do not
//! reconstruct these relationships from node kinds or configuration.

use crate::{DefinitionId, NodeDefinition, ValueType, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{GraphRelations, ScopeRelations, SubgraphScope, WorkflowDefinitionVersion};
use serde::Deserialize;
use serde_json::json;
use snafu::{ResultExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// Lowering-only traversal state. Only `relations` crosses a phase boundary;
/// ancestry and diagnostic paths are carried explicitly during source lowering.
#[derive(Clone, Debug, Default)]
pub struct ScopeContext {
    pub relations: GraphRelations,
    pub path: Vec<DefinitionId>,
    /// The original authored root, unchanged by explicit child version overrides.
    pub root_version: Option<WorkflowDefinitionVersion>,
    inside_items: bool,
    /// Resolved-definition adapters may receive sources inserted by an earlier lowering pass.
    lowered_sources: bool,
    /// Provider-declared child-frame semantics; empty when no provider metadata is loaded.
    pub(crate) subgraphs: BTreeMap<String, SubgraphScope>,
}

impl ScopeContext {
    pub fn resolved() -> Self {
        Self {
            lowered_sources: true,
            ..Self::default()
        }
    }

    pub fn with_subgraphs(mut self, subgraphs: BTreeMap<String, SubgraphScope>) -> Self {
        self.subgraphs = subgraphs;
        self
    }

    /// Enter a child only after its receiving callback authorizes this binding.
    pub fn child(
        &self,
        version: WorkflowDefinitionVersion,
        owner: &NodeDefinition,
        binding: &str,
    ) -> Result<Self, WorkflowCompileError> {
        // Reject a new frame before decoding its configuration. Callable scopes
        // reset state ownership, but do not erase an immutable-item ancestor.
        if self.inside_items
            && binding == "body"
            && matches!(
                self.subgraphs.get(owner.kind.as_str()),
                Some(SubgraphScope::Loop | SubgraphScope::Iteration)
            )
        {
            let mut path = self.path.clone();
            path.push(owner.id.clone());
            return crate::compiler::InvalidScopeSnafu {
                path: crate::loops::path_label(&path),
                message: "child scopes inside an item scope cannot contain container scopes",
            }
            .fail();
        }
        let relations = child_relations_at(version, owner, binding, &self.path, &self.subgraphs)?;
        let mut path = self.path.clone();
        path.push(
            relations
                .key
                .clone()
                .unwrap_or_else(|| {
                    mf_runtime::ScopePathEntry::binding_key(owner.id.as_str(), binding)
                })
                .into(),
        );
        let inside_items = self.inside_items
            || relations
                .scope
                .as_ref()
                .is_some_and(|scope| !scope.is_stateful());
        Ok(Self {
            relations,
            path,
            root_version: self.root_version,
            inside_items,
            lowered_sources: self.lowered_sources,
            subgraphs: self.subgraphs.clone(),
        })
    }

    pub fn lower_scope(
        &mut self,
        definition: &mut WorkflowDefinition,
    ) -> Result<(), WorkflowCompileError> {
        self.root_version.get_or_insert(definition.version);
        lower_scope_at(
            definition,
            &mut self.relations,
            &self.path,
            &self.subgraphs,
            self.lowered_sources,
        )
    }
}

pub fn child_relations(
    version: WorkflowDefinitionVersion,
    owner: &NodeDefinition,
    binding: &str,
    subgraphs: &BTreeMap<String, SubgraphScope>,
) -> Result<GraphRelations, WorkflowCompileError> {
    child_relations_at(version, owner, binding, &[], subgraphs)
}

/// Lower child relationships with the enclosing authored scope's full path.
pub fn child_relations_at(
    version: WorkflowDefinitionVersion,
    owner: &NodeDefinition,
    binding: &str,
    path: &[DefinitionId],
    subgraphs: &BTreeMap<String, SubgraphScope>,
) -> Result<GraphRelations, WorkflowCompileError> {
    let mut owner_path = path.to_vec();
    owner_path.push(owner.id.clone());
    let mut relations = GraphRelations::default();
    if binding != "body" {
        return Ok(relations);
    }
    match subgraphs.get(owner.kind.as_str()) {
        Some(SubgraphScope::Loop) => {
            let config = crate::loops::parse_config(owner)?;
            ensure!(
                version.supports_loop_inputs()
                    || (config.inputs.is_empty()
                        && config
                            .variables
                            .iter()
                            .all(|variable| variable.initial.is_none())),
                crate::compiler::InvalidScopeSnafu {
                    path: crate::loops::path_label(&owner_path),
                    message: "Loop inputs and variable defaults require definition version 2026-11-11",
                }
            );
            let variables = mf_runtime::loop_variable_types(&config.variables).context(
                crate::compiler::ScopeVariablesSnafu {
                    path: crate::loops::path_label(&owner_path),
                },
            )?;
            ensure!(
                config.inputs.keys().all(|name| !name.trim().is_empty()
                    && name != "index"
                    && !variables.contains_key(name)),
                crate::compiler::InvalidScopeSnafu {
                    path: crate::loops::path_label(&owner_path),
                    message: "Loop inputs must be nonblank, distinct from variables, and not index",
                }
            );
            let state_outputs = variables
                .keys()
                .map(|name| (name.clone(), name.clone()))
                .collect();
            let mut values = config.inputs;
            values.extend(
                variables
                    .iter()
                    .map(|(name, value_type)| (name.clone(), value_type.clone())),
            );
            relations.scope = Some(ScopeRelations {
                input: mf_runtime::LOOP_SOURCE_ID.into(),
                values,
                state_outputs,
                writes: BTreeMap::new(),
                exits: BTreeSet::new(),
            });
            relations.streams = config.stream_outputs;
            relations.key = Some(owner.id.to_string());
        }
        Some(SubgraphScope::Iteration) => {
            let config = crate::iteration::parse_config(owner)?;
            let mut inputs = config.inputs;
            ensure!(
                inputs.keys().all(|name| !name.trim().is_empty()
                    && !matches!(name.as_str(), "item" | "key" | "items" | "index")),
                crate::compiler::InvalidScopeSnafu {
                    path: crate::loops::path_label(&owner_path),
                    message: "Iteration shared inputs must be nonblank and not item, key, items or index",
                }
            );
            inputs.insert("item".into(), ValueType::Any);
            inputs.insert("key".into(), ValueType::Any);
            relations.scope = Some(ScopeRelations {
                input: mf_runtime::ITERATION_INPUT_ID.into(),
                values: inputs,
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: BTreeSet::new(),
            });
            relations.streams = config.stream_outputs;
            relations.key = Some(owner.id.to_string());
        }
        Some(SubgraphScope::Host) => {
            #[derive(Deserialize)]
            struct Selector {
                flow: String,
            }
            let selector: Selector = serde_json::from_value(owner.config.clone()).context(
                crate::compiler::NodeConfigurationSnafu {
                    definition_id: owner.id.clone(),
                },
            )?;
            relations.host = Some(selector.flow);
            relations.key = Some(owner.id.to_string());
        }
        _ => {}
    }
    ensure!(
        relations.streams.is_empty() || version.supports_child_streams(),
        crate::compiler::InvalidGraphSnafu {
            definition_id: owner.id.clone(),
            message: "child stream exports require definition version 2026-10-28",
        }
    );
    Ok(relations)
}

/// Lower only the current authored scope. Executable children are selected by
/// receiving callbacks and enter this boundary with their own relationship record.
pub fn lower_scope(
    definition: &mut WorkflowDefinition,
    relations: &mut GraphRelations,
) -> Result<(), WorkflowCompileError> {
    lower_scope_at(definition, relations, &[], &BTreeMap::new(), false)
}

pub fn lower_scope_at(
    definition: &mut WorkflowDefinition,
    relations: &mut GraphRelations,
    path: &[DefinitionId],
    subgraphs: &BTreeMap<String, SubgraphScope>,
    lowered_sources: bool,
) -> Result<(), WorkflowCompileError> {
    // Scope sources are implicit references, never authored node declarations.
    // Reject both reserved IDs and internal kinds before injecting any source.
    // Resolved-definition adapters retain canonical sources from an earlier pass;
    // authored definitions cannot opt into that path. Artifact replay uses stored relations.
    for node in &definition.nodes {
        let reserved = matches!(
            node.id.as_str(),
            mf_runtime::ROOT_SOURCE_ID
                | mf_runtime::LOOP_SOURCE_ID
                | mf_runtime::ITERATION_INPUT_ID
        ) || matches!(
            node.kind.as_str(),
            mf_runtime::ROOT_SOURCE_ID
                | mf_runtime::LOOP_SOURCE_ID
                | mf_runtime::ITERATION_INPUT_KIND
        );
        if !reserved {
            continue;
        }
        let mut node_path = path.to_vec();
        node_path.push(node.id.clone());
        ensure!(
            lowered_sources
                && node.config == json!({})
                && ((relations.scope.is_none()
                    && !definition.variables.is_empty()
                    && node.id.as_str() == mf_runtime::ROOT_SOURCE_ID
                    && node.kind == mf_runtime::ROOT_SOURCE_ID)
                    || relations.scope.as_ref().is_some_and(|scope| {
                        let kind = if scope.is_stateful() {
                            mf_runtime::LOOP_SOURCE_ID
                        } else {
                            mf_runtime::ITERATION_INPUT_KIND
                        };
                        node.id == scope.input && node.kind == kind
                    })),
            crate::compiler::InvalidScopeSnafu {
                path: crate::loops::path_label(&node_path),
                message: "scope sources are implicit; reference them without declaring a node",
            }
        );
    }
    // Callable Flows own their own root state; container bodies cannot redeclare it.
    relations.root = None;
    if !definition.variables.is_empty() {
        let mut variables_path = path.to_vec();
        variables_path.push(DefinitionId::from("variables"));
        let label = crate::loops::path_label(&variables_path);
        ensure!(
            definition.version.supports_workflow_variables(),
            crate::compiler::InvalidScopeSnafu {
                path: &label,
                message: "workflow variables require definition version 2026-11-13",
            }
        );
        ensure!(
            relations.scope.is_none(),
            crate::compiler::InvalidScopeSnafu {
                path: &label,
                message: "workflow variables require a callable Flow; container bodies cannot declare them",
            }
        );
        relations.root = Some(
            mf_runtime::root_scope(&definition.variables, mf_runtime::ROOT_SOURCE_ID.into())
                .context(crate::compiler::WorkflowVariablesSnafu { path: &label })?,
        );
        if !definition
            .nodes
            .iter()
            .any(|node| node.id.as_str() == mf_runtime::ROOT_SOURCE_ID)
        {
            definition.nodes.insert(
                0,
                NodeDefinition {
                    id: mf_runtime::ROOT_SOURCE_ID.into(),
                    kind: mf_runtime::ROOT_SOURCE_ID.into(),
                    config: json!({}),
                },
            );
        }
    }
    if let Some(scope) = &relations.scope {
        ensure!(
            !scope.is_stateful()
                || definition.outputs.is_empty()
                || definition.version.supports_loop_output_updates(),
            crate::compiler::InvalidScopeSnafu {
                path: crate::loops::path_label(path),
                message: "Loop body output updates require definition version 2026-11-13",
            }
        );
        if !scope.is_stateful() {
            crate::iteration::validate_body_structure(definition, subgraphs).context(
                crate::compiler::ScopeStructureSnafu {
                    definition_id: DefinitionId::from("workflow"),
                },
            )?;
        }
        let expected_kind = if scope.is_stateful() {
            mf_runtime::LOOP_SOURCE_ID
        } else {
            mf_runtime::ITERATION_INPUT_KIND
        };
        if !definition.nodes.iter().any(|node| node.id == scope.input) {
            definition.nodes.insert(
                0,
                NodeDefinition {
                    id: scope.input.clone(),
                    kind: expected_kind.into(),
                    config: json!({}),
                },
            );
        }
    }
    // Validate authoring source/control syntax before turning it into operands.
    // This is the only boundary that interprets these node-kind tokens.
    let state_types = relations
        .scope
        .as_ref()
        .filter(|scope| scope.is_stateful())
        .map(|scope| {
            scope
                .state_outputs
                .keys()
                .filter_map(|name| {
                    scope
                        .values
                        .get(name)
                        .map(|value_type| (name.clone(), value_type.clone()))
                })
                .collect::<BTreeMap<_, _>>()
        });
    crate::loops::validate_scope(
        definition,
        state_types.as_ref(),
        relations.root.as_ref().map(|root| &root.values),
        path,
        subgraphs,
    )?;
    if let Some(scope) = relations.scope.as_mut() {
        scope.writes.clear();
        scope.exits.clear();
    }
    for node in &definition.nodes {
        let mut node_path = path.to_vec();
        node_path.push(node.id.clone());
        let path = node_path.as_slice();
        match node.kind.as_str() {
            // `validate_scope` already proved the canonical endpoint above.
            mf_runtime::ROOT_SOURCE_ID => {}
            mf_runtime::LOOP_SOURCE_ID | mf_runtime::ITERATION_INPUT_KIND => {
                let scope = relations.scope.as_ref();
                let valid = scope.is_some_and(|scope| {
                    let expected_kind = if scope.is_stateful() {
                        mf_runtime::LOOP_SOURCE_ID
                    } else {
                        mf_runtime::ITERATION_INPUT_KIND
                    };
                    node.id == scope.input && node.kind == expected_kind && node.config == json!({})
                });
                ensure!(
                    valid,
                    crate::compiler::InvalidScopeSnafu {
                        path: crate::loops::path_label(path),
                        message: "reserved scope source requires its canonical enclosing scope",
                    }
                );
            }
            mf_runtime::ASSIGN_KIND => {
                // validate_scope checked ownership, target existence and spelling.
                let target = crate::loops::assignment_target(&node.config, path)?;
                if let Some(scope) = relations.scope.as_mut() {
                    scope.writes.insert(node.id.clone(), target);
                } else {
                    relations
                        .root
                        .as_mut()
                        .expect("validated root state")
                        .writes
                        .insert(node.id.clone(), target);
                }
            }
            mf_runtime::EXIT_LOOP_KIND => {
                relations
                    .scope
                    .as_mut()
                    .expect("validated state scope")
                    .exits
                    .insert(node.id.clone());
            }
            _ => {
                ensure!(
                    node.id.as_str() != mf_runtime::LOOP_SOURCE_ID
                        && node.id.as_str() != mf_runtime::ITERATION_INPUT_ID
                        && node.id.as_str() != mf_runtime::ROOT_SOURCE_ID,
                    crate::compiler::InvalidScopeSnafu {
                        path: crate::loops::path_label(path),
                        message: "reserved scope source must use its canonical declaration",
                    }
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iteration_shared_inputs_lower_to_immutable_scope_values() {
        let subgraphs =
            BTreeMap::from([(mf_runtime::ITERATION_KIND.into(), SubgraphScope::Iteration)]);
        let owner = NodeDefinition {
            id: "each".into(),
            kind: mf_runtime::ITERATION_KIND.into(),
            config: json!({"inputs":{"flow_0":"flow", "settings":"object"}}),
        };
        let relations = child_relations(
            WorkflowDefinitionVersion::CURRENT,
            &owner,
            "body",
            &subgraphs,
        )
        .unwrap();
        let scope = relations.scope.unwrap();
        assert_eq!(
            scope.values,
            BTreeMap::from([
                ("flow_0".into(), ValueType::Flow),
                ("settings".into(), ValueType::Object),
                ("item".into(), ValueType::Any),
                ("key".into(), ValueType::Any),
            ])
        );
        assert!(scope.state_outputs.is_empty());
        assert!(scope.writes.is_empty());
        assert!(scope.exits.is_empty());
    }

    #[test]
    fn item_scope_rejects_declared_container_scopes_by_metadata() {
        let item_owner = NodeDefinition {
            id: "each".into(),
            kind: "test.custom-item".into(),
            config: json!({"inputs":{}}),
        };
        let context = ScopeContext::default().with_subgraphs(BTreeMap::from([(
            "test.custom-item".to_owned(),
            SubgraphScope::Iteration,
        )]));
        let item = context
            .child(WorkflowDefinitionVersion::CURRENT, &item_owner, "body")
            .unwrap();
        let loop_owner = NodeDefinition {
            id: "repeat".into(),
            kind: "test.custom-loop".into(),
            config: json!({"variables":[]}),
        };
        let item = item.with_subgraphs(BTreeMap::from([(
            "test.custom-loop".to_owned(),
            SubgraphScope::Loop,
        )]));
        let error = item
            .child(WorkflowDefinitionVersion::CURRENT, &loop_owner, "body")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot contain container scopes")
        );
    }

    #[test]
    fn declared_subgraph_scope_selects_relationships_instead_of_kind() {
        let custom_loop = NodeDefinition {
            id: "custom".into(),
            kind: "test.custom-loop".into(),
            config: json!({"variables":[{"name":"count", "type":"int"}]}),
        };
        let declared = BTreeMap::from([("test.custom-loop".to_owned(), SubgraphScope::Loop)]);
        let relations = child_relations(
            WorkflowDefinitionVersion::CURRENT,
            &custom_loop,
            "body",
            &declared,
        )
        .unwrap();
        assert!(relations.scope.as_ref().unwrap().is_stateful());
        assert!(
            child_relations(
                WorkflowDefinitionVersion::CURRENT,
                &custom_loop,
                "body",
                &BTreeMap::new()
            )
            .unwrap()
            .scope
            .is_none(),
            "a kind without provider metadata has no frame"
        );
        let builtin_loop = NodeDefinition {
            id: "repeat".into(),
            kind: mf_runtime::LOOP_KIND.into(),
            config: json!({"variables":[{"name":"count", "type":"int"}]}),
        };
        assert!(
            child_relations(
                WorkflowDefinitionVersion::CURRENT,
                &builtin_loop,
                "body",
                &BTreeMap::new()
            )
            .unwrap()
            .scope
            .is_none(),
            "built-in kind strings do not imply a frame"
        );
        let custom_host = NodeDefinition {
            id: "invoke".into(),
            kind: "test.custom-host".into(),
            config: json!({"flow":"child"}),
        };
        let relations = child_relations(
            WorkflowDefinitionVersion::CURRENT,
            &custom_host,
            "body",
            &BTreeMap::from([("test.custom-host".to_owned(), SubgraphScope::Host)]),
        )
        .unwrap();
        assert_eq!(relations.host.as_deref(), Some("child"));
    }

    #[test]
    fn lower_scope_keeps_full_authoring_paths_across_generic_bindings() {
        let subgraphs = BTreeMap::from([
            ("test.receiver".into(), SubgraphScope::Host),
            (mf_runtime::LOOP_KIND.into(), SubgraphScope::Loop),
        ]);
        let owner = NodeDefinition {
            id: "receiver".into(),
            kind: "test.receiver".into(),
            config: json!({}),
        };
        let outer = ScopeContext::default()
            .with_subgraphs(subgraphs)
            .child(WorkflowDefinitionVersion::CURRENT, &owner, "selected")
            .unwrap();
        let frame = NodeDefinition {
            id: "repeat".into(),
            kind: mf_runtime::LOOP_KIND.into(),
            config: json!({"variables":[{"name":"count", "type":"int"}]}),
        };
        let mut context = outer
            .child(WorkflowDefinitionVersion::CURRENT, &frame, "body")
            .unwrap();
        assert_eq!(context.relations.key.as_deref(), Some("repeat"));
        let mut definition = WorkflowDefinition::from_json(
            &json!({
                "version":"2026-11-13", "nodes":[{
                    "id":"write", "kind":mf_runtime::ASSIGN_KIND,
                    "config":{"variable":"missing"}
                }]
            })
            .to_string(),
        )
        .unwrap();
        let error = context.lower_scope(&mut definition).unwrap_err();
        let expected_path = crate::loops::path_label(&[
            mf_runtime::ScopePathEntry::binding_key("receiver", "selected").into(),
            "repeat".into(),
            "write".into(),
        ]);
        assert!(
            matches!(error, WorkflowCompileError::InvalidScope { path, message }
            if path == expected_path && message == "unknown variable `missing`")
        );
    }
}
