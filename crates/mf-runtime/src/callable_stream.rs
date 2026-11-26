//! A callable stream graph shares its frozen wiring and stateless Tasks, never a running operator.

use crate::{
    DefinitionId, EmissionSink, ExecutionContext, FlowOutputs, NodeBuildError, NodeContract,
    NodeExecution, NodeRegistration, PreparedNode, PreparedStream, PreparedSubgraph, StreamPlan,
    TaskStreamExecutor, WorkflowRunError,
};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, ensure};
use std::{collections::BTreeMap, sync::Arc};

struct Recipe {
    registration: NodeRegistration,
    config: Value,
    children: BTreeMap<String, PreparedSubgraph>,
    declared: NodeContract,
    kind: crate::generated::ExecutorKind,
}

impl Recipe {
    fn create(&self, id: &DefinitionId) -> Result<NodeExecution, NodeBuildError> {
        let node = instantiate(
            &self.registration,
            id,
            self.config.clone(),
            self.children.clone(),
        )?;
        ensure!(
            crate::generated::ExecutorKind::of(&node.execution) == self.kind,
            crate::NodeInvalidSubgraphSnafu {
                message: format!("stream operator `{id}` changed execution kind")
            }
        );
        crate::verify_generated_metadata(&node.metadata, &self.declared)?;
        Ok(node.execution)
    }
}

fn instantiate(
    registration: &NodeRegistration,
    id: &DefinitionId,
    config: Value,
    children: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    if children.is_empty() {
        registration.instantiate(config)
    } else {
        registration.instantiate_subgraphs(id.as_str(), config, children)
    }
}

/// Retains only the constructors for stateful providers in one compiler-prepared scope.
#[derive(Default)]
pub struct StreamOperatorFactories(BTreeMap<DefinitionId, Recipe>);

impl StreamOperatorFactories {
    pub fn prepare(
        &mut self,
        registration: &NodeRegistration,
        id: &DefinitionId,
        config: &Value,
        children: BTreeMap<String, PreparedSubgraph>,
    ) -> Result<PreparedNode, NodeBuildError> {
        let node = instantiate(registration, id, config.clone(), children.clone())?;
        if matches!(
            node.execution,
            NodeExecution::Event(_) | NodeExecution::Stream(_)
        ) {
            self.0.insert(
                id.clone(),
                Recipe {
                    registration: *registration,
                    config: config.clone(),
                    children,
                    declared: (&node.metadata).into(),
                    kind: crate::generated::ExecutorKind::of(&node.execution),
                },
            );
        }
        Ok(node)
    }
}

enum Operator {
    Combined(Box<TaskStreamExecutor>),
    Stateful(Box<Recipe>),
}

pub struct CallableStream {
    pub plan: Arc<StreamPlan>,
    operators: Vec<Option<Operator>>,
    stream_ports: Vec<String>,
}

impl CallableStream {
    pub fn new(
        prepared: PreparedStream,
        mut factories: StreamOperatorFactories,
        stream_ports: &[crate::PortSpec],
    ) -> Result<Self, NodeBuildError> {
        let (plan, states) = prepared.into_parts();
        let operators = states
            .into_iter()
            .zip(plan.nodes())
            .map(|(state, node)| {
                Ok(match state {
                    None => None,
                    Some(NodeExecution::TaskStream(task)) => Some(Operator::Combined(task)),
                    Some(NodeExecution::Event(_) | NodeExecution::Stream(_)) => {
                        Some(Operator::Stateful(Box::new(
                            factories.0.remove(&node.definition_id).with_context(|| {
                                crate::NodeInvalidSubgraphSnafu {
                                    message: format!(
                                        "stream operator `{}` has no reusable constructor",
                                        node.definition_id
                                    ),
                                }
                            })?,
                        )))
                    }
                    Some(NodeExecution::Task(_)) => {
                        unreachable!("ordinary tasks belong to the shared plan")
                    }
                })
            })
            .collect::<Result<_, NodeBuildError>>()?;
        Ok(Self {
            plan,
            operators,
            stream_ports: stream_ports.iter().map(|p| p.name.to_string()).collect(),
        })
    }

    pub fn execute(
        &self,
        state: &mut ExecutionContext,
        sink: &mut dyn EmissionSink,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        use crate::flow_invocation::workflow_result;
        let states = self
            .operators
            .iter()
            .zip(self.plan.nodes())
            .map(|(operator, node)| {
                operator
                    .as_ref()
                    .map(|op| match op {
                        Operator::Combined(task) => Ok(NodeExecution::TaskStream(task.clone())),
                        Operator::Stateful(recipe) => recipe
                            .create(&node.definition_id)
                            .boxed()
                            .context(crate::NodePluginFailedSnafu)
                            .context(crate::NodeExecutionSnafu {
                                definition_id: node.definition_id.clone(),
                            }),
                    })
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        let context = state.fork_domain_with_observation(state.observation().cloned());
        let instance = workflow_result(
            PreparedStream::from_parts(self.plan.clone(), states).start_in_context(context),
        )?;
        let mut terminal = FlowOutputs::new();
        while let Some(delivery) = workflow_result(instance.receive())? {
            let output = delivery.output();
            if output.message.domain == 0 {
                terminal = output.outputs.clone();
            } else {
                let skipped = self
                    .stream_ports
                    .iter()
                    .filter(|name| !output.outputs.contains_key(*name))
                    .cloned()
                    .collect();
                let packet = crate::NodeResult {
                    outputs: output.outputs.clone(),
                    skipped,
                    ..Default::default()
                };
                if let Err(error) = sink.send(packet) {
                    delivery.fail(error.clone());
                    state.cancellation().cancel(error.clone());
                    return workflow_result(Err(error));
                }
            }
            workflow_result(delivery.acknowledge())?;
        }
        // Domain zero can finish before downstream operators flush. Never publish it early.
        workflow_result(instance.join())?;
        Ok(terminal)
    }
}
