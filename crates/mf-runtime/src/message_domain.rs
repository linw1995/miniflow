use crate::execution_domains::ExecutionDomains;
use std::borrow::Cow;

/// Validated message boundaries and output ownership for one streaming graph.
#[derive(Clone, Debug)]
pub struct MessageDomains {
    sources: Cow<'static, [Option<usize>]>,
    output_domains: Cow<'static, [usize]>,
    selected_domain: Option<usize>,
    execution_domains: ExecutionDomains,
    execution_domains_by_message: Cow<'static, [Cow<'static, [usize]>]>,
    message_domain_by_execution: Cow<'static, [usize]>,
}

impl MessageDomains {
    /// Accepts compiler-validated message and execution ownership without planning a graph.
    pub const fn from_parts(
        sources: Cow<'static, [Option<usize>]>,
        output_domains: Cow<'static, [usize]>,
        selected_domain: Option<usize>,
        execution_domains: ExecutionDomains,
        execution_domains_by_message: Cow<'static, [Cow<'static, [usize]>]>,
        message_domain_by_execution: Cow<'static, [usize]>,
    ) -> Self {
        Self {
            sources,
            output_domains,
            selected_domain,
            execution_domains,
            execution_domains_by_message,
            message_domain_by_execution,
        }
    }

    /// Message sources by ownership index; index zero is the startup frame.
    pub fn sources(&self) -> &[Option<usize>] {
        &self.sources
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
