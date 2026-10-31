use crate::{ExecutionContext, FlowOutputs, NodeIdentity, PortSpec, WorkflowRunError};
use snafu::ResultExt;
use std::collections::BTreeMap;
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
    pub graph: Option<Arc<crate::WorkflowGraph>>,
    pub input_aliases: Option<BTreeMap<String, String>>,
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
            graph: None,
            input_aliases: None,
            body: Arc::new(body),
        }
    }

    pub fn with_graph(mut self, graph: std::sync::Arc<crate::WorkflowGraph>) -> Self {
        self.graph = Some(graph);
        self
    }

    pub fn with_input_schema(mut self, inputs: crate::WorkflowInputSchema) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_input_aliases(mut self, aliases: BTreeMap<String, String>) -> Self {
        self.input_aliases = Some(aliases);
        self
    }

    pub fn project_inputs(
        mut self,
        node: bool,
        projections: &[BTreeMap<String, String>],
    ) -> Result<Self, crate::NodeBuildError> {
        if node || !projections.is_empty() {
            self.input_aliases = Some(project_subgraph_inputs(
                &crate::WorkflowGraph::describe_inputs(&self.inputs),
                node,
                projections,
            )?);
        }
        Ok(self)
    }

    pub fn interface(&self) -> SubgraphInterface {
        SubgraphInterface {
            inputs: self.public_inputs(),
            outputs: self.outputs.clone(),
            stream_outputs: self.stream_outputs.clone(),
        }
    }

    pub fn public_inputs(&self) -> BTreeMap<String, crate::GraphInput> {
        let inputs = crate::WorkflowGraph::describe_inputs(&self.inputs);
        let Some(aliases) = &self.input_aliases else {
            return inputs;
        };
        aliases
            .iter()
            .filter_map(|(name, target)| {
                inputs
                    .get(target)
                    .map(|input| (name.clone(), input.clone()))
            })
            .collect()
    }

    pub fn bind_arguments(
        &self,
        values: crate::Inputs,
    ) -> Result<crate::WorkflowArguments, crate::WorkflowInputError> {
        let mut arguments = crate::WorkflowArguments::default();
        for (name, value) in values {
            let target = if let Some(aliases) = &self.input_aliases {
                aliases.get(&name).cloned().ok_or_else(|| {
                    crate::workflow_inputs::InvalidSnafu {
                        path: crate::workflow_inputs::pointer("", &name),
                        message: "unknown subgraph input".to_owned(),
                    }
                    .build()
                })?
            } else {
                name.clone()
            };
            arguments.0.insert(target, value);
        }
        self.inputs.validate(&arguments)?;
        self.inputs.mark_secrets(&mut arguments);
        Ok(arguments)
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

pub fn project_subgraph_inputs(
    inputs: &BTreeMap<String, crate::GraphInput>,
    node: bool,
    projections: &[BTreeMap<String, String>],
) -> Result<BTreeMap<String, String>, crate::NodeBuildError> {
    let mut aliases: BTreeMap<String, String> = inputs
        .iter()
        .map(|(name, input)| {
            (
                if node {
                    input.port.clone()
                } else {
                    name.clone()
                },
                name.clone(),
            )
        })
        .collect();
    for projection in projections {
        let mut selected = BTreeMap::new();
        for (name, source) in projection {
            snafu::ensure!(
                !name.is_empty(),
                crate::NodeInvalidSubgraphSnafu {
                    message: "subgraph input aliases must not be empty"
                }
            );
            let target = aliases.get(source).ok_or_else(|| {
                crate::NodeInvalidSubgraphSnafu {
                    message: format!("unknown subgraph input `{source}`"),
                }
                .build()
            })?;
            selected.insert(name.clone(), target.clone());
        }
        aliases = selected;
    }
    let targets: std::collections::BTreeSet<_> = aliases.values().collect();
    snafu::ensure!(
        targets.len() == aliases.len(),
        crate::NodeInvalidSubgraphSnafu {
            message: "subgraph input aliases must select distinct inputs"
        }
    );
    for (name, input) in inputs {
        snafu::ensure!(
            !input.required || targets.contains(name),
            crate::NodeInvalidSubgraphSnafu {
                message: format!("required subgraph input `{name}` is not exposed")
            }
        );
    }
    Ok(aliases)
}

#[derive(Clone)]
pub struct SubgraphInterface {
    pub inputs: BTreeMap<String, crate::GraphInput>,
    pub outputs: Vec<PortSpec>,
    pub stream_outputs: Vec<PortSpec>,
}

impl SubgraphInterface {
    pub fn validate_supplied(
        &self,
        values: &crate::Inputs,
    ) -> Result<(), crate::WorkflowInputError> {
        for (name, value) in values {
            let input = self.inputs.get(name).ok_or_else(|| {
                crate::workflow_inputs::InvalidSnafu {
                    path: crate::workflow_inputs::pointer("", name),
                    message: "unknown subgraph input".to_owned(),
                }
                .build()
            })?;
            input.value_type.validate_shared(value).context(
                crate::workflow_inputs::TypeMismatchSnafu {
                    path: crate::workflow_inputs::pointer("", name),
                },
            )?;
            if let Some(schema) = &input.schema {
                schema.validate_shared(value).context(
                    crate::workflow_inputs::TypeMismatchSnafu {
                        path: crate::workflow_inputs::pointer("", name),
                    },
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum PreparedSubgraphParameter {
    Single(PreparedSubgraph),
    Collection(BTreeMap<String, PreparedSubgraph>),
}

pub type PreparedSubgraphs = BTreeMap<String, PreparedSubgraphParameter>;

pub fn take_subgraph(
    parameters: &mut PreparedSubgraphs,
    name: &str,
) -> Result<PreparedSubgraph, crate::NodeBuildError> {
    match parameters.remove(name) {
        Some(PreparedSubgraphParameter::Single(body)) => Ok(body),
        _ => crate::NodeInvalidSubgraphSnafu {
            message: format!("subgraph parameter `{name}` requires one subgraph"),
        }
        .fail(),
    }
}

pub fn take_subgraph_collection(
    parameters: &mut PreparedSubgraphs,
    name: &str,
) -> Result<BTreeMap<String, PreparedSubgraph>, crate::NodeBuildError> {
    match parameters.remove(name) {
        Some(PreparedSubgraphParameter::Collection(bodies)) => Ok(bodies),
        _ => crate::NodeInvalidSubgraphSnafu {
            message: format!("subgraph parameter `{name}` requires a collection"),
        }
        .fail(),
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

pub fn take_subgraph_interface(
    parameters: &mut PreparedSubgraphs,
    name: &str,
) -> Result<SubgraphInterface, crate::NodeBuildError> {
    take_subgraph(parameters, name).map(|body| body.interface())
}

pub fn take_subgraph_interfaces(
    parameters: &mut PreparedSubgraphs,
    name: &str,
) -> Result<BTreeMap<String, SubgraphInterface>, crate::NodeBuildError> {
    match parameters.remove(name) {
        Some(PreparedSubgraphParameter::Collection(bodies)) => Ok(bodies
            .into_iter()
            .map(|(name, body)| (name, body.interface()))
            .collect()),
        _ => crate::NodeInvalidSubgraphSnafu {
            message: format!("subgraph parameter `{name}` requires a collection"),
        }
        .fail(),
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
