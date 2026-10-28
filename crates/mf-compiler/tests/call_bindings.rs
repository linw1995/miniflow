use mf_compiler::{CallBindingError, FlowBuilder};
use mf_runtime::{
    CallArguments, CallBinding, CallContractError, CallInputDefaults, CallInputSource,
    CallPortMapping, ControlEdgeDefinition, EdgeDefinition, FlowNode, InputBinding, Inputs,
    NodeExecution, NodeId, NodeMetadata, NodePorts, NodeResult, NodeValues, OrderEdgeDefinition,
    PortSpec, PreparedNode, ResolvedCallBinding, TaskNode, ValueRef, ValueType,
};
use std::error::Error;
extern crate mfn_core as _;

struct Probe;
impl TaskNode for Probe {
    fn execute(
        &self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<NodeResult, mf_runtime::NodeExecutionError> {
        panic!("binding resolution must not execute business code");
    }
}

fn port(name: &str, value_type: ValueType, required: bool) -> PortSpec {
    PortSpec::owned(name, value_type, required)
}

fn node(
    id: &str,
    inputs: Vec<PortSpec>,
    outputs: Vec<PortSpec>,
    call_outputs: Vec<PortSpec>,
) -> FlowNode {
    FlowNode::new(
        id,
        PreparedNode::from_parts(
            NodeExecution::Task(Box::new(Probe)),
            NodeMetadata {
                ports: NodePorts { inputs, outputs },
                call_outputs,
                ..Default::default()
            },
        ),
    )
}

fn caller(outputs: Vec<PortSpec>) -> FlowNode {
    node("parent", vec![], vec![], outputs)
}

fn tool(inputs: Vec<PortSpec>) -> FlowNode {
    node("tool", inputs, vec![], vec![])
}

fn edge(source: &str, output: &str, target: &str, input: &str) -> EdgeDefinition {
    EdgeDefinition {
        exclusive: false,
        from_node: source.into(),
        from_output: output.into(),
        to_node: target.into(),
        to_input: input.into(),
    }
}

fn build(nodes: Vec<FlowNode>, edges: Vec<EdgeDefinition>) -> FlowBuilder {
    let order = nodes
        .iter()
        .map(|node| node.definition_id.clone())
        .collect();
    FlowBuilder::prepare(nodes, edges, order, vec![]).unwrap()
}

fn declaration(
    name: &str,
    target: usize,
    mappings: &[(&str, &str)],
    defaults: CallInputDefaults,
) -> CallBinding {
    CallBinding::new(
        name,
        NodeId::new(target),
        mappings
            .iter()
            .map(|(output, input)| CallPortMapping {
                output: (*output).into(),
                input: (*input).into(),
            })
            .collect(),
        defaults,
    )
    .unwrap()
}

fn pair(
    source: PortSpec,
    inputs: Vec<PortSpec>,
    defaults: CallInputDefaults,
) -> Result<ResolvedCallBinding, CallBindingError> {
    let input = inputs[0].name.to_string();
    let output = source.name.to_string();
    let flow = build(vec![caller(vec![source]), tool(inputs)], vec![]);
    Ok(flow
        .resolve_call_bindings(
            NodeId::new(0),
            &[declaration("tool", 1, &[(&output, &input)], defaults)],
        )?
        .remove(0))
}

#[test]
fn port_bindings_win_over_defaults_and_keep_external_inputs_private() {
    let mut tool = node(
        "search",
        vec![
            port("query", ValueType::String, true),
            port("limit", ValueType::Int64, false),
            port("token", ValueType::String, true),
        ],
        vec![],
        vec![],
    );
    tool.metadata.input_bindings.insert(
        "query".into(),
        InputBinding {
            secret: true,
            ..Default::default()
        },
    );
    // Node indexes deliberately differ from scheduling positions.
    let flow = FlowBuilder::prepare(
        vec![
            tool,
            caller(vec![
                port("request.query", ValueType::String, false),
                port("other_tool", ValueType::String, true),
            ]),
            node(
                "config",
                vec![],
                vec![port("token", ValueType::String, true)],
                vec![],
            ),
        ],
        vec![edge("config", "token", "search", "token")],
        vec!["config".into(), "parent".into(), "search".into()],
        vec![],
    )
    .unwrap();
    let binding = flow
        .resolve_call_bindings(
            NodeId::new(1),
            &[declaration(
                "search",
                0,
                &[("request.query", "query")],
                CallInputDefaults::RemainingInputs,
            )],
        )
        .unwrap()
        .remove(0);
    assert_eq!(binding.id.index(), 0);
    assert_eq!(binding.name, "search");
    assert_eq!(binding.caller, NodeId::new(1));
    assert_eq!(binding.target, NodeId::new(0));
    assert_eq!(binding.prerequisites, vec![NodeId::new(2)]);
    assert_eq!(binding.dependencies[0].source_key, "config.token");
    assert!(matches!(binding.inputs[0].source, CallInputSource::Default));
    assert!(matches!(
        binding.inputs[1].source,
        CallInputSource::Output(_)
    ));
    assert!(matches!(
        binding.inputs[2].source,
        CallInputSource::External
    ));
    let query = ValueRef::from("private query");
    let arguments = CallArguments {
        outputs: NodeValues::from([("request.query".into(), query.clone())]),
        defaults: NodeValues::from([("limit".into(), 3.into())]),
    };
    let bound = binding.bind_arguments(&arguments).unwrap();
    assert_eq!(bound.len(), 2);
    assert!(bound["query"].is_secret());
    assert!(bound["query"].ptr_eq(&query.clone().into_secret()));
    assert!(!query.is_secret());
    assert_eq!(bound["limit"].as_i64(), Some(3));
    for (namespace, name) in [
        ("output", "token"),
        ("default", "token"),
        ("default", "query"),
        ("output", "other_tool"),
    ] {
        let mut invalid = arguments.clone();
        let values = if namespace == "output" {
            &mut invalid.outputs
        } else {
            &mut invalid.defaults
        };
        values.insert(name.into(), "override".into());
        let error = binding.bind_arguments(&invalid).unwrap_err();
        assert!(
            matches!(&error, CallContractError::UnknownArgument { namespace: actual, name: argument } if *actual == namespace && argument == name)
        );
        assert!(error.to_string().contains(name));
    }
    assert!(matches!(
        binding.bind_arguments(&CallArguments::default()),
        Err(CallContractError::MissingArgument { .. })
    ));
}

#[test]
fn explicit_only_calls_preserve_optional_omission_and_requiredness() {
    let inputs = vec![
        port("query", ValueType::String, true),
        port("limit", ValueType::Int64, false),
    ];
    let binding = pair(
        port("query", ValueType::String, false),
        inputs,
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    assert!(matches!(binding.inputs[0].source, CallInputSource::Omitted));
    let secret = ValueRef::from("query").into_secret();
    let arguments = CallArguments {
        outputs: NodeValues::from([("query".into(), secret.clone())]),
        ..Default::default()
    };
    let bound = binding.bind_arguments(&arguments).unwrap();
    assert!(bound["query"].ptr_eq(&secret));
    assert!(!bound.contains_key("limit"));
    let error = binding
        .bind_arguments(&CallArguments::default())
        .unwrap_err();
    assert!(error.to_string().contains("query"));
    assert!(matches!(
        error,
        CallContractError::MissingArgument {
            namespace: "output",
            ..
        }
    ));
    let error = pair(
        port("query", ValueType::String, true),
        vec![
            port("query", ValueType::String, true),
            port("limit", ValueType::Int64, true),
        ],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap_err();
    assert!(matches!(error, CallBindingError::MissingInput { input, .. } if input == "limit"));
}

#[test]
fn default_only_and_empty_calls_need_no_caller_output_ports() {
    let flow = build(
        vec![
            caller(vec![]),
            tool(vec![
                port("count", ValueType::Int64, true),
                port("label", ValueType::String, false),
                port(
                    "nullable",
                    ValueType::Nullable(Box::new(ValueType::String)),
                    false,
                ),
            ]),
            node("empty", vec![], vec![], vec![]),
        ],
        vec![],
    );
    let resolved = flow
        .resolve_call_bindings(
            NodeId::new(0),
            &[
                declaration("tool", 1, &[], CallInputDefaults::RemainingInputs),
                declaration("empty", 2, &[], CallInputDefaults::ExplicitOnly),
            ],
        )
        .unwrap();
    let binding = &resolved[0];
    assert!(
        binding
            .inputs
            .iter()
            .all(|input| matches!(input.source, CallInputSource::Default))
    );
    assert!(matches!(
        binding.bind_arguments(&CallArguments::default()),
        Err(CallContractError::MissingArgument {
            namespace: "default",
            ..
        })
    ));
    let arguments = CallArguments {
        defaults: NodeValues::from([
            ("count".into(), 1.into()),
            ("nullable".into(), ValueRef::null()),
        ]),
        ..Default::default()
    };
    let bound = binding.bind_arguments(&arguments).unwrap();
    assert!(bound["nullable"].is_null());
    assert!(!bound.contains_key("label"));
    let mut invalid = arguments.clone();
    invalid.defaults.insert("label".into(), ValueRef::null());
    assert!(matches!(
        binding.bind_arguments(&invalid),
        Err(CallContractError::ArgumentType {
            namespace: "default",
            ..
        })
    ));
    assert_eq!(resolved[1].id.index(), 1);
    assert!(
        resolved[1]
            .bind_arguments(&CallArguments::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn static_and_checked_types_share_ordinary_connection_rules() {
    let error = pair(
        port("value", ValueType::String, true),
        vec![port("input", ValueType::Int64, true)],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap_err();
    assert!(matches!(error, CallBindingError::ConnectionType { .. }));
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<Box<mf_compiler::WorkflowCompileError>>()
            .is_some()
    );
    assert!(error.to_string().contains("parent") && error.to_string().contains("tool"));
    let binding = pair(
        port("value", ValueType::Any, true),
        vec![port(
            "input",
            ValueType::List(Box::new(ValueType::Int64)),
            true,
        )],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    let arguments = CallArguments {
        outputs: NodeValues::from([("value".into(), serde_json::json!(["invalid"]).into())]),
        ..Default::default()
    };
    let error = binding.bind_arguments(&arguments).unwrap_err();
    let CallContractError::ArgumentType {
        source, namespace, ..
    } = &error
    else {
        panic!("expected a typed failure")
    };
    assert_eq!(*namespace, "input");
    assert_eq!(source.path, "/0");
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<Box<mf_runtime::TypeMismatch>>()
            .is_some()
    );
    assert!(error.to_string().contains("input"));
    let binding = pair(
        port("value", ValueType::String, true),
        vec![port("input", ValueType::Any, true)],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    let arguments = CallArguments {
        outputs: NodeValues::from([("value".into(), 1.into())]),
        ..Default::default()
    };
    assert!(matches!(
        binding.bind_arguments(&arguments),
        Err(CallContractError::ArgumentType {
            namespace: "output",
            ..
        })
    ));
    let binding = pair(
        port("value", ValueType::Number, true),
        vec![port("input", ValueType::Int64, true)],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    for (value, valid) in [
        (serde_json::json!(1), true),
        (serde_json::json!(1.5), false),
    ] {
        let arguments = CallArguments {
            outputs: NodeValues::from([("value".into(), value.into())]),
            ..Default::default()
        };
        assert_eq!(binding.bind_arguments(&arguments).is_ok(), valid);
    }
}

#[test]
fn shared_outputs_check_every_destination_and_keep_default_namespace_separate() {
    let flow = build(
        vec![
            caller(vec![port("value", ValueType::Any, false)]),
            tool(vec![
                port("left", ValueType::String, false),
                port("right", ValueType::String, false),
                port("value", ValueType::Int64, false),
            ]),
        ],
        vec![],
    );
    let binding = flow
        .resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "tool",
                1,
                &[("value", "left"), ("value", "right")],
                CallInputDefaults::RemainingInputs,
            )],
        )
        .unwrap()
        .remove(0);
    assert!(
        binding
            .bind_arguments(&CallArguments::default())
            .unwrap()
            .is_empty()
    );
    let value = ValueRef::from("shared");
    let arguments = CallArguments {
        outputs: NodeValues::from([("value".into(), value.clone())]),
        defaults: NodeValues::from([("value".into(), 2.into())]),
    };
    let bound = binding.bind_arguments(&arguments).unwrap();
    assert!(bound["left"].ptr_eq(&value) && bound["right"].ptr_eq(&value));
    assert_eq!(bound["value"].as_i64(), Some(2));
}

#[test]
fn external_port_conflicts_and_external_type_errors_are_not_default_fallbacks() {
    for conflict in [true, false] {
        let flow = build(
            vec![
                caller(vec![port("value", ValueType::Int64, true)]),
                node(
                    "config",
                    vec![],
                    vec![port(
                        "value",
                        if conflict {
                            ValueType::Int64
                        } else {
                            ValueType::String
                        },
                        true,
                    )],
                    vec![],
                ),
                tool(vec![port("input", ValueType::Int64, true)]),
            ],
            vec![edge("config", "value", "tool", "input")],
        );
        let mappings = if conflict {
            vec![("value", "input")]
        } else {
            vec![]
        };
        let error = flow
            .resolve_call_bindings(
                NodeId::new(0),
                &[declaration(
                    "tool",
                    2,
                    &mappings,
                    CallInputDefaults::RemainingInputs,
                )],
            )
            .unwrap_err();
        if conflict {
            assert!(matches!(error, CallBindingError::ConflictingInput { .. }));
        } else {
            assert!(matches!(error, CallBindingError::ConnectionType { .. }));
        }
        assert!(error.to_string().contains("input"));
    }
}

#[test]
fn exclusive_external_inputs_reuse_branch_proofs_and_preserve_controls() {
    for proven in [true, false] {
        let mut router = node(
            "router",
            vec![],
            vec![
                port("yes", ValueType::Boolean, false),
                port("no", ValueType::Boolean, false),
            ],
            vec![],
        );
        if proven {
            router.metadata.exclusive_outputs = vec!["yes".into(), "no".into()];
        }
        let mut yes = edge("router", "yes", "tool", "selected");
        yes.exclusive = true;
        let mut no = edge("router", "no", "tool", "selected");
        no.exclusive = true;
        let flow = build(
            vec![
                caller(vec![]),
                router,
                node(
                    "ready",
                    vec![],
                    vec![port("value", ValueType::Boolean, true)],
                    vec![],
                ),
                tool(vec![
                    port("selected", ValueType::Boolean, true),
                    port("query", ValueType::String, true),
                ]),
            ],
            vec![yes, no],
        )
        .with_control_edges(vec![ControlEdgeDefinition {
            from_node: "ready".into(),
            from_output: "value".into(),
            to_node: "tool".into(),
        }])
        .unwrap()
        .with_order_edges(vec![OrderEdgeDefinition {
            from_node: "ready".into(),
            to_node: "tool".into(),
        }])
        .unwrap();
        let resolved = flow.resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "tool",
                3,
                &[],
                CallInputDefaults::RemainingInputs,
            )],
        );
        if proven {
            let binding = resolved.unwrap().remove(0);
            assert_eq!(binding.prerequisites, vec![NodeId::new(1), NodeId::new(2)]);
            assert_eq!(binding.success_dependencies, vec![NodeId::new(2)]);
            assert_eq!(
                binding
                    .dependencies
                    .iter()
                    .filter(|dep| dep.exclusive)
                    .count(),
                2
            );
            assert!(binding.dependencies.iter().any(|dep| dep.input.is_none()));
            assert!(matches!(binding.inputs[0].source, CallInputSource::Default));
            assert!(matches!(
                binding.inputs[1].source,
                CallInputSource::External
            ));
        } else {
            assert!(matches!(resolved, Err(CallBindingError::Flow { .. })));
        }
    }
}

#[test]
fn caller_and_callable_target_dependencies_cannot_form_wait_cycles() {
    let flow = build(
        vec![
            caller(vec![]),
            node(
                "config",
                vec![],
                vec![port("value", ValueType::String, true)],
                vec![],
            ),
            tool(vec![port("input", ValueType::String, true)]),
        ],
        vec![edge("config", "value", "tool", "input")],
    )
    .with_order_edges(vec![OrderEdgeDefinition {
        from_node: "parent".into(),
        to_node: "config".into(),
    }])
    .unwrap();
    let error = flow
        .resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "tool",
                2,
                &[],
                CallInputDefaults::RemainingInputs,
            )],
        )
        .unwrap_err();
    assert!(matches!(error, CallBindingError::DependencyCycle { .. }));
    assert!(error.to_string().contains("config") && error.to_string().contains("parent"));
    let flow = build(vec![tool(vec![]), caller(vec![])], vec![])
        .with_order_edges(vec![OrderEdgeDefinition {
            from_node: "tool".into(),
            to_node: "parent".into(),
        }])
        .unwrap();
    for target in [0, 1] {
        assert!(matches!(
            flow.resolve_call_bindings(
                NodeId::new(1),
                &[declaration(
                    "tool",
                    target,
                    &[],
                    CallInputDefaults::ExplicitOnly
                )]
            ),
            Err(CallBindingError::DependencyCycle { .. })
        ));
    }
    let flow = build(
        vec![
            caller(vec![]),
            node(
                "first",
                vec![],
                vec![port("value", ValueType::String, true)],
                vec![],
            ),
            node(
                "second",
                vec![port("input", ValueType::String, true)],
                vec![],
                vec![],
            ),
        ],
        vec![edge("first", "value", "second", "input")],
    );
    assert!(matches!(
        flow.resolve_call_bindings(
            NodeId::new(0),
            &[
                declaration("first", 1, &[], CallInputDefaults::ExplicitOnly),
                declaration("second", 2, &[], CallInputDefaults::RemainingInputs),
            ]
        ),
        Err(CallBindingError::DependencyCycle { .. })
    ));
}

