use crate::execution_domains::ExecutionDomains;
use crate::stream_plan::InvalidPlanSnafu;
use crate::{
    FlowNode, NodeExecution, NodeId, StreamBuildError, StreamDependency, WorkflowOutputDefinition,
    output_id,
};
use snafu::{OptionExt, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// A message source and the nodes that consume its messages, retained in topological order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDomain {
    pub source: Option<usize>,
    pub steps: Vec<usize>,
}

/// Validated message boundaries and output ownership for one streaming graph.
#[derive(Debug)]
pub struct MessageDomains {
    domains: Vec<StreamDomain>,
    output_domains: Vec<usize>,
    selected_domain: Option<usize>,
    execution_domains: ExecutionDomains,
    execution_domains_by_message: Vec<Vec<usize>>,
    message_domain_by_execution: Vec<usize>,
}

impl MessageDomains {
    /// Partitions topologically ordered nodes after input-source validation.
    pub fn new(
        nodes: &[FlowNode],
        dependencies: &[Vec<StreamDependency>],
        outputs: &[WorkflowOutputDefinition],
    ) -> Result<Self, StreamBuildError> {
        let mut indices = BTreeMap::new();
        let mut output_index = BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            ensure!(
                indices.insert(node.definition_id.as_str(), index).is_none(),
                InvalidPlanSnafu {
                    message: format!("duplicate node `{}`", node.definition_id),
                }
            );
            for port in &node.metadata.ports.outputs {
                let name = output_id(node.definition_id.as_str(), &port.name);
                ensure!(
                    output_index.insert(name.clone(), index).is_none(),
                    InvalidPlanSnafu {
                        message: format!("ambiguous output `{name}`"),
                    }
                );
            }
        }
        let mut domains = vec![StreamDomain {
            source: None,
            steps: Vec::new(),
        }];
        let mut output_domains = vec![0; nodes.len()];
        let mut node_message_domains = vec![0; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let mut incoming_domains = BTreeSet::new();
            for dependency in &dependencies[index] {
                let source = indices
                    .get(dependency.source_node.as_str())
                    .copied()
                    .with_context(|| InvalidPlanSnafu {
                        message: format!(
                            "node `{}` has unknown dependency `{}`",
                            node.definition_id, dependency.source_node
                        ),
                    })?;
                ensure!(
                    source < index,
                    InvalidPlanSnafu {
                        message: format!(
                            "dependency `{}` must precede `{}`",
                            dependency.source_node, node.definition_id
                        ),
                    }
                );
                ensure!(
                    nodes[source]
                        .metadata
                        .ports
                        .outputs
                        .iter()
                        .any(|port| port.name == dependency.source_output),
                    InvalidPlanSnafu {
                        message: format!(
                            "node `{}` has no output `{}`",
                            dependency.source_node, dependency.source_output
                        ),
                    }
                );
                if let Some(input) = &dependency.input {
                    ensure!(
                        node.metadata
                            .ports
                            .inputs
                            .iter()
                            .any(|port| port.name == *input),
                        InvalidPlanSnafu {
                            message: format!(
                                "node `{}` has no input `{input}`",
                                node.definition_id
                            ),
                        }
                    );
                }
                incoming_domains.insert(output_domains[source]);
            }
            ensure!(incoming_domains.len() <= 1, {
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
                InvalidPlanSnafu {
                    message: format!(
                        "node `{}` requires one message domain; incoming domains: {incoming_domains:?}; dependencies: {edges}",
                        node.definition_id
                    ),
                }
            });
            let domain = incoming_domains.first().copied().unwrap_or(0);
            ensure!(
                !incoming_domains.is_empty() || !matches!(node.node, Some(NodeExecution::Event(_))),
                InvalidPlanSnafu {
                    message: format!(
                        "initial event node `{}` requires an explicit activation source",
                        node.definition_id
                    ),
                }
            );
            domains[domain].steps.push(index);
            node_message_domains[index] = domain;
            output_domains[index] = match node.node.as_ref().with_context(|| InvalidPlanSnafu {
                message: format!(
                    "node `{}` has no execution implementation",
                    node.definition_id
                ),
            })? {
                NodeExecution::Task(_) => domain,
                NodeExecution::Event(_) | NodeExecution::Stream(_) => {
                    let new_domain = domains.len();
                    domains.push(StreamDomain {
                        source: Some(index),
                        steps: Vec::new(),
                    });
                    new_domain
                }
            };
            for reference in &node.metadata.context_references {
                let producer = output_index
                    .get(&reference.output)
                    .copied()
                    .with_context(|| InvalidPlanSnafu {
                        message: format!(
                            "node `{}` references unknown output `{}`",
                            node.definition_id, reference.output
                        ),
                    })?;
                ensure!(
                    output_domains[producer] == domain && producer < index,
                    InvalidPlanSnafu {
                        message: format!(
                            "node `{}` context reference `{}` crosses a message boundary",
                            node.definition_id, reference.output
                        ),
                    }
                );
            }
        }
        let mut selected_domain = None;
        for output in outputs {
            let index = indices
                .get(output.node.as_str())
                .copied()
                .with_context(|| InvalidPlanSnafu {
                    message: format!("unknown selected output node `{}`", output.node),
                })?;
            ensure!(
                nodes[index]
                    .metadata
                    .ports
                    .outputs
                    .iter()
                    .any(|port| port.name == output.port),
                InvalidPlanSnafu {
                    message: format!(
                        "selected output `{}` references unknown port `{}.{}`",
                        output.name, output.node, output.port
                    ),
                }
            );
            let domain = output_domains[index];
            ensure!(
                selected_domain.is_none_or(|selected| selected == domain),
                InvalidPlanSnafu {
                    message: format!(
                        "selected output `{}` belongs to a different message domain",
                        output.name
                    ),
                }
            );
            selected_domain = Some(domain);
        }

        let mut execution_edges = Vec::new();
        for (target, node_dependencies) in dependencies.iter().enumerate() {
            for dependency in node_dependencies {
                let source = indices[dependency.source_node.as_str()];
                if node_message_domains[source] == node_message_domains[target] {
                    execution_edges.push((source, target));
                }
            }
        }
        let order: Vec<_> = (0..nodes.len()).map(NodeId::new).collect();
        let boundaries: Vec<_> = nodes
            .iter()
            .map(|node| {
                matches!(
                    node.node,
                    Some(NodeExecution::Event(_) | NodeExecution::Stream(_))
                )
            })
            .collect();
        let execution_domains = ExecutionDomains::partition(&order, &execution_edges, &boundaries);
        let mut execution_domains_by_message = vec![Vec::new(); domains.len()];
        let mut message_domain_by_execution = vec![0; execution_domains.len()];
        for execution_domain in execution_domains.domains() {
            let node = execution_domain.nodes[0].index();
            let message_domain = node_message_domains[node];
            execution_domains_by_message[message_domain].push(execution_domain.id);
            message_domain_by_execution[execution_domain.id] = message_domain;
        }
        Ok(Self {
            domains,
            output_domains,
            selected_domain,
            execution_domains,
            execution_domains_by_message,
            message_domain_by_execution,
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

    pub fn execution_domains(&self) -> &ExecutionDomains {
        &self.execution_domains
    }

    pub fn execution_domains_for_message(&self, message_domain: usize) -> &[usize] {
        &self.execution_domains_by_message[message_domain]
    }

    pub fn message_domain_for_execution(&self, execution_domain: usize) -> usize {
        self.message_domain_by_execution[execution_domain]
    }
}
