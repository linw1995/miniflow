use crate::{
    ContextReference, Flow, FlowNode, NodeMetadata, PreparedStream, TaskNode, WorkflowInput,
    WorkflowInputSchema, output_id,
};
use mf_telemetry::{description::WorkflowDescription, identity::WorkflowId};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, Snafu, ensure};
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
    pub inputs: WorkflowInputSchema,
    pub outputs: BTreeMap<String, WorkflowInput>,
    pub references: Vec<ContextReference>,
    pub context: BTreeMap<String, WorkflowInput>,
    pub original_workflow_id: WorkflowId,
    pub description: WorkflowDescription,
}

#[derive(Debug, Snafu)]
pub enum NodeTargetError {
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
    pub(super) contract: &'a NodeTarget,
    pub(super) node: FlowNode<&'a dyn TaskNode>,
}

impl PreparedTaskTarget<'_> {
    pub fn contract(&self) -> &NodeTarget {
        self.contract
    }

    pub fn metadata(&self) -> &NodeMetadata {
        &self.node.metadata
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
                },
            )
        })
        .collect()
}

impl NodeTarget {
    pub fn find<'a>(targets: &'a [Self], id: &str) -> Result<&'a Self, NodeTargetError> {
        let target = targets
            .iter()
            .find(|target| target.id == id)
            .context(UnknownSnafu { id })?;
        target.require_task()?;
        Ok(target)
    }

    pub fn require_task(&self) -> Result<(), NodeTargetError> {
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
        for (direction, expected, actual) in [
            ("input", inputs, target_ports(&metadata.ports.inputs)),
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
        ensure!(
            self.references == metadata.context_references,
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
                    .ports
                    .outputs
                    .iter()
                    .map(move |port| (output_id(id, &port.name), &port.value_type))
            })
            .collect();
        for (key, reference) in &self.context {
            ensure!(
                outputs.get(key) == Some(&&reference.value_type),
                DriftSnafu {
                    id: &self.id,
                    component: format!("referenced output `{key}` type"),
                }
            );
        }
        Ok(PreparedTaskTarget {
            contract: self,
            node: FlowNode {
                definition_id: node.definition_id.clone(),
                metadata: node.metadata.clone(),
                node: task,
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
        )
    }

    pub fn bind_stream<'a>(
        &'a self,
        stream: &'a PreparedStream,
    ) -> Result<PreparedTaskTarget<'a>, NodeTargetError> {
        let nodes = stream.plan().nodes();
        let node = nodes.get(self.index).context(DriftSnafu {
            id: &self.id,
            component: "node index",
        })?;
        self.bind(
            node,
            node.node.as_deref(),
            nodes
                .iter()
                .map(|node| (node.definition_id.as_str(), &node.metadata)),
        )
    }
}