#[test]
fn unknown_nodes_ports_and_duplicate_aliases_fail_before_execution() {
    let flow = build(
        vec![
            node(
                "parent",
                vec![],
                vec![port("ordinary", ValueType::String, true)],
                vec![port("call", ValueType::String, true)],
            ),
            tool(vec![port("input", ValueType::String, true)]),
        ],
        vec![],
    );
    assert!(matches!(
        flow.resolve_call_bindings(NodeId::new(9), &[]),
        Err(CallBindingError::UnknownNode { role: "caller", .. })
    ));
    assert!(matches!(
        flow.resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "missing",
                9,
                &[],
                CallInputDefaults::RemainingInputs
            )]
        ),
        Err(CallBindingError::UnknownNode { role: "target", .. })
    ));
    for mappings in [[("ordinary", "input")], [("call", "missing")]] {
        let error = flow
            .resolve_call_bindings(
                NodeId::new(0),
                &[declaration(
                    "tool",
                    1,
                    &mappings,
                    CallInputDefaults::RemainingInputs,
                )],
            )
            .unwrap_err();
        assert!(matches!(error, CallBindingError::UnknownPort { .. }));
        assert!(error.to_string().contains("port"));
    }
    let declaration = declaration("same", 1, &[], CallInputDefaults::RemainingInputs);
    assert!(matches!(
        flow.resolve_call_bindings(NodeId::new(0), &[declaration.clone(), declaration]),
        Err(CallBindingError::DuplicateName { .. })
    ));
}

