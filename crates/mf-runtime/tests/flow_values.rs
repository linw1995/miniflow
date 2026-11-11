use mf_runtime::{
    ContextValue, ExecutionContext, ExecutionDependency, FlowNode, FlowOutputs, FlowValue,
    InputBinding, InputValue, Inputs, NodeExecutionError, NodeIdentity, NodePortContract,
    NodePorts, NodeResult, NodeValue, Nullable, OutputValue, PortSpec, PreparedNode,
    PreparedSubgraph, Shared, SnapshotOutcome, SnapshotRecorder, TaskNode, TypeCompatibility,
    TypedNodeResult, TypedTaskNode, ValueKind, ValueRef, ValueType, WorkflowArguments,
    WorkflowInput, WorkflowInputError, WorkflowInputSchema, execute_node_in_context,
    resolve_workflow_inputs,
};
use serde_json::json;
use std::{collections::BTreeMap, sync::mpsc};

fn flow_value() -> FlowValue {
    FlowValue::new(PreparedSubgraph::new(
        vec![NodeIdentity {
            id: "private-body-node".into(),
            kind: "private.body-kind".into(),
            path: vec![],
        }],
        vec![PortSpec::new("private-output", ValueType::String, true)],
        |_| {
            Ok(FlowOutputs::from([(
                "private-output".into(),
                "private-body-payload".into(),
            )]))
        },
    ))
    .unwrap()
}

