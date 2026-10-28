extern crate mfn_code as _;
extern crate mfn_core as _;
use mf_compiler::{CompiledCallLayout, NodeRegistry, WorkflowCompileError, WorkflowDefinition};
use mf_runtime::{
    CallInputDefaults, CallInputSource, NodeExecution, NodeMetadata, NodePorts, PortSpec,
    PreparedNode, ValueType,
};
use serde_json::{Value, json};

struct NeverRun;
impl mf_runtime::TaskNode for NeverRun {
    fn execute(
        &self,
        _: mf_runtime::Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        panic!("planning must not execute a caller")
    }
}
fn parent(_: Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(NeverRun)),
        NodeMetadata {
            ports: NodePorts::default(),
            call_outputs: vec![PortSpec::new("query", ValueType::String, true)],
            ..Default::default()
        },
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.call_definition_parent", factory:mf_runtime::NodeFactory::Plain(parent) } }

fn document() -> Value {
    json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"parent","kind":"test.call_definition_parent"},
        {"id":"config","kind":"builtin.constant","config":{"value":"top-secret"}},
        {"id":"tool","kind":"builtin.code","config":{"language":"cel","inputs":{"query":"string","token":"string","limit":"int"},"code":{"text":"query"}},
            "inputs":{"token":{"from":{"node":"config","port":"value"}}}}
    ], "calls":{"parent":{"search":{"target":"tool","ports":[{"output":"query","input":"query"}],"defaults":"remaining_inputs"}}}})
}

fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

#[test]
fn versioned_definitions_lower_to_stable_private_layouts_and_public_interfaces() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = parse(document());
    assert!(definition.has_directed_calls());
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    let layout = plan.declared_call_layout(&registry).unwrap();
    let binding = &layout.callers[&"parent".into()][0];
    assert_eq!(binding.name, "search");
    assert!(matches!(binding.inputs[0].source, CallInputSource::Default));
    assert!(matches!(
        binding.inputs[1].source,
        CallInputSource::Output(_)
    ));
    assert!(matches!(
        binding.inputs[2].source,
        CallInputSource::External
    ));
    let encoded = layout.to_json().unwrap();
    assert!(!encoded.contains("top-secret"));
    let decoded = CompiledCallLayout::from_json(encoded.as_bytes()).unwrap();
    decoded.verify(&layout).unwrap();
    assert_eq!(decoded, layout);
    let view =
        serde_json::to_string(layout.interfaces().unwrap().for_caller(&"parent".into())).unwrap();
    assert!(!view.contains("token") && !view.contains("config"));
    let mut changed = document();
    changed["nodes"][1]["config"]["value"] = "other-secret".into();
    let changed = mf_compiler::compile_definition(&parse(changed), &registry)
        .unwrap()
        .declared_call_layout(&registry)
        .unwrap();
    assert!(layout.verify(&changed).is_err());
    let roundtrip = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(roundtrip, plan);
    assert_eq!(roundtrip.declared_call_layout(&registry).unwrap(), layout);
    let mut reordered = document();
    reordered["nodes"].as_array_mut().unwrap().reverse();
    let reordered = mf_compiler::compile_definition(&parse(reordered), &registry).unwrap();
    assert_eq!(reordered.declared_call_layout(&registry).unwrap(), layout);
}

#[test]
fn call_definitions_and_layouts_reject_ambiguous_or_unknown_wire_data() {
    for version in ["2026-09-26", "2026-10-19", "2026-10-25"] {
        let mut old = document();
        old["version"] = version.into();
        assert!(serde_json::from_value::<WorkflowDefinition>(old).is_err());
    }
    for field in ["unexpected", "defaults"] {
        let mut invalid = document();
        invalid["calls"]["parent"]["search"][field] = "invalid".into();
        assert!(serde_json::from_value::<WorkflowDefinition>(invalid).is_err());
    }
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&parse(document()), &registry).unwrap();
    let layout = plan
        .declared_call_layout(&registry)
        .unwrap()
        .to_json()
        .unwrap();
    let mut wrong_version: Value = serde_json::from_str(&layout).unwrap();
    wrong_version["version"] = "future".into();
    assert!(CompiledCallLayout::from_json(wrong_version.to_string().as_bytes()).is_err());
    assert!(
        CompiledCallLayout::from_json(br#"{"version":"2026-10-28","version":"2026-10-28"}"#)
            .is_err()
    );
    let mut defaults = document();
    defaults["calls"]["parent"]["search"]
        .as_object_mut()
        .unwrap()
        .remove("defaults");
    assert_eq!(
        parse(defaults).calls[&"parent".into()]["search"].defaults,
        CallInputDefaults::ExplicitOnly
    );
}

