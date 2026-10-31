use crate::{WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    NodeDefinition, SubgraphSource, SubgraphSourceKind, WorkflowGraph, WorkflowOutputDefinition,
};
use snafu::ResultExt;
use std::collections::BTreeMap;

/// The closed, post-resolution form of a subgraph source.
///
/// References, parameter declarations, and path/builtin lookups are gone: every value is an
/// executable inline body, a collection of them, or a view over one.
#[derive(Clone, Debug)]
pub enum ResolvedSubgraphSource {
    Inline {
        node: Option<Box<NodeDefinition>>,
        definition: Box<WorkflowDefinition>,
    },
    Collection {
        collection: BTreeMap<String, ResolvedSubgraphSource>,
    },
    View {
        source: Box<ResolvedSubgraphSource>,
        inputs: BTreeMap<String, String>,
        readonly: bool,
    },
}

impl ResolvedSubgraphSource {
    pub fn inline(definition: Box<WorkflowDefinition>) -> Self {
        Self::Inline {
            node: None,
            definition,
        }
    }

    pub fn inline_node(node: Box<NodeDefinition>, definition: Box<WorkflowDefinition>) -> Self {
        Self::Inline {
            node: Some(node),
            definition,
        }
    }

    pub fn view(source: Box<Self>, inputs: BTreeMap<String, String>, readonly: bool) -> Self {
        Self::View {
            source,
            inputs,
            readonly,
        }
    }

    pub fn from_source(
        source: &SubgraphSource,
        parent: &WorkflowDefinition,
    ) -> Result<Self, WorkflowCompileError> {
        match source {
            SubgraphSource::Source(SubgraphSourceKind::Inline {
                node: Some(node),
                definition: None,
            }) => {
                let (definition, _) = source
                    .frozen_definition(parent)
                    .expect("inline node source");
                Ok(Self::inline_node(node.clone(), Box::new(definition)))
            }
            SubgraphSource::Source(SubgraphSourceKind::Inline {
                definition: Some(definition),
                node: None,
            }) => Ok(Self::inline(definition.clone())),
            SubgraphSource::Collection { collection } => {
                let collection = collection
                    .iter()
                    .map(|(name, source)| Ok((name.clone(), Self::from_source(source, parent)?)))
                    .collect::<Result<_, WorkflowCompileError>>()?;
                Ok(Self::Collection { collection })
            }
            SubgraphSource::View {
                source,
                inputs,
                readonly,
            } => Ok(Self::view(
                Box::new(Self::from_source(source, parent)?),
                inputs.clone(),
                *readonly,
            )),
            _ => crate::compiler::InvalidCallSnafu {
                definition_id: "subgraph",
                message: "subgraph parameter has not been resolved",
            }
            .fail(),
        }
    }

    pub fn into_source(self) -> SubgraphSource {
        match self {
            Self::Inline {
                node: Some(node), ..
            } => SubgraphSource::inline_node(node),
            Self::Inline {
                node: None,
                definition,
            } => SubgraphSource::inline(definition),
            Self::Collection { collection } => SubgraphSource::Collection {
                collection: collection
                    .into_iter()
                    .map(|(name, source)| (name, source.into_source()))
                    .collect(),
            },
            Self::View {
                source,
                inputs,
                readonly,
            } => SubgraphSource::view(Box::new(source.into_source()), inputs, readonly),
        }
    }

    pub fn innermost(&self) -> &Self {
        match self {
            Self::View { source, .. } => source.innermost(),
            source => source,
        }
    }

    pub fn node_definition(&self) -> Option<&NodeDefinition> {
        match self.innermost() {
            Self::Inline { node, .. } => node.as_deref(),
            _ => None,
        }
    }

    pub fn is_readonly(&self) -> bool {
        match self {
            Self::View {
                source, readonly, ..
            } => *readonly || source.is_readonly(),
            _ => false,
        }
    }

    pub fn frozen_definition(&self) -> Option<(WorkflowDefinition, bool)> {
        match self {
            Self::Inline { node, definition } => Some(((**definition).clone(), node.is_some())),
            Self::View { source, .. } => source.frozen_definition(),
            Self::Collection { .. } => None,
        }
    }
}

pub struct SourceDefinition {
    pub definition: WorkflowDefinition,
    pub node: bool,
    pub projections: Vec<BTreeMap<String, String>>,
}

pub fn source_definition(
    parent: &WorkflowDefinition,
    source: &SubgraphSource,
    graph: Option<&WorkflowGraph>,
) -> Result<SourceDefinition, WorkflowCompileError> {
    let source = ResolvedSubgraphSource::from_source(source, parent)?;
    source_definition_resolved(&source, graph)
}

