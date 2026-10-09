#![cfg(feature = "codegen")]
use mf_compiler::{
    NodeRegistry, TypedFallbackReason, WorkflowDefinition, compile_definition,
    instantiate_compiled, plan_typed_segments,
};
use mf_runtime::{
    NodeMetadata, NodePorts, NodeValue, OutputDerivation, TypedConstructor, TypedNodeResult,
    TypedTaskHandle, TypedTaskNode, ValueType,
};
use serde_json::{Value, json};

#[derive(NodeValue)]
#[value(typed)]
struct Payload<T> {
    value: T,
}

struct Echo<T> {
    _type: std::marker::PhantomData<fn() -> T>,
}
impl<T: mf_runtime::TypedField> TypedTaskNode for Echo<T> {
    type Input = Payload<T>;
    type Output = Payload<T>;
    fn execute(
        &self,
        input: Self::Input,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, mf_runtime::NodeExecutionError> {
        Ok(input.into())
    }
}

fn prepare<T: mf_runtime::TypedField + 'static>(
    config: &Value,
) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let mut metadata = NodeMetadata::new(NodePorts::default());
    if config["read"] == true {
        metadata
            .context_references
            .push(mf_runtime::ContextReference::new("a.value", "ancestor"));
    }
    if config["refine"] == true {
        metadata
            .output_derivations
            .push(OutputDerivation::known_type("value", ValueType::Int64));
    }
    let task = Echo::<T> {
        _type: Default::default(),
    };
    Ok(TypedTaskHandle::new(
        task,
        metadata,
        TypedConstructor {
            package: "fixture",
            path: &["echo"],
        },
        config["read"] != true,
    )?
    .prepared())
}

fn factory(config: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let mut prepared = match config["type"].as_str() {
        Some("shared") => prepare::<mf_runtime::ValueRef>(&config),
        Some("optional") => prepare::<Option<String>>(&config),
        _ => prepare::<String>(&config),
    }?;
    if config["dynamic"] == true {
        prepared.metadata.typed_generation = None;
    }
    Ok(prepared)
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.generated_plan", factory: mf_runtime::NodeFactory::Plain(factory) } }

fn chain() -> Value {
    json!({ "version": "2026-10-03", "dependencies": {},
        "nodes": [{"id":"a", "kind":"test.generated_plan"}, {"id":"b", "kind":"test.generated_plan"}, {"id":"c", "kind":"test.generated_plan"}],
        "edges": [{"from_node":"a", "from_output":"value", "to_node":"b", "to_input":"value"}, {"from_node":"b", "from_output":"value", "to_node":"c", "to_input":"value"}],
        "outputs": [{"name":"result", "node":"c", "port":"value"}]
    })
}

fn planned(value: Value) -> (mf_runtime::Flow, mf_compiler::TypedPlan) {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let compiled = compile_definition(&definition, &registry).unwrap();
    let flow = instantiate_compiled(&compiled, &registry).unwrap();
    let plan = plan_typed_segments(&flow, true, true);
    (flow, plan)
}

#[test]
fn plans_maximal_chains_without_repartitioning_domains() {
    let (flow, plan) = planned(chain());
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(plan.segments[0].positions, [0, 1, 2]);
    assert_eq!(flow.execution_domains().len(), 1);
    assert!(plan.connections.iter().all(|edge| edge.fallback.is_none()));
    assert!(plan_typed_segments(&flow, false, true).segments.is_empty());
    assert!(plan_typed_segments(&flow, true, false).segments.is_empty());
}

#[test]
fn observed_outputs_end_a_segment_without_losing_a_later_chain() {
    let mut value = chain();
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"first", "node":"a", "port":"value"}));
    let (_, plan) = planned(value);
    assert_eq!(plan.segments[0].positions, [1, 2]);
    assert_eq!(
        plan.connections[0].fallback,
        Some(TypedFallbackReason::ObservedOutput)
    );
    let mut value = chain();
    value["nodes"][2]["config"] = json!({"read": true});
    let (_, plan) = planned(value);
    assert!(plan.segments.is_empty());
}

#[test]
fn optional_representation_and_refinement_evidence_remain_dynamic() {
    for (config, reason) in [
        (
            json!({"type":"optional"}),
            TypedFallbackReason::OptionalBinding,
        ),
        (
            json!({"type":"shared"}),
            TypedFallbackReason::DifferentRustType,
        ),
    ] {
        let mut value = chain();
        value["nodes"][1]["config"] = config.clone();
        value["nodes"][2]["config"] = config;
        let (_, plan) = planned(value);
        assert_eq!(plan.connections[0].fallback, Some(reason));
    }
    let mut value = chain();
    for node in value["nodes"].as_array_mut().unwrap() {
        node["config"] = json!({"type":"shared", "refine":true});
    }
    let (_, plan) = planned(value);
    assert!(plan.segments.is_empty());
    assert_eq!(
        plan.connections[0].fallback,
        Some(TypedFallbackReason::UnprovenRefinement)
    );
}

#[test]
fn fan_out_and_control_readers_prevent_moves() {
    let mut value = chain();
    value["edges"][1] =
        json!({"from_node":"a", "from_output":"value", "to_node":"c", "to_input":"value"});
    let (_, plan) = planned(value);
    assert!(plan.segments.is_empty());
    let mut value = chain();
    value["control_edges"] = json!([{"from_node":"a", "from_output":"value", "to_node":"b"}]);
    let (_, plan) = planned(value);
    assert_eq!(
        plan.connections[0].fallback,
        Some(TypedFallbackReason::ObservedOutput)
    );
}

#[test]
fn dynamic_providers_and_declaration_order_have_stable_reports() {
    let (_, expected) = planned(chain());
    let mut reordered = chain();
    reordered["nodes"].as_array_mut().unwrap().reverse();
    reordered["edges"].as_array_mut().unwrap().reverse();
    assert_eq!(planned(reordered).1, expected);
    let mut dynamic = chain();
    dynamic["nodes"][1]["config"] = json!({"dynamic":true});
    let (_, plan) = planned(dynamic);
    assert!(plan.segments.is_empty());
    assert!(
        plan.connections
            .iter()
            .all(|edge| edge.fallback == Some(TypedFallbackReason::ProviderMissing))
    );
}
