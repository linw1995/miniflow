use mf_chat::contract::ChatContract;
use mf_runtime::{NodeValue, WorkflowArguments, WorkflowManifest};
use serde_json::json;

fn manifest() -> WorkflowManifest {
    assert_eq!(mfn_core::PARAMETERS_KIND, "builtin.parameters");
    let definition =
        mf_runtime::WorkflowDefinition::from_json(include_str!("../../../examples/chat-flow.json"))
            .unwrap();
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    mf_compiler::compile_definition(&definition, &registry)
        .unwrap()
        .manifest(&registry)
        .unwrap()
}

fn parameters(contract: &ChatContract) -> WorkflowArguments {
    mf_runtime::resolve_workflow_inputs(
        &contract.parameters,
        &WorkflowArguments::from_json(
            br#"{"params.model":"chosen-model","params.api_key":"synthetic-key"}"#,
        )
        .unwrap(),
        |_| None,
    )
    .unwrap()
}

#[test]
fn conversation_inputs_are_bound_separately_from_parameters() {
    let manifest = manifest();
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    let parameters = parameters(&contract);
    let input = mf_chat::ConversationInputs {
        context: None,
        prompt: "hello".into(),
        system: None,
        history: None,
    };
    let arguments = contract.arguments(input, &parameters).unwrap();
    manifest.input_schema.validate(&arguments).unwrap();
    assert_eq!(arguments.0["params.prompt"].as_str(), Some("hello"));
    assert_eq!(arguments.0["params.system"].as_str(), Some(""));
    assert_eq!(
        serde_json::to_value(&arguments).unwrap()["params.history"],
        json!([])
    );
    assert!(
        contract
            .arguments(
                mf_chat::ConversationInputs {
                    context: None,
                    prompt: "override".into(),
                    system: None,
                    history: None
                },
                &arguments
            )
            .is_err()
    );
}

#[test]
fn contract_accepts_multiple_models_and_arbitrary_output_sources() {
    let mut manifest = manifest();
    let mut second = manifest.definition.nodes[1].clone();
    second.id = "another-model".into();
    manifest.definition.nodes.push(second);
    manifest.execution_order.push("another-model".into());
    for mut edge in manifest.definition.edges.clone() {
        edge.to_node = "another-model".into();
        manifest.definition.edges.push(edge);
    }
    manifest
        .definition
        .outputs
        .iter_mut()
        .find(|output| output.name == "text")
        .unwrap()
        .node = "another-model".into();
    ChatContract::from_manifest(&manifest).unwrap();
    manifest.definition.nodes[1].kind = "custom.generator".into();
    manifest.definition.nodes[1].config = json!({});
    ChatContract::from_manifest(&manifest).unwrap();
}

#[test]
fn preflight_checks_transport_and_external_ports() {
    let original = manifest();
    let mut wrong_type = original.clone();
    wrong_type
        .input_schema
        .inputs
        .get_mut("params")
        .unwrap()
        .get_mut("prompt")
        .unwrap()
        .value_type = mf_runtime::ValueType::Boolean;
    let mut missing = original.clone();
    missing
        .input_schema
        .inputs
        .get_mut("params")
        .unwrap()
        .remove("prompt");
    let mut ambiguous = original.clone();
    let mut source = ambiguous.definition.nodes[0].clone();
    source.id = "other".into();
    ambiguous.definition.nodes.push(source);
    ambiguous.execution_order.insert(0, "other".into());
    ambiguous.input_schema.inputs.insert(
        "other".into(),
        original.input_schema.inputs["params"].clone(),
    );
    let mut stdin = original.clone();
    stdin
        .input_schema
        .stdin
        .insert("params".into(), mf_runtime::StdinRequirement::Always);
    let mut streaming = original.clone();
    streaming.definition.execution = Some(serde_json::from_value(json!({})).unwrap());
    assert!(ChatContract::from_manifest(&streaming).unwrap().streaming);
    assert!(!ChatContract::from_manifest(&original).unwrap().streaming);
    for invalid in [wrong_type, missing, ambiguous, stdin] {
        assert!(ChatContract::from_manifest(&invalid).is_err());
    }
    for name in ["status", "text", "usages"] {
        let mut invalid = original.clone();
        invalid
            .definition
            .outputs
            .retain(|output| output.name != name);
        assert!(ChatContract::from_manifest(&invalid).is_err());
    }
    let mut optional = original;
    optional
        .definition
        .outputs
        .iter_mut()
        .find(|output| output.name == "usages")
        .unwrap()
        .optional = true;
    assert!(ChatContract::from_manifest(&optional).is_err());
}

