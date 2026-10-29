use crate::{
    Flow, FlowNode, NodeMetadata, PreparedStream, TaskNode, WorkflowInput, WorkflowInputSchema,
    output_id,
};
use mf_telemetry::{description::WorkflowDescription, identity::WorkflowId};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeTargetKind {
    Task,
    Event,
    Stream,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTarget {
    pub id: String,
    pub index: usize,
    pub kind: NodeTargetKind,
    #[serde(default)]
    pub worker_limit: Option<std::num::NonZeroUsize>,
    pub inputs: WorkflowInputSchema,
    pub outputs: BTreeMap<String, WorkflowInput>,
    pub context: BTreeMap<String, crate::ValueType>,
    pub original_workflow_id: WorkflowId,
    pub description: WorkflowDescription,
}

#[derive(Debug, Snafu)]
pub enum NodeTargetError {
    #[snafu(transparent)]
    Observation { source: mf_telemetry::ContractError },
    #[snafu(display("node target `{id}` context `{key}`: {reason}"))]
    ContextKey {
        id: String,
        key: String,
        reason: &'static str,
    },
    #[snafu(display("node target `{id}` context `{key}`: {source}"))]
    ContextType {
        id: String,
        key: String,
        source: crate::TypeMismatch,
    },
    #[snafu(display("unknown top-level node target `{id}`"))]
    Unknown { id: String },
    #[snafu(display(
        "node target `{id}` is {kind:?}; isolated execution supports only top-level Tasks"
    ))]
    Unsupported { id: String, kind: NodeTargetKind },
    #[snafu(display("compiled node target `{id}` contract drift: {component}"))]
    Drift { id: String, component: String },
}

pub struct PreparedTaskTarget<'a> {
    contract: &'a NodeTarget,
    node: FlowNode<&'a dyn TaskNode>,
    call_interfaces: &'a crate::CallInterfaces,
}

impl crate::FlowRuntime {
    pub fn execute_target(
        &self,
        target: &PreparedTaskTarget<'_>,
        inputs: crate::WorkflowArguments,
        context: crate::WorkflowArguments,
    ) -> Result<crate::FlowOutputs, crate::WorkflowRunError> {
        self.execute_target_with_observation(target, inputs, context, None)
    }

    pub fn execute_target_with_observation(
        &self,
        target: &PreparedTaskTarget<'_>,
        inputs: crate::WorkflowArguments,
        context: crate::WorkflowArguments,
        observation: Option<crate::RunObservation>,
    ) -> Result<crate::FlowOutputs, crate::WorkflowRunError> {
        crate::ExecutionContext::run(observation, |state| {
            self.execute_target_in_context(target, state, inputs, context)
        })
    }

    pub fn execute_target_in_context(
        &self,
        target: &PreparedTaskTarget<'_>,
        state: &mut crate::ExecutionContext,
        inputs: crate::WorkflowArguments,
        context: crate::WorkflowArguments,
    ) -> Result<crate::FlowOutputs, crate::WorkflowRunError> {
        state.with_call_interfaces(target.call_interfaces, |state| {
            let limit = target
                .contract
                .worker_limit
                .unwrap_or(self.options().max_parallel_domains);
            state.configure_worker_limit(self.options().max_parallel_domains.min(limit));
            state
                .initialize_target(target.contract, inputs, context)
                .inspect_err(|error| {
                    if let Some(observation) = state.observation_mut() {
                        observation.preparation_failed_unattributed(error.to_string());
                    }
                })?;
            let result = (|| {
                crate::context::execute_ordered_task_in_context(
                    &target.node,
                    target.node.node,
                    [],
                    state,
                )?;
                state.target_outputs(&target.node)
            })();
            if let Err(error) = &result {
                state.select_observation_failure(error);
            }
            result
        })
    }
}

impl PreparedTaskTarget<'_> {
    pub fn contract(&self) -> &NodeTarget {
        self.contract
    }
}

pub fn target_ports(ports: &[crate::PortSpec]) -> BTreeMap<String, WorkflowInput> {
    ports
        .iter()
        .map(|port| {
            (
                port.name.to_string(),
                WorkflowInput {
                    value_type: port.value_type.clone(),
                    required: port.required,
                    schema: None,
                    binding: crate::InputBinding::default(),
                },
            )
        })
        .collect()
}

impl NodeTarget {
    pub fn start_observation(
        &self,
        observer: &mf_telemetry::observation::Observer,
        run_id: mf_telemetry::identity::RunId,
    ) -> Result<crate::RunObservation, NodeTargetError> {
        use opentelemetry::trace::TraceContextExt;
        self.require_task()?;
        let observation = observer.start_with_description(self.description.clone(), run_id)?;
        {
            let _context = observation.enter();
            let context = opentelemetry::Context::current();
            context.span().set_attribute(opentelemetry::KeyValue::new(
                "mf.workflow.original_id",
                self.original_workflow_id.to_string(),
            ));
            context.span().set_attribute(opentelemetry::KeyValue::new(
                "mf.node.target_id",
                self.id.clone(),
            ));
        }
        Ok(observation)
    }

