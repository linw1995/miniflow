use crate::NodeId;
use std::borrow::Cow;

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