#[test]
fn planning_rejects_unknown_endpoints_mixed_activation_and_recursive_calls() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for scenario in [
        "caller",
        "target",
        "alias",
        "empty",
        "output",
        "outgoing",
        "recursion",
        "ports",
    ] {
        let mut value = document();
        match scenario {
            "caller" => {
                let calls = value["calls"]["parent"].take();
                value["calls"] = json!({"unknown":calls});
            }
            "target" => value["calls"]["parent"]["search"]["target"] = "unknown".into(),
            "alias" => {
                let call = value["calls"]["parent"]["search"].take();
                value["calls"]["parent"] = json!({" ":call});
            }
            "empty" => value["calls"]["parent"] = json!({}),
            "output" => value["outputs"] = json!([{"name":"value","node":"tool","port":"text"}]),
            "outgoing" => value["edges"] = json!([{"from_node":"tool","to_node":"parent"}]),
            "recursion" => value["calls"]["tool"] = json!({"back":{"target":"parent"}}),
            "ports" => {
                value["calls"]["parent"]["search"]["ports"] =
                    json!([{"output":"query","input":"query"},{"output":"query","input":"query"}])
            }
            _ => unreachable!(),
        }
        let error = mf_compiler::compile_definition(&parse(value), &registry).unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn executable_entry_points_fail_closed_before_directed_dispatch_exists() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&parse(document()), &registry).unwrap();
    assert!(matches!(
        mf_compiler::instantiate_compiled(&plan, &registry),
        Err(WorkflowCompileError::DirectedCallExecution)
    ));
    assert!(matches!(
        mf_compiler::instantiate_stream(&plan, &registry),
        Err(WorkflowCompileError::DirectedCallExecution)
    ));
    assert!(matches!(
        mf_compiler::resolve_nodes(&plan.definition, &registry),
        Err(WorkflowCompileError::DirectedCallExecution)
    ));
    assert!(plan.manifest(&registry).is_err());
    #[cfg(feature = "codegen")]
    {
        assert!(plan.generate_artifacts().is_err());
        assert!(plan.generate_embedded_artifacts().is_err());
        assert!(plan.generate_runner_artifacts().is_err());
        assert!(plan.generate_execution_plans(&registry).is_err());
        assert!(plan.generate_runner_execution_plans(&registry).is_err());
    }
}

#[test]
fn nested_calls_roundtrip_without_losing_scope_or_enabling_execution() {
    let mut value = json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"repeat","kind":"workflow.loop","loop":{"max_iterations":1,"variables":[{"name":"count","type":"int"}],
            "body":{"nodes":[{"id":"parent","kind":"test.call_definition_parent"},{"id":"tool","kind":"builtin.identity"}],
                "calls":{"parent":{"identity":{"target":"tool","defaults":"remaining_inputs"}}}}}}
    ]});
    let definition = parse(value.clone());
    assert!(definition.has_directed_calls());
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    assert_eq!(
        plan.definition.nodes[0]
            .loop_definition
            .as_ref()
            .unwrap()
            .body
            .calls
            .len(),
        1
    );
    let roundtrip: WorkflowDefinition =
        serde_json::from_str(&serde_json::to_string(&plan.definition).unwrap()).unwrap();
    assert!(roundtrip.has_directed_calls());
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(mf_compiler::instantiate_compiled(&plan, &registry).is_err());
    assert!(mf_compiler::compile_definition(&definition, &registry).is_err());
    value["version"] = "2026-10-25".into();
    assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
}