#[test]
fn prepared_origin_metadata_and_invocation_are_available_from_the_runtime_entry() {
    use mf_runtime::{DiscardSink, FlowOrigin, SubgraphOrigin};
    use std::sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    };

    let origin = FlowOrigin {
        workflow_id: mf_telemetry::identity::WorkflowId::from_definition(&"public origin", &[])
            .unwrap(),
        path: vec![mf_runtime::ScopePathEntry::binding_key("producer", "body")],
    };
    let executions = Arc::new(AtomicUsize::new(0));
    let effects = executions.clone();
    let original = PreparedSubgraph::new(vec![], vec![], move |_| {
        effects.fetch_add(1, Ordering::SeqCst);
        Ok(FlowOutputs::new())
    });
    let fixed = original.clone().with_origin(origin.clone());
    assert!(original.origin.is_none());
    assert!(matches!(&fixed.origin, Some(SubgraphOrigin::Fixed(actual)) if actual == &origin));
    assert_eq!(fixed.origin(), Some(origin.clone()));
    assert_eq!(fixed.clone().origin(), fixed.origin());
    assert_eq!(fixed.scope_key("invoker", "body"), "invoker");

    let identity = Arc::new(OnceLock::new());
    let body = fixed.with_origin_source(identity.clone(), origin.path.clone());
    assert!(matches!(
        &body.origin,
        Some(SubgraphOrigin::Deferred { .. })
    ));
    let cloned = body.clone();
    let flow = FlowValue::new(body.clone()).unwrap();
    let captured = flow.clone();
    assert!(body.origin().is_none());
    assert!(cloned.origin().is_none());
    assert!(flow.body().origin().is_none());
    assert!(captured.body().origin().is_none());
    let origin = FlowOrigin {
        workflow_id: mf_telemetry::identity::WorkflowId::from_definition(
            &"final resolved graph",
            &[],
        )
        .unwrap(),
        path: origin.path,
    };
    identity.set(origin.workflow_id.clone()).unwrap();
    assert_eq!(body.origin(), Some(origin.clone()));
    assert_eq!(cloned.origin(), Some(origin.clone()));
    assert_eq!(flow.body().origin(), Some(origin.clone()));
    assert_eq!(captured.body().origin(), Some(origin));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(
        body.invoke_with_sink(&mut ExecutionContext::default(), &mut DiscardSink)
            .unwrap()
            .is_empty()
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[derive(Debug, NodeValue)]
struct FlowPort {
    value: FlowValue,
}

#[derive(Debug, NodeValue)]
struct AnyPort {
    value: ValueRef,
}

#[test]
fn flow_is_an_atomic_native_type_with_identity_equality() {
    let flow = flow_value();
    let value = ValueRef::from(flow.clone());
    assert!(matches!(value.kind(), ValueKind::Flow(_)));
    assert_eq!(value.as_flow(), Some(&flow));
    assert!(std::ptr::eq(value.as_flow().unwrap().body(), flow.body()));
    assert!(value.ptr_eq(&value.clone()));
    let rewrapped = ValueRef::from(flow.clone());
    assert_eq!(value, rewrapped);
    assert!(!value.ptr_eq(&rewrapped));
    assert_ne!(value, ValueRef::from(flow_value()));
    assert!(!value.is_object() && !value.is_string());
    assert_eq!(
        serde_json::to_value(ValueType::Flow).unwrap(),
        json!("flow")
    );
    assert_eq!(
        serde_json::from_value::<ValueType>(json!("flow")).unwrap(),
        ValueType::Flow
    );
    assert_eq!(ValueType::Flow.to_string(), "flow");
    assert!(!ValueType::Flow.is_concrete());
    assert!(ValueType::Flow.is_runtime_only());
    ValueType::Flow.check_depth().unwrap();
    assert_eq!(ValueType::infer_shared(&value), ValueType::Flow);
    ValueType::Flow.validate_shared(&value).unwrap();
    ValueType::Any.validate_shared(&value).unwrap();
    assert!(ValueType::Flow.is_assignable_to(&ValueType::Flow));
    assert!(ValueType::Flow.is_assignable_to(&ValueType::Any));
    assert_eq!(
        ValueType::Any.compatibility_with(&ValueType::Flow),
        TypeCompatibility::Checked
    );
    for descriptor in [ValueType::Object, ValueType::String] {
        assert_eq!(
            descriptor.compatibility_with(&ValueType::Flow),
            TypeCompatibility::Incompatible
        );
        let mismatch = descriptor.validate_shared(&value).unwrap_err();
        assert_eq!(mismatch.actual, "flow");
        assert_eq!(mismatch.expected, descriptor);
    }
    assert_eq!(
        ValueType::infer_shared(&ValueRef::array([value.clone()])),
        ValueType::List(Box::new(ValueType::Flow))
    );
    assert_eq!(
        ValueType::infer_shared(&ValueRef::object([("handler".into(), value)])),
        ValueType::Map(Box::new(ValueType::Flow))
    );
}

#[test]
fn native_codecs_and_node_value_derive_clone_capabilities_without_serde() {
    #[derive(Debug, NodeValue)]
    struct Handlers {
        selected: FlowValue,
        optional: Option<FlowValue>,
        nested: Vec<BTreeMap<String, Nullable<FlowValue>>>,
    }

    let flow = flow_value();
    let encoded = flow.clone().encode().unwrap();
    let decoded = <FlowValue as InputValue>::decode(encoded.clone()).unwrap();
    assert_eq!(decoded, flow);
    assert!(std::ptr::eq(decoded.body(), flow.body()));
    assert_eq!(<FlowValue as InputValue>::value_type(), ValueType::Flow);
    assert_eq!(<FlowValue as OutputValue>::value_type(), ValueType::Flow);
    let handlers = Handlers {
        selected: flow.clone(),
        optional: None,
        nested: vec![BTreeMap::from([
            ("handler".into(), Nullable::Value(flow.clone())),
            ("empty".into(), Nullable::Null),
        ])],
    };
    let ports = <Handlers as NodeValue>::ports();
    assert_eq!(ports[0].value_type, ValueType::Flow);
    assert!(ports[0].required);
    assert_eq!(ports[1].value_type, ValueType::Flow);
    assert!(!ports[1].required);
    assert_eq!(
        ports[2].value_type,
        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Nullable(
            Box::new(ValueType::Flow)
        )))))
    );
    let values = handlers.into_values().unwrap();
    assert!(!values.contains_key("optional"));
    let handlers = Handlers::from_values(values).unwrap();
    assert_eq!(handlers.selected, flow);
    assert!(handlers.optional.is_none());
    assert_eq!(handlers.nested[0]["handler"], Nullable::Value(flow.clone()));
    assert_eq!(handlers.nested[0]["empty"], Nullable::Null);
    let object = handlers.encode().unwrap();
    let handlers = <Handlers as InputValue>::decode(object).unwrap();
    assert!(std::ptr::eq(handlers.selected.body(), flow.body()));
    let shared = Shared::<FlowValue>::new(encoded.clone().into_secret()).unwrap();
    assert_eq!(shared.decode().unwrap(), flow);
    let secret = shared.encode().unwrap();
    assert!(secret.is_secret());
    assert_eq!(secret.as_flow(), Some(&flow));
    let container = ValueRef::object([("handlers".into(), ValueRef::array([encoded.clone()]))]);
    let forwarded = <BTreeMap<String, Vec<ValueRef>> as InputValue>::decode(container)
        .unwrap()
        .encode()
        .unwrap();
    assert!(forwarded["handlers"][0].ptr_eq(&encoded));
    assert_eq!(forwarded["handlers"][0].as_flow(), Some(&flow));
}

