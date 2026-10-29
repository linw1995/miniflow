use crate::{DefinitionId, FlowDependency, FlowNode, NodeExecution, NodeId, PortSpec};
use mf_runtime::{
    CallBinding, CallBindingId, CallInputDefaults, CallInputSource, ResolvedCallBinding,
    ResolvedCallInput,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Snafu)]
pub enum CallBindingError {
    #[snafu(
        display("invalid directed-call declaration: {message}"),
        visibility(pub)
    )]
    Definition { message: String },
    #[snafu(transparent)]
    Contract {
        source: mf_runtime::CallContractError,
    },
    #[snafu(transparent)]
    Preparation {
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
    #[snafu(
        display("call {role} references unknown node index {index}"),
        visibility(pub)
    )]
    UnknownNode { role: &'static str, index: usize },
    #[snafu(display("call alias `{name}` is declared more than once"))]
    DuplicateName { name: String },
    #[snafu(display("node `{node}` is not a supported call {role}: {reason}"))]
    UnsupportedNode {
        node: DefinitionId,
        role: &'static str,
        reason: &'static str,
    },
    #[snafu(display("node `{node}` has no {direction} port `{port}`"))]
    UnknownPort {
        node: DefinitionId,
        direction: &'static str,
        port: String,
    },
    #[snafu(display("node `{node}` has an empty or duplicate {direction} port `{port}`"))]
    InvalidPort {
        node: DefinitionId,
        direction: &'static str,
        port: String,
    },
    #[snafu(display("node `{node}` {direction} port `{port}`: {source}"))]
    PortDepth {
        node: DefinitionId,
        direction: &'static str,
        port: String,
        source: mf_runtime::TypeDepthError,
    },
    #[snafu(display("call `{name}` required input `{input}` has no source"))]
    MissingInput { name: String, input: String },
    #[snafu(display("call `{name}` connection: {source}"))]
    ConnectionType {
        name: String,
        #[snafu(source(from(crate::WorkflowCompileError, Box::new)))]
        source: Box<crate::WorkflowCompileError>,
    },
    #[snafu(display(
        "call `{name}` cannot use producer `{producer}` because it depends on `{blocker}`"
    ))]
    DependencyCycle {
        name: String,
        producer: DefinitionId,
        blocker: DefinitionId,
    },
    #[snafu(transparent)]
    Flow { source: crate::FlowBuildError },
}

pub fn declarations(
    definition: &crate::WorkflowDefinition,
) -> Result<BTreeMap<NodeId, Vec<CallBinding>>, CallBindingError> {
    ensure!(
        definition.calls.is_empty() || definition.version.supports_directed_calls(),
        DefinitionSnafu {
            message: "directed calls require version 2026-10-28",
        }
    );
    let indices: BTreeMap<_, _> = definition
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), NodeId::new(index)))
        .collect();
    let mut result = BTreeMap::new();
    for (caller, aliases) in &definition.calls {
        ensure!(
            !aliases.is_empty(),
            DefinitionSnafu {
                message: format!("caller `{caller}` must declare at least one call")
            }
        );
        let caller_id = indices.get(caller).with_context(|| DefinitionSnafu {
            message: format!("unknown caller `{caller}`"),
        })?;
        ensure!(
            !matches!(
                definition.nodes[caller_id.index()].kind.as_str(),
                mf_runtime::CALL_KIND | mf_runtime::LOOP_KIND | mf_runtime::ITERATION_KIND
            ),
            DefinitionSnafu {
                message: format!("caller `{caller}` is a container; declare calls inside its body"),
            }
        );
        let mut bindings = Vec::new();
        for (name, declaration) in aliases {
            let target = indices
                .get(&declaration.target)
                .with_context(|| DefinitionSnafu {
                    message: format!(
                        "call `{caller}`.`{name}` references unknown target `{}`",
                        declaration.target
                    ),
                })?;
            ensure!(
                definition.nodes[target.index()].kind == mf_runtime::CALL_KIND,
                DefinitionSnafu {
                    message: format!(
                        "call `{caller}`.`{name}` must target a workflow.call with explicit workflow outputs"
                    ),
                }
            );
            let binding = CallBinding::new(name, *target, declaration.defaults)?;
            bindings.push(if let Some(description) = &declaration.description {
                binding.with_description(description)
            } else {
                binding
            });
        }
        result.insert(*caller_id, bindings);
    }
    let targets: BTreeSet<_> = definition
        .calls
        .values()
        .flat_map(|aliases| aliases.values().map(|call| &call.target))
        .collect();
    ensure!(
        !definition
            .edges
            .iter()
            .any(|edge| targets.contains(&edge.from_node))
            && !definition
                .control_edges
                .iter()
                .any(|edge| targets.contains(&edge.from_node))
            && !definition
                .order_edges
                .iter()
                .any(|edge| targets.contains(&edge.from_node))
            && !definition
                .outputs
                .iter()
                .any(|output| targets.contains(&output.node)),
        DefinitionSnafu {
            message: "on-demand targets cannot publish through ordinary connections or workflow outputs",
        }
    );
    Ok(result)
}

