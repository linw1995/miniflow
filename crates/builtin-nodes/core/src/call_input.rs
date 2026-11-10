use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeMetadata, NodePortContract,
    NodePorts, NodeResult, PortSpec, PreparedNode, TaskNode, ValueType, WorkflowArguments,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    inputs: Vec<PortSpec>,
    outputs: Vec<PortSpec>,
    #[serde(default)]
    stream_outputs: Vec<PortSpec>,
}

#[derive(Debug, Snafu)]
pub enum CallInputBuildError {
    #[snafu(display("Flow invocation {direction} port `{name}` must be nonblank and unique"))]
    InvalidPort {
        direction: &'static str,
        name: String,
    },
    #[snafu(display("Flow invocation input `flow` is reserved for the callable handle"))]
    ReservedFlow,
}

impl From<CallInputBuildError> for NodeBuildError {
    fn from(source: CallInputBuildError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Snafu)]
pub enum CallInputError {
    #[snafu(display("Flow invocation requires input `flow`"))]
    MissingFlow,
    #[snafu(display("Flow invocation input `flow` is not a callable Flow value"))]
    InvalidFlow,
    #[snafu(display(
        "Flow invocation signature differs from its declared contract: declared {declared:?}, actual {actual:?}"
    ))]
    Signature {
        declared: NodePorts,
        actual: NodePorts,
    },
    #[snafu(display("Flow stream signature differs: declared {declared:?}, actual {actual:?}"))]
    StreamSignature {
        declared: Vec<PortSpec>,
        actual: Vec<PortSpec>,
    },
    #[snafu(display("Flow invocation `{node}`: {source}"))]
    Execution {
        node: String,
        source: mf_runtime::WorkflowRunError,
    },
}

impl From<CallInputError> for NodeExecutionError {
    fn from(source: CallInputError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

/// Input-call mode for `workflow.call`, with an exact edge-carried Flow signature.
///
/// Arguments are conservatively secret before the selected body's input privacy
/// is available. Public callable selection and unrelated results stay public.
struct CallInputNode {
    signature: NodePorts,
    stream_outputs: Vec<PortSpec>,
}

impl NodePortContract for CallInputNode {
    fn stream_outputs(&self) -> Vec<PortSpec> {
        self.stream_outputs.clone()
    }

    fn ports(&self) -> NodePorts {
        let mut inputs = vec![PortSpec::new("flow", ValueType::Flow, true)];
        inputs.extend(self.signature.inputs.clone());
        NodePorts {
            inputs,
            outputs: self.signature.outputs.clone(),
        }
    }
}

impl TaskNode for CallInputNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_call(inputs, context, &mut mf_runtime::DiscardSink)
    }
}

impl mf_runtime::TaskStreamNode for CallInputNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.execute_call(inputs, context, emitter)
    }
}

impl CallInputNode {
    fn execute_call(
        &self,
        mut inputs: Inputs,
        context: &mut ExecutionContext,
        sink: &mut dyn mf_runtime::EmissionSink,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value = inputs.remove("flow").context(MissingFlowSnafu)?;
        let flow = value.as_flow().context(InvalidFlowSnafu)?;
        let actual = flow.signature();
        ensure!(
            actual.inputs == self.signature.inputs && actual.outputs == self.signature.outputs,
            SignatureSnafu {
                declared: self.signature.clone(),
                actual,
            }
        );
        let mut actual = flow.stream_output_ports().to_vec();
        actual.sort_by(|left, right| left.name.cmp(&right.name));
        ensure!(
            actual == self.stream_outputs,
            StreamSignatureSnafu {
                declared: self.stream_outputs.clone(),
                actual,
            }
        );
        let id = context
            .current_node()
            .unwrap_or(mf_runtime::CALL_KIND)
            .to_owned();
        let outputs: mf_runtime::Outputs = flow
            .invoke_with_sink(
                &id,
                WorkflowArguments(inputs),
                context,
                &mut SelectionSink {
                    sink,
                    selection: &value,
                },
            )
            .context(ExecutionSnafu { node: &id })?
            .into_iter()
            // The chosen callable's identity can itself be sensitive, even when
            // its body returns a public constant. Keep that selection metadata.
            .map(|(name, output)| (name, output.inherit_secrets([&value])))
            .collect();
        let skipped = self
            .signature
            .outputs
            .iter()
            .filter(|port| !outputs.contains_key(port.name.as_ref()))
            .map(|port| port.name.to_string())
            .collect();
        Ok(NodeResult {
            outputs,
            skipped,
            ..Default::default()
        })
    }
}