    pub fn validate_context(
        &self,
        arguments: &crate::WorkflowArguments,
    ) -> Result<(), NodeTargetError> {
        for (key, value) in &arguments.0 {
            let input = self.context.get(key).context(ContextKeySnafu {
                id: &self.id,
                key,
                reason: "undeclared external reference",
            })?;
            input
                .validate_shared(value)
                .context(ContextTypeSnafu { id: &self.id, key })?;
        }
        for key in self.context.keys() {
            ensure!(
                arguments.0.contains_key(key),
                ContextKeySnafu {
                    id: &self.id,
                    key,
                    reason: "required external reference was not supplied",
                }
            );
        }
        Ok(())
    }

    pub fn find<'a>(targets: &'a [Self], id: &str) -> Result<&'a Self, NodeTargetError> {
        let target = targets
            .iter()
            .find(|target| target.id == id)
            .context(UnknownSnafu { id })?;
        target.require_task()?;
        Ok(target)
    }

    fn require_task(&self) -> Result<(), NodeTargetError> {
        ensure!(
            self.kind == NodeTargetKind::Task,
            UnsupportedSnafu {
                id: &self.id,
                kind: self.kind
            }
        );
        Ok(())
    }

    fn validate_metadata(&self, id: &str, metadata: &NodeMetadata) -> Result<(), NodeTargetError> {
        let drift = |component: &'static str| DriftSnafu {
            id: &self.id,
            component,
        };
        ensure!(id == self.id, drift("node identity"));
        let inputs = self
            .inputs
            .inputs
            .get(id)
            .context(drift("input node declaration"))?;
        let mut input_ports = target_ports(&metadata.ports.inputs);
        for (name, binding) in &metadata.input_bindings {
            let input = input_ports.get_mut(name).context(drift("input binding"))?;
            input.binding = binding.clone();
        }
        for (direction, expected, actual) in [
            ("input", inputs, input_ports),
            (
                "output",
                &self.outputs,
                target_ports(&metadata.ports.outputs),
            ),
        ] {
            for port in expected.keys().chain(actual.keys()) {
                ensure!(
                    expected.get(port) == actual.get(port),
                    DriftSnafu {
                        id: &self.id,
                        component: format!("{direction} port `{port}`"),
                    }
                );
            }
        }
        let references: std::collections::BTreeSet<_> = metadata
            .context_references
            .iter()
            .map(|reference| reference.output.as_str())
            .collect();
        ensure!(
            references
                .into_iter()
                .eq(self.context.keys().map(String::as_str)),
            drift("context references")
        );
        ensure!(
            self.inputs.stdin.get(id) == metadata.stdin.as_ref(),
            drift("stdin condition")
        );
        Ok(())
    }

    fn bind<'a, N>(
        &'a self,
        node: &'a FlowNode<N>,
        task: Option<&'a dyn TaskNode>,
        nodes: impl IntoIterator<Item = (&'a str, &'a NodeMetadata)>,
        call_interfaces: &'a crate::CallInterfaces,
    ) -> Result<PreparedTaskTarget<'a>, NodeTargetError> {
        self.require_task()?;
        self.validate_metadata(node.definition_id.as_str(), &node.metadata)?;
        let task = task.context(DriftSnafu {
            id: &self.id,
            component: "executor category",
        })?;
        let outputs: BTreeMap<_, _> = nodes
            .into_iter()
            .flat_map(|(id, metadata)| {
                metadata
                    .output_ports()
                    .map(move |port| (output_id(id, &port.name), &port.value_type))
            })
            .collect();
        for (key, reference) in &self.context {
            ensure!(
                outputs.get(key) == Some(&reference),
                DriftSnafu {
                    id: &self.id,
                    component: format!("referenced output `{key}` type"),
                }
            );
        }
        Ok(PreparedTaskTarget {
            contract: self,
            call_interfaces,
            node: FlowNode {
                definition_id: node.definition_id.clone(),
                metadata: node.metadata.clone(),
                node: task,
                output_keys: Default::default(),
            },
        })
    }

    pub fn bind_flow<'a>(
        &'a self,
        flow: &'a Flow,
    ) -> Result<PreparedTaskTarget<'a>, NodeTargetError> {
        let node = flow.nodes().get(self.index).context(DriftSnafu {
            id: &self.id,
            component: "node index",
        })?;
        self.bind(
            node,
            Some(node.node.as_ref()),
            flow.nodes()
                .iter()
                .map(|node| (node.definition_id.as_str(), &node.metadata)),
            flow.call_interfaces(),
        )
    }

    pub fn bind_stream<'a>(
        &'a self,
        stream: &'a PreparedStream,
    ) -> Result<PreparedTaskTarget<'a>, NodeTargetError> {
        ensure!(
            self.worker_limit.map(std::num::NonZeroUsize::get)
                == Some(stream.plan().execution().limits.workers),
            DriftSnafu {
                id: &self.id,
                component: "stream worker limit"
            }
        );
        let nodes = stream.plan().nodes();
        let node = nodes.get(self.index).context(DriftSnafu {
            id: &self.id,
            component: "node index",
        })?;
        self.bind(
            node,
            stream.task_node(self.index),
            nodes
                .iter()
                .map(|node| (node.definition_id.as_str(), &node.metadata)),
            stream.plan().call_interfaces(),
        )
    }
}
