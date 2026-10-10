use mf_runtime::{Flow, RustValueType, output_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedFallbackReason {
    UnsupportedMode,
    CustomRunner,
    ProviderMissing,
    ContextReads,
    UnprovenRefinement,
    ObservedOutput,
    OptionalBinding,
    DifferentRustType,
    DomainBoundary,
    MultiplePredecessors,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedConnectionReport {
    pub source: String,
    pub output: String,
    pub target: String,
    pub input: String,
    pub fallback: Option<TypedFallbackReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedSegment {
    pub domain: usize,
    pub positions: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedPlan {
    pub segments: Vec<TypedSegment>,
    pub connections: Vec<TypedConnectionReport>,
}

/// Computes ownership and codec evidence without changing the validated domain graph.
pub fn plan_typed_segments(flow: &Flow, standard_runner: bool, oneshot: bool) -> TypedPlan {
    let nodes = flow.nodes();
    let mut domain_of = vec![usize::MAX; nodes.len()];
    for domain in flow.execution_domains().domains() {
        for node in domain.nodes.iter() {
            domain_of[node.index()] = domain.id;
        }
    }
    let mut uses = BTreeMap::<(usize, String), usize>::new();
    let bindings: BTreeMap<_, _> = flow
        .connections()
        .iter()
        .map(|edge| ((edge.to_node.index(), edge.to_input.as_ref()), edge))
        .collect();
    let mut moves = BTreeMap::<(usize, usize, &str), usize>::new();
    for edge in flow.connections() {
        *moves
            .entry((
                edge.from_node.index(),
                edge.to_node.index(),
                edge.from_output.as_ref(),
            ))
            .or_default() += 1;
    }
    for edge in flow.connections() {
        *uses
            .entry((edge.from_node.index(), edge.from_output.to_string()))
            .or_default() += 1;
    }
    for dependencies in flow.plan().dependencies.iter() {
        for dependency in dependencies.iter().filter(|dep| dep.input.is_none()) {
            if let Some(index) = nodes
                .iter()
                .position(|node| node.definition_id.as_str() == dependency.source_node.as_ref())
            {
                *uses
                    .entry((index, dependency.source_output.to_string()))
                    .or_default() += 1;
            }
        }
    }
    for output in flow.plan().outputs.iter() {
        *uses
            .entry((output.node_id.index(), output.port.to_string()))
            .or_default() += 1;
    }
    let producers: BTreeMap<_, _> = nodes
        .iter()
        .enumerate()
        .flat_map(|(index, producer)| {
            producer.metadata.ports.outputs.iter().map(move |port| {
                (
                    output_id(producer.definition_id.as_str(), &port.name),
                    (index, port.name.to_string()),
                )
            })
        })
        .collect();
    for reader in nodes {
        for reference in &reader.metadata.context_references {
            if let Some(producer) = producers.get(&reference.output) {
                *uses.entry(producer.clone()).or_default() += 1;
            }
        }
    }
    let classify = |source: usize, target: usize| -> Option<TypedFallbackReason> {
        use TypedFallbackReason::*;
        if !oneshot {
            return Some(UnsupportedMode);
        }
        if !standard_runner {
            return Some(CustomRunner);
        }
        if domain_of[source] != domain_of[target] {
            return Some(DomainBoundary);
        }
        let (Some(from), Some(to)) = (
            &nodes[source].metadata.typed_generation,
            &nodes[target].metadata.typed_generation,
        ) else {
            return Some(ProviderMissing);
        };
        if !from.context_free
            || !to.context_free
            || !nodes[source].metadata.context_references.is_empty()
            || !nodes[target].metadata.context_references.is_empty()
        {
            return Some(ContextReads);
        }
        for (index, generation) in [(source, from), (target, to)] {
            if !generation
                .outputs
                .iter()
                .map(|field| &field.port)
                .eq(nodes[index].metadata.ports.outputs.iter())
            {
                return Some(UnprovenRefinement);
            }
        }
        for field in &from.outputs {
            let count = uses
                .get(&(source, field.port.name.to_string()))
                .copied()
                .unwrap_or(0);
            let moved = moves
                .get(&(source, target, field.port.name.as_ref()))
                .copied()
                .unwrap_or(0);
            if count > 1 || count != moved {
                return Some(ObservedOutput);
            }
        }
        for field in &to.inputs {
            let edge = bindings.get(&(target, field.port.name.as_ref()));
            let Some(edge) = edge else {
                if field.port.required {
                    return Some(MultiplePredecessors);
                }
                if matches!(field.rust_type, RustValueType::Defaulted(_)) {
                    return Some(OptionalBinding);
                }
                continue;
            };
            if edge.from_node.index() != source {
                return Some(MultiplePredecessors);
            }
            let Some(producer) = from
                .outputs
                .iter()
                .find(|field| field.port.name == edge.from_output)
            else {
                return Some(ProviderMissing);
            };
            if !producer.port.required
                || !field.port.required
                || matches!(field.rust_type, RustValueType::Optional(_))
            {
                return Some(OptionalBinding);
            }
            if producer.rust_type != field.rust_type {
                return Some(DifferentRustType);
            }
            if !producer
                .port
                .value_type
                .is_assignable_to(&field.port.value_type)
            {
                return Some(UnprovenRefinement);
            }
        }
        None
    };
    let mut plan = TypedPlan::default();
    for edge in flow.connections() {
        let source = edge.from_node.index();
        let target = edge.to_node.index();
        plan.connections.push(TypedConnectionReport {
            source: nodes[source].definition_id.to_string(),
            output: edge.from_output.to_string(),
            target: nodes[target].definition_id.to_string(),
            input: edge.to_input.to_string(),
            fallback: classify(source, target),
        });
    }
    plan.connections.sort_by(|a, b| {
        (&a.source, &a.output, &a.target, &a.input)
            .cmp(&(&b.source, &b.output, &b.target, &b.input))
    });
    for domain in flow.execution_domains().domains() {
        let mut chain: Vec<usize> = Vec::new();
        for &position in domain.positions.iter() {
            if let Some(&previous) = chain.last() {
                let source = flow.execution_order()[previous].index();
                let target = flow.execution_order()[position].index();
                let connected = flow
                    .connections()
                    .iter()
                    .any(|edge| edge.from_node.index() == source && edge.to_node.index() == target);
                if !connected || classify(source, target).is_some() {
                    if chain.len() >= 2 {
                        plan.segments.push(TypedSegment {
                            domain: domain.id,
                            positions: chain.clone(),
                        });
                    }
                    chain.clear();
                }
            }
            chain.push(position);
        }
        if chain.len() >= 2 {
            plan.segments.push(TypedSegment {
                domain: domain.id,
                positions: chain,
            });
        }
    }
    plan
}