fn source_definition_resolved(
    source: &ResolvedSubgraphSource,
    graph: Option<&WorkflowGraph>,
) -> Result<SourceDefinition, WorkflowCompileError> {
    let (mut definition, node) = source
        .frozen_definition()
        .expect("resolved subgraph is callable");
    if let Some(node) = source.node_definition() {
        definition.outputs = graph
            .into_iter()
            .flat_map(|graph| graph.nodes.get(&node.id))
            .flat_map(|node| node.ports.outputs.iter().chain(&node.stream_outputs))
            .map(|port| WorkflowOutputDefinition {
                name: port.name.clone(),
                node: node.id.clone(),
                port: port.name.clone(),
                optional: !port.required,
            })
            .collect();
    }
    let mut projections = Vec::new();
    collect_projections(source, &mut projections);
    Ok(SourceDefinition {
        definition,
        node,
        projections,
    })
}

fn collect_projections(
    source: &ResolvedSubgraphSource,
    output: &mut Vec<BTreeMap<String, String>>,
) {
    if let ResolvedSubgraphSource::View {
        source,
        inputs,
        readonly,
    } = source
    {
        collect_projections(source, output);
        if !readonly || !inputs.is_empty() {
            output.push(inputs.clone());
        }
    }
}

fn empty_collection(source: &SubgraphSource) -> Option<SubgraphSource> {
    match source.innermost() {
        SubgraphSource::Collection { .. } => Some(replace_collection(source)),
        _ => None,
    }
}

fn replace_collection(source: &SubgraphSource) -> SubgraphSource {
    if let SubgraphSource::View {
        source,
        inputs,
        readonly,
    } = source
    {
        SubgraphSource::view(
            Box::new(replace_collection(source)),
            inputs.clone(),
            *readonly,
        )
    } else {
        SubgraphSource::Collection {
            collection: BTreeMap::new(),
        }
    }
}

fn collection_mut(source: &mut SubgraphSource) -> Option<&mut BTreeMap<String, SubgraphSource>> {
    match source {
        SubgraphSource::View { source, .. } => collection_mut(source),
        SubgraphSource::Collection { collection } => Some(collection),
        _ => None,
    }
}

pub fn aliases(
    graph: &WorkflowGraph,
    node: bool,
    projections: &[BTreeMap<String, String>],
) -> Result<BTreeMap<String, String>, WorkflowCompileError> {
    mf_runtime::project_subgraph_inputs(&graph.inputs, node, projections).context(
        crate::compiler::NodeConstructionSnafu {
            definition_id: "subgraph",
            kind: "subgraph",
        },
    )
}