#[test]
fn unsupported_nodes_and_context_require_explicit_invocation_support() {
    for role in [
        "caller",
        "target",
        "producer",
        "context",
        "stdin",
        "environment",
        "default",
    ] {
        let mut nodes = vec![
            caller(vec![]),
            node(
                "config",
                vec![],
                vec![port("value", ValueType::String, true)],
                vec![],
            ),
            tool(vec![port("input", ValueType::String, true)]),
        ];
        match role {
            "caller" => nodes[0].node = None,
            "target" => nodes[2].node = None,
            "producer" => nodes[1].node = None,
            "context" => {
                nodes[2]
                    .metadata
                    .context_references
                    .push(mf_runtime::ContextReference::new(
                        "config.value",
                        "credential",
                    ))
            }
            "stdin" => nodes[2].metadata.stdin = Some(mf_runtime::StdinRequirement::Always),
            "environment" => {
                nodes[2].metadata.input_bindings.insert(
                    "input".into(),
                    InputBinding {
                        env: Some("CALL_TEST_CONFIG".into()),
                        ..Default::default()
                    },
                );
            }
            "default" => {
                nodes[2].metadata.input_bindings.insert(
                    "input".into(),
                    InputBinding {
                        default: Some("configured".into()),
                        ..Default::default()
                    },
                );
            }
            _ => unreachable!(),
        }
        let flow = build(nodes, vec![edge("config", "value", "tool", "input")]);
        let error = flow
            .resolve_call_bindings(
                NodeId::new(0),
                &[declaration(
                    "tool",
                    2,
                    &[],
                    CallInputDefaults::RemainingInputs,
                )],
            )
            .unwrap_err();
        assert!(matches!(error, CallBindingError::UnsupportedNode { .. }));
        assert!(error.to_string().contains("not a supported call"));
    }
}

