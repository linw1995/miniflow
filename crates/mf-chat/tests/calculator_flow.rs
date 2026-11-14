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
    let producer = &plan.graph.children[&"calculator".into()]["value"];
    assert!(
        producer
            .definition
            .nodes
            .iter()
            .any(|node| node.id.as_str() == "calculate")
    );
    assert_eq!(
        plan.graph
            .definition
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "params",
            "calculator",
            "calculator_properties",
            "tool_properties",
            "tools",
            "agent",
        ])
    );
    assert!(plan.graph.definition.edges.iter().any(|edge| {
        edge.from_node.as_str() == "tools"
            && edge.from_output == "value"
            && edge.to_node.as_str() == "agent"
            && edge.to_input == "params.tools"
    }));
    let agent = &plan.graph.children[&"agent".into()]["body"];
    let builtins = BuiltinFlowRegistry::from_inventory().unwrap();
    let registration = builtins.get("mfn-llm.agent").unwrap();
    let definition = (registration.definition)().unwrap();
    assert_eq!(
        agent
            .definition
            .nodes
            .iter()
            .map(|node| &node.id)
            .collect::<std::collections::BTreeSet<_>>(),
        definition
            .nodes
            .iter()
            .map(|node| &node.id)
            .collect::<std::collections::BTreeSet<_>>(),
        "the Agent child must keep the fixed builtin graph, not generate tool IDs"
    );
    let params = agent
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "params")
        .unwrap();
    assert_eq!(
        params.config["parameters"]["tools"]["type"],
        json!({"map":"flow"})
    );
    assert_eq!(
        params.config["parameters"]["tool_properties"]["type"],
        json!({"map":"object"})
    );
    assert!(params.config["parameters"].get("descriptions").is_none());
    assert!(
        params.config["parameters"]["tools"]
            .get("default")
            .is_none()
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
