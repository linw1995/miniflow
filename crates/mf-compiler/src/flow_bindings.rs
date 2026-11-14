//! Freeze edge-carried Flow capabilities at their invocation sites. This pass
//! interprets only provider-declared provenance and lowered scope relationships;
//! it neither executes nodes nor uses port types as evidence of a dynamic value.

use crate::{ResolvedWorkflow, WorkflowCompileError};
use mf_runtime::{
    FLOW_CANDIDATES, FlowBindingSource, FlowBindings, FlowCandidate, NodeDefinition,
    ScopePathEntry, flow_candidate_binding, flow_candidate_parameters,
};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

type Path = Vec<String>;
type Facts = BTreeSet<(Path, Path)>;
pub type BindingExtractors = BTreeMap<String, fn(&Value) -> FlowBindings>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Endpoint {
    Output(Path, String, String),
    Argument(Path, String),
}

type State = BTreeMap<Endpoint, Facts>;

fn output(path: &[String], node: &str, port: &str) -> Endpoint {
    Endpoint::Output(path.to_vec(), node.into(), port.into())
}

fn read(state: &State, key: Endpoint) -> Facts {
    state.get(&key).cloned().unwrap_or_default()
}

fn merge(state: &mut State, key: Endpoint, facts: Facts) -> bool {
    if facts.is_empty() {
        return false;
    }
    let target = state.entry(key).or_default();
    let previous = target.len();
    target.extend(facts);
    target.len() != previous
}

fn incoming(
    scope: &ResolvedWorkflow,
    path: &[String],
    node: &str,
    state: &State,
) -> BTreeMap<String, Facts> {
    let mut inputs: BTreeMap<String, Facts> = BTreeMap::new();
    let prefix = format!("{node}.");
    for (endpoint, facts) in state {
        if let Endpoint::Argument(argument_path, name) = endpoint
            && argument_path == path
            && let Some(port) = name.strip_prefix(&prefix)
        {
            inputs.entry(port.into()).or_default().extend(facts.clone());
        }
    }
    for edge in &scope.definition.edges {
        if edge.to_node.as_str() == node {
            inputs
                .entry(edge.to_input.to_string())
                .or_default()
                .extend(read(
                    state,
                    output(path, edge.from_node.as_str(), &edge.from_output),
                ));
        }
    }
    inputs
}

fn key(child: &ResolvedWorkflow, owner: &NodeDefinition, binding: &str) -> String {
    child
        .relations
        .key
        .clone()
        .unwrap_or_else(|| ScopePathEntry::binding_key(owner.id.as_str(), binding))
}

fn prefixed(field: &str, facts: Facts) -> Result<Facts, WorkflowCompileError> {
    facts
        .into_iter()
        .map(|(mut shape, source)| {
            ensure!(
                shape.len() < mf_telemetry::MAX_SCOPE_DEPTH,
                crate::compiler::InvalidGraphSnafu {
                    definition_id: mf_runtime::DefinitionId::from("workflow"),
                    message: "Flow capability provenance exceeds the nesting limit",
                }
            );
            shape.insert(0, field.into());
            Ok((shape, source))
        })
        .collect()
}

