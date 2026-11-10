use crate::{
    ExecutionContext, Flow, FlowOutputs, NodeBuildError, NodeIdentity, NodePorts, PortSpec,
    PreparedSubgraph, WorkflowArguments, WorkflowRunError,
};
use snafu::{ResultExt, Snafu, ensure};
use std::{collections::BTreeSet, fmt, sync::Arc};

/// An immutable, process-local handle to a finite callable Flow.
///
/// Clones share preparation, never invocation state or the producer's context. Handles
/// have identity equality and deliberately have no serialization implementation.
#[derive(Clone)]
pub struct FlowValue(Arc<PreparedSubgraph>, InvocationBoundary);

#[derive(Clone, Copy)]
enum InvocationBoundary {
    Local,
    External,
}

#[derive(Debug, Snafu)]
pub enum FlowValueBuildError {
    #[snafu(display("Flow values require a finite body, not an Event Flow"))]
    EventBody,
    #[snafu(display("Flow values require a callable body without an implicit scope frame"))]
    ScopedBody,
    #[snafu(display("Flow values cannot read stdin"))]
    Stdin,
    #[snafu(display("Flow values cannot export stream outputs"))]
    StreamOutputs,
    #[snafu(display("Flow value {direction} port `{name}` must be nonblank and unique"))]
    InvalidPort {
        direction: &'static str,
        name: String,
    },
    #[snafu(display("Flow value port `{name}`: {source}"))]
    TypeDepth {
        name: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display("Flow value input binding: {source}"))]
    InputBinding { source: crate::WorkflowInputError },
}

