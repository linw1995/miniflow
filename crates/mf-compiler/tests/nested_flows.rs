extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/controlled_source.rs"]
mod controlled;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, ValueRef, WorkflowArguments};
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn graph(nodes: Value, outputs: Value) -> WorkflowDefinition {
    serde_json::from_value(
        json!({"version":"2026-10-25", "dependencies":{}, "nodes":nodes, "outputs":outputs}),
    )
    .unwrap()
}

fn child() -> WorkflowDefinition {
    graph(
        json!([
            {"id":"input","kind":"builtin.parameters","config":{"parameters":{"value":{"type":"string","default":"fallback","secret":true}}}},
            {"id":"echo","kind":"builtin.identity","inputs":{"input":{"from":{"node":"input","port":"value"}}}}
        ]),
        json!([{"name":"value","node":"echo","port":"value"}]),
    )
}

fn call(id: &str, body: &WorkflowDefinition) -> Value {
    json!({"id":id,"kind":"workflow.call","config":{"flow":"child","definition":body}})
}

#[test]
fn calls_isolate_bindings_and_preserve_shared_secret_values() {
    let body = child();
    let mut first = call("first", &body);
    first["inputs"] = json!({"input.value":{"from":{"node":"input","port":"value"}}});
    let definition = graph(
        json!([
            {"id":"input","kind":"builtin.parameters","config":{"parameters":{"value":{"type":"string","secret":false}}}},
            first, call("second", &body),
            {"id":"after","kind":"builtin.identity","inputs":{"input":{"from":{"node":"input","port":"value"}}}}
        ]),
        json!([
            {"name":"first","node":"first","port":"value"},
            {"name":"second","node":"second","port":"value"},
            {"name":"after","node":"after","port":"value"}
        ]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let flow = instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap();
    let value = ValueRef::from("secret").into_secret();
    let outputs = flow
        .execute_with_inputs(WorkflowArguments(std::collections::BTreeMap::from([(
            "input.value".into(),
            value.clone(),
        )])))
        .unwrap();
    assert!(outputs["first"].ptr_eq(&value));
    assert!(outputs["after"].ptr_eq(&value));
    assert!(outputs["first"].is_secret());
    assert_eq!(outputs["second"].as_str(), Some("fallback"));
    assert!(outputs["second"].is_secret());
}

#[test]
fn child_secret_contract_redacts_the_call_boundary_before_observation() {
    let definition = graph(
        json!([call("call", &child())]),
        json!([{"name":"value","node":"call","port":"value"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    assert!(
        manifest.input_schema.inputs["call"]["input.value"]
            .binding
            .secret
    );
    let captures = Arc::new(Mutex::new(Vec::new()));
    let captured = captures.clone();
    let mut context = ExecutionContext::default();
    context.set_workflow_arguments(
        WorkflowArguments::try_from(json!({"call.input.value":"unmarked-credential"})).unwrap(),
    );
    context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event)
    }));
    let result = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_in_context(&mut context)
        .unwrap();
    assert_eq!(result["value"].as_str(), Some("unmarked-credential"));
    assert!(result["value"].is_secret());
    let captures = captures.lock().unwrap();
    let boundary = captures
        .iter()
        .find(|event| event.node.as_ref() == "call")
        .unwrap();
    assert_eq!(
        boundary.snapshot.inputs["input.value"].as_str(),
        Some("[REDACTED]")
    );
    assert!(
        !serde_json::to_string(&*captures)
            .unwrap()
            .contains("unmarked-credential")
    );
}

#[test]
fn nested_calls_have_distinct_lifecycle_and_redacted_capture_paths() {
    let inner = graph(
        json!([call("inner", &child())]),
        json!([{"name":"value","node":"inner","port":"value"}]),
    );
    let definition = graph(
        json!([call("outer", &inner)]),
        json!([{"name":"value","node":"outer","port":"value"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    let harness = capture::Harness::new(true);
    let observation = harness
        .observer()
        .start_with_description(description.clone(), Default::default())
        .unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let recorded = captures.clone();
    ExecutionContext::run(Some(observation), |state| {
        state.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
            recorded.lock().unwrap().push(event)
        }));
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_in_context(state)
    })
    .unwrap();
    let events: Vec<_> = harness
        .records()
        .into_iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| record.decode().unwrap())
        .collect();
    for event in &events {
        event.validate_for(&description).unwrap();
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, mf_telemetry::event::Event::CallStarted { .. }))
            .count(),
        2
    );
    assert!(!events.iter().any(|event| matches!(
        event.event,
        mf_telemetry::event::Event::LoopPassStarted { .. }
    )));
    harness.traces.force_flush().unwrap();
    assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
    let plain = capture::Harness::new(false);
    let observation = plain
        .observer()
        .start(
            description.workflow_id,
            Default::default(),
            vec![mf_runtime::NodeIdentity {
                id: "outer".into(),
                kind: "workflow.call".into(),
                path: vec![],
            }],
        )
        .unwrap();
    mf_compiler::execute_compiled(&plan, &registry, Some(observation)).unwrap();
    let plain_events: Vec<_> = plain
        .records()
        .into_iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| record.decode().unwrap())
        .collect();
    assert_eq!(
        plain_events
            .iter()
            .map(|event| event.sequence.get())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(captures.lock().unwrap().iter().any(|event| {
        event.node.as_ref() == "echo"
            && event.scope.len() == 2
            && event
                .scope
                .iter()
                .all(mf_telemetry::event::LoopPathEntry::is_call)
    }));
    assert!(
        !serde_json::to_string(&*captures.lock().unwrap())
            .unwrap()
            .contains("fallback")
    );
}