fn propagate(
    scope: &ResolvedWorkflow,
    path: &mut Path,
    extractors: &BindingExtractors,
    state: &mut State,
) -> Result<bool, WorkflowCompileError> {
    let mut changed = false;
    if let Some(root) = &scope.relations.root {
        for (writer, variable) in &root.writes {
            let facts = incoming(scope, path, writer.as_str(), state)
                .remove("value")
                .unwrap_or_default();
            changed |= merge(state, output(path, root.input.as_str(), variable), facts);
        }
    }
    for node in &scope.definition.nodes {
        let inputs = incoming(scope, path, node.id.as_str(), state);
        let bindings = extractors
            .get(&node.kind)
            .map(|extract| extract(&node.config))
            .unwrap_or_default();
        for (port, source) in &bindings.outputs {
            let facts = match source {
                FlowBindingSource::Child(binding) => scope
                    .children
                    .get(&node.id)
                    .and_then(|children| children.get(binding))
                    .map(|child| {
                        let mut source = path.clone();
                        source.push(key(child, node, binding));
                        BTreeSet::from([(Vec::new(), source)])
                    })
                    .unwrap_or_default(),
                FlowBindingSource::Input(input) => inputs.get(input).cloned().unwrap_or_default(),
                FlowBindingSource::Object(fields) => {
                    let mut facts = Facts::new();
                    for (field, input) in fields {
                        facts.extend(prefixed(
                            field,
                            inputs.get(input).cloned().unwrap_or_default(),
                        )?);
                    }
                    facts
                }
            };
            changed |= merge(state, output(path, node.id.as_str(), port), facts);
        }
        // An opaque transformation can select, filter, or forward any incoming
        // capability. Its inputs bound the superset, not its runtime key set.
        if !extractors.contains_key(&node.kind) && !scope.children.contains_key(&node.id) {
            let facts: Facts = inputs.values().flatten().cloned().collect();
            let ports: BTreeSet<_> = scope
                .definition
                .edges
                .iter()
                .filter(|edge| edge.from_node == node.id)
                .map(|edge| edge.from_output.as_ref())
                .chain(
                    scope
                        .definition
                        .outputs
                        .iter()
                        .filter(|selection| selection.node == node.id)
                        .map(|selection| selection.port.as_ref()),
                )
                .collect();
            for port in ports {
                changed |= merge(state, output(path, node.id.as_str(), port), facts.clone());
            }
        }
        for (binding, child) in scope.children.get(&node.id).into_iter().flatten() {
            let mut child_path = path.clone();
            child_path.push(key(child, node, binding));
            if let Some(frame) = &child.relations.scope {
                for name in frame.values.keys() {
                    let facts = if name == "item" && !frame.is_stateful() {
                        inputs
                            .get("items")
                            .into_iter()
                            .flatten()
                            .filter(|(shape, _)| !shape.is_empty())
                            .map(|(shape, source)| (shape[1..].to_vec(), source.clone()))
                            .collect()
                    } else {
                        inputs.get(name).cloned().unwrap_or_default()
                    };
                    changed |= merge(
                        state,
                        output(&child_path, frame.input.as_str(), name),
                        facts,
                    );
                }
                for (writer, variable) in &frame.writes {
                    let values = incoming(child, &child_path, writer.as_str(), state)
                        .remove("value")
                        .unwrap_or_default();
                    changed |= merge(
                        state,
                        output(&child_path, frame.input.as_str(), variable),
                        values,
                    );
                }
                for (variable, port) in &frame.state_outputs {
                    changed |= merge(
                        state,
                        output(path, node.id.as_str(), port),
                        read(state, output(&child_path, frame.input.as_str(), variable)),
                    );
                }
            } else if child.relations.host.is_some() {
                for (name, facts) in &inputs {
                    changed |= merge(
                        state,
                        Endpoint::Argument(child_path.clone(), name.clone()),
                        facts.clone(),
                    );
                }
            }
            changed |= propagate(child, &mut child_path, extractors, state)?;
            for selection in &child.definition.outputs {
                let facts = read(
                    state,
                    output(&child_path, selection.node.as_str(), &selection.port),
                );
                if child.relations.host.is_some() {
                    changed |= merge(
                        state,
                        output(path, node.id.as_str(), &selection.name),
                        facts,
                    );
                } else if let Some(frame) = &child.relations.scope
                    && frame.state_outputs.contains_key(selection.name.as_ref())
                {
                    changed |= merge(
                        state,
                        output(&child_path, frame.input.as_str(), &selection.name),
                        facts,
                    );
                }
            }
        }
    }
    Ok(changed)
}

fn find_scope<'a>(root: &'a ResolvedWorkflow, path: &[String]) -> Option<&'a ResolvedWorkflow> {
    let Some((first, rest)) = path.split_first() else {
        return Some(root);
    };
    for node in &root.definition.nodes {
        for (binding, child) in root.children.get(&node.id).into_iter().flatten() {
            if key(child, node, binding) == *first {
                return find_scope(child, rest);
            }
        }
    }
    None
}

