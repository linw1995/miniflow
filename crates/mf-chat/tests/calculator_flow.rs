use mf_chat::WorkflowProfile;
use mf_compiler::NodeRegistry;
use mf_runtime::{BuiltinFlowRegistry, WorkflowArguments};
use serde_json::json;

#[test]
fn agent_turn_is_provider_owned_and_keeps_the_streaming_chat_contract() {
    let builtins = BuiltinFlowRegistry::from_inventory().unwrap();
    let registration = builtins.get("mfn-llm.agent-turn").unwrap();
    let definition = (registration.definition)().unwrap();
    assert!(
        definition
            .outputs
            .iter()
            .any(|output| output.name == "answer" && output.port == "delta" && output.optional)
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    mf_compiler::compile_definition(&definition, &registry).unwrap();
}

#[test]
fn calculator_profile_loads_real_tool_bodies_without_specialized_nodes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let profile = WorkflowProfile::from_path(
        "calculator",
        root.join("examples/chat-calculator-flow.json"),
        WorkflowArguments::try_from(json!({"params.api_key":"synthetic-key"})).unwrap(),
    )
    .unwrap();
    let backend = profile.backend();
    let (plan, _) = backend
        .embedded_workflow()
        .expect("finite native Agent is composable");
    let producer = &plan.graph.children[&"tool_0".into()]["value"];
    assert!(
        producer
            .definition
            .nodes
            .iter()
            .any(|node| node.id.as_str() == "calculate")
    );
    let serialized = serde_json::to_string(&plan.graph).unwrap();
    for removed in [
        "llm.tool\"",
        "llm.tool_set",
        "llm.tool_calls",
        "llm.tool_call\"",
    ] {
        assert!(!serialized.contains(removed));
    }
}