#[test]
fn local_sources_are_relative_frozen_and_cycle_checked() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("flows")).unwrap();
    fs::create_dir(root.path().join("provider")).unwrap();
    let mut body = child();
    body.dependencies.insert(
        "provider".into(),
        serde_json::from_value(json!({"package":"example-provider","path":"../provider"})).unwrap(),
    );
    let child_path = root.path().join("flows/child.json");
    fs::write(&child_path, serde_json::to_vec(&body).unwrap()).unwrap();
    let parent_path = root.path().join("parent.json");
    fs::write(&parent_path, json!({"version":"2026-10-25","dependencies":{},"flows":{"child":{"path":"flows/child.json"}},"nodes":[{"id":"call","kind":"workflow.call","config":{"flow":"child"}}],"outputs":[{"name":"value","node":"call","port":"value"}]}).to_string()).unwrap();
    let resolved = mf_compiler::load_workflow(&parent_path).unwrap();
    assert_eq!(resolved.sources.len(), 2);
    assert_eq!(
        resolved
            .definition
            .dependencies
            .values()
            .next()
            .unwrap()
            .path
            .as_ref()
            .unwrap(),
        &fs::canonicalize(root.path().join("provider")).unwrap()
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&resolved.definition, &registry).unwrap();
    fs::remove_file(&child_path).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["value"]
            .as_str(),
        Some("fallback")
    );
    fs::write(&child_path, fs::read(&parent_path).unwrap()).unwrap();
    let mut cyclic: Value = serde_json::from_slice(&fs::read(&child_path).unwrap()).unwrap();
    cyclic["flows"]["child"]["path"] = json!("../parent.json");
    fs::write(&child_path, cyclic.to_string()).unwrap();
    assert!(
        mf_compiler::load_workflow(&parent_path)
            .err()
            .unwrap()
            .to_string()
            .contains("recursive")
    );
}

#[test]
fn missing_inputs_unknown_aliases_and_incompatible_ports_fail_early() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = child();
    body.nodes[0].config["parameters"]["value"]
        .as_object_mut()
        .unwrap()
        .remove("default");
    let definition = graph(
        json!([
            {"id":"source","kind":"builtin.constant","config":{"value":1}},
            {"id":"call","kind":"workflow.call","config":{"flow":"child","definition":body},"inputs":{"input.value":{"from":{"node":"source","port":"value"}}}}
        ]),
        json!([]),
    );
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("string"), "{error}");
    let unknown = graph(
        json!([{"id":"call","kind":"workflow.call","config":{"flow":"missing"}}]),
        json!([]),
    );
    assert!(
        compile_definition(&unknown, &registry)
            .unwrap_err()
            .to_string()
            .contains("unknown Flow alias")
    );
    let mut invalid = graph(
        json!([call("call", &body), {"id":"trigger","kind":"builtin.constant","config":{"value":0}}]),
        json!([]),
    );
    invalid.order_edges.push(mf_runtime::OrderEdgeDefinition {
        from_node: "trigger".into(),
        to_node: "call".into(),
    });
    let error = compile_definition(&invalid, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("input.value"), "{error}");
}

#[test]
fn call_rejects_stdin_and_streaming_children() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let body = graph(
        json!([{"id":"read","kind":"builtin.readline"}]),
        json!([{"name":"line","node":"read","port":"line"}]),
    );
    let definition = graph(json!([call("call", &body)]), json!([]));
    assert!(compile_definition(&definition, &registry).is_err());
}