impl From<FlowValueBuildError> for NodeBuildError {
    fn from(source: FlowValueBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl FlowValue {
    /// Checks the callable contract without running the body or resolving its inputs.
    pub fn new(body: PreparedSubgraph) -> Result<Self, NodeBuildError> {
        ensure!(!body.is_event_stream(), EventBodySnafu);
        ensure!(
            body.relations
                .as_ref()
                .is_none_or(|relations| relations.is_callable()),
            ScopedBodySnafu
        );
        ensure!(body.inputs.stdin.is_empty(), StdinSnafu);
        ensure!(body.stream_outputs.is_empty(), StreamOutputsSnafu);
        let mut names = BTreeSet::new();
        for (node, ports) in &body.inputs.inputs {
            for (port, input) in ports {
                let name = crate::output_id(node, port);
                ensure!(
                    !node.trim().is_empty()
                        && !port.trim().is_empty()
                        && names.insert(name.clone()),
                    InvalidPortSnafu {
                        direction: "input",
                        name: &name,
                    }
                );
                input
                    .value_type
                    .check_depth()
                    .context(TypeDepthSnafu { name: &name })?;
                input
                    .binding
                    .validate(&name, &input.value_type)
                    .context(InputBindingSnafu)?;
            }
        }
        names.clear();
        for port in &body.outputs {
            ensure!(
                !port.name.trim().is_empty() && names.insert(port.name.to_string()),
                InvalidPortSnafu {
                    direction: "output",
                    name: port.name.as_ref(),
                }
            );
            port.value_type.check_depth().context(TypeDepthSnafu {
                name: port.name.as_ref(),
            })?;
        }
        Ok(Self(Arc::new(body), InvocationBoundary::Local))
    }

    /// Wraps a finite executor whose internal execution is observed independently,
    /// such as a runner process. The caller observes the invoking node's boundary;
    /// its definition identities and snapshots never leak into the external body.
    pub fn external(
        inputs: crate::WorkflowInputSchema,
        outputs: Vec<PortSpec>,
        execute: impl Fn(&mut ExecutionContext) -> Result<FlowOutputs, WorkflowRunError>
        + Send
        + Sync
        + 'static,
    ) -> Result<Self, NodeBuildError> {
        let mut value = Self::new(
            PreparedSubgraph::new(Vec::new(), outputs, execute).with_input_schema(inputs),
        )?;
        value.1 = InvocationBoundary::External;
        Ok(value)
    }

    /// Wraps an already prepared native Flow without capturing any caller state.
    ///
    /// `Flow` retains node IDs but not registry kind names or a definition origin.
    /// These identities are therefore explicitly native; observation requires an
    /// origin supplied through [`PreparedSubgraph::with_origin`] instead.
    pub fn from_flow(flow: Flow) -> Result<Self, NodeBuildError> {
        let nodes = flow
            .nodes()
            .iter()
            .map(|node| NodeIdentity {
                id: node.definition_id.to_string(),
                kind: "native.task".into(),
                path: Vec::new(),
            })
            .collect();
        Self::new(PreparedSubgraph::from_flow(flow, nodes))
    }

    pub fn body(&self) -> &PreparedSubgraph {
        &self.0
    }

    /// Uses Call's qualified `node.port` names and effective requiredness.
    pub fn input_ports(&self) -> Vec<PortSpec> {
        let mut ports: Vec<_> = self
            .0
            .inputs
            .inputs
            .iter()
            .flat_map(|(node, ports)| {
                ports.iter().map(move |(name, input)| {
                    PortSpec::owned(
                        crate::output_id(node, name),
                        input.value_type.clone(),
                        input.required
                            && input.binding.env.is_none()
                            && input.binding.default.is_none(),
                    )
                })
            })
            .collect();
        ports.sort_by(|left, right| left.name.cmp(&right.name));
        ports
    }

    pub fn output_ports(&self) -> &[PortSpec] {
        &self.0.outputs
    }

    /// Returns a canonical signature. Declaration order is not part of the contract.
    pub fn signature(&self) -> NodePorts {
        let mut outputs = self.output_ports().to_vec();
        outputs.sort_by(|left, right| left.name.cmp(&right.name));
        NodePorts {
            inputs: self.input_ports(),
            outputs,
        }
    }

    /// Executes with fresh arguments, values, and loop control, sharing only the
    /// invoking context's execution services, limits, cancellation, and step budget.
    pub fn invoke(
        &self,
        id: &str,
        mut arguments: WorkflowArguments,
        context: &mut ExecutionContext,
    ) -> Result<FlowOutputs, WorkflowRunError> {
        self.0.inputs.mark_secrets(&mut arguments);
        if matches!(self.1, InvocationBoundary::External) {
            return context.run_external_flow(id, arguments, |child| {
                child.bind_workflow_inputs(&self.0.inputs)?;
                let outputs = self.0.execute_in_context(child)?;
                self.validate_outputs(id, &outputs)?;
                Ok(outputs)
            });
        }
        let origin = self.0.origin();
        context.run_flow(id, arguments, origin.as_ref(), &self.0.nodes, |child| {
            child.bind_workflow_inputs(&self.0.inputs)?;
            let outputs = self.0.execute_in_context(child)?;
            self.validate_outputs(id, &outputs)?;
            Ok(outputs)
        })
    }

    fn validate_outputs(&self, id: &str, outputs: &FlowOutputs) -> Result<(), WorkflowRunError> {
        for name in outputs.keys() {
            ensure!(
                self.0.outputs.iter().any(|port| port.name == name.as_str()),
                crate::runner::InvalidOutputSnafu {
                    definition_id: id,
                    message: format!("Flow returned undeclared output `{name}`"),
                }
            );
        }
        for port in &self.0.outputs {
            if let Some(value) = outputs.get(port.name.as_ref()) {
                port.value_type
                    .validate_shared(value)
                    .context(crate::runner::OutputTypeSnafu {
                        definition_id: id,
                        output: port.name.as_ref(),
                    })?;
            } else {
                ensure!(
                    !port.required,
                    crate::runner::InvalidOutputSnafu {
                        definition_id: id,
                        message: format!("Flow did not return required output `{}`", port.name),
                    }
                );
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FlowValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FlowValue").finish_non_exhaustive()
    }
}

impl PartialEq for FlowValue {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FlowValue {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputBinding, ValueType, WorkflowInput, WorkflowInputSchema};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn body() -> PreparedSubgraph {
        PreparedSubgraph::new(Vec::new(), Vec::new(), |_| {
            panic!("inspecting a Flow handle must not execute its body")
        })
    }

    fn business_error(error: &NodeBuildError) -> &FlowValueBuildError {
        let NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected shared Flow contract error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn identity_equality_and_safe_debug_do_not_inspect_preparation() {
        let prepared = body();
        let flow = FlowValue::new(prepared.clone()).unwrap();
        assert_eq!(flow, flow.clone());
        assert_ne!(flow, FlowValue::new(prepared).unwrap());
        assert_eq!(format!("{flow:?}"), "FlowValue { .. }");
        assert!(flow.input_ports().is_empty());
        assert!(flow.output_ports().is_empty());
    }

    #[test]
    fn finite_callable_contract_is_checked_without_running_or_instantiating() {
        let event = PreparedSubgraph::from_stream_factory(
            Vec::new(),
            WorkflowInputSchema::default(),
            Vec::new(),
            || panic!("rejecting Event bodies must not instantiate a stream"),
        );
        assert!(matches!(
            business_error(&FlowValue::new(event).unwrap_err()),
            FlowValueBuildError::EventBody
        ));
        let scoped = body().with_relations(crate::GraphRelations {
            scope: Some(crate::ScopeRelations {
                input: "%frame".into(),
                values: BTreeMap::new(),
                state_outputs: BTreeMap::new(),
                writes: BTreeMap::new(),
                exits: Default::default(),
            }),
            ..Default::default()
        });
        assert!(matches!(
            business_error(&FlowValue::new(scoped).unwrap_err()),
            FlowValueBuildError::ScopedBody
        ));
        for requirement in [
            crate::StdinRequirement::Always,
            crate::StdinRequirement::UnlessInput("value".into()),
        ] {
            let stdin = body().with_input_schema(WorkflowInputSchema {
                stdin: BTreeMap::from([("reader".into(), requirement)]),
                ..Default::default()
            });
            assert!(matches!(
                business_error(&FlowValue::new(stdin).unwrap_err()),
                FlowValueBuildError::Stdin
            ));
        }
        let streaming = PreparedSubgraph::streaming(
            Vec::new(),
            Vec::new(),
            vec![PortSpec::new("stream", ValueType::Any, true)],
            |_, _| panic!("rejecting exported streams must not execute them"),
        );
        assert!(matches!(
            business_error(&FlowValue::new(streaming).unwrap_err()),
            FlowValueBuildError::StreamOutputs
        ));
        FlowValue::new(body().with_relations(crate::GraphRelations::default())).unwrap();
    }

    #[test]
    fn actual_signature_flattens_inputs_and_applies_call_requiredness() {
        let inputs = serde_json::from_value(json!({
            "inputs": {
                "z": {"b": {"type":"string", "required":true}},
                "a": {
                    "optional": {"type":"int", "required":false},
                    "default": {"type":"int", "required":true, "binding":{"default":1}},
                    "environment": {"type":"string", "required":true, "binding":{"env":"NOT_READ_DURING_FLOW_PREPARATION"}}
                }
            },
            "stdin": {}
        }))
        .unwrap();
        let mut prepared = body().with_input_schema(inputs);
        prepared.outputs = vec![
            PortSpec::new("z", ValueType::Any, false),
            PortSpec::new("a", ValueType::String, true),
        ];
        let flow = FlowValue::new(prepared).unwrap();
        assert_eq!(
            flow.input_ports(),
            [
                PortSpec::new("a.default", ValueType::Int64, false),
                PortSpec::new("a.environment", ValueType::String, false),
                PortSpec::new("a.optional", ValueType::Int64, false),
                PortSpec::new("z.b", ValueType::String, true),
            ]
        );
        assert_eq!(
            flow.signature().outputs,
            [
                PortSpec::new("a", ValueType::String, true),
                PortSpec::new("z", ValueType::Any, false),
            ]
        );
    }

    #[test]
    fn ambiguous_blank_or_deep_ports_and_bad_bindings_are_rejected() {
        let input = WorkflowInput {
            value_type: ValueType::Any,
            required: true,
            binding: InputBinding::default(),
        };
        let ambiguous = WorkflowInputSchema {
            inputs: BTreeMap::from([
                ("a.b".into(), BTreeMap::from([("c".into(), input.clone())])),
                ("a".into(), BTreeMap::from([("b.c".into(), input.clone())])),
            ]),
            stdin: BTreeMap::new(),
        };
        for inputs in [
            ambiguous,
            WorkflowInputSchema {
                inputs: BTreeMap::from([(
                    " ".into(),
                    BTreeMap::from([("x".into(), input.clone())]),
                )]),
                ..Default::default()
            },
            WorkflowInputSchema {
                inputs: BTreeMap::from([(
                    "x".into(),
                    BTreeMap::from([(" ".into(), input.clone())]),
                )]),
                ..Default::default()
            },
        ] {
            assert!(matches!(
                business_error(&FlowValue::new(body().with_input_schema(inputs)).unwrap_err()),
                FlowValueBuildError::InvalidPort {
                    direction: "input",
                    ..
                }
            ));
        }
        let mut deep = ValueType::Flow;
        for _ in 0..ValueType::MAX_DEPTH {
            deep = ValueType::List(Box::new(deep));
        }
        for ports in [
            vec![PortSpec::new(" ", ValueType::Any, true)],
            vec![PortSpec::new("x", ValueType::Any, true); 2],
            vec![PortSpec::new("deep", deep.clone(), true)],
        ] {
            let mut prepared = body();
            prepared.outputs = ports;
            assert!(FlowValue::new(prepared).is_err());
        }
        let inputs = WorkflowInputSchema {
            inputs: BTreeMap::from([(
                "x".into(),
                BTreeMap::from([(
                    "deep".into(),
                    WorkflowInput {
                        value_type: deep,
                        ..input.clone()
                    },
                )]),
            )]),
            ..Default::default()
        };
        assert!(matches!(
            business_error(&FlowValue::new(body().with_input_schema(inputs)).unwrap_err()),
            FlowValueBuildError::TypeDepth { .. }
        ));
        let inputs = WorkflowInputSchema {
            inputs: BTreeMap::from([(
                "x".into(),
                BTreeMap::from([(
                    "bound".into(),
                    WorkflowInput {
                        binding: InputBinding {
                            env: Some("invalid=name".into()),
                            ..Default::default()
                        },
                        ..input
                    },
                )]),
            )]),
            ..Default::default()
        };
        assert!(matches!(
            business_error(&FlowValue::new(body().with_input_schema(inputs)).unwrap_err()),
            FlowValueBuildError::InputBinding { .. }
        ));
    }

    #[test]
    fn native_invocation_validates_arguments_before_manual_callback_effects() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let effects = Arc::new(AtomicUsize::new(0));
        let executions = effects.clone();
        let inputs = serde_json::from_value(json!({
            "inputs": {"input": {"value": {"type":"int", "required":true, "binding":{"secret":true}}}},
            "stdin": {}
        }))
        .unwrap();
        let flow = FlowValue::new(
            PreparedSubgraph::new(
                Vec::new(),
                vec![PortSpec::new("value", ValueType::Int64, true)],
                move |child| {
                    executions.fetch_add(1, Ordering::SeqCst);
                    let argument = child.workflow_arguments().0["input.value"].clone();
                    assert!(argument.is_secret());
                    Ok(FlowOutputs::from([("value".into(), argument)]))
                },
            )
            .with_input_schema(inputs),
        )
        .unwrap();
        for arguments in [
            json!({}),
            json!({"unknown":1}),
            json!({"input.value":"wrong"}),
        ] {
            let error = flow
                .invoke(
                    "native",
                    WorkflowArguments::try_from(arguments).unwrap(),
                    &mut ExecutionContext::default(),
                )
                .unwrap_err();
            assert!(matches!(error, WorkflowRunError::WorkflowInputs { .. }));
            assert_eq!(effects.load(Ordering::SeqCst), 0);
        }
        let outputs = flow
            .invoke(
                "native",
                WorkflowArguments::try_from(json!({"input.value":1})).unwrap(),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(outputs["value"], json!(1));
        assert!(outputs["value"].is_secret());
        assert_eq!(effects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn native_invocation_validates_manual_callback_outputs_including_required_presence() {
        for (outputs, valid, type_error) in [
            (FlowOutputs::new(), false, false),
            (
                FlowOutputs::from([("unknown".into(), 1.into())]),
                false,
                false,
            ),
            (
                FlowOutputs::from([("value".into(), "wrong".into())]),
                false,
                true,
            ),
            (FlowOutputs::from([("value".into(), 1.into())]), true, false),
        ] {
            let flow = FlowValue::new(PreparedSubgraph::new(
                Vec::new(),
                vec![
                    PortSpec::new("value", ValueType::Int64, true),
                    PortSpec::new("optional", ValueType::String, false),
                ],
                move |_| Ok(outputs.clone()),
            ))
            .unwrap();
            let result = flow.invoke(
                "native",
                Default::default(),
                &mut ExecutionContext::default(),
            );
            assert_eq!(result.is_ok(), valid);
            if !valid {
                let error = result.unwrap_err();
                if type_error {
                    assert!(
                        matches!(error, WorkflowRunError::OutputType { definition_id, source, .. }
                        if definition_id.as_str() == "native" && source.expected == ValueType::Int64)
                    );
                } else {
                    assert!(
                        matches!(error, WorkflowRunError::InvalidOutput { definition_id, .. }
                        if definition_id.as_str() == "native")
                    );
                }
            }
        }
    }

    #[test]
    fn native_flow_helper_retains_real_node_ids_and_prepared_interfaces() {
        struct Never;
        impl crate::TaskNode for Never {
            fn execute(
                &self,
                _: crate::Inputs,
                _: &mut ExecutionContext,
            ) -> Result<crate::NodeResult, crate::NodeExecutionError> {
                panic!("wrapping a native Flow must not execute its nodes")
            }
        }
        let schema = serde_json::from_value(json!({
            "inputs": {"native": {"input": {"type":"flow", "required":true}}},
            "stdin": {}
        }))
        .unwrap();
        let native = Flow::from_plan(
            vec![crate::FlowNode {
                definition_id: "native".into(),
                node: Box::new(Never),
                metadata: NodePorts {
                    inputs: vec![PortSpec::new("input", ValueType::Flow, true)],
                    outputs: vec![PortSpec::new("value", ValueType::Flow, true)],
                }
                .into(),
                output_keys: Default::default(),
            }],
            crate::FlowPlan {
                connections: Vec::new().into(),
                dependencies: vec![Vec::new().into()].into(),
                execution_order: vec![0.into()].into(),
                outputs: vec![crate::FlowOutput {
                    name: "result".into(),
                    node_id: 0.into(),
                    port: "value".into(),
                    optional: false,
                    key: "native.value".into(),
                }]
                .into(),
                execution_domains: crate::ExecutionDomains::from_parts(
                    vec![crate::ExecutionDomain {
                        id: 0,
                        nodes: vec![0.into()].into(),
                        positions: vec![0].into(),
                        predecessors: Vec::new().into(),
                        successors: Vec::new().into(),
                        first_position: 0,
                        snapshot_visibility: crate::OutputVisibility::Borrowed(&[]),
                    }]
                    .into(),
                    vec![Vec::new().into()].into(),
                ),
            },
            schema,
        );
        let flow = FlowValue::from_flow(native).unwrap();
        assert!(flow.body().origin().is_none());
        assert_eq!(flow.body().nodes[0].id, "native");
        assert_eq!(flow.body().nodes[0].kind, "native.task");
        assert_eq!(
            flow.input_ports(),
            [PortSpec::new("native.input", ValueType::Flow, true)]
        );
        assert_eq!(
            flow.output_ports(),
            [PortSpec::new("result", ValueType::Flow, true)]
        );
    }
}
