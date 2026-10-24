use mf_runtime::ExecutionDomains;
use std::borrow::Cow;

/// Validated message boundaries and output ownership for one streaming graph.
#[derive(Clone, Debug)]
pub struct MessageDomains {
    sources: Cow<'static, [Option<usize>]>,
    output_domains: Cow<'static, [usize]>,
    stream_output_domains: Cow<'static, [Option<usize>]>,
    selected_domains: Cow<'static, [usize]>,
    execution_domains: ExecutionDomains,
    execution_domains_by_message: Cow<'static, [Cow<'static, [usize]>]>,
}

impl MessageDomains {
    pub const fn from_parts_with_streams(
        sources: Cow<'static, [Option<usize>]>,
        output_domains: Cow<'static, [usize]>,
        stream_output_domains: Cow<'static, [Option<usize>]>,
        selected_domains: Cow<'static, [usize]>,
        execution_domains: ExecutionDomains,
        execution_domains_by_message: Cow<'static, [Cow<'static, [usize]>]>,
    ) -> Self {
        Self {
            sources,
            output_domains,
            stream_output_domains,
            selected_domains,
            execution_domains,
            execution_domains_by_message,
        }
    }

    pub fn lower(
        &self,
        nodes: &[mf_runtime::FlowNode],
        dependencies: &[Cow<'static, [mf_runtime::FlowDependency]>],
        outputs: &[mf_runtime::WorkflowOutputDefinition],
    ) -> mf_runtime::StreamProgram {
        use std::sync::Arc;
        let messages = build_message_schedules(self)
            .into_iter()
            .enumerate()
            .map(|(message, schedule)| {
                let selected: Vec<_> = outputs
                    .iter()
                    .filter(|output| {
                        let index = nodes
                            .iter()
                            .position(|node| node.definition_id == output.node)
                            .expect("validated output source");
                        let incremental = nodes[index]
                            .metadata
                            .stream_outputs
                            .iter()
                            .any(|port| port.name == output.port);
                        let owner = if incremental {
                            self.stream_output_domain(index)
                                .expect("selected stream has a channel")
                        } else {
                            self.output_domain(index)
                        };
                        owner == message
                    })
                    .map(|output| {
                        (
                            output.name.to_string(),
                            output.node.to_string(),
                            mf_runtime::output_id(output.node.as_str(), &output.port),
                            output.optional,
                        )
                    })
                    .collect();
                let select_outputs = (!selected.is_empty()).then(|| {
                    Arc::new(move |context: &mut mf_runtime::ExecutionContext| {
                        let mut values = mf_runtime::FlowOutputs::new();
                        for (name, node, key, optional) in &selected {
                            if let Some(value) =
                                context.select_output_keyed(name, node, key, *optional)?
                            {
                                values.insert(name.clone(), value);
                            }
                        }
                        Ok(values)
                    }) as Arc<mf_runtime::StreamOutputSelector>
                });
                mf_runtime::StreamMessageProgram {
                    source: self.sources()[message],
                    schedule,
                    select_outputs,
                }
            })
            .collect();
        let emissions = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let terminal = matches!(node.node, Some(mf_runtime::NodeExecution::TaskStream(_)));
                let message = if terminal {
                    self.stream_output_domain(index)
                } else {
                    self.sources()
                        .iter()
                        .position(|source| *source == Some(index))
                }?;
                Some(mf_runtime::StreamEmissionBinding {
                    message,
                    connected: self.selected_domains().contains(&message)
                        || !self.execution_domains_for_message(message).is_empty(),
                    terminal,
                    initialize: Arc::new(move |plan, result| {
                        mf_runtime::ExecutionContext::for_message(plan.emission_node(index), result)
                    }),
                })
            })
            .collect();
        let inputs = dependencies
            .iter()
            .enumerate()
            .map(|(index, dependencies)| {
                let dependencies = dependencies.clone();
                Arc::new(
                    move |plan: &mf_runtime::StreamPlan,
                          context: &mf_runtime::ExecutionContext,
                          inputs: &mut mf_runtime::Inputs,
                          causes| {
                        context.resolve_node_inputs(
                            &plan.nodes()[index],
                            dependencies
                                .iter()
                                .map(mf_runtime::FlowDependency::borrowed),
                            &[],
                            inputs,
                            causes,
                        )
                    },
                ) as Arc<mf_runtime::StreamInputResolver>
            })
            .collect();
        mf_runtime::StreamProgram {
            execution_domains: self.execution_domains().clone(),
            messages,
            emissions,
            inputs,
        }
    }

    /// Message sources by ownership index; index zero is the startup frame.
    pub fn sources(&self) -> &[Option<usize>] {
        &self.sources
    }

    pub fn output_domain(&self, node: usize) -> usize {
        self.output_domains[node]
    }

    pub fn stream_output_domain(&self, node: usize) -> Option<usize> {
        self.stream_output_domains.get(node).copied().flatten()
    }

    pub fn selected_domains(&self) -> &[usize] {
        &self.selected_domains
    }

    pub fn execution_domains(&self) -> &ExecutionDomains {
        &self.execution_domains
    }

    pub fn execution_domains_for_message(&self, message_domain: usize) -> &[usize] {
        &self.execution_domains_by_message[message_domain]
    }
}

fn build_message_schedules(domains: &MessageDomains) -> Vec<mf_runtime::MessageSchedule> {
    let execution_domains = domains.execution_domains();
    let count = execution_domains.len();
    (0..domains.sources().len())
        .map(|message| {
            let message_domains = domains.execution_domains_for_message(message);
            let mut local = vec![usize::MAX; count];
            for (rank, &domain) in message_domains.iter().enumerate() {
                local[domain] = rank;
            }
            let initial_deps = message_domains
                .iter()
                .map(|&domain| {
                    execution_domains.domains()[domain]
                        .predecessors
                        .iter()
                        .filter(|&&predecessor| local[predecessor] != usize::MAX)
                        .count()
                })
                .collect();
            let successors = message_domains
                .iter()
                .map(|&domain| {
                    execution_domains.domains()[domain]
                        .successors
                        .iter()
                        .filter_map(|&successor| {
                            (local[successor] != usize::MAX).then_some(local[successor])
                        })
                        .collect()
                })
                .collect();
            mf_runtime::MessageSchedule {
                domains: message_domains.to_vec().into_boxed_slice(),
                local: local.into_boxed_slice(),
                initial_deps,
                successors,
            }
        })
        .collect()
}