struct Emit(ValueRef);

impl NodePortContract for Emit {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new(
                "value",
                ValueType::infer_shared(&self.0),
                true,
            )],
        }
    }
}

impl TaskNode for Emit {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(FlowOutputs::from([("value".into(), self.0.clone())]).into())
    }
}

struct EchoFlow;

impl TypedTaskNode for EchoFlow {
    type Input = FlowPort;
    type Output = FlowPort;

    fn execute(
        &self,
        input: FlowPort,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(input.into())
    }
}

struct EchoAny;

impl TypedTaskNode for EchoAny {
    type Input = AnyPort;
    type Output = AnyPort;

    fn execute(
        &self,
        input: AnyPort,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(input.into())
    }
}

fn dependency(source_node: &str) -> ExecutionDependency<'_> {
    ExecutionDependency {
        exclusive: false,
        input: Some("value"),
        source_node,
        source_output: "value",
        source_key: format!("{source_node}.value").into(),
    }
}

#[test]
fn ordinary_edges_fan_out_the_same_capability_and_snapshot_only_opaque_values() {
    let flow = flow_value();
    let value = ValueRef::from(flow.clone());
    let source = FlowNode::new(
        "source",
        PreparedNode::new(Emit(value.clone()), NodePorts::default()).unwrap(),
    )
    .into_task()
    .unwrap();
    let any = FlowNode::new(
        "any",
        PreparedNode::typed_task(EchoAny, NodePorts::default()).unwrap(),
    )
    .into_task()
    .unwrap();
    let (sender, snapshots) = mpsc::channel();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(SnapshotRecorder::new(move |event| {
        sender.send(event).unwrap();
    }));
    execute_node_in_context(&source, &[], &mut context).unwrap();
    execute_node_in_context(&any, &[dependency("source")], &mut context).unwrap();
    let ContextValue::Value(forwarded) = context.output("any.value").unwrap() else {
        panic!("Any must forward the native Flow");
    };
    assert!(forwarded.ptr_eq(&value));
    for (id, source) in [("left", "source"), ("right", "any")] {
        let node = FlowNode::new(
            id,
            PreparedNode::typed_task(EchoFlow, NodePorts::default()).unwrap(),
        )
        .into_task()
        .unwrap();
        execute_node_in_context(&node, &[dependency(source)], &mut context).unwrap();
        let ContextValue::Value(output) = context.output(&format!("{id}.value")).unwrap() else {
            panic!("Flow output must be present");
        };
        assert_eq!(output.as_flow(), Some(&flow));
        assert!(std::ptr::eq(output.as_flow().unwrap().body(), flow.body()));
    }
    let snapshots: Vec<_> = snapshots.try_iter().collect();
    assert_eq!(snapshots.len(), 8);
    for event in snapshots {
        if event.snapshot.outcome == SnapshotOutcome::Succeeded {
            assert_eq!(event.snapshot.outputs["value"], json!("<Flow>"));
        }
        if event.node.as_ref() != "source" {
            assert_eq!(event.snapshot.inputs["value"], json!("<Flow>"));
        }
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("private-"));
        if event.node.as_ref() != "source" || event.snapshot.outcome == SnapshotOutcome::Succeeded {
            assert!(json.contains("<Flow>"));
        }
    }
}