#[test]
fn history_metadata_round_trips_through_argument_binding() {
    let contract = ChatContract::from_manifest(&manifest()).unwrap();
    let metadata = std::collections::BTreeMap::from([(
        "citations".into(),
        json!([{"uri":"https://example.com/reference"}]).into(),
    )]);
    let arguments = contract
        .arguments(
            mf_chat::ConversationInputs {
                context: None,
                prompt: "continue".into(),
                system: None,
                history: Some(vec![mfn_llm::ChatMessage {
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning: None,
                    role: "assistant".into(),
                    content: "prior reply".into(),
                    metadata: Some(metadata.clone()),
                }]),
            },
            &parameters(&contract),
        )
        .unwrap();
    let values = mf_runtime::NodeValues::from([
        ("prompt".into(), arguments.0["params.prompt"].clone()),
        ("history".into(), arguments.0["params.history"].clone()),
    ]);
    assert_eq!(
        mfn_llm::LlmInputs::from_values(values)
            .unwrap()
            .history
            .unwrap()[0]
            .metadata,
        Some(metadata)
    );
}

#[test]
fn workflow_explicitly_combines_multiple_model_calls_on_success_and_failure() {
    use mfn_llm::test_support::{Reply, Server};
    for (fail_first, fail_second) in [(false, false), (true, false), (false, true)] {
        let mut first_reply = Reply::text("first reply");
        if fail_first {
            first_reply = Reply::json(json!({"error":{"message":"unavailable"}}));
            first_reply.status = 503;
        }
        let first = Server::start(vec![first_reply]);
        let mut response = Reply::text("second reply");
        if fail_second {
            response = Reply::json(json!({"error":{"message":"unavailable"}}));
            response.status = 503;
        }
        let second = Server::start(vec![response]);
        let mut definition = mf_runtime::WorkflowDefinition::from_json(include_str!(
            "../../../examples/chat-flow.json"
        ))
        .unwrap();
        let mut extra = definition.nodes[1].clone();
        extra.id = "first".into();
        extra.config["model"] = json!("first-model");
        extra.config["base_url"] = json!(first.base_url);
        definition.nodes[1].config["base_url"] = json!(second.base_url);
        definition.nodes.push(extra);
        for mut edge in definition.edges.clone() {
            edge.to_node = "first".into();
            definition.edges.push(edge);
        }
        definition.nodes.push(
            serde_json::from_value(json!({"id":"collect","kind":mfn_code::KIND,"config":{
                "language":"cel",
                "inputs":{"left":{"list":"object"},"right":{"list":"object"}},
                "code":{"value":"left + right"}
            }}))
            .unwrap(),
        );
        for (node, input) in [("first", "left"), ("chat", "right")] {
            definition.edges.push(serde_json::from_value(json!({"from_node":node,"from_output":"usages","to_node":"collect","to_input":input})).unwrap());
        }
        let usages = definition
            .outputs
            .iter_mut()
            .find(|output| output.name == "usages")
            .unwrap();
        usages.node = "collect".into();
        usages.port = "value".into();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
        let manifest = plan.manifest(&registry).unwrap();
        let contract = ChatContract::from_manifest(&manifest).unwrap();
        let root = tempfile::tempdir().unwrap();
        let database = mf_chat::ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        let session = database.create_session("default", None, None).unwrap();
        let parameters = parameters(&contract);
        let begun = database
            .begin_turn(
                &session.id,
                "hello",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                1024,
            )
            .unwrap();
        let arguments = contract.arguments(begun.request, &parameters).unwrap();
        let outputs = mf_runtime::FlowRuntime::default()
            .execute_with_inputs(
                &mf_compiler::instantiate_compiled(&plan, &registry).unwrap(),
                arguments,
            )
            .unwrap();
        let output = serde_json::to_value(&outputs).unwrap();
        let calls = output["usages"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["node"]["id"], "first");
        assert_eq!(calls[1]["node"]["id"], "chat");
        assert_ne!(calls[0]["call_id"], calls[1]["call_id"]);
        let bytes = serde_json::to_vec(&outputs).unwrap();
        let resolution = database
            .reconcile_completion(mf_chat::contract::ConversationExit {
                invocation: &begun.invocation,
                stdout: Some(&bytes),
                outcome: mf_chat::contract::TurnExit::Failed,
                summary: "workflow finished",
            })
            .unwrap();
        if fail_second {
            assert!(
                matches!(resolution.state, mf_chat::contract::CompletionState::Finished(turn) if turn.state == "failed")
            );
        } else {
            let mf_chat::contract::CompletionState::Uncommitted(reply) = resolution.state else {
                panic!("expected unsaved reply");
            };
            database.save_recovered(reply).unwrap();
        }
        let usage = database.usage(Default::default()).unwrap().overall;
        assert_eq!(usage.attempted_calls, 2);
        assert_eq!(
            usage.statuses["failed"],
            i64::from(fail_first) + i64::from(fail_second)
        );
        assert_eq!(first.requests().len(), 1);
        assert_eq!(second.requests().len(), 1);
    }
}

