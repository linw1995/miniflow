use mf_chat::ModelProfile;
use mf_chat::executable::ChatContract;
use serde_json::json;

fn manifest() -> mf_runtime::WorkflowManifest {
    let definition = mf_runtime::WorkflowDefinition::from_json(
        &json!({
            "version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"chat./~", "kind":"builtin.llm", "config":{"model":"fixture-model"}}],
            "outputs":[{"name":"text", "node":"chat./~", "port":"text"}]
        })
        .to_string(),
    )
    .unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    plan.manifest(&registry).unwrap()
}

#[test]
fn single_node_contract_uses_embedded_model_identity_and_terminal_owned_inputs() {
    let contract = ChatContract::from_manifest(&manifest()).unwrap();
    let profile = ModelProfile::new("default", json!({"model":"fixture-model"})).unwrap();
    assert_eq!(
        contract
            .model_identity("default", &Default::default())
            .unwrap(),
        *profile.identity()
    );
    let input = mfn_llm::LlmInputs {
        prompt: "new 🦀 prompt".into(),
        system: Some("be concise".into()),
        history: Some(vec![mfn_llm::ChatMessage {
            role: "user".into(),
            content: "previous".into(),
        }]),
    };
    let arguments = contract.arguments(input, &Default::default()).unwrap();
    let encoded = serde_json::to_value(&arguments).unwrap();
    assert_eq!(encoded["chat./~.prompt"], "new 🦀 prompt");
    assert_eq!(encoded["chat./~.history"][0]["content"], "previous");
    assert_eq!(encoded.as_object().unwrap().len(), 3);
}

#[test]
fn preflight_rejects_multiple_nodes_streams_and_missing_model_information() {
    let original = manifest();
    let mut changed = original.clone();
    changed.definition.nodes[0].config = json!({});
    assert!(ChatContract::from_manifest(&changed).is_err());
    changed = original.clone();
    changed.definition.nodes[0].config["model"] = json!(42);
    assert!(ChatContract::from_manifest(&changed).is_err());
    changed = original.clone();
    changed.definition.nodes[0].kind = "builtin.identity".into();
    assert!(ChatContract::from_manifest(&changed).is_err());
    changed = original.clone();
    changed
        .definition
        .nodes
        .push(serde_json::from_value(json!({"id":"extra","kind":"builtin.llm"})).unwrap());
    changed.execution_order.push("extra".into());
    assert!(ChatContract::from_manifest(&changed).is_err());
    changed = original.clone();
    changed.definition.execution = Some(serde_json::from_value(json!({"mode":"stream"})).unwrap());
    assert!(ChatContract::from_manifest(&changed).is_err());
    changed = original;
    changed
        .input_schema
        .stdin
        .insert("chat./~".into(), mf_runtime::StdinRequirement::Always);
    assert!(ChatContract::from_manifest(&changed).is_err());
}

fn parameter_manifest() -> mf_runtime::WorkflowManifest {
    assert_eq!(mfn_core::PARAMETERS_KIND, "builtin.parameters");
    let definition =
        mf_runtime::WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json"))
            .unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    plan.manifest(&registry).unwrap()
}

#[test]
fn parameterized_contract_freezes_configuration_and_owns_conversation_inputs() {
    let manifest = parameter_manifest();
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    assert!(contract.parameterized);
    let arguments = mf_runtime::WorkflowArguments::from_json(
        br#"{"params.model":"chosen-model","params.api_key":"synthetic-key"}"#,
    )
    .unwrap();
    let parameters =
        mf_runtime::resolve_workflow_inputs(&contract.parameters, &arguments, |_| None).unwrap();
    assert!(parameters.0["params.api_key"].is_secret());
    assert_eq!(
        parameters.0["params.base_url"].as_str(),
        Some("https://api.openai.com/v1")
    );
    let identity = contract.model_identity("default", &parameters).unwrap();
    assert_eq!(identity.model, "chosen-model");
    let input = mfn_llm::LlmInputs {
        prompt: "hello".into(),
        system: None,
        history: None,
    };
    let invocation = contract.arguments(input, &parameters).unwrap();
    manifest.input_schema.validate(&invocation).unwrap();
    assert_eq!(invocation.0["params.prompt"].as_str(), Some("hello"));
    assert_eq!(invocation.0["params.system"].as_str(), Some(""));
    assert_eq!(
        serde_json::to_value(&invocation).unwrap()["params.history"],
        json!([])
    );
    assert!(contract.parameters.validate(&invocation).is_err());
    let mut rotated = parameters.clone();
    rotated.0.insert(
        "params.api_key".into(),
        mf_runtime::ValueRef::from("rotated-key").into_secret(),
    );
    assert_eq!(
        contract.model_identity("default", &rotated).unwrap(),
        identity
    );
    rotated
        .0
        .insert("params.base_url".into(), "https://other.example/v1".into());
    assert_ne!(
        contract
            .model_identity("default", &rotated)
            .unwrap()
            .config_id,
        identity.config_id
    );
}

#[test]
fn parameterized_model_templates_preserve_types_and_reject_sensitive_accounting() {
    let mut manifest = parameter_manifest();
    manifest
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.kind == "builtin.llm")
        .unwrap()
        .config["model"] = json!({"$template":"prefix-{{ params.model }}"});
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    let parameters = mf_runtime::resolve_workflow_inputs(
        &contract.parameters,
        &mf_runtime::WorkflowArguments::from_json(
            br#"{"params.model":"chosen","params.api_key":"synthetic-key"}"#,
        )
        .unwrap(),
        |_| None,
    )
    .unwrap();
    assert_eq!(
        contract
            .model_identity("default", &parameters)
            .unwrap()
            .model,
        "prefix-chosen"
    );
    let mut secret = parameters.clone();
    secret.0.insert(
        "params.model".into(),
        mf_runtime::ValueRef::from("hidden-model").into_secret(),
    );
    let error = contract.model_identity("default", &secret).unwrap_err();
    assert!(!format!("{error:?}").contains("hidden-model"));
    manifest
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.kind == "builtin.llm")
        .unwrap()
        .config["model"] = json!({"$ref":"params.prompt"});
    assert!(ChatContract::from_manifest(&manifest).is_err());
    manifest = parameter_manifest();
    manifest
        .definition
        .edges
        .iter_mut()
        .find(|edge| edge.to_input == "system")
        .unwrap()
        .from_output = "prompt".into();
    assert!(ChatContract::from_manifest(&manifest).is_err());
    manifest = parameter_manifest();
    manifest.definition.edges.pop();
    assert!(ChatContract::from_manifest(&manifest).is_err());
}

#[test]
fn parameterized_contract_rejects_incomplete_or_aliased_outputs_before_execution() {
    let original = parameter_manifest();
    for port in ["text", "history", "usage", "finish_reason"] {
        let mut missing = original.clone();
        missing
            .definition
            .outputs
            .retain(|output| output.name != port);
        assert!(ChatContract::from_manifest(&missing).is_err());
        let mut aliased = original.clone();
        aliased
            .definition
            .outputs
            .iter_mut()
            .find(|output| output.name == port)
            .unwrap()
            .name = "renamed".into();
        assert!(ChatContract::from_manifest(&aliased).is_err());
    }
    let mut mistyped = original;
    mistyped
        .definition
        .outputs
        .iter_mut()
        .find(|output| output.name == "usage")
        .unwrap()
        .optional = false;
    assert!(ChatContract::from_manifest(&mistyped).is_err());
}
