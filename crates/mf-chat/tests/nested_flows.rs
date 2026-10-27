use mf_chat as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::WorkflowArguments;
use serde_json::json;

#[test]
fn ineligible_builtin_title_returns_no_title_and_empty_usage() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25","dependencies":{},"flows":{"title":{"builtin":"mf-chat.chat-title"}},
        "nodes":[{"id":"title","kind":"workflow.call","config":{"flow":"title"}}],
        "outputs":[{"name":"title","node":"title","port":"title","optional":true},{"name":"usages","node":"title","port":"usages"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let flow = instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap();
    let outputs = flow.execute_with_inputs(WorkflowArguments::try_from(json!({
        "title.input.prompt":"Hello", "title.input.text":"Answer", "title.input.status":"succeeded", "title.input.api_key":"synthetic-key",
        "title.input.context":{"title":{"enabled":false,"source":"provisional","attempted":false}}
    })).unwrap()).unwrap();
    assert!(!outputs.contains_key("title"));
    assert_eq!(serde_json::to_value(&outputs["usages"]).unwrap(), json!([]));
}

#[test]
fn nested_title_composition_preserves_answers_and_usage_after_provider_failure() {
    use mfn_llm::test_support::{Reply, Server};
    for failed in [false, true] {
        let mut title = if failed {
            Reply::json(json!({"error":{"message":"title failed"}}))
        } else {
            Reply::text("Generated title")
        };
        if failed {
            title.status = 500;
        }
        let server = Server::start(vec![Reply::text("Primary answer"), title]);
        let mut definition = WorkflowDefinition::from_json(include_str!(
            "../../../examples/chat-nested-title-flow.json"
        ))
        .unwrap();
        definition.outputs.retain(|output| output.name != "answer");
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let outputs = instantiate_compiled(&plan, &registry).unwrap().execute_with_inputs(WorkflowArguments::try_from(json!({
            "input.prompt":"Hello", "input.model":"example-model", "input.base_url":server.base_url, "input.api_key":"synthetic-key"
        })).unwrap()).unwrap();
        assert_eq!(outputs["text"].as_str(), Some("Primary answer"));
        assert_eq!(outputs.contains_key("title"), !failed);
        let usages = serde_json::to_value(&outputs["usages"]).unwrap();
        assert_eq!(usages.as_array().unwrap().len(), 2);
        assert_eq!(usages[1]["attribution"]["purpose"], "title");
        assert_eq!(usages[1]["node"]["path"][0]["node"], "title");
        assert_eq!(server.requests().len(), 2);
    }
}

#[test]
fn builtin_title_policy_preserves_captured_failure_usage_without_a_title() {
    for failed in [false, true] {
        let authored = WorkflowDefinition::from_json(include_str!(
            "../../../examples/chat-nested-title-flow.json"
        ))
        .unwrap();
        let mut definition = mf_compiler::resolve_flows(&authored, None)
            .unwrap()
            .definition;
        definition.outputs.retain(|output| output.name != "answer");
        let mut child: WorkflowDefinition = serde_json::from_value(
            definition
                .nodes
                .iter()
                .find(|node| node.id.as_str() == "title")
                .unwrap()
                .config["definition"]
                .clone(),
        )
        .unwrap();
        let usage = |purpose: &str| json!({"attribution":{"purpose":purpose},"model":"fixture-model","call_id":purpose});
        let replace =
            |node: &mut mf_runtime::NodeDefinition, status: &str, text: &str, purpose: &str| {
                node.kind = "builtin.code".into();
                let mut inputs = json!({"prompt":"string","system":"string"});
                if purpose == "conversation" {
                    inputs["history"] = json!({"list":"object"});
                }
                node.config = json!({
                    "language":"cel", "inputs":inputs,
                    "code":{
                        "status":serde_json::to_string(status).unwrap(),
                        "text":serde_json::to_string(text).unwrap(),
                        "usages":serde_json::to_string(&[usage(purpose)]).unwrap(),
                        "error":"null", "metadata":"null", "finish_reason":"null"
                    }
                });
            };
        replace(
            child
                .nodes
                .iter_mut()
                .find(|node| node.id.as_str() == "title_model")
                .unwrap(),
            if failed { "failed" } else { "succeeded" },
            "Generated title",
            "title",
        );
        for node in &mut definition.nodes {
            if node.id.as_str() == "model" {
                replace(node, "succeeded", "Primary answer", "conversation");
            }
            if node.id.as_str() == "title" {
                node.config["definition"] = serde_json::to_value(&child).unwrap();
            }
        }
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(
                WorkflowArguments::try_from(
                    json!({"input.prompt":"Hello","input.api_key":"synthetic-key"}),
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(outputs["text"].as_str(), Some("Primary answer"));
        assert_eq!(outputs.contains_key("title"), !failed);
        assert_eq!(
            serde_json::to_value(&outputs["usages"]).unwrap(),
            json!([usage("conversation"), usage("title")])
        );
    }
}

#[test]
fn title_definition_is_owned_and_registered_by_chat() {
    let registry = mf_runtime::BuiltinFlowRegistry::from_inventory().unwrap();
    let definition = (registry.get("mf-chat.chat-title").unwrap().definition)().unwrap();
    assert_eq!(definition, mf_chat::flows::title_definition().unwrap());
    assert!(
        definition
            .nodes
            .iter()
            .all(|node| !node.kind.starts_with("mf_chat."))
    );
}

#[test]
fn profiles_load_local_child_flows_with_host_owned_provider_paths() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("flows")).unwrap();
    let mut definition = WorkflowDefinition::from_json(include_str!(
        "../../../examples/chat-nested-title-flow.json"
    ))
    .unwrap();
    for dependency in definition.dependencies.values_mut() {
        dependency.path = Some(std::path::PathBuf::from("unused-by-host"));
    }
    definition.flows.insert(
        "title".into(),
        mf_runtime::FlowSource::Path {
            path: "flows/title.json".into(),
        },
    );
    std::fs::write(
        root.path().join("flows/title.json"),
        serde_json::to_vec(&mf_chat::flows::title_definition().unwrap()).unwrap(),
    )
    .unwrap();
    let path = root.path().join("conversation.json");
    std::fs::write(&path, serde_json::to_vec(&definition).unwrap()).unwrap();
    let profile = mf_chat::WorkflowProfile::from_path(
        "host",
        path,
        WorkflowArguments::try_from(json!({"input.api_key":"synthetic-key"})).unwrap(),
    )
    .unwrap();
    assert_eq!(profile.name, "host");
}