#[test]
fn loop_explicitly_accumulates_call_usages_and_stops_after_failure() {
    use mfn_llm::test_support::{Reply, Server};
    for failed in [false, true] {
        let mut response = Reply::text("reply");
        if failed {
            response = Reply::json(json!({"error":{"message":"unavailable"}}));
            response.status = 503;
        }
        let server = Server::start(vec![response, Reply::text("second reply")]);
        let definition = mf_runtime::WorkflowDefinition::from_json(include_str!(
            "../../../examples/chat-loop-flow.json"
        ))
        .unwrap();
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
        let contract = ChatContract::from_manifest(&plan.manifest(&registry).unwrap()).unwrap();
        let mut parameters = parameters(&contract);
        parameters
            .0
            .insert("params.base_url".into(), server.base_url.clone().into());
        let input = mf_chat::ConversationInputs {
            context: None,
            prompt: "hello".into(),
            system: None,
            history: None,
        };
        let outputs = mf_compiler::execute_compiled_with_inputs(
            &plan,
            &registry,
            contract.arguments(input, &parameters).unwrap(),
            None,
        )
        .unwrap();
        let result = serde_json::to_value(&outputs).unwrap();
        let calls = result["usages"].as_array().unwrap();
        assert_eq!(calls.len(), if failed { 1 } else { 2 });
        assert_eq!(server.requests().len(), calls.len());
        for (index, call) in calls.iter().enumerate() {
            assert_eq!(call["node"]["id"], "chat");
            assert_eq!(
                call["node"]["path"],
                json!([{"node":"repeat","index":index}])
            );
        }
        if failed {
            assert_eq!(result["status"], "failed");
            assert!(!result["error"].as_str().unwrap().is_empty());
        } else {
            assert_ne!(calls[0]["call_id"], calls[1]["call_id"]);
            assert_eq!(result["metadata"]["strategy"], "iterative");
        }
    }
}

#[test]
fn application_context_is_bound_by_the_common_contract_and_optional_titles_are_persisted() {
    let root = tempfile::tempdir().unwrap();
    let database = mf_chat::ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
    let session = database.create_session("default", None, None).unwrap();
    let started = database
        .begin_turn(
            &session.id,
            "hello",
            mf_chat::ModelIdentity {
                profile: "default".into(),
                model: None,
            },
            None,
            1024,
        )
        .unwrap();
    let mut definition = manifest().definition;
    definition.nodes[0].config["parameters"]["context"] = json!({"type":"object"});
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let manifest = mf_compiler::compile_definition(&definition, &registry)
        .unwrap()
        .manifest(&registry)
        .unwrap();
    let contract = ChatContract::from_manifest(&manifest).unwrap();
    assert!(!contract.parameters.inputs["params"].contains_key("context"));
    let context = started.request.context.clone().unwrap();
    let encoded = serde_json::to_vec(&context).unwrap();
    let decoded: mf_chat::ConversationContext = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, context);
    let arguments = contract
        .arguments(started.request.clone(), &parameters(&contract))
        .unwrap();
    manifest.input_schema.validate(&arguments).unwrap();
    let context = serde_json::to_value(&arguments).unwrap()["params.context"].clone();
    assert_eq!(
        context["title"],
        json!({"source":"provisional","revision":0,"attempted":false,"enabled":true})
    );
    let bytes = serde_json::to_vec(&json!({
        "status":"succeeded","text":"answer","title":"Generated","usages":[{
            "call_id":"title-attempt","model":"summary-model",
            "attribution":{"purpose":"title","profile":"summary"}
        }]
    }))
    .unwrap();
    let resolution = database
        .reconcile_completion(mf_chat::contract::ConversationExit {
            invocation: &started.invocation,
            stdout: Some(&bytes),
            outcome: mf_chat::contract::TurnExit::Failed,
            summary: "workflow exited",
        })
        .unwrap();
    let mf_chat::contract::CompletionState::Uncommitted(pending) = resolution.state else {
        panic!("expected a complete retained result");
    };
    assert_eq!(
        pending.completion.result.title.as_deref(),
        Some("Generated")
    );
    database.save_recovered(pending.clone()).unwrap();
    database.save_recovered(pending).unwrap();
    assert_eq!(database.session(&session.id).unwrap().title, "Generated");
    assert_eq!(
        database
            .usage(mf_chat::UsageQuery {
                purpose: Some(mf_chat::CallPurpose::Title),
                ..Default::default()
            })
            .unwrap()
            .overall
            .attempted_calls,
        1
    );
}

#[test]
fn common_results_preserve_sparse_fields_across_typed_and_json_transports() {
    let result = mf_chat::ConversationResult {
        text: "answer".into(),
        usages: vec![mf_chat::CallUsage {
            timing: None,
            call_id: "call".into(),
            model: "model".into(),
            started_at: None,
            node: None,
            attribution: Some(mf_chat::UsageAttribution {
                purpose: Some("title".into()),
                profile: None,
            }),
            usage: None,
            error: None,
        }],
        ..Default::default()
    };
    let bytes = serde_json::to_vec(&result).unwrap();
    let values = mf_runtime::decode_unique_json(&bytes).unwrap();
    assert_eq!(mf_chat::ConversationResult::decode(values).unwrap(), result);
    assert_eq!(
        mf_chat::ConversationResult::decode(result.clone().into_values().unwrap()).unwrap(),
        result
    );
}
