use mf_runtime::{
    CallArguments, CallBindingId, CallContractError, CallInputSource, CallInterfaces, DefinitionId,
    ExecutionContext, FlowDependency, NodeId, NodeValues, PortSpec, ResolvedCallBinding,
    ResolvedCallInput, ValueType,
};
use serde_json::json;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn binding(name: &str, id: usize) -> ResolvedCallBinding {
    ResolvedCallBinding {
        description: None,
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
        schema: None,
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

fn catalog(alias: &str) -> CallInterfaces {
    CallInterfaces::new([(
        DefinitionId::from("parent"),
        vec![binding(alias, 0).interface()],
    )])
    .unwrap()
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

#[test]
fn catalogs_preserve_local_ids_and_reject_ambiguous_entries() {
    let row = |name: &str, id| binding(name, id).interface();
    let interfaces = CallInterfaces::new([
        ("right".into(), vec![row("same_alias", 0)]),
        ("parent".into(), vec![row("later", 5), row("same_alias", 0)]),
    ])
    .unwrap();
    assert_eq!(
        interfaces
            .callers()
            .map(DefinitionId::as_str)
            .collect::<Vec<_>>(),
        vec!["parent", "right"]
    );
    let parent = interfaces.for_caller(&"parent".into());
    assert_eq!(
        parent.iter().map(|p| p.id().index()).collect::<Vec<_>>(),
        vec![0, 5]
    );
    assert!(parent[0].outputs().is_empty() && parent[0].defaults().is_empty());
    parent[0]
        .validate_arguments(&CallArguments::default())
        .unwrap();
    let clone = interfaces.clone();
    assert!(std::ptr::eq(parent, clone.for_caller(&"parent".into())));
    assert!(interfaces.for_caller(&"unknown".into()).is_empty());
    assert!(!format!("{interfaces:?}").contains("same_alias"));
    for entries in [
        vec![(" ".into(), vec![])],
        vec![("parent".into(), vec![]), ("parent".into(), vec![])],
        vec![("parent".into(), vec![row(" ", 0)])],
        vec![("parent".into(), vec![row("a", 0), row("b", 0)])],
        vec![("parent".into(), vec![row("a", 0), row("a", 1)])],
    ] {
        assert!(
            !CallInterfaces::new(entries)
                .unwrap_err()
                .to_string()
                .is_empty()
        );
    }
    assert!(
        CallInterfaces::new([("parent".into(), vec![])])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn context_catalogs_restore_across_errors_panics_and_child_scopes() {
    let outer = catalog("outer");
    let inner = catalog("inner");
    let empty = CallInterfaces::default();
    let mut context = ExecutionContext::default();
    assert!(context.call_targets().is_empty());
    context
        .with_call_interfaces(&outer, |ctx| {
            assert!(ctx.current_node().is_none() && ctx.call_targets().is_empty());
            ctx.with_node("parent".into(), |ctx| {
                assert_eq!(ctx.call_targets()[0].name(), "outer");
                let expected = ctx.call_targets().as_ptr();
                for mut fork in [
                    ctx.fork_domain_with_observation(None),
                    ctx.fork_stream(None),
                ] {
                    assert!(fork.call_targets().is_empty());
                    fork.with_node("parent".into(), |fork| {
                        assert_eq!(fork.call_targets().as_ptr(), expected);
                        Ok::<_, ()>(())
                    })
                    .unwrap();
                }
                let mut child = ctx.fork_body(None);
                child
                    .with_node("parent".into(), |child| {
                        assert!(child.call_targets().is_empty());
                        Ok::<_, ()>(())
                    })
                    .unwrap();
                for scope in [&inner, &empty] {
                    let failure = ctx.with_call_interfaces(scope, |ctx| {
                        assert!(ctx.current_node().is_none());
                        ctx.with_node("parent".into(), |ctx| {
                            if scope.is_empty() {
                                assert!(ctx.call_targets().is_empty());
                            } else {
                                assert_eq!(ctx.call_targets()[0].name(), "inner");
                            }
                            Err::<(), _>("expected failure")
                        })
                    });
                    assert_eq!(failure, Err("expected failure"));
                    assert_eq!(ctx.current_node(), Some("parent"));
                    assert_eq!(ctx.call_targets()[0].name(), "outer");
                }
                let panic = catch_unwind(AssertUnwindSafe(|| {
                    let _: Result<(), ()> = ctx.with_call_interfaces(&inner, |ctx| {
                        ctx.with_node("parent".into(), |_| panic!("scope unwind"))
                    });
                }));
                assert!(panic.is_err());
                assert_eq!(ctx.call_targets()[0].name(), "outer");
                assert!(!format!("{ctx:?}").contains("outer"));
                Ok::<_, ()>(())
            })
        })
        .unwrap();
    assert!(context.current_node().is_none() && context.call_targets().is_empty());
    context
        .with_node("parent".into(), |ctx| {
            ctx.with_call_interfaces(&empty, |ctx| {
                assert!(ctx.current_node().is_none());
                Ok::<_, ()>(())
            })
        })
        .unwrap();
}

#[test]
fn schema_projection_keeps_decoded_refinements_and_descriptions_without_external_fields() {
    let mut call = binding("search", 0);
    call.description = Some("Search with a structured request".into());
    let schema = mf_runtime::ValueSchema::Object(std::collections::BTreeMap::from([(
        "query".into(),
        mf_runtime::SchemaField {
            schema: mf_runtime::ValueSchema::Type(ValueType::String).described("Search text"),
            required: true,
        },
    )]));
    call.inputs = vec![
        mf_runtime::ResolvedCallInput {
            schema: Some(schema.clone()),
            port: PortSpec::new("private_payload", ValueType::Object, true),
            source: CallInputSource::Output(PortSpec::new("payload", ValueType::Any, true)),
            secret: false,
        },
        mf_runtime::ResolvedCallInput {
            schema: Some(mf_runtime::ValueSchema::opaque(ValueType::Object)),
            port: PortSpec::new("secret_config", ValueType::Object, true),
            source: CallInputSource::External,
            secret: true,
        },
    ];
    let interface = call.interface();
    assert_eq!(
        interface.description(),
        Some("Search with a structured request")
    );
    assert_eq!(interface.outputs()[0].schema_constraints(), &[schema]);
    let encoded = serde_json::to_string(&interface).unwrap();
    assert!(
        !encoded.contains("private_payload")
            && !encoded.contains("secret_config")
            && !encoded.contains("opaque")
    );
    let incomplete = CallArguments {
        outputs: NodeValues::from([("payload".into(), json!({}).into())]),
        ..Default::default()
    };
    interface.validate_arguments(&incomplete).unwrap();
    let error = interface
        .validate_decoded_arguments(&incomplete)
        .unwrap_err();
    assert!(
        error.to_string().contains("payload") && !error.to_string().contains("private_payload")
    );
    interface
        .validate_decoded_arguments(&CallArguments {
            outputs: NodeValues::from([("payload".into(), json!({"query":"example"}).into())]),
            ..Default::default()
        })
        .unwrap();
}

#[test]
fn decoded_fanout_intersects_schemas_deterministically_and_allows_optional_omission() {
    use mf_runtime::ValueSchema;
    let mut binding = binding("lookup", 0);
    for (name, variants) in [("left", vec!["a", "b"]), ("right", vec!["b", "c"])] {
        let mut parameter = input(
            name,
            ValueType::String,
            false,
            CallInputSource::Output(PortSpec::new("query", ValueType::String, false)),
            false,
        );
        parameter.schema = Some(ValueSchema::Enum(
            variants.into_iter().map(String::from).collect(),
        ));
        binding.inputs.push(parameter);
    }
    let interface = binding.interface();
    assert_eq!(interface.outputs()[0].schema_constraints().len(), 2);
    interface
        .validate_decoded_arguments(&CallArguments::default())
        .unwrap();
    for (value, accepted) in [("a", false), ("b", true), ("c", false)] {
        let arguments = CallArguments {
            outputs: NodeValues::from([("query".into(), value.into())]),
            ..Default::default()
        };
        assert!(interface.validate_arguments(&arguments).is_ok());
        assert_eq!(
            interface.validate_decoded_arguments(&arguments).is_ok(),
            accepted
        );
    }
    binding.inputs.reverse();
    assert_eq!(binding.interface(), interface);
    let map = ValueSchema::Map(Box::new(ValueSchema::Enum(vec!["a".into()])));
    assert_eq!(
        map.value_type(),
        ValueType::Map(Box::new(ValueType::String))
    );
    map.validate_shared(&json!({"field":"a"}).into()).unwrap();
    assert!(map.validate_shared(&json!({"field":"b"}).into()).is_err());
}