#[test]
fn malformed_prepared_ports_are_rejected_with_typed_depth_errors() {
    for inputs in [
        vec![port("", ValueType::String, true)],
        vec![port("input", ValueType::String, true); 2],
    ] {
        let flow = build(vec![caller(vec![]), tool(inputs)], vec![]);
        assert!(matches!(
            flow.resolve_call_bindings(
                NodeId::new(0),
                &[declaration(
                    "tool",
                    1,
                    &[],
                    CallInputDefaults::RemainingInputs
                )]
            ),
            Err(CallBindingError::InvalidPort { .. })
        ));
    }
    let mut nested = ValueType::String;
    for _ in 0..100 {
        nested = ValueType::List(Box::new(nested));
    }
    let flow = build(
        vec![caller(vec![]), tool(vec![port("input", nested, true)])],
        vec![],
    );
    let error = flow
        .resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "tool",
                1,
                &[],
                CallInputDefaults::RemainingInputs,
            )],
        )
        .unwrap_err();
    assert!(matches!(error, CallBindingError::PortDepth { .. }));
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<mf_runtime::TypeDepthError>()
            .is_some()
    );
}

struct EmptyTask;
impl TaskNode for EmptyTask {
    fn execute(
        &self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<NodeResult, mf_runtime::NodeExecutionError> {
        Ok(NodeResult::default())
    }
}

fn parent_factory(config: serde_json::Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    let mut metadata = NodeMetadata {
        call_outputs: vec![port("query", ValueType::String, true)],
        ..Default::default()
    };
    if config["collision"] == true {
        metadata.ports.outputs = metadata.call_outputs.clone();
    }
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(EmptyTask)),
        metadata,
    ))
}
fn target_factory(_: serde_json::Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(Probe)),
        NodeMetadata::new(NodePorts {
            inputs: vec![
                port("query", ValueType::String, true),
                port("token", ValueType::String, true),
            ],
            outputs: vec![],
        }),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.call_parent", factory: mf_runtime::NodeFactory::Plain(parent_factory) } }
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.call_target", factory: mf_runtime::NodeFactory::Plain(target_factory) } }