pub fn freeze(
    definition: &WorkflowDefinition,
    layouts: &crate::compiler::ScopeLayouts,
    path: &mut Vec<mf_runtime::DefinitionId>,
) -> Result<WorkflowDefinition, WorkflowCompileError> {
    let mut frozen = definition.clone();
    for node in &mut frozen.nodes {
        let original = node.clone();
        path.push(node.id.clone());
        let mut arguments = BTreeMap::new();
        for (name, source) in &node.subgraphs {
            if let Some(source) = empty_collection(source) {
                arguments.insert(name.clone(), source);
            }
        }
        for member in original.subgraph_members() {
            path.push(member.id.clone());
            path.push(member.call_id().into());
            let source_layout = layouts.get(path.as_slice());
            let fallback;
            let graph = if let Some((_, _, graph)) = source_layout {
                graph.as_ref()
            } else {
                let (_, _, wrapper) = layouts
                    .get(&path[..path.len() - 1])
                    .expect("prepared parameter wrapper");
                fallback = source_interface_graph(wrapper, &member.call_id());
                &fallback
            };
            let SourceDefinition {
                definition: source,
                node: single_node,
                projections,
            } = source_definition(definition, member.source, Some(graph))?;
            let inputs = aliases(graph, single_node, &projections)?;
            let source = if source_layout.is_some() {
                freeze(&source, layouts, path)?
            } else {
                source
            };
            path.pop();
            path.pop();
            let source = SubgraphSource::view(
                Box::new(SubgraphSource::inline(Box::new(source))),
                inputs,
                false,
            );
            let source = if member.interface_only && member.entry.is_none() {
                SubgraphSource::view(Box::new(source), BTreeMap::new(), true)
            } else {
                source
            };
            if let Some(entry) = member.entry {
                let collection = arguments
                    .get_mut(member.parameter)
                    .and_then(collection_mut)
                    .expect("collection initialized");
                collection.insert(entry.to_owned(), source);
            } else {
                arguments.insert(member.parameter.to_owned(), source);
            }
        }
        node.subgraphs = arguments;
        if let Some(body) = &mut node.loop_definition {
            let definition = crate::loops::body_definition(
                &body.body,
                &definition.dependencies,
                definition.version,
            );
            let frozen = freeze(&definition, layouts, path)?;
            body.body.nodes = frozen
                .nodes
                .into_iter()
                .filter(|node| node.kind != mf_runtime::LOOP_SOURCE_ID)
                .collect();
        } else if node.kind == mf_runtime::CALL_KIND && layouts.contains_key(path.as_slice()) {
            let mut body = crate::calls::body_definition(node)?;
            if node.config["node_source"] == true {
                let (_, _, graph) = layouts
                    .get(path.as_slice())
                    .expect("prepared Call body layout");
                body.outputs = graph
                    .nodes
                    .iter()
                    .flat_map(|(id, node)| {
                        node.ports
                            .outputs
                            .iter()
                            .chain(&node.stream_outputs)
                            .map(|port| WorkflowOutputDefinition {
                                name: port.name.clone(),
                                node: id.clone(),
                                port: port.name.clone(),
                                optional: !port.required,
                            })
                    })
                    .collect();
                let config: mf_runtime::CallConfig = serde_json::from_value(node.config.clone())
                    .context(crate::compiler::CallConfigurationSnafu {
                        definition_id: node.id.clone(),
                    })?;
                let inputs = aliases(graph, true, &config.input_projections)?;
                node.config["input_projections"] = serde_json::json!([inputs]);
                node.config
                    .as_object_mut()
                    .expect("Call config")
                    .remove("node_source");
            }
            let body = freeze(&body, layouts, path)?;
            node.config["definition"] =
                serde_json::to_value(body).expect("workflow definition serializes");
        } else if node.kind == mf_runtime::ITERATION_KIND {
            let mut config = crate::iteration::parse_config(node)?;
            let body = crate::iteration::body_definition(definition, &config).context(
                crate::compiler::IterationValidationSnafu {
                    definition_id: node.id.clone(),
                },
            )?;
            let body = freeze(&body, layouts, path)?;
            config.body.nodes = body
                .nodes
                .into_iter()
                .filter(|node| node.kind != mf_runtime::ITERATION_INPUT_KIND)
                .collect();
            node.config = serde_json::to_value(config).expect("iteration configuration serializes");
        }
        path.pop();
    }
    Ok(frozen)
}

pub fn source_interface_graph(wrapper: &WorkflowGraph, call: &str) -> WorkflowGraph {
    let prefix = format!("{call}.");
    WorkflowGraph {
        inputs: wrapper
            .inputs
            .iter()
            .filter_map(|(name, input)| {
                name.strip_prefix(&prefix)
                    .map(|name| (name.to_owned(), input.clone()))
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition() -> WorkflowDefinition {
        serde_json::from_value(json!({
            "version": "2026-10-31",
            "dependencies": {},
            "nodes": []
        }))
        .unwrap()
    }

    fn inline_source() -> SubgraphSource {
        serde_json::from_value(json!({
            "definition": {
                "version": "2026-10-31",
                "dependencies": {},
                "nodes": []
            }
        }))
        .unwrap()
    }

    fn node_source() -> SubgraphSource {
        serde_json::from_value(json!({
            "node": {
                "id": "constant",
                "kind": "builtin.constant",
                "config": {"value": 1}
            }
        }))
        .unwrap()
    }

    #[test]
    fn resolved_sources_are_closed_and_convert_back_to_wire_sources() {
        let parent = definition();

        let collection = SubgraphSource::Collection {
            collection: BTreeMap::from([("one".into(), inline_source())]),
        };
        let resolved = ResolvedSubgraphSource::from_source(&collection, &parent).unwrap();
        assert!(matches!(
            resolved.innermost(),
            ResolvedSubgraphSource::Collection { .. }
        ));
        assert!(resolved.frozen_definition().is_none());
        assert!(!resolved.is_readonly());
        assert!(resolved.node_definition().is_none());
        assert!(matches!(
            resolved.into_source(),
            SubgraphSource::Collection { .. }
        ));

        let view = SubgraphSource::view(Box::new(node_source()), BTreeMap::new(), true);
        let resolved = ResolvedSubgraphSource::from_source(&view, &parent).unwrap();
        assert!(resolved.is_readonly());
        assert!(resolved.node_definition().is_some());
        let (definition, node) = resolved.frozen_definition().unwrap();
        assert!(node);
        assert_eq!(definition.nodes.len(), 1);
        assert!(matches!(
            resolved.into_source(),
            SubgraphSource::View { readonly: true, .. }
        ));

        let unresolved = SubgraphSource::Reference {
            reference: "missing".into(),
        };
        assert!(ResolvedSubgraphSource::from_source(&unresolved, &parent).is_err());
    }
}
