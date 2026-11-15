//! Freeze edge-carried Flow capabilities at their invocation sites. This pass
//! interprets only provider-declared provenance and lowered scope relationships;
//! it neither executes nodes nor uses port types as evidence of a dynamic value.

use crate::{ResolvedWorkflow, WorkflowCompileError};
use mf_runtime::{FlowBindingSource, FlowBindings, NodeDefinition, ScopePathEntry};
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
    for node in &scope.definition.nodes {
        let Some(mf_runtime::NodeOperation::Assign { target }) = &node.operation else {
            continue;
        };
        let writer = &node.id;
        let source = match target {
            mf_runtime::StateTarget::Root { .. } => {
                scope.relations.root.as_ref().map(|root| &root.input)
            }
            mf_runtime::StateTarget::Frame { .. } => {
                scope.relations.scope.as_ref().map(|frame| &frame.input)
            }
            mf_runtime::StateTarget::Node { .. } => None,
        };
        if let Some(source) = source {
            let facts = incoming(scope, path, writer.as_str(), state)
                .remove("value")
                .unwrap_or_default();
            changed |= merge(state, output(path, source.as_str(), target.field()), facts);
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
                message: "static Flow dispatch exceeds the scope nesting limit",
            }
        );
        for index in 0..scope.definition.nodes.len() {
            let node = scope.definition.nodes[index].clone();
            let invocation = self
                .extractors
                .get(&node.kind)
                .and_then(|extract| extract(&node.config).invocation);
            if let Some(input) = invocation {
                let inputs = incoming(scope, original, node.id.as_str(), self.state);
                let facts = inputs.get(&input).cloned().unwrap_or_default();
                let mut bodies = BTreeMap::new();
                for (shape, source) in facts {
                    let [route] = shape.as_slice() else {
                        continue;
                    };
                    ensure!(
                        !route.trim().is_empty() && route != "else" && !bodies.contains_key(route),
                        crate::compiler::InvalidGraphSnafu {
                            definition_id: node.id.clone(),
                            message: "dispatch routes must be nonblank, unreserved, and bound to exactly one static Flow",
                        }
                    );
                    ensure!(
                        !self.sources.contains(&source),
                        crate::compiler::InvalidGraphSnafu {
                            definition_id: node.id.clone(),
                            message: "recursive static Flow dispatch expansion is unsupported",
                        }
                    );
                    let mut body = find_scope(self.root, &source)
                        .context(crate::compiler::InvalidGraphSnafu {
                            definition_id: node.id.clone(),
                            message: "Flow dispatch refers to an unknown compiled source",
                        })?
                        .clone();
                    let binding = ScopePathEntry::binding_key(node.id.as_str(), route);
                    body.relations.key = Some(binding.clone());
                    body.relations.static_binding = true;
                    let mut target = actual.clone();
                    target.push(binding);
                    self.sources.insert(source.clone());
                    self.freeze(&mut body, &mut source.clone(), &mut target)?;
                    self.sources.remove(&source);
                    bodies.insert(route.clone(), body);
                }
                bind_routes(scope, index, bodies)?;
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
                if !child.relations.static_binding {
                    node.set_node_flow(binding, child.definition.clone())
                        .context(crate::compiler::FlowParametersSnafu {
                            definition_id: node.id.clone(),
                        })?;
                }
            }
        }
        Ok(())
    }
}

/// Bind static branches to their authored owner without changing node topology.
fn bind_routes(
    scope: &mut ResolvedWorkflow,
    index: usize,
    bodies: BTreeMap<String, ResolvedWorkflow>,
) -> Result<(), WorkflowCompileError> {
    let node = &mut scope.definition.nodes[index];
    let configured: Vec<String> = serde_json::from_value(node.config["routes"].clone()).context(
        crate::compiler::NodeConfigurationSnafu {
            definition_id: node.id.clone(),
        },
    )?;
    let routes = if configured.is_empty() {
        bodies.keys().cloned().collect()
    } else {
        ensure!(
            configured
                .iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .eq(bodies.keys()),
            crate::compiler::InvalidGraphSnafu {
                definition_id: node.id.clone(),
                message: "router routes differ from the statically bound Flow set",
            }
        );
        configured
    };
    node.config["routes"] =
        serde_json::to_value(routes).context(crate::compiler::NodeConfigurationSnafu {
            definition_id: node.id.clone(),
        })?;
    if bodies.is_empty() {
        scope.children.remove(&node.id);
    } else {
        scope.children.insert(node.id.clone(), bodies);
    }
    Ok(())
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
