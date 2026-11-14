use mf_chat::WorkflowProfile;
use mf_compiler::NodeRegistry;
use mf_runtime::{BuiltinFlowRegistry, WorkflowArguments};
use serde_json::json;

#[test]
fn custom_flow_rewrites_complete_context_and_reopened_next_provider_receives_only_that_version() {
    use mf_chat::{ChatDatabase, ModelIdentity};
    use mfn_llm::test_support::{Reply, Server};
    let server = Server::start(vec![
        Reply::text("first reply"),
        Reply::text("second reply"),
    ]);
    // An ordinary custom Flow: consume generic messages, call a model, return a compacted context.
    // No persistence counts, source ordinals, or Agent-specific append protocol are needed.
    let definition = serde_json::from_value(json!({
        "version":"2026-11-13", "dependencies":{},
        "nodes":[
            {"id":"params","kind":"builtin.parameters","config":{"parameters":{
                "messages":{"type":{"list":"object"}},
                "model":{"type":"string","default":"example-model","secret":false},
                "base_url":{"type":"string","secret":false},"api_key":{"type":"string"}
            }}},
            {"id":"model","kind":"builtin.llm","config":{
                "model":{"$ref":"params.model"},"base_url":{"$ref":"params.base_url"},"api_key":{"$ref":"params.api_key"}
            },"inputs":{"messages":{"from":{"node":"params","port":"messages"}}}},
            {"id":"compact","kind":"builtin.code","config":{
                "language":"cel","inputs":{"reply":"object"},
                "code":{"messages":"[dyn({'role':'system','content':'Replacement instruction'}), dyn({'role':'assistant','content':'Compacted summary'}), reply]"}
            },"inputs":{"reply":{"from":{"node":"model","port":"message"}}}}
        ],
        "outputs":[
            {"name":"status","node":"model","port":"status"},
            {"name":"text","node":"model","port":"text"},
            {"name":"usages","node":"model","port":"usages"},
            {"name":"messages","node":"compact","port":"messages"}
        ]
    })).unwrap();
    let profile = WorkflowProfile::new(
        "custom",
        definition,
        WorkflowArguments::try_from(json!({
            "params.base_url":server.base_url, "params.api_key":"synthetic-key"
        }))
        .unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let database = ChatDatabase::open(&path).unwrap();
    let session = database
        .create_session("custom", Some("Old instruction".into()), None)
        .unwrap();
    let model = ModelIdentity {
        profile: "custom".into(),
        model: Some("example-model".into()),
    };
    let first = database
        .begin_turn(&session.id, "Old prompt", model.clone(), None)
        .unwrap();
    let reply = profile
        .backend()
        .invoke(
            &first.invocation,
            first.request,
            &mut mf_runtime::ExecutionContext::default(),
            None,
            None,
        )
        .unwrap();
    let first_context = reply.messages.clone();
    database
        .complete_turn(first.invocation.clone(), reply)
        .unwrap();
    drop(database);
    let database = ChatDatabase::open(&path).unwrap();
    let second = database
        .begin_turn(&session.id, "New prompt", model, None)
        .unwrap();
    let reply = profile
        .backend()
        .invoke(
            &second.invocation,
            second.request,
            &mut mf_runtime::ExecutionContext::default(),
            None,
            None,
        )
        .unwrap();
    database.complete_turn(second.invocation, reply).unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].body["messages"],
        json!([
            {"role":"system","content":[{"type":"text","text":"Replacement instruction"}]},
            {"role":"assistant","content":[{"type":"text","text":"Compacted summary"}]},
            {"role":"assistant","content":[{"type":"text","text":"first reply"}]},
            {"role":"user","content":"New prompt"}
        ])
    );
    assert!(!requests[1].body.to_string().contains("Old prompt"));
    assert!(!requests[1].body.to_string().contains("Old instruction"));
    assert_eq!(
        database
            .conversation_messages(&first.invocation.call_id)
            .unwrap(),
        Some(first_context)
    );
    let current = database.history(&session.id).unwrap();
    assert!(current.system.is_none());
    assert_eq!(current.history.len(), 3);
    assert!(
        !serde_json::to_string(&current.history)
            .unwrap()
            .contains("first reply")
    );
}

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