struct Freezing<'a> {
    root: &'a ResolvedWorkflow,
    state: &'a State,
    extractors: &'a BindingExtractors,
    sources: BTreeSet<Path>,
}

impl Freezing<'_> {
    fn freeze(
        &mut self,
        scope: &mut ResolvedWorkflow,
        original: &mut Path,
        actual: &mut Path,
    ) -> Result<(), WorkflowCompileError> {
        ensure!(
            actual.len() <= mf_telemetry::MAX_SCOPE_DEPTH,
            crate::compiler::InvalidGraphSnafu {
                definition_id: mf_runtime::DefinitionId::from("workflow"),
                message: "compiled Flow candidates exceed the scope nesting limit",
            }
        );
        for index in 0..scope.definition.nodes.len() {
            let node = scope.definition.nodes[index].clone();
            let invocation = self
                .extractors
                .get(&node.kind)
                .and_then(|extract| extract(&node.config).invocation);
            if let Some(invocation) = invocation {
                let inputs = incoming(scope, original, node.id.as_str(), self.state);
                let facts = inputs.get(&invocation.input).cloned().unwrap_or_default();
                let mut candidates = BTreeMap::new();
                let mut children = BTreeMap::new();
                for (shape, source) in facts {
                    let route = match (shape.as_slice(), invocation.field.as_deref()) {
                        ([route], None) => route,
                        ([route, field], Some(expected)) if field == expected => route,
                        _ => continue,
                    };
                    ensure!(
                        !self.sources.contains(&source),
                        crate::compiler::InvalidGraphSnafu {
                            definition_id: node.id.clone(),
                            message: "recursive static Flow candidate expansion is unsupported",
                        }
                    );
                    let body = find_scope(self.root, &source).context(
                        crate::compiler::InvalidGraphSnafu {
                            definition_id: node.id.clone(),
                            message: "Flow candidate refers to an unknown compiled source",
                        },
                    )?;
                    let mut body = body.clone();
                    body.relations.key = None;
                    let binding = flow_candidate_binding(candidates.len());
                    let candidate = FlowCandidate {
                        route: route.clone(),
                        source: source.clone(),
                    };
                    let mut target = actual.clone();
                    target.push(key(&body, &node, &binding));
                    self.sources.insert(source.clone());
                    self.freeze(&mut body, &mut source.clone(), &mut target)?;
                    self.sources.remove(&source);
                    candidates.insert(binding.clone(), candidate);
                    children.insert(binding, body);
                }
                let node = &mut scope.definition.nodes[index];
                for binding in flow_candidate_parameters(&node.config).keys() {
                    node.config
                        .as_object_mut()
                        .expect("candidate table requires an object")
                        .remove(binding);
                }
                node.config[FLOW_CANDIDATES] = serde_json::to_value(candidates).context(
                    crate::compiler::NodeConfigurationSnafu {
                        definition_id: node.id.clone(),
                    },
                )?;
                if children.is_empty() {
                    scope.children.remove(&node.id);
                } else {
                    scope.children.insert(node.id.clone(), children);
                }
            } else {
                for (binding, child) in scope.children.get_mut(&node.id).into_iter().flatten() {
                    let child_key = key(child, &node, binding);
                    original.push(child_key.clone());
                    actual.push(child_key);
                    self.freeze(child, original, actual)?;
                    original.pop();
                    actual.pop();
                }
            }
            let node = &mut scope.definition.nodes[index];
            for (binding, child) in scope.children.get(&node.id).into_iter().flatten() {
                node.set_node_flow(binding, child.definition.clone())
                    .context(crate::compiler::FlowParametersSnafu {
                        definition_id: node.id.clone(),
                    })?;
            }
        }
        Ok(())
    }
}

pub fn freeze(
    scope: &mut ResolvedWorkflow,
    extractors: &BindingExtractors,
) -> Result<(), WorkflowCompileError> {
    let mut state = State::new();
    while propagate(scope, &mut Vec::new(), extractors, &mut state)? {}
    let root = scope.clone();
    Freezing {
        root: &root,
        state: &state,
        extractors,
        sources: BTreeSet::new(),
    }
    .freeze(scope, &mut Vec::new(), &mut Vec::new())
}
