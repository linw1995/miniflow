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
                message: "static Flow dispatch exceeds the scope nesting limit",
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
                    let call_id = format!("{}:route:{route}", node.id);
                    body.relations.key = Some(call_id.clone());
                    let mut target = actual.clone();
                    target.push(call_id);
                    self.sources.insert(source.clone());
                    self.freeze(&mut body, &mut source.clone(), &mut target)?;
                    self.sources.remove(&source);
                    bodies.insert(route.clone(), body);
                }
                expand_dispatch(scope, index, bodies)?;
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
        crate::routers::lower(&mut scope.definition, true)?;
        Ok(())
    }
}

/// Dispatch is authoring syntax only: every executable target is an ordinary
/// static Call with a body binding. The generated router never receives Flows.
fn expand_dispatch(
    scope: &mut ResolvedWorkflow,
    index: usize,
    bodies: BTreeMap<String, ResolvedWorkflow>,
) -> Result<(), WorkflowCompileError> {
    use mf_runtime::{ControlEdgeDefinition, EdgeDefinition};
    use serde_json::json;
    let template = scope.definition.nodes[index].clone();
    let id = &template.id;
    ensure!(
        scope
            .definition
            .edges
            .iter()
            .filter(|edge| edge.to_node == *id)
            .all(|edge| matches!(edge.to_input.as_str(), "flows" | "route" | "arguments"))
            && ["flows", "route", "arguments"].iter().all(|input| scope
                .definition
                .edges
                .iter()
                .any(|edge| edge.to_node == *id && edge.to_input == *input))
            && scope
                .definition
                .edges
                .iter()
                .filter(|edge| edge.from_node == *id)
                .all(|edge| edge.from_output == "result")
            && scope
                .definition
                .outputs
                .iter()
                .filter(|output| output.node == *id)
                .all(|output| output.port == "result")
            && scope
                .definition
                .control_edges
                .iter()
                .filter(|edge| edge.from_node == *id)
                .all(|edge| edge.from_output == "result"),
        crate::compiler::InvalidGraphSnafu {
            definition_id: id.clone(),
            message: "dispatch requires flows, route, and arguments inputs and only exposes result",
        }
    );
    let result_id = mf_runtime::DefinitionId::from(format!("{id}:result"));
    let keys_id = mf_runtime::DefinitionId::from(format!("{id}:enables"));
    let assign_id = mf_runtime::DefinitionId::from(format!("{id}:set_enables"));
    let else_id = mf_runtime::DefinitionId::from(format!("{id}:else"));
    let mut generated = vec![
        NodeDefinition {
            id: result_id.clone(),
            kind: "builtin.identity".into(),
            config: json!({}),
        },
        NodeDefinition {
            id: else_id.clone(),
            kind: "builtin.constant".into(),
            config: json!({"value":{"error":"unknown or disabled route"}}),
        },
    ];
    let mut candidates = vec![else_id.clone()];
    let arguments: Vec<_> = scope
        .definition
        .edges
        .iter()
        .filter(|edge| edge.to_node == *id && edge.to_input == "arguments")
        .cloned()
        .collect();
    // Rewrite only authored result consumers. A route may itself be named
    // result, so generated branch gates must not enter this remapping.
    for edge in &mut scope.definition.control_edges {
        if edge.from_node == *id && edge.from_output == "result" {
            edge.from_node = result_id.clone();
            edge.from_output = "value".into();
        }
    }
    for (route, body) in &bodies {
        let call_id = mf_runtime::DefinitionId::from(format!("{id}:route:{route}"));
        let mut call = NodeDefinition {
            id: call_id.clone(),
            kind: mf_runtime::CALL_KIND.into(),
            config: json!({"flow":route,"body":{},"pack":true}),
        };
        call.set_node_flow("body", body.definition.clone())
            .context(crate::compiler::FlowParametersSnafu {
                definition_id: call_id.clone(),
            })?;
        generated.push(call);
        scope.children.insert(
            call_id.clone(),
            BTreeMap::from([("body".into(), body.clone())]),
        );
        scope.definition.control_edges.push(ControlEdgeDefinition {
            from_node: id.clone(),
            from_output: route.clone().into(),
            to_node: call_id.clone(),
        });
        for argument in &arguments {
            let mut argument = argument.clone();
            argument.to_node = call_id.clone();
            scope.definition.edges.push(argument);
        }
        candidates.push(call_id);
    }
    {
        // The template's prerequisites also protect reading mutable map sources;
        // moving that read before a workflow.assign would observe stale keys.
        let controls: Vec<_> = scope
            .definition
            .control_edges
            .iter()
            .filter(|edge| edge.to_node == *id)
            .cloned()
            .collect();
        for mut edge in controls {
            edge.to_node = keys_id.clone();
            scope.definition.control_edges.push(edge);
        }
        let orders: Vec<_> = scope
            .definition
            .order_edges
            .iter()
            .filter(|edge| edge.to_node == *id)
            .cloned()
            .collect();
        for mut edge in orders {
            edge.to_node = keys_id.clone();
            scope.definition.order_edges.push(edge);
        }
        generated.extend([
            NodeDefinition {
                id: keys_id.clone(),
                kind: "builtin.keys".into(),
                config: json!({}),
            },
            NodeDefinition {
                id: assign_id.clone(),
                kind: mf_runtime::ASSIGN_KIND.into(),
                config: json!({"variable":format!("%{id}.enables")}),
            },
        ]);
        scope.definition.edges.push(EdgeDefinition {
            from_node: keys_id.clone(),
            from_output: "keys".into(),
            to_node: assign_id.clone(),
            to_input: "value".into(),
            exclusive: false,
        });
        scope.definition.control_edges.push(ControlEdgeDefinition {
            from_node: assign_id,
            from_output: "done".into(),
            to_node: id.clone(),
        });
    }
    ensure!(
        generated.iter().all(|generated| !scope
            .definition
            .nodes
            .iter()
            .any(|node| node.id == generated.id)),
        crate::compiler::InvalidGraphSnafu {
            definition_id: id.clone(),
            message: "generated static dispatch node ID collides with an authored node",
        }
    );
    scope.definition.edges.retain_mut(|edge| {
        if edge.from_node == *id && edge.from_output == "result" {
            edge.from_node = result_id.clone();
            edge.from_output = "value".into();
        }
        if edge.to_node != *id {
            return true;
        }
        match edge.to_input.as_str() {
            "route" => true,
            "flows" => {
                edge.to_node = keys_id.clone();
                edge.to_input = "object".into();
                true
            }
            _ => false,
        }
    });
    for selection in &mut scope.definition.outputs {
        if selection.node == *id && selection.port == "result" {
            selection.node = result_id.clone();
            selection.port = "value".into();
        }
    }
    for edge in &mut scope.definition.order_edges {
        if edge.from_node == *id {
            edge.from_node = result_id.clone();
        }
    }
    scope.definition.control_edges.push(ControlEdgeDefinition {
        from_node: id.clone(),
        from_output: "else".into(),
        to_node: else_id.clone(),
    });
    let exclusive = candidates.len() > 1;
    for candidate in candidates {
        let port = if candidate == else_id {
            "value"
        } else {
            "result"
        };
        scope.definition.edges.push(EdgeDefinition {
            from_node: candidate,
            from_output: port.into(),
            to_node: result_id.clone(),
            to_input: "input".into(),
            exclusive,
        });
    }
    let routes: Vec<_> = bodies.keys().collect();
    scope.definition.nodes[index].kind = "builtin.router".into();
    scope.definition.nodes[index].config = json!({"routes":routes});
    scope.definition.nodes.extend(generated);
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