fn ports<'a>(
    node: &FlowNode,
    direction: &'static str,
    specs: &'a [PortSpec],
) -> Result<BTreeMap<&'a str, &'a PortSpec>, CallBindingError> {
    let mut result = BTreeMap::new();
    for port in specs {
        ensure!(
            !port.name.is_empty() && result.insert(port.name.as_ref(), port).is_none(),
            InvalidPortSnafu {
                node: node.definition_id.clone(),
                direction,
                port: port.name.as_ref()
            }
        );
        port.value_type.check_depth().context(PortDepthSnafu {
            node: node.definition_id.clone(),
            direction,
            port: port.name.as_ref(),
        })?;
    }
    Ok(result)
}

fn ancestors(start: NodeId, incoming: &[Vec<NodeId>]) -> BTreeSet<NodeId> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(node) = pending.pop() {
        if seen.insert(node) {
            pending.extend(&incoming[node.index()]);
        }
    }
    seen
}

pub fn resolve(
    nodes: &[FlowNode],
    order: &[NodeId],
    dependencies: &[Vec<FlowDependency>],
    caller: NodeId,
    bindings: &[CallBinding],
) -> Result<Vec<ResolvedCallBinding>, CallBindingError> {
    let parent = nodes.get(caller.index()).context(UnknownNodeSnafu {
        role: "caller",
        index: caller.index(),
    })?;
    ensure!(
        parent
            .node
            .as_ref()
            .and_then(NodeExecution::as_task_node)
            .is_some(),
        UnsupportedNodeSnafu {
            node: parent.definition_id.clone(),
            role: "caller",
            reason: "a task executor is required",
        }
    );
    let outputs = ports(parent, "output", &parent.metadata.ports.outputs)?;
    let indices: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.definition_id.as_str(), NodeId::new(index)))
        .collect();
    let mut incoming = vec![Vec::new(); nodes.len()];
    let mut by_node = vec![&[][..]; nodes.len()];
    for (node, deps) in order.iter().zip(dependencies) {
        by_node[node.index()] = deps.as_slice();
        incoming[node.index()] = deps
            .iter()
            .map(|dep| indices[dep.borrowed().source_node()])
            .collect();
    }
    // Reuse ordinary exclusive-branch proofs rather than trusting an `exclusive` flag.
    crate::construction::validate_exclusive_branches(order.iter().map(|id| {
        (
            &nodes[id.index()],
            by_node[id.index()].iter().map(FlowDependency::borrowed),
        )
    }))?;
    let targets: BTreeSet<_> = bindings.iter().map(CallBinding::target).collect();
    let mut names = BTreeSet::new();
    let mut resolved = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let name = binding.name();
        ensure!(names.insert(name), DuplicateNameSnafu { name });
        let target_id = binding.target();
        let target = nodes.get(target_id.index()).context(UnknownNodeSnafu {
            role: "target",
            index: target_id.index(),
        })?;
        ensure!(
            target
                .node
                .as_ref()
                .and_then(NodeExecution::as_task_node)
                .is_some_and(|task| task.supports_success_dependencies()),
            UnsupportedNodeSnafu {
                node: target.definition_id.clone(),
                role: "target",
                reason: "a terminal task executor is required"
            }
        );
        ensure!(
            target.metadata.context_references.is_empty()
                && target.metadata.stdin.is_none()
                && target
                    .metadata
                    .input_bindings
                    .values()
                    .all(|input| input.env.is_none() && input.default.is_none()),
            UnsupportedNodeSnafu {
                node: target.definition_id.clone(),
                role: "target",
                reason: "external context, startup defaults, and stdin require invocation scope support",
            }
        );
        ensure!(
            !ancestors(caller, &incoming).contains(&target_id),
            DependencyCycleSnafu {
                name,
                producer: parent.definition_id.clone(),
                blocker: target.definition_id.clone(),
            }
        );
        let inputs = ports(target, "input", &target.metadata.ports.inputs)?;
        let mut mapped = BTreeMap::new();
        for dependency in by_node[target_id.index()] {
            let dependency = dependency.borrowed();
            if dependency.source_node() != parent.definition_id.as_str() {
                continue;
            }
            let Some(input_name) = dependency.input() else {
                continue;
            };
            ensure!(
                !dependency.exclusive(),
                DefinitionSnafu {
                    message: "caller argument inputs require one non-exclusive port connection"
                }
            );
            let output_name = dependency
                .source_output()
                .expect("data connection has an output");
            let source = outputs.get(output_name).context(UnknownPortSnafu {
                node: parent.definition_id.clone(),
                direction: "output",
                port: output_name,
            })?;
            let input = inputs.get(input_name).context(UnknownPortSnafu {
                node: target.definition_id.clone(),
                direction: "input",
                port: input_name,
            })?;
            crate::compiler::validate_connection_types(
                &parent.definition_id,
                output_name,
                &source.value_type,
                &target.definition_id,
                input_name,
                &input.value_type,
            )
            .context(ConnectionTypeSnafu { name })?;
            if let Some(schema) = target.metadata.input_schemas.get(input_name) {
                crate::compiler::validate_connection_types(
                    &parent.definition_id,
                    output_name,
                    &source.value_type,
                    &target.definition_id,
                    input_name,
                    &schema.value_type(),
                )
                .context(ConnectionTypeSnafu { name })?;
            }
            mapped.insert(input_name, *source);
        }
        let mut external = BTreeSet::new();
        let mut prerequisites = BTreeSet::new();
        let mut success_dependencies = BTreeSet::new();
        let mut output_dependencies = Vec::new();
        for dependency in by_node[target_id.index()] {
            let dep = dependency.borrowed();
            let source_id = indices[dep.source_node()];
            if source_id == caller && dep.input().is_some_and(|input| mapped.contains_key(input)) {
                continue;
            }
            let source = &nodes[source_id.index()];
            ensure!(
                source
                    .node
                    .as_ref()
                    .and_then(NodeExecution::as_task_node)
                    .is_some(),
                UnsupportedNodeSnafu {
                    node: source.definition_id.clone(),
                    role: "producer",
                    reason: "external inputs require terminal task values",
                }
            );
            let ancestors = ancestors(source_id, &incoming);
            for blocker in std::iter::once(caller).chain(targets.iter().copied()) {
                ensure!(
                    !ancestors.contains(&blocker),
                    DependencyCycleSnafu {
                        name,
                        producer: source.definition_id.clone(),
                        blocker: nodes[blocker.index()].definition_id.clone(),
                    }
                );
            }
            prerequisites.insert(source_id);
            if let Some(output) = dep.source_output() {
                let source_ports = ports(source, "output", &source.metadata.ports.outputs)?;
                let port = source_ports.get(output).context(UnknownPortSnafu {
                    node: source.definition_id.clone(),
                    direction: "terminal output",
                    port: output,
                })?;
                if let Some(input) = dep.input() {
                    let input_port = inputs[input];
                    crate::compiler::validate_connection_types(
                        &source.definition_id,
                        output,
                        &port.value_type,
                        &target.definition_id,
                        input,
                        &input_port.value_type,
                    )
                    .context(ConnectionTypeSnafu { name })?;
                    external.insert(input);
                }
                output_dependencies
                    .push(dependency.output_dependency().expect("output dependency"));
            } else {
                success_dependencies.insert(source_id);
            }
        }
        let mut resolved_inputs = Vec::new();
        for (input, port) in inputs {
            let source = if let Some(output) = mapped.get(input) {
                CallInputSource::Output((*output).clone())
            } else if external.contains(input) {
                CallInputSource::External
            } else if binding.defaults() == CallInputDefaults::RemainingInputs {
                CallInputSource::Default
            } else {
                ensure!(!port.required, MissingInputSnafu { name, input });
                CallInputSource::Omitted
            };
            resolved_inputs.push(ResolvedCallInput {
                schema: target.metadata.input_schemas.get(input).cloned(),
                port: port.clone(),
                source,
                secret: target
                    .metadata
                    .input_bindings
                    .get(input)
                    .is_some_and(|binding| binding.secret),
            });
        }
        resolved.push(ResolvedCallBinding {
            description: binding.description().map(str::to_owned),
            id: CallBindingId::new(index),
            name: name.to_owned(),
            caller,
            target: target_id,
            inputs: resolved_inputs,
            dependencies: output_dependencies,
            success_dependencies: success_dependencies.into_iter().collect(),
            prerequisites: prerequisites.into_iter().collect(),
        });
    }
    Ok(resolved)
}