#[test]
fn ineligible_builtin_title_returns_no_title_and_empty_usage() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25","dependencies":{},"flows":{"title":{"builtin":"chat-title"}},
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
                node.kind = "fixture.context".into();
                node.config = json!({
                    "inputs":["prompt","system","history"],
                    "ports":["status","text","usages","error","metadata","finish_reason"],
                    "outputs":{"status":status,"text":text,"usages":[usage(purpose)]},
                    "skipped":["error","metadata","finish_reason"]
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
fn provider_features_merge_and_source_conflicts_are_rejected() {
    let mut body = child();
    body.dependencies.insert("child_provider".into(), serde_json::from_value(json!({"package":"example-provider","version":"1.0","features":["child"],"default-features":false})).unwrap());
    let mut definition = graph(json!([call("call", &body)]), json!([]));
    definition.dependencies.insert(
        "parent_provider".into(),
        serde_json::from_value(
            json!({"package":"example-provider","version":"1.0","features":["parent"]}),
        )
        .unwrap(),
    );
    let resolved = mf_compiler::resolve_flows(&definition, None).unwrap();
    let dependency = &resolved.definition.dependencies["parent_provider"];
    assert_eq!(dependency.features, ["child", "parent"]);
    assert!(dependency.default_features);
    body.dependencies.get_mut("child_provider").unwrap().version = Some("2.0".into());
    definition.nodes[0].config["definition"] = serde_json::to_value(body).unwrap();
    assert!(
        mf_compiler::resolve_flows(&definition, None)
            .err()
            .unwrap()
            .to_string()
            .contains("conflicting provider sources")
    );
}

#[test]
fn generated_calls_match_memory_without_flow_sources() {
    let root = tempfile::tempdir().unwrap();
    let mut body = child();
    body.nodes[0].config["parameters"]["value"]["secret"] = json!(false);
    let inner = graph(
        json!([call("inner", &body)]),
        json!([{"name":"value","node":"inner","port":"value"}]),
    );
    let mut definition = graph(
        json!([call("outer", &inner)]),
        json!([{"name":"value","node":"outer","port":"value"}]),
    );
    definition.dependencies.insert(
        "core".into(),
        serde_json::from_value(
            json!({"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")}),
        )
        .unwrap(),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let expected = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    let project = root.path().join("build");
    mf_compiler::write_dependency_project(
        &project,
        &plan,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let build = mf_compiler::cargo_command(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let output = std::process::Command::new(common::runner_executable(&project, "debug"))
        .current_dir(root.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual, serde_json::to_value(expected).unwrap());
}

#[test]
fn calls_inside_loops_share_the_callers_step_budget_and_cancellation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = child();
    for index in 0..12 {
        body.nodes.push(serde_json::from_value(json!({"id":format!("extra_{index}"),"kind":"builtin.constant","config":{"value":0}})).unwrap());
    }
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25","dependencies":{},
        "nodes":[{"id":"repeat","kind":"workflow.loop","loop":{
            "max_iterations":1000,"variables":[{"name":"value","type":"string"}],
            "body":{"nodes":[call("call", &body)]}
        }}],
        "outputs":[{"name":"value","node":"repeat","port":"value"}]
    }))
    .unwrap();
    let flow = instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap();
    let error = flow
        .execute_with_inputs(WorkflowArguments::try_from(json!({"repeat.value":"seed"})).unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("budget"), "{error}");
    let mut context = ExecutionContext::default();
    context
        .cancellation()
        .cancel(mf_runtime::StreamError::Execution {
            message: "cancelled before nested work".into(),
        });
    context.set_workflow_arguments(
        WorkflowArguments::try_from(json!({"repeat.value":"seed"})).unwrap(),
    );
    assert!(flow.execute_in_context(&mut context).is_err());
}

#[test]
fn stream_messages_invoke_terminal_children_with_correlated_lifecycle() {
    let mut body = child();
    body.nodes[0].config["parameters"]["value"]["secret"] = json!(false);
    let mut invocation = call("call", &body);
    invocation["inputs"] = json!({"input.value":{"from":{"node":"feed","port":"item"}}});
    let definition = graph(
        json!([controlled::source(json!("string")), invocation]),
        json!([{"name":"value","node":"call","port":"value"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), Default::default())
        .unwrap();
    let run = controlled::SourceRun::start(
        mf_compiler::instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        mf_runtime::StreamOptions {
            observation: Some(observation),
            ..Default::default()
        },
    )
    .unwrap();
    run.source.send(json!("first")).unwrap();
    run.source.send(json!("second")).unwrap();
    run.source.close();
    let mut outputs = Vec::new();
    while let Some(output) = run.recv().unwrap() {
        outputs.push(output.outputs["value"].as_str().unwrap().to_owned());
    }
    run.join().unwrap();
    assert_eq!(outputs, ["first", "second"]);
    let records: Vec<_> = harness
        .records()
        .into_iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
        .map(|record| mf_telemetry::stream::StreamRecord::decode(&record).unwrap())
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(
                &record.payload,
                mf_telemetry::stream::StreamPayload::Execution(
                    mf_telemetry::event::Event::CallStarted { .. }
                )
            ))
            .count(),
        2,
        "{records:#?}"
    );
    for record in &records {
        record.validate().unwrap();
        if matches!(
            &record.payload,
            mf_telemetry::stream::StreamPayload::Execution(
                mf_telemetry::event::Event::CallStarted { .. }
            )
        ) {
            assert!(
                record
                    .identity
                    .as_ref()
                    .is_some_and(|identity| identity.message.is_some())
            );
        }
    }
    harness.traces.force_flush().unwrap();
    assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
}
