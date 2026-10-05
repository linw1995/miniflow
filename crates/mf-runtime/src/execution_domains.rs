use crate::NodeId;
use std::{borrow::Cow, collections::BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
/// A maximal linear region of a workflow graph executed synchronously.
pub struct ExecutionDomain {
    /// Stable index in the execution-domain plan.
    pub id: usize,
    /// Flow node IDs in validated topological order.
    pub nodes: Cow<'static, [NodeId]>,
    /// Positions corresponding to the domain's nodes in the Flow execution order.
    pub positions: Cow<'static, [usize]>,
    /// Domain IDs whose outputs must commit before this domain can start.
    pub predecessors: Cow<'static, [usize]>,
    /// Domain IDs that become eligible after this domain completes.
    pub successors: Cow<'static, [usize]>,
    /// Position of this domain's first node in the Flow execution order.
    pub first_position: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Fork/join partition of a validated Flow graph.
pub struct ExecutionDomains {
    domains: Cow<'static, [ExecutionDomain]>,
    ancestors: Cow<'static, [Cow<'static, [usize]>]>,
}

impl ExecutionDomains {
    /// Accepts a compiler-validated domain layout without partitioning a graph.
    pub const fn from_parts(
        domains: Cow<'static, [ExecutionDomain]>,
        ancestors: Cow<'static, [Cow<'static, [usize]>]>,
    ) -> Self {
        Self { domains, ancestors }
    }

    /// Partitions a validated topological order, with edges indexed by position.
    /// Nodes marked as boundaries form their own domains.
    pub fn partition(order: &[NodeId], edges: &[(usize, usize)], boundaries: &[bool]) -> Self {
        assert_eq!(order.len(), boundaries.len());
        let mut predecessors = vec![BTreeSet::new(); order.len()];
        let mut successors = vec![BTreeSet::new(); order.len()];
        for &(source, target) in edges {
            predecessors[target].insert(source);
            successors[source].insert(target);
        }

        let mut domains: Vec<ExecutionDomain> = Vec::new();
        let mut node_domains = vec![0; order.len()];
        for position in 0..order.len() {
            let append_to = if !boundaries[position] && predecessors[position].len() == 1 {
                let predecessor = *predecessors[position].first().unwrap();
                (!boundaries[predecessor] && successors[predecessor].len() == 1)
                    .then_some(node_domains[predecessor])
            } else {
                None
            };

            let domain_id = append_to.unwrap_or_else(|| {
                let domain_id = domains.len();
                domains.push(ExecutionDomain {
                    id: domain_id,
                    nodes: Vec::new().into(),
                    positions: Vec::new().into(),
                    predecessors: Vec::new().into(),
                    successors: Vec::new().into(),
                    first_position: position,
                });
                domain_id
            });
            node_domains[position] = domain_id;
            domains[domain_id].nodes.to_mut().push(order[position]);
            domains[domain_id].positions.to_mut().push(position);
        }

        let mut domain_predecessors = vec![BTreeSet::new(); domains.len()];
        let mut domain_successors = vec![BTreeSet::new(); domains.len()];
        for (target, node_predecessors) in predecessors.iter().enumerate() {
            let target_domain = node_domains[target];
            for &source in node_predecessors {
                let source_domain = node_domains[source];
                if source_domain != target_domain {
                    domain_predecessors[target_domain].insert(source_domain);
                    domain_successors[source_domain].insert(target_domain);
                }
            }
        }
        for (index, domain) in domains.iter_mut().enumerate() {
            domain.predecessors = domain_predecessors[index]
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .into();
            domain.successors = domain_successors[index]
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .into();
        }
        let mut ancestors = vec![BTreeSet::new(); domains.len()];
        for id in 0..domains.len() {
            for &predecessor in domains[id].predecessors.iter() {
                ancestors[id].insert(predecessor);
                let inherited = ancestors[predecessor].iter().copied().collect::<Vec<_>>();
                ancestors[id].extend(inherited);
            }
        }

        Self {
            domains: domains.into(),
            ancestors: ancestors
                .into_iter()
                .map(|values| values.into_iter().collect::<Vec<_>>().into())
                .collect::<Vec<_>>()
                .into(),
        }
    }

    pub fn domains(&self) -> &[ExecutionDomain] {
        &self.domains
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    pub fn ancestor_domains(&self, id: usize) -> &[usize] {
        &self.ancestors[id]
    }
}