#[test]
fn json_serialization_rejects_native_handles_even_when_nested_or_secret() {
    let flow = ValueRef::from(flow_value());
    let nested = ValueRef::object([(
        "items".into(),
        ValueRef::array([ValueRef::object([("handler".into(), flow.clone())])]),
    )]);
    for value in [
        flow.clone(),
        nested.clone(),
        flow.into_secret(),
        nested.into_secret(),
    ] {
        for error in [
            serde_json::to_value(&value).unwrap_err(),
            serde_json::to_vec(&value).unwrap_err(),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("Flow values cannot be serialized")
            );
            assert!(!error.to_string().contains("private-"));
        }
    }
}

#[test]
fn observations_redact_opaque_descendants_without_calling_them_secrets() {
    let handle = flow_value();
    let flow = ValueRef::from(handle.clone());
    let public = ValueRef::from("visible");
    let value = ValueRef::object([
        ("handler".into(), flow.clone()),
        (
            "nested".into(),
            ValueRef::array([ValueRef::object([("handler".into(), flow.clone())])]),
        ),
        ("public".into(), public.clone()),
    ]);
    assert!(!value.contains_secret());
    let redacted = value.redacted();
    assert_eq!(
        redacted,
        json!({"handler": "<Flow>", "nested": [{"handler": "<Flow>"}], "public": "visible"})
    );
    assert!(redacted["public"].ptr_eq(&public));
    assert!(redacted.as_object().is_some());
    serde_json::to_value(&redacted).unwrap();
    let secret = flow.clone().into_secret();
    assert!(secret.is_secret());
    assert!(!flow.is_secret());
    assert_eq!(secret.as_flow(), Some(&handle));
    assert_eq!(secret.redacted(), json!("[REDACTED]"));
    assert_eq!(value.clone().into_secret().redacted(), json!("[REDACTED]"));
    let mixed = ValueRef::array([value.clone(), secret]);
    assert_eq!(mixed.redacted()[0]["handler"], json!("<Flow>"));
    assert_eq!(mixed.redacted()[1], json!("[REDACTED]"));
    let extracted = value
        .into_secret()
        .pointer("/nested/0/handler")
        .unwrap()
        .clone();
    assert!(extracted.is_secret());
    assert_eq!(extracted.as_flow(), Some(&handle));
    let derived = ValueRef::from("derived").inherit_secrets([&extracted]);
    assert!(derived.is_secret());
    for rendered in [
        format!("{flow}"),
        format!("{flow:?}"),
        format!("{mixed}"),
        format!("{mixed:#?}"),
        format!("{extracted:?}"),
        format!("{:?}", flow.kind()),
        format!("{handle:?}"),
    ] {
        assert!(!rendered.contains("private-"));
    }
}

fn input_schema(value_type: ValueType, binding: InputBinding) -> WorkflowInputSchema {
    WorkflowInputSchema {
        inputs: BTreeMap::from([(
            "consumer".into(),
            BTreeMap::from([(
                "value".into(),
                WorkflowInput {
                    value_type,
                    required: true,
                    binding,
                },
            )]),
        )]),
        stdin: BTreeMap::new(),
    }
}

#[test]
fn native_workflow_arguments_bind_real_handles_and_preserve_sensitivity() {
    let flow = flow_value();
    let value = ValueRef::from(flow.clone());
    let arguments = WorkflowArguments(Inputs::from([("consumer.value".into(), value.clone())]));
    let schema = input_schema(ValueType::Flow, InputBinding::default());
    schema.validate(&arguments).unwrap();
    let resolved = resolve_workflow_inputs(&schema, &arguments, |_| {
        panic!("native Flow arguments must not need external sources")
    })
    .unwrap();
    assert!(resolved.0["consumer.value"].ptr_eq(&value));
    let mut secret_schema = schema;
    secret_schema
        .inputs
        .get_mut("consumer")
        .unwrap()
        .get_mut("value")
        .unwrap()
        .binding
        .secret = true;
    let resolved = resolve_workflow_inputs(&secret_schema, &arguments, |_| None).unwrap();
    assert!(resolved.0["consumer.value"].is_secret());
    assert_eq!(resolved.0["consumer.value"].as_flow(), Some(&flow));
    assert!(!arguments.0["consumer.value"].is_secret());
    let container = ValueRef::object([("handlers".into(), ValueRef::array([value]))]);
    let schema = input_schema(
        ValueType::Map(Box::new(ValueType::List(Box::new(ValueType::Nullable(
            Box::new(ValueType::Flow),
        ))))),
        InputBinding::default(),
    );
    let arguments = WorkflowArguments(Inputs::from([("consumer.value".into(), container.clone())]));
    let resolved = resolve_workflow_inputs(&schema, &arguments, |_| None).unwrap();
    assert!(resolved.0["consumer.value"].ptr_eq(&container));
    assert_eq!(
        resolved.0["consumer.value"]["handlers"][0].as_flow(),
        Some(&flow)
    );
}