struct SelectionSink<'a> {
    sink: &'a mut dyn mf_runtime::EmissionSink,
    selection: &'a mf_runtime::ValueRef,
}

impl mf_runtime::EmissionSink for SelectionSink<'_> {
    fn is_connected(&self) -> bool {
        self.sink.is_connected()
    }

    fn send(&mut self, mut result: NodeResult) -> Result<(), mf_runtime::StreamError> {
        for output in result.outputs.values_mut() {
            *output = output.clone().inherit_secrets([self.selection]);
        }
        self.sink.send(result)
    }
}

fn validate_ports<'a>(
    ports: impl IntoIterator<Item = &'a PortSpec>,
    direction: &'static str,
) -> Result<(), CallInputBuildError> {
    let mut names = BTreeSet::new();
    for port in ports {
        ensure!(
            !port.name.trim().is_empty() && names.insert(port.name.as_ref()),
            InvalidPortSnafu {
                direction,
                name: port.name.as_ref(),
            }
        );
        ensure!(
            direction != "input" || port.name != "flow",
            ReservedFlowSnafu
        );
    }
    Ok(())
}

pub fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let Config {
        mut inputs,
        mut outputs,
        mut stream_outputs,
    } = mf_runtime::deserialize_config(config)?;
    validate_ports(&inputs, "input")?;
    validate_ports(outputs.iter().chain(&stream_outputs), "output")?;
    inputs.sort_by(|left, right| left.name.cmp(&right.name));
    outputs.sort_by(|left, right| left.name.cmp(&right.name));
    stream_outputs.sort_by(|left, right| left.name.cmp(&right.name));
    let input_bindings = inputs
        .iter()
        .map(|port| {
            (
                port.name.to_string(),
                mf_runtime::InputBinding {
                    secret: true,
                    ..Default::default()
                },
            )
        })
        .collect();
    let streaming = !stream_outputs.is_empty();
    let node = CallInputNode {
        signature: NodePorts { inputs, outputs },
        stream_outputs,
    };
    let metadata = NodeMetadata {
        input_bindings,
        ..Default::default()
    };
    if streaming {
        PreparedNode::task_stream(node, metadata)
    } else {
        PreparedNode::new(node, metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{FlowValue, Outputs, PreparedSubgraph, ValueRef};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn prepare(inputs: Vec<PortSpec>, outputs: Vec<PortSpec>) -> PreparedNode {
        mf_runtime::NodeRegistry::from_inventory()
            .unwrap()
            .get(mf_runtime::CALL_KIND)
            .unwrap()
            .instantiate(json!({"inputs":inputs,"outputs":outputs}))
            .unwrap()
    }

    fn input(name: &str, required: bool) -> PortSpec {
        PortSpec::owned(name, ValueType::Int64, required)
    }

    fn schema() -> mf_runtime::WorkflowInputSchema {
        serde_json::from_value(json!({
            "inputs": {"input": {"value": {"type":"int", "required":true}}},
            "stdin": {}
        }))
        .unwrap()
    }

    fn invocation_error(error: &NodeExecutionError) -> &CallInputError {
        let NodeExecutionError::PluginFailed { source } = error else {
            panic!("expected owning-node invocation error, got {error}");
        };
        source.downcast_ref().unwrap()
    }

    #[test]
    fn declaration_is_strict_and_rejects_reserved_blank_duplicate_or_deep_ports() {
        for config in [
            json!({}),
            json!({"inputs": []}),
            json!({"outputs": []}),
            json!({"inputs": [], "outputs": [], "body": {}}),
            json!({"inputs": [{"name":"x","value_type":"int"}], "outputs": []}),
            json!({"inputs": [{"name":"x","value_type":"int", "required":true,"extra":1}], "outputs": []}),
            json!({"inputs": [input("flow", true)], "outputs": []}),
            json!({"inputs": [input(" ", true)], "outputs": []}),
            json!({"inputs": [input("x", true), input("x", false)], "outputs": []}),
            json!({"inputs": [], "outputs": [input("", true)]}),
            json!({"inputs": [], "outputs": [input("x", true), input("x", true)]}),
            json!({"inputs": [], "outputs": [], "stream_outputs": [input("", true)]}),
            json!({"inputs": [], "outputs": [], "stream_outputs": [input("x", true), input("x", true)]}),
            json!({"inputs": [], "outputs": [input("x", true)], "stream_outputs": [input("x", true)]}),
        ] {
            assert!(factory(config.clone()).is_err(), "accepted {config}");
        }
        let mut value_type = ValueType::Flow;
        for _ in 0..ValueType::MAX_DEPTH {
            value_type = ValueType::List(Box::new(value_type));
        }
        let ports = [PortSpec::new("deep", value_type, true)];
        assert!(matches!(
            factory(json!({"inputs":ports,"outputs":[]})),
            Err(NodeBuildError::InvalidConfiguration { .. })
        ));
        let prepared = prepare(Vec::new(), vec![input("flow", true)]);
        assert_eq!(
            prepared.metadata.ports.inputs,
            [PortSpec::new("flow", ValueType::Flow, true)]
        );
        assert!(matches!(
            prepared.execution,
            mf_runtime::NodeExecution::Task(_)
        ));
    }

    #[test]
    fn exact_signature_is_checked_before_any_body_effects() {
        let effects = Arc::new(AtomicUsize::new(0));
        let executions = effects.clone();
        let flow = FlowValue::new(
            PreparedSubgraph::new(Vec::new(), vec![input("value", true)], move |_| {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(Outputs::from([("value".into(), 1.into())]))
            })
            .with_input_schema(schema()),
        )
        .unwrap();
        for (inputs, outputs) in [
            (Vec::new(), vec![input("value", true)]),
            (vec![input("input.other", true)], vec![input("value", true)]),
            (
                vec![input("input.value", false)],
                vec![input("value", true)],
            ),
            (
                vec![PortSpec::new("input.value", ValueType::Any, true)],
                vec![input("value", true)],
            ),
            (vec![input("input.value", true)], Vec::new()),
            (vec![input("input.value", true)], vec![input("other", true)]),
            (
                vec![input("input.value", true)],
                vec![input("value", false)],
            ),
            (
                vec![input("input.value", true)],
                vec![PortSpec::new("value", ValueType::Number, true)],
            ),
        ] {
            let prepared = prepare(inputs, outputs);
            let error = prepared
                .execution
                .as_task_node()
                .unwrap()
                .execute(
                    Inputs::from([
                        ("flow".into(), flow.clone().into()),
                        ("input.value".into(), 1.into()),
                    ]),
                    &mut ExecutionContext::default(),
                )
                .unwrap_err();
            assert!(matches!(
                invocation_error(&error),
                CallInputError::Signature { .. }
            ));
            assert_eq!(effects.load(Ordering::SeqCst), 0);
        }
        let prepared = prepare(vec![input("input.value", true)], vec![input("value", true)]);
        let result = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([
                    ("flow".into(), flow.into()),
                    ("input.value".into(), 1.into()),
                ]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(result.outputs["value"], json!(1));
        assert_eq!(effects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn streaming_calls_forward_before_completion_and_preserve_selection_privacy() {
        struct Sink {
            received: Arc<AtomicUsize>,
            reject: bool,
        }
        impl mf_runtime::EmissionSink for Sink {
            fn send(&mut self, result: NodeResult) -> Result<(), mf_runtime::StreamError> {
                assert!(result.outputs["delta"].is_secret());
                self.received.fetch_add(1, Ordering::SeqCst);
                if self.reject {
                    return mf_runtime::StreamExecutionSnafu {
                        message: "receiver closed",
                    }
                    .fail();
                }
                Ok(())
            }
        }
        let received = Arc::new(AtomicUsize::new(0));
        let seen = received.clone();
        let completed = Arc::new(AtomicUsize::new(0));
        let finished = completed.clone();
        let flow = FlowValue::new(PreparedSubgraph::streaming(
            vec![],
            vec![input("answer", true)],
            vec![input("delta", true)],
            move |_, sink| {
                sink.send(Outputs::from([("delta".into(), 1.into())]).into())
                    .map_err(NodeExecutionError::from)
                    .context(mf_runtime::NodeExecutionSnafu {
                        definition_id: "body",
                    })?;
                assert_eq!(
                    seen.load(Ordering::SeqCst),
                    1,
                    "delivery must precede completion"
                );
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(Outputs::from([("answer".into(), 42.into())]))
            },
        ))
        .unwrap();
        // A terminal-only declaration must not silently drain a streaming callable.
        let mismatch = prepare(vec![], vec![input("answer", true)])
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([("flow".into(), flow.clone().into())]),
                &mut Default::default(),
            )
            .unwrap_err();
        assert!(matches!(
            invocation_error(&mismatch),
            CallInputError::StreamSignature { .. }
        ));
        assert_eq!(received.load(Ordering::SeqCst), 0);
        let prepared = factory(json!({"inputs":[], "outputs":[input("answer", true)], "stream_outputs":[input("delta", true)]})).unwrap();
        let mf_runtime::NodeExecution::TaskStream(executor) = prepared.execution else {
            panic!("stream executor required");
        };
        let mut sink = Sink {
            received,
            reject: false,
        };
        let arguments = Inputs::from([("flow".into(), ValueRef::from(flow).into_secret())]);
        let result = executor
            .execute_with_sink(arguments.clone(), &mut Default::default(), &mut sink)
            .unwrap();
        assert_eq!(result.outputs["answer"], json!(42));
        assert!(result.outputs["answer"].is_secret());
        sink.reject = true;
        let mut context = ExecutionContext::default();
        assert!(
            executor
                .execute_with_sink(arguments, &mut context, &mut sink)
                .is_err()
        );
        assert!(context.cancellation().failure().is_some());
        assert_eq!(completed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invocation_normalizes_port_order_and_uses_effective_requiredness() {
        let inputs = serde_json::from_value(json!({
            "inputs": {"input": {
                "z": {"type":"int", "required":true, "binding":{"env":"MINIFLOW_FLOW_TEST_ABSENT_ENV"}},
                "a": {"type":"int", "required":true, "binding":{"default":2}}
            }},
            "stdin": {}
        }))
        .unwrap();
        let flow = FlowValue::new(
            PreparedSubgraph::new(
                Vec::new(),
                vec![input("z", false), input("a", true)],
                |_| Ok(Outputs::from([("a".into(), 2.into())])),
            )
            .with_input_schema(inputs),
        )
        .unwrap();
        let prepared = prepare(
            vec![input("input.z", false), input("input.a", false)],
            vec![input("z", false), input("a", true)],
        );
        let result = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(
                Inputs::from([("flow".into(), flow.into()), ("input.z".into(), 1.into())]),
                &mut ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(result.outputs["a"], json!(2));
        assert_eq!(result.skipped.into_iter().collect::<Vec<_>>(), ["z"]);
    }

    #[test]
    fn secret_callable_selection_taints_outputs_without_blanket_tainting_arguments() {
        let output = PortSpec::new("choice", ValueType::Object, true);
        let flow = FlowValue::new(
            PreparedSubgraph::new(Vec::new(), vec![output.clone()], |_| {
                Ok(Outputs::from([(
                    "choice".into(),
                    ValueRef::object([("selected".into(), "B".into())]),
                )]))
            })
            .with_input_schema(schema()),
        )
        .unwrap();
        let prepared = prepare(vec![input("input.value", true)], vec![output]);
        for secret_flow in [false, true] {
            for secret_argument in [false, true] {
                let callable: ValueRef = flow.clone().into();
                let argument = ValueRef::from(1);
                let result = prepared
                    .execution
                    .as_task_node()
                    .unwrap()
                    .execute(
                        Inputs::from([
                            (
                                "flow".into(),
                                if secret_flow {
                                    callable.into_secret()
                                } else {
                                    callable
                                },
                            ),
                            (
                                "input.value".into(),
                                if secret_argument {
                                    argument.into_secret()
                                } else {
                                    argument
                                },
                            ),
                        ]),
                        &mut ExecutionContext::default(),
                    )
                    .unwrap();
                let output = &result.outputs["choice"];
                assert_eq!(output.is_secret(), secret_flow);
                assert_eq!(output["selected"].is_secret(), secret_flow);
                assert_eq!(output["selected"].as_str(), Some("B"));
                assert_eq!(
                    serde_json::to_value(output.redacted()).unwrap(),
                    if secret_flow {
                        json!("[REDACTED]")
                    } else {
                        json!({"selected":"B"})
                    }
                );
            }
        }
    }

    #[test]
    fn dynamic_callee_secret_inputs_are_redacted_before_invocation_snapshots() {
        let mut inputs = schema();
        inputs
            .inputs
            .get_mut("input")
            .unwrap()
            .get_mut("value")
            .unwrap()
            .binding
            .secret = true;
        let flow = FlowValue::new(
            PreparedSubgraph::new(Vec::new(), vec![input("value", true)], |child| {
                let argument = child.workflow_arguments().0["input.value"].clone();
                assert!(argument.is_secret());
                Ok(Outputs::from([("value".into(), argument)]))
            })
            .with_input_schema(inputs)
            .with_origin(mf_runtime::FlowOrigin {
                workflow_id: mf_telemetry::identity::WorkflowId::from_definition(
                    &json!({"test":"private Flow snapshot"}),
                    &[],
                )
                .unwrap(),
                path: vec![mf_runtime::ScopePathEntry::binding_key("source", "body")],
            }),
        )
        .unwrap();
        let source = mf_runtime::FlowNode {
            definition_id: "source".into(),
            node: (),
            metadata: NodePorts {
                inputs: Vec::new(),
                outputs: vec![
                    PortSpec::new("flow", ValueType::Flow, true),
                    input("private", true),
                ],
            }
            .into(),
            output_keys: Default::default(),
        };
        let mut context = ExecutionContext::for_message(
            &source,
            Outputs::from([
                ("flow".into(), flow.into()),
                ("private".into(), 8675309.into()),
            ])
            .into(),
        )
        .unwrap();
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events = captured.clone();
        context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
            events.lock().unwrap().push(event);
        }));
        let node = mf_runtime::FlowNode::new(
            "invoke",
            prepare(vec![input("input.value", true)], vec![input("value", true)]),
        )
        .into_task()
        .unwrap();
        assert_eq!(
            node.metadata.input_bindings["input.value"],
            mf_runtime::InputBinding {
                secret: true,
                ..Default::default()
            }
        );
        assert!(!node.metadata.input_bindings.contains_key("flow"));
        let dependencies = [("flow", "flow"), ("input.value", "private")].map(|(input, output)| {
            mf_runtime::ExecutionDependency {
                exclusive: false,
                input: Some(input),
                source_node: "source",
                source_output: output,
                source_key: format!("source.{output}").into(),
            }
        });
        mf_runtime::execute_node_in_context(&node, &dependencies, &mut context).unwrap();
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(
            captured[0].snapshot.outcome,
            mf_runtime::SnapshotOutcome::Started
        );
        assert_eq!(
            captured[0].snapshot.inputs["input.value"].as_str(),
            Some("[REDACTED]")
        );
        assert_eq!(
            captured[1].snapshot.inputs["input.value"].as_str(),
            Some("[REDACTED]")
        );
        assert_eq!(
            captured[1].snapshot.outputs["value"].as_str(),
            Some("[REDACTED]")
        );
        assert!(
            !serde_json::to_string(&*captured)
                .unwrap()
                .contains("8675309")
        );
    }

    #[test]
    fn repeated_invocations_do_not_capture_arguments_outputs_or_loop_frames() {
        let body_node = mf_runtime::FlowNode {
            definition_id: "body".into(),
            node: (),
            metadata: NodePorts {
                inputs: Vec::new(),
                outputs: vec![input("value", true)],
            }
            .into(),
            output_keys: Default::default(),
        };
        let mut context = ExecutionContext::for_message(
            &body_node,
            Outputs::from([("value".into(), 99.into())]).into(),
        )
        .unwrap();
        context.set_workflow_arguments(WorkflowArguments(Inputs::from([(
            "ambient.secret".into(),
            "not captured".into(),
        )])));
        let flow = FlowValue::new(
            PreparedSubgraph::new(Vec::new(), vec![input("value", true)], move |child| {
                assert!(child.output("body.value").is_err());
                assert!(child.scope_path().is_empty());
                assert!(child.current_node().is_none());
                assert_eq!(child.workflow_arguments().0.len(), 1);
                let value = child.workflow_arguments().0["input.value"].clone();
                mf_runtime::execute_generated_node_in_context(
                    &body_node,
                    [],
                    child,
                    &[],
                    |_, _| {
                        Ok(mf_runtime::GeneratedNodeResult::Dynamic(
                            Outputs::from([("value".into(), value.clone())]).into(),
                        ))
                    },
                )?;
                Ok(Outputs::from([("value".into(), value)]))
            })
            .with_input_schema(schema()),
        )
        .unwrap();
        let prepared = prepare(vec![input("input.value", true)], vec![input("value", true)]);
        let task = prepared.execution.as_task_node().unwrap();
        for value in [1, 2] {
            let result = context
                .with_node("invoke-owner".into(), |context| {
                    task.execute(
                        Inputs::from([
                            ("flow".into(), flow.clone().into()),
                            ("input.value".into(), value.into()),
                        ]),
                        context,
                    )
                })
                .unwrap();
            assert_eq!(result.outputs["value"], json!(value));
            assert_eq!(
                context
                    .select_output("value", "body", "value", false)
                    .unwrap(),
                Some(99.into())
            );
            assert!(context.invocation_path().is_empty());
            assert_eq!(context.workflow_arguments().0.len(), 1);
            assert!(
                context
                    .workflow_arguments()
                    .0
                    .contains_key("ambient.secret")
            );
        }
    }

    #[test]
    fn invocation_errors_remain_typed_and_keep_the_body_failure_source() {
        let prepared = prepare(Vec::new(), Vec::new());
        let task = prepared.execution.as_task_node().unwrap();
        for (inputs, missing) in [
            (Inputs::new(), true),
            (
                Inputs::from([("flow".into(), ValueRef::from("not a flow"))]),
                false,
            ),
        ] {
            let error = task
                .execute(inputs, &mut ExecutionContext::default())
                .unwrap_err();
            assert!(matches!(invocation_error(&error), CallInputError::MissingFlow) == missing);
            if !missing {
                assert!(matches!(
                    invocation_error(&error),
                    CallInputError::InvalidFlow
                ));
            }
        }
        let flow = FlowValue::new(PreparedSubgraph::new(Vec::new(), Vec::new(), |_| {
            mf_runtime::NodeExecutionFailedSnafu {
                message: "body failure",
            }
            .fail()
            .context(mf_runtime::NodeExecutionSnafu {
                definition_id: "body",
            })
        }));
        // Build the typed source directly; no stringify/wrap step is permitted.
        let flow = flow.unwrap();
        let error = task
            .execute(
                Inputs::from([("flow".into(), flow.into())]),
                &mut ExecutionContext::default(),
            )
            .unwrap_err();
        let CallInputError::Execution { source, .. } = invocation_error(&error) else {
            panic!("expected body execution source");
        };
        assert!(
            matches!(source, mf_runtime::WorkflowRunError::NodeExecution { definition_id, source }
            if definition_id.as_str() == "body" && matches!(source, NodeExecutionError::ExecutionFailed { message } if message == "body failure"))
        );
    }

    #[derive(mf_runtime::NodeValue)]
    struct NativeFlowPorts {
        flow: FlowValue,
    }

    struct NativeForward;

    impl mf_runtime::TypedTaskNode for NativeForward {
        type Input = NativeFlowPorts;
        type Output = NativeFlowPorts;

        fn execute(
            &self,
            input: Self::Input,
            context: &mut ExecutionContext,
        ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
            let id = context.current_node().unwrap().to_owned();
            assert_eq!(
                input.flow.invoke(&id, Default::default(), context)?["value"],
                json!(42)
            );
            Ok(input.into())
        }
    }

    #[test]
    fn native_typed_tasks_receive_invoke_and_return_flow_values_over_ordinary_edges() {
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let effects = Arc::new(AtomicUsize::new(0));
        let executions = effects.clone();
        let body = PreparedSubgraph::new(Vec::new(), vec![input("value", true)], move |child| {
            let path = child.invocation_path();
            assert!(matches!(
                path.last().unwrap().node_id(),
                "forward" | "invoke"
            ));
            executions.fetch_add(1, Ordering::SeqCst);
            Ok(Outputs::from([("value".into(), 42.into())]))
        });
        let producer = registry
            .get(crate::CONSTANT_KIND)
            .unwrap()
            .instantiate_subgraphs(
                "producer",
                json!({"type":"flow", "value":{}}),
                BTreeMap::from([("value".into(), body)]),
            )
            .unwrap();
        let forward = PreparedNode::typed_task(NativeForward, NodeMetadata::default()).unwrap();
        let invoke = prepare(Vec::new(), vec![input("value", true)]);
        let nodes = [
            ("producer", producer),
            ("forward", forward),
            ("invoke", invoke),
        ]
        .into_iter()
        .map(|(id, prepared)| mf_runtime::FlowNode::new(id, prepared).into_task().unwrap())
        .collect::<Vec<_>>();
        let dependencies = [("producer", "value"), ("forward", "flow")].map(|(source, output)| {
            mf_runtime::FlowDependency {
                exclusive: false,
                input: Some("flow".into()),
                source_node: source.into(),
                source_output: output.into(),
                source_key: format!("{source}.{output}").into(),
            }
        });
        let flow = mf_runtime::Flow::from_plan(
            nodes,
            mf_runtime::FlowPlan {
                connections: (0..2)
                    .map(|index| mf_runtime::FlowConnection {
                        exclusive: false,
                        from_node: index.into(),
                        from_output: if index == 0 { "value" } else { "flow" }.into(),
                        to_node: (index + 1).into(),
                        to_input: "flow".into(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                dependencies: vec![
                    Vec::new().into(),
                    vec![dependencies[0].clone()].into(),
                    vec![dependencies[1].clone()].into(),
                ]
                .into(),
                execution_order: vec![0.into(), 1.into(), 2.into()].into(),
                outputs: vec![mf_runtime::FlowOutput {
                    name: "value".into(),
                    node_id: 2.into(),
                    port: "value".into(),
                    optional: false,
                    key: "invoke.value".into(),
                }]
                .into(),
                execution_domains: mf_runtime::ExecutionDomains::from_parts(
                    vec![mf_runtime::ExecutionDomain {
                        id: 0,
                        nodes: vec![0.into(), 1.into(), 2.into()].into(),
                        positions: vec![0, 1, 2].into(),
                        predecessors: Vec::new().into(),
                        successors: Vec::new().into(),
                        first_position: 0,
                        snapshot_visibility: mf_runtime::OutputVisibility::Borrowed(&[]),
                    }]
                    .into(),
                    vec![Vec::new().into()].into(),
                ),
            },
            Default::default(),
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert_eq!(flow.execute().unwrap()["value"], json!(42));
        assert_eq!(effects.load(Ordering::SeqCst), 2);
    }
}
