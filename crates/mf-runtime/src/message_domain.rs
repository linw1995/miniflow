use crate::stream_plan::InvalidPlanSnafu;
use crate::{
    FlowNode, NodeExecution, STREAM_INPUT_ID, StreamBuildError, StreamDependency,
    WorkflowOutputDefinition, output_id,
};
use std::collections::{BTreeMap, BTreeSet};

/// A message source and the ordered steps that consume its messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDomain {
    pub source: usize,
    pub steps: Vec<usize>,
}

/// Validated message boundaries and output ownership for one streaming graph.
#[derive(Debug)]
pub struct MessageDomains {
    domains: Vec<StreamDomain>,
    output_domains: Vec<usize>,
    selected_domain: Option<usize>,
}

impl MessageDomains {
    /// Partitions topologically ordered nodes after input-source validation.
    pub fn new(
        nodes: &[FlowNode],
        dependencies: &[Vec<StreamDependency>],
        outputs: &[WorkflowOutputDefinition],
    ) -> Result<Self, StreamBuildError> {
        let invalid = |message: String| InvalidPlanSnafu { message }.build();
        let mut indices = BTreeMap::new();
        let mut output_index = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            if indices.insert(node.definition_id.as_str(), index).is_some() {
                return Err(invalid(format!("duplicate node `{}`", node.definition_id)));
            }
            for port in &node.metadata.ports.outputs {
                let name = output_id(node.definition_id.as_str(), &port.name);
                if output_index.insert(name.clone(), index).is_some() {
                    return Err(invalid(format!("ambiguous output `{name}`")));
                }
            }
        }
        let mut domains = vec![StreamDomain {
            source: 0,
            steps: Vec::new(),
        }];
        let mut output_domains = vec![0; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            if index == 0 {
                if !dependencies[index].is_empty() {
                    return Err(invalid(
                        "stream input cannot have incoming dependencies".into(),
                    ));
                }
                continue;
            }
            let mut incoming_domains = BTreeSet::new();
            for dependency in &dependencies[index] {
                let source = indices
                    .get(dependency.source_node.as_str())
                    .copied()
                    .ok_or_else(|| {
                        invalid(format!(
                            "node `{}` has unknown dependency `{}`",
                            node.definition_id, dependency.source_node
                        ))
                    })?;
                if source >= index {
                    return Err(invalid(format!(
                        "dependency `{}` must precede `{}`",
                        dependency.source_node, node.definition_id
                    )));
                }
                incoming_domains.insert(output_domains[source]);
            }
            if incoming_domains.len() != 1 {
                let edges = dependencies[index]
                    .iter()
                    .map(|dependency| {
                        format!(
                            "{}.{} -> {}.{}",
                            dependency.source_node,
                            dependency.source_output,
                            node.definition_id,
                            dependency.input.as_deref().unwrap_or("<control>"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid(format!(
                    "node `{}` requires one message domain and an explicit path from {STREAM_INPUT_ID}; incoming domains: {incoming_domains:?}; dependencies: {edges}",
                    node.definition_id
                )));
            }
            let domain = *incoming_domains.first().unwrap();
            domains[domain].steps.push(index);
            output_domains[index] = match &node.node {
                Some(NodeExecution::Task(_)) => domain,
                Some(NodeExecution::Event(_)) => {
                    let new_domain = domains.len();
                    domains.push(StreamDomain {
                        source: index,
                        steps: Vec::new(),
                    });
                    new_domain
                }
                None => {
                    return Err(invalid(format!(
                        "node `{}` has no execution implementation",
                        node.definition_id
                    )));
                }
            };
            for reference in &node.metadata.context_references {
                let producer = output_index
                    .get(&reference.output)
                    .copied()
                    .ok_or_else(|| {
                        invalid(format!(
                            "node `{}` references unknown output `{}`",
                            node.definition_id, reference.output
                        ))
                    })?;
                if output_domains[producer] != domain || producer >= index {
                    return Err(invalid(format!(
                        "node `{}` context reference `{}` crosses a message boundary",
                        node.definition_id, reference.output
                    )));
                }
            }
        }
        let mut selected_domain = None;
        for output in outputs {
            let index = indices.get(output.node.as_str()).copied().ok_or_else(|| {
                invalid(format!("unknown selected output node `{}`", output.node))
            })?;
            let domain = output_domains[index];
            if selected_domain.is_some_and(|selected| selected != domain) {
                return Err(invalid(format!(
                    "selected output `{}` belongs to a different message domain",
                    output.name
                )));
            }
            selected_domain = Some(domain);
        }
        Ok(Self {
            domains,
            output_domains,
            selected_domain,
        })
    }

    pub fn domains(&self) -> &[StreamDomain] {
        &self.domains
    }

    pub fn output_domain(&self, node: usize) -> usize {
        self.output_domains[node]
    }

    pub fn selected_domain(&self) -> Option<usize> {
        self.selected_domain
    }
}
