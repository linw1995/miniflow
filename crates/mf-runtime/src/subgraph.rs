use crate::{ExecutionContext, FlowOutputs, NodeIdentity, PortSpec, WorkflowRunError};
use snafu::ResultExt;
use std::sync::Arc;

type SubgraphBody = dyn Fn(&mut ExecutionContext, &mut dyn crate::EmissionSink) -> Result<FlowOutputs, WorkflowRunError>
    + Send
    + Sync
    + 'static;

#[derive(Clone)]
pub struct PreparedSubgraph {
    pub nodes: Vec<NodeIdentity>,
    pub inputs: crate::WorkflowInputSchema,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
    body: Arc<SubgraphBody>,
}

impl PreparedSubgraph {
    pub fn new(
        nodes: Vec<NodeIdentity>,
        outputs: Vec<PortSpec>,
        body: impl Fn(&mut ExecutionContext) -> Result<FlowOutputs, WorkflowRunError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::streaming(nodes, outputs, Vec::new(), move |state, _| body(state))
    }

    pub fn streaming(
        nodes: Vec<NodeIdentity>,
        outputs: Vec<PortSpec>,
        stream_outputs: Vec<PortSpec>,
        body: impl Fn(
            &mut ExecutionContext,
            &mut dyn crate::EmissionSink,
        ) -> Result<FlowOutputs, WorkflowRunError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            nodes,
            inputs: Default::default(),
            outputs,
            stream_outputs,
            body: Arc::new(body),
        }
    }

    pub fn with_input_schema(mut self, inputs: crate::WorkflowInputSchema) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn execute_in_context(
        &self,
        state: &mut ExecutionContext,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        self.execute_with_sink(state, &mut crate::DiscardSink)
    }

    pub fn execute_with_sink(
        &self,
        state: &mut ExecutionContext,
        sink: &mut dyn crate::EmissionSink,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        (self.body)(state, sink)
    }
}

/// One frozen child emission selected by a containing scope.
#[derive(Clone, Debug)]
pub struct SubgraphStreamOutput {
    pub node: crate::NodeId,
    pub port: PortSpec,
    pub name: String,
    pub optional: bool,
}

pub(super) struct ProjectedSink<'a> {
    pub sink: &'a mut dyn crate::EmissionSink,
    pub outputs: &'a [SubgraphStreamOutput],
    pub node: crate::NodeId,
    pub definition_id: &'a crate::DefinitionId,
}

impl crate::EmissionSink for ProjectedSink<'_> {
    fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    fn send(&mut self, result: crate::NodeResult) -> Result<(), crate::StreamError> {
        let mut projected = crate::NodeResult::default();
        for output in self.outputs {
            if output.node == self.node
                && let Some(value) = result.outputs.get(output.port.name.as_ref())
            {
                projected.outputs.insert(output.name.clone(), value.clone());
            } else {
                if output.node == self.node && !output.optional {
                    return Err(crate::NodeExecutionError::from(
                        crate::runner::InvalidOutputSnafu {
                            definition_id: self.definition_id.clone(),
                            message: format!(
                                "missing required exported stream output `{}`",
                                output.name
                            ),
                        }
                        .build(),
                    ))
                    .context(crate::stream_instance::ProducerSnafu {
                        definition_id: self.definition_id.clone(),
                    });
                }
                projected.skipped.insert(output.name.clone());
            }
        }
        self.sink.send(projected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmissionSink, NodeId, NodeResult, ValueRef, ValueType};

    #[test]
    fn projection_preserves_nested_sensitivity_and_marks_other_children_skipped() {
        struct Collect(Option<NodeResult>);
        impl EmissionSink for Collect {
            fn send(&mut self, result: NodeResult) -> Result<(), crate::StreamError> {
                self.0 = Some(result);
                Ok(())
            }
        }
        let payload = ValueRef::object([(
            "token".into(),
            ValueRef::from("private-value").into_secret(),
        )]);
        let outputs = vec![
            SubgraphStreamOutput {
                node: NodeId::new(0),
                port: PortSpec::new("value", ValueType::Object, true),
                name: "selected".into(),
                optional: false,
            },
            SubgraphStreamOutput {
                node: NodeId::new(1),
                port: PortSpec::new("value", ValueType::String, true),
                name: "other".into(),
                optional: false,
            },
        ];
        let mut collected = Collect(None);
        let mut sink = ProjectedSink {
            sink: &mut collected,
            outputs: &outputs,
            node: NodeId::new(0),
            definition_id: &"child".into(),
        };
        assert!(sink.is_connected());
        sink.send(crate::Outputs::from([("value".into(), payload.clone())]).into())
            .unwrap();
        let result = collected.0.unwrap();
        assert_eq!(result.outputs["selected"], payload);
        assert_eq!(
            serde_json::to_value(result.outputs["selected"].redacted()).unwrap(),
            serde_json::json!({"token":"[REDACTED]"})
        );
        assert!(!result.outputs.contains_key("other"));
        assert!(result.skipped.contains("other"));
    }
}
