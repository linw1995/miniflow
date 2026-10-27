use crate::{DefinitionId, FlowDependency, FlowNode, NodeExecution, NodeId, PortSpec};
use mf_runtime::{
    CallBinding, CallBindingId, CallInputDefaults, CallInputSource, ResolvedCallBinding,
    ResolvedCallInput,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Snafu)]
pub enum CallBindingError {
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
    #[snafu(display("call `{name}` input `{input}` has both caller and external port bindings"))]
    ConflictingInput { name: String, input: String },
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
    let outputs = ports(parent, "call output", &parent.metadata.call_outputs)?;
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
            matches!(target.node.as_ref(), Some(NodeExecution::Task(task)) if task.supports_success_dependencies()),
            UnsupportedNodeSnafu {
                node: target.definition_id.clone(),
                role: "target",
                reason: "an ordinary terminal Task is required"
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
        for mapping in binding.ports() {
            let source = outputs
                .get(mapping.output.as_str())
                .context(UnknownPortSnafu {
                    node: parent.definition_id.clone(),
                    direction: "call output",
                    port: &mapping.output,
                })?;
            let input = inputs
                .get(mapping.input.as_str())
                .context(UnknownPortSnafu {
                    node: target.definition_id.clone(),
                    direction: "input",
                    port: &mapping.input,
                })?;
            crate::compiler::validate_connection_types(
                &parent.definition_id,
                &source.name,
                &source.value_type,
                &target.definition_id,
                &input.name,
                &input.value_type,
            )
            .context(ConnectionTypeSnafu { name })?;
            mapped.insert(mapping.input.as_str(), *source);
        }
        let mut external = BTreeSet::new();
        let mut prerequisites = BTreeSet::new();
        let mut success_dependencies = BTreeSet::new();
        let mut output_dependencies = Vec::new();
        for dependency in by_node[target_id.index()] {
            let dep = dependency.borrowed();
            let source_id = indices[dep.source_node()];
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
                    ensure!(
                        !mapped.contains_key(input),
                        ConflictingInputSnafu { name, input }
                    );
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
