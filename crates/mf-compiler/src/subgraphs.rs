use crate::{WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{SubgraphSource, WorkflowGraph, WorkflowOutputDefinition};
use snafu::ResultExt;
use std::collections::BTreeMap;

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
    match source {
        SubgraphSource::Node { node } => {
            let (mut definition, _) = source.frozen_definition(parent).expect("node source");
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
            Ok(SourceDefinition {
                definition,
                node: true,
                projections: vec![],
            })
        }
        SubgraphSource::Definition { definition } => Ok(SourceDefinition {
            definition: *definition.clone(),
            node: false,
            projections: vec![],
        }),
        SubgraphSource::Interface { source, inputs } => {
            let mut source = source_definition(parent, source, graph)?;
            source.projections.push(inputs.clone());
            Ok(source)
        }
        _ => crate::compiler::InvalidCallSnafu {
            definition_id: "subgraph",
            message: "subgraph parameter has not been resolved",
        }
        .fail(),
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
            match source {
                SubgraphSource::Collection { .. } => {
                    arguments.insert(
                        name.clone(),
                        SubgraphSource::Collection {
                            collection: BTreeMap::new(),
                        },
                    );
                }
                SubgraphSource::Interfaces { interfaces }
                    if matches!(interfaces.as_ref(), SubgraphSource::Collection { .. }) =>
                {
                    arguments.insert(
                        name.clone(),
                        SubgraphSource::Interfaces {
                            interfaces: Box::new(SubgraphSource::Collection {
                                collection: BTreeMap::new(),
                            }),
                        },
                    );
                }
                _ => {}
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
            let source = SubgraphSource::Interface {
                source: Box::new(SubgraphSource::Definition {
                    definition: Box::new(source),
                }),
                inputs,
            };
            if let Some(entry) = member.entry {
                let value = arguments
                    .get_mut(member.parameter)
                    .expect("collection initialized");
                let value = if let SubgraphSource::Interfaces { interfaces } = value {
                    interfaces.as_mut()
                } else {
                    value
                };
                let SubgraphSource::Collection { collection } = value else {
                    unreachable!()
                };
                collection.insert(entry.to_owned(), source);
            } else {
                arguments.insert(
                    member.parameter.to_owned(),
                    if member.interface_only {
                        SubgraphSource::Interfaces {
                            interfaces: Box::new(source),
                        }
                    } else {
                        source
                    },
                );
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