#[test]
fn compiled_call_planning_reuses_normal_preparation_and_type_inference() {
    let definition: mf_compiler::WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-25", "dependencies": {},
        "nodes": [
            {"id":"tool", "kind":"test.call_target", "inputs":{"token":{"from":{"node":"lookup","port":"value"}}}},
            {"id":"parent", "kind":"test.call_parent"},
            {"id":"lookup", "kind":"builtin.identity", "inputs":{"input":{"from":{"node":"config","port":"value"}}}},
            {"id":"config", "kind":"builtin.constant", "config":{"value":"secret"}}
        ]
    })).unwrap();
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    assert!(matches!(
        mf_compiler::compile_definition(&definition, &registry),
        Err(mf_compiler::WorkflowCompileError::MissingRequiredInput { .. })
    ));
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    let index = |name| {
        plan.definition
            .nodes
            .iter()
            .position(|node| node.id.as_str() == name)
            .unwrap()
    };
    let caller = NodeId::new(index("parent"));
    assert!(matches!(
        plan.resolve_call_bindings(&registry, caller, &[]),
        Err(CallBindingError::Preparation { .. })
    ));
    let declarations = [declaration(
        "search",
        index("tool"),
        &[("query", "query")],
        CallInputDefaults::ExplicitOnly,
    )];
    let resolved = plan
        .resolve_call_bindings(&registry, caller, &declarations)
        .unwrap();
    assert!(matches!(
        resolved[0].inputs[0].source,
        CallInputSource::Output(_)
    ));
    assert!(matches!(
        resolved[0].inputs[1].source,
        CallInputSource::External
    ));
    assert_eq!(resolved[0].dependencies[0].source_key, "lookup.value");
    let mut invalid = plan.clone();
    invalid.definition.nodes[index("config")].config["value"] = true.into();
    assert!(matches!(
        invalid.resolve_call_bindings(&registry, caller, &declarations),
        Err(CallBindingError::Preparation { .. })
    ));
    let mut invalid = plan.clone();
    invalid.execution_order.reverse();
    assert!(matches!(
        invalid.resolve_call_bindings(&registry, caller, &declarations),
        Err(CallBindingError::Preparation { .. })
    ));
    assert!(matches!(
        plan.resolve_call_bindings(&registry, NodeId::new(99), &[]),
        Err(CallBindingError::UnknownNode { .. })
    ));
    assert!(matches!(
        plan.resolve_call_bindings(
            &registry,
            caller,
            &[declaration("bad", 99, &[], CallInputDefaults::ExplicitOnly)]
        ),
        Err(CallBindingError::UnknownNode { .. })
    ));
}