#[test]
fn json_and_cli_inputs_cannot_reconstitute_flow_handles() {
    let schema = input_schema(ValueType::Flow, InputBinding::default());
    for forged in [
        json!("<Flow>"),
        json!("flow-name"),
        json!({"type": "flow", "inputs": [], "outputs": [], "body": {"nodes": []}}),
    ] {
        let value: ValueRef = serde_json::from_value(forged.clone()).unwrap();
        assert!(value.as_flow().is_none());
        assert_ne!(value, ValueRef::from(flow_value()));
        assert!(ValueType::Flow.validate_value(&forged).is_err());
        assert!(<FlowValue as InputValue>::decode(value.clone()).is_err());
        let error = FlowPort::from_values(Inputs::from([("value".into(), value)])).unwrap_err();
        assert_eq!(error.pointer(), "/value");
        let json = serde_json::to_vec(&json!({"consumer.value": forged})).unwrap();
        let arguments = WorkflowArguments::from_json(&json).unwrap();
        assert!(schema.validate(&arguments).is_err());
        let flag = format!("--consumer.value={forged}");
        assert!(WorkflowArguments::parse([flag.into()], &schema).is_err());
    }
}

#[test]
fn runtime_only_input_bindings_reject_env_and_even_vacuous_json_defaults() {
    let descriptors = [
        (ValueType::Flow, json!("<Flow>")),
        (ValueType::List(Box::new(ValueType::Flow)), json!([])),
        (ValueType::Map(Box::new(ValueType::Flow)), json!({})),
        (ValueType::Nullable(Box::new(ValueType::Flow)), json!(null)),
        (
            ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Nullable(
                Box::new(ValueType::Flow),
            ))))),
            json!([]),
        ),
    ];
    for (descriptor, default) in descriptors {
        assert!(descriptor.is_runtime_only());
        assert!(!descriptor.is_concrete());
        assert_eq!(
            serde_json::from_value::<ValueType>(serde_json::to_value(&descriptor).unwrap())
                .unwrap(),
            descriptor
        );
        for binding in [
            InputBinding {
                env: Some("HANDLER".into()),
                ..Default::default()
            },
            InputBinding {
                default: Some(default),
                ..Default::default()
            },
        ] {
            let error = binding.validate("consumer.value", &descriptor).unwrap_err();
            assert!(
                matches!(error, WorkflowInputError::Invalid { path, message }
                if path == "/consumer.value" && message.contains("runtime-only Flow"))
            );
            let schema = input_schema(descriptor.clone(), binding);
            assert!(
                resolve_workflow_inputs(&schema, &WorkflowArguments::default(), |_| {
                    panic!("binding validation must precede environment access")
                })
                .is_err()
            );
        }
        InputBinding {
            secret: true,
            ..Default::default()
        }
        .validate("consumer.value", &descriptor)
        .unwrap();
    }
    let any = InputBinding {
        default: Some(json!({"type": "flow"})),
        ..Default::default()
    };
    any.validate("consumer.value", &ValueType::Any).unwrap();
    let resolved = resolve_workflow_inputs(
        &input_schema(ValueType::Any, any),
        &WorkflowArguments::default(),
        |_| None,
    )
    .unwrap();
    assert!(resolved.0["consumer.value"].as_flow().is_none());
    assert_eq!(resolved.0["consumer.value"], json!({"type": "flow"}));
}