/// Readiness edges order external producers without changing caller skip semantics.
pub fn readiness_pairs(
    definition: &crate::WorkflowDefinition,
) -> BTreeSet<(DefinitionId, DefinitionId)> {
    let ordinary: Vec<_> = definition
        .edges
        .iter()
        .map(|edge| (&edge.from_node, &edge.to_node, true))
        .chain(
            definition
                .control_edges
                .iter()
                .map(|edge| (&edge.from_node, &edge.to_node, false)),
        )
        .chain(
            definition
                .order_edges
                .iter()
                .map(|edge| (&edge.from_node, &edge.to_node, false)),
        )
        .collect();
    let mut pairs = BTreeSet::new();
    for (caller, aliases) in &definition.calls {
        for call in aliases.values() {
            pairs.extend(
                ordinary
                    .iter()
                    .filter(|(from, to, argument)| {
                        **to == call.target && (!argument || *from != caller)
                    })
                    .map(|(from, _, _)| ((*from).clone(), caller.clone())),
            );
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    #[test]
    fn container_callers_must_declare_calls_inside_their_bodies() {
        for kind in [
            mf_runtime::CALL_KIND,
            mf_runtime::LOOP_KIND,
            mf_runtime::ITERATION_KIND,
        ] {
            let definition = serde_json::from_value(serde_json::json!({
                "version":"2026-10-28","dependencies":{},
                "nodes":[{"id":"parent","kind":kind},{"id":"target","kind":"builtin.identity"}],
                "calls":{"parent":{"run":{"target":"target","defaults":"remaining_inputs"}}}
            }))
            .unwrap();
            assert!(
                super::declarations(&definition)
                    .unwrap_err()
                    .to_string()
                    .contains("inside its body")
            );
        }
    }
}