#[test]
fn ordinary_execution_does_not_require_or_publish_call_outputs() {
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    let definition: mf_compiler::WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-25", "dependencies": {}, "nodes":[{"id":"parent","kind":"test.call_parent"}]
    })).unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    assert!(
        mf_compiler::instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()
            .is_empty()
    );
    let mut invalid = definition.clone();
    invalid.nodes[0].config = serde_json::json!({"collision":true});
    assert!(matches!(
        mf_compiler::compile_definition(&invalid, &registry),
        Err(mf_compiler::WorkflowCompileError::CallOutputs { .. })
    ));
    let nodes = vec![
        caller(vec![port("query", ValueType::String, true)]),
        tool(vec![port("input", ValueType::String, true)]),
    ];
    assert!(matches!(
        FlowBuilder::prepare(
            nodes,
            vec![edge("parent", "query", "tool", "input")],
            vec!["parent".into(), "tool".into()],
            vec![]
        ),
        Err(mf_compiler::FlowBuildError::UnknownPort { .. })
    ));
    let mut invalid = caller(vec![port("query", ValueType::String, true)]);
    invalid.metadata.ports.outputs = invalid.metadata.call_outputs.clone();
    assert!(matches!(
        FlowBuilder::prepare(vec![invalid], vec![], vec!["parent".into()], vec![]),
        Err(mf_compiler::FlowBuildError::CallOutputs { .. })
    ));
}

