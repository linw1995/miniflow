use mf_runtime::{
    CallArguments, CallBindingId, CallContractError, CallInputSource, FlowDependency, NodeId,
    NodeValues, PortSpec, ResolvedCallBinding, ResolvedCallInput, ValueType,
};
use serde_json::json;

fn binding(name: &str, id: usize) -> ResolvedCallBinding {
    ResolvedCallBinding {
        id: CallBindingId::new(id),
        name: name.into(),
        caller: NodeId::new(7),
        target: NodeId::new(9),
        inputs: vec![],
        dependencies: vec![],
        prerequisites: vec![],
        success_dependencies: vec![],
    }
}

fn input(
    name: &str,
    value_type: ValueType,
    required: bool,
    source: CallInputSource,
    secret: bool,
) -> ResolvedCallInput {
    ResolvedCallInput {
        port: PortSpec::owned(name, value_type, required),
        source,
        secret,
    }
}

fn composite() -> ResolvedCallBinding {
    let mut result = binding("search", 3);
    let source = CallInputSource::Output(PortSpec::new("value", ValueType::Any, false));
    result.inputs = vec![
        input(
            "private_left",
            ValueType::Nullable(Box::new(ValueType::String)),
            false,
            source.clone(),
            false,
        ),
        input(
            "private_right",
            ValueType::Nullable(Box::new(ValueType::Int64)),
            true,
            source,
            true,
        ),
        input(
            "value",
            ValueType::String,
            false,
            CallInputSource::Default,
            false,
        ),
        input(
            "credential",
            ValueType::String,
            true,
            CallInputSource::External,
            true,
        ),
        input(
            "omitted",
            ValueType::Any,
            false,
            CallInputSource::Omitted,
            false,
        ),
    ];
    result.dependencies.push(FlowDependency {
        exclusive: false,
        input: Some("credential".into()),
        source_node: "vault".into(),
        source_output: "token".into(),
        source_key: "vault.token".into(),
    });
    result.prerequisites.push(NodeId::new(42));
    result.success_dependencies.push(NodeId::new(43));
    result
}

#[test]
fn projection_hides_external_planning_data_and_retains_conjunctive_constraints() {
    let binding = composite();
    let interface = binding.interface();
    assert_eq!(interface.id(), binding.id);
    assert_eq!(interface.name(), "search");
    assert_eq!(interface.outputs().len(), 1);
    let parameter = &interface.outputs()[0];
    assert_eq!(parameter.name(), "value");
    assert!(parameter.required() && parameter.is_secret());
    assert_eq!(parameter.constraints().len(), 3);
    assert_eq!(interface.defaults().len(), 1);
    assert_eq!(interface.defaults()[0].name(), "value");
    assert!(!interface.defaults()[0].required() && !interface.defaults()[0].is_secret());
    let public = serde_json::to_value(&interface).unwrap();
    assert_eq!(public["id"], 3);
    for forbidden in [
        "caller",
        "target",
        "dependencies",
        "private_left",
        "private_right",
        "credential",
        "vault",
        "omitted",
    ] {
        assert!(!public.to_string().contains(forbidden));
        assert!(!format!("{interface:?}").contains(forbidden));
    }
    let mut reordered = binding.clone();
    reordered.inputs.reverse();
    assert_eq!(reordered.interface(), interface);
    assert_eq!(serde_json::to_value(reordered.interface()).unwrap(), public);
}

#[test]
fn public_validation_agrees_with_resolved_binding_without_weakening_types() {
    let binding = composite();
    let interface = binding.interface();
    // Nullable strings and nullable integers overlap at null, not at all JSON values.
    for value in [json!(null), json!("text"), json!(7), json!({}), json!([1])] {
        let arguments = CallArguments {
            outputs: NodeValues::from([("value".into(), value.clone().into())]),
            defaults: NodeValues::from([("value".into(), "default namespace".into())]),
        };
        assert_eq!(
            interface.validate_arguments(&arguments).is_ok(),
            value.is_null()
        );
        assert_eq!(
            interface.validate_arguments(&arguments).is_ok(),
            binding.bind_arguments(&arguments).is_ok()
        );
    }
    assert!(matches!(
        interface.validate_arguments(&CallArguments::default()),
        Err(CallContractError::MissingArgument {
            namespace: "output",
            ..
        })
    ));
    let arguments = CallArguments {
        outputs: NodeValues::from([("value".into(), json!(null).into())]),
        ..Default::default()
    };
    interface.validate_arguments(&arguments).unwrap();
    for namespace in ["output", "default"] {
        let mut invalid = arguments.clone();
        let values = if namespace == "output" {
            &mut invalid.outputs
        } else {
            &mut invalid.defaults
        };
        values.insert("credential".into(), "override".into());
        assert!(matches!(
            interface.validate_arguments(&invalid),
            Err(CallContractError::UnknownArgument { .. })
        ));
    }
    let mut invalid = arguments.clone();
    invalid.defaults.insert("value".into(), json!(null).into());
    assert!(matches!(
        interface.validate_arguments(&invalid),
        Err(CallContractError::ArgumentType {
            namespace: "default",
            ..
        })
    ));
    assert!(arguments.outputs["value"].ptr_eq(&invalid.outputs["value"]));
}

#[test]
fn source_requiredness_optional_omission_and_numeric_constraints_are_preserved() {
    let mut resolved = binding("convert", 0);
    resolved.inputs = vec![
        input(
            "target",
            ValueType::Int64,
            false,
            CallInputSource::Output(PortSpec::new("source", ValueType::Number, true)),
            false,
        ),
        input(
            "optional",
            ValueType::String,
            false,
            CallInputSource::Output(PortSpec::new("optional", ValueType::String, false)),
            false,
        ),
        input(
            "default",
            ValueType::String,
            true,
            CallInputSource::Default,
            true,
        ),
    ];
    let interface = resolved.interface();
    assert!(!interface.outputs()[0].required());
    assert!(interface.outputs()[1].required());
    assert_eq!(interface.outputs()[0].constraints(), &[ValueType::String]);
    let mut args = CallArguments {
        outputs: NodeValues::from([("source".into(), 3.into())]),
        ..Default::default()
    };
    assert!(matches!(
        interface.validate_arguments(&args),
        Err(CallContractError::MissingArgument {
            namespace: "default",
            ..
        })
    ));
    args.defaults.insert("default".into(), "secret".into());
    interface.validate_arguments(&args).unwrap();
    for value in [json!(3), json!(3.5), json!("3"), json!(null)] {
        args.outputs.insert("source".into(), value.into());
        assert_eq!(
            interface.validate_arguments(&args).is_ok(),
            resolved.bind_arguments(&args).is_ok()
        );
    }
}