struct CombinedProbe;
impl mf_runtime::NodePortContract for CombinedProbe {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![port("value", ValueType::String, false)],
        }
    }
    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![port("delta", ValueType::String, false)]
    }
}
impl mf_runtime::TaskStreamNode for CombinedProbe {
    fn execute(
        &self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<NodeResult, mf_runtime::NodeExecutionError> {
        panic!("planning must not start a task stream")
    }
}
fn combined(id: &str, call_outputs: Vec<PortSpec>) -> FlowNode {
    FlowNode::new(
        id,
        PreparedNode::task_stream(
            CombinedProbe,
            NodeMetadata {
                call_outputs,
                ..Default::default()
            },
        )
        .unwrap(),
    )
}

#[test]
fn task_stream_callers_and_terminal_producers_keep_emissions_out_of_arguments() {
    for output in ["value", "delta"] {
        let flow = build(
            vec![
                combined("parent", vec![port("query", ValueType::String, true)]),
                combined("producer", vec![]),
                tool(vec![
                    port("query", ValueType::String, true),
                    port("input", ValueType::String, true),
                ]),
            ],
            vec![edge("producer", output, "tool", "input")],
        );
        let resolved = flow.resolve_call_bindings(
            NodeId::new(0),
            &[declaration(
                "tool",
                2,
                &[("query", "query")],
                CallInputDefaults::ExplicitOnly,
            )],
        );
        if output == "value" {
            let binding = resolved.unwrap().remove(0);
            let bound = binding
                .bind_arguments(&CallArguments {
                    outputs: NodeValues::from([("query".into(), "search".into())]),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(bound["query"].as_str(), Some("search"));
            assert!(!bound.contains_key("input"));
        } else {
            assert!(matches!(
                resolved,
                Err(CallBindingError::UnknownPort {
                    direction: "terminal output",
                    ..
                })
            ));
        }
        assert!(
            flow.resolve_call_bindings(
                NodeId::new(0),
                &[declaration(
                    "producer",
                    1,
                    &[],
                    CallInputDefaults::ExplicitOnly
                )]
            )
            .is_ok()
        );
    }
}

#[test]
fn root_call_targets_do_not_waive_nested_input_requirements() {
    let definition: mf_compiler::WorkflowDefinition = serde_json::from_value(serde_json::json!({
        "version":"2026-10-25", "dependencies":{}, "nodes":[
            {"id":"parent","kind":"test.call_parent"},
            {"id":"tool","kind":"test.call_target"},
            {"id":"repeat","kind":"workflow.loop","loop":{
                "max_iterations":1,"variables":[{"name":"count","type":"int"}],
                "body":{"nodes":[{"id":"tool","kind":"test.call_target"}]}
            }}
        ]
    }))
    .unwrap();
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    let index = |name| {
        plan.definition
            .nodes
            .iter()
            .position(|node| node.id.as_str() == name)
            .unwrap()
    };
    let error = plan
        .resolve_call_bindings(
            &registry,
            NodeId::new(index("parent")),
            &[declaration(
                "tool",
                index("tool"),
                &[],
                CallInputDefaults::RemainingInputs,
            )],
        )
        .unwrap_err();
    assert!(matches!(error, CallBindingError::Preparation { .. }));
    assert!(error.to_string().contains("tool") && error.to_string().contains("requires input"));
}
