extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/controlled_source.rs"]
mod controlled;

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

fn iteration_call_graph(body: &WorkflowDefinition, mode: &str) -> WorkflowDefinition {
    let mut invocation = call("invoke", body);
    invocation["inputs"] =
        json!({"inner.input.value":{"from":{"node":"%iteration","port":"item"}}});
    graph(
        json!([
            {"id":"items","kind":"builtin.constant","config":{"value":["first","second"]}},
            {"id":"each","kind":"builtin.iteration","config":{"mode":mode,"body":{
                "nodes":[invocation],"result":{"node":"invoke","port":"value"}
            }},"inputs":{"items":{"from":{"node":"items","port":"value"}}}}
        ]),
        json!([{"name":"results","node":"each","port":"results"}]),
    )
}

#[test]
fn iteration_calls_isolate_item_bindings_and_observe_nested_skips() {
    let mut leaf = child();
    leaf.nodes.extend(serde_json::from_value::<Vec<mf_runtime::NodeDefinition>>(json!([
        {"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"yes","condition":{"source":{"output":"input.value","path":""},"operator":"eq","value":"first"}}]}},
        {"id":"gated","kind":"builtin.identity"}
    ])).unwrap());
    leaf.edges.push(mf_runtime::EdgeDefinition {
        exclusive: false,
        from_node: "input".into(),
        from_output: "value".into(),
        to_node: "gated".into(),
        to_input: "input".into(),
    });
    leaf.order_edges.push(mf_runtime::OrderEdgeDefinition {
        from_node: "input".into(),
        to_node: "route".into(),
    });
    leaf.control_edges.push(mf_runtime::ControlEdgeDefinition {
        from_node: "route".into(),
        from_output: "yes".into(),
        to_node: "gated".into(),
    });
    leaf.outputs.push(mf_runtime::WorkflowOutputDefinition {
        name: "optional".into(),
        node: "gated".into(),
        port: "value".into(),
        optional: true,
    });
    let body = graph(
        json!([call("inner", &leaf)]),
        json!([{"name":"value","node":"inner","port":"value"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    for mode in ["sequential", "parallel"] {
        let definition = iteration_call_graph(&body, mode);
        let plan = compile_definition(&definition, &registry).unwrap();
        let harness = capture::Harness::new(true);
        let observation = plan
            .start_observation(&harness.observer(), Default::default())
            .unwrap();
        let output = mf_compiler::execute_compiled(&plan, &registry, Some(observation)).unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            json!({"results":["first","second"]})
        );
        let records = harness.records();
        assert!(
            records
                .iter()
                .any(|record| record.event_name == "mf.call.node.skipped")
        );
        let scope = records
            .iter()
            .find(|record| record.event_name == "mf.call.node.started")
            .unwrap()
            .attributes["mf.scope.path"]
            .as_str()
            .unwrap();
        assert!(scope.contains("invoke"));
        harness.traces.force_flush().unwrap();
        assert!(
            harness
                .spans
                .get_finished_spans()
                .unwrap()
                .iter()
                .any(|span| span.name == "mf.call.node")
        );
    }
}

#[test]
fn iteration_calls_cannot_hide_loop_containers_from_either_preparation_path() {
    let loop_body: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25","dependencies":{},
        "nodes":[{"id":"repeat","kind":"workflow.loop","loop":{"max_iterations":1,"variables":[{"name":"value","type":"string"}],"body":{"nodes":[]}}}],
        "outputs":[{"name":"value","node":"repeat","port":"value"}]
    })).unwrap();
    let body = graph(
        json!([call("inner", &loop_body)]),
        json!([{"name":"value","node":"inner","port":"value"}]),
    );
    let definition = iteration_call_graph(&body, "sequential");
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(
        compile_definition(&definition, &registry)
            .unwrap_err()
            .to_string()
            .contains("cannot contain Loop or Iteration")
    );
    assert!(
        mf_compiler::plan_definition(&definition)
            .unwrap_err()
            .to_string()
            .contains("cannot contain Loop or Iteration")
    );
}

fn builtin_echo() -> Result<WorkflowDefinition, mf_runtime::DefinitionParseError> {
    let mut definition = child();
    definition.dependencies.insert(
        "provider".into(),
        serde_json::from_value(
            json!({"package":"test-provider","version":"=1","features":["required"]}),
        )
        .unwrap(),
    );
    Ok(definition)
}

mf_runtime::register_builtin_flow!("echo", builtin_echo);

#[test]
fn builtin_sources_are_resolved_from_linked_providers() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-25", "dependencies":{"provider":{"package":"test-provider","version":"=2","features":["selected"],"default-features":false}},
        "flows":{"echo":{"builtin":"mf-compiler.echo"}},
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"echo"}}],
        "outputs":[{"name":"value","node":"invoke","port":"value"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(
        plan.definition.dependencies["provider"].version.as_deref(),
        Some("=2")
    );
    assert_eq!(
        plan.definition.dependencies["provider"].features,
        ["required", "selected"]
    );
    assert!(plan.definition.dependencies["provider"].default_features);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["value"]
            .as_str(),
        Some("fallback")
    );
    let mut unknown = definition;
    unknown.flows.insert(
        "echo".into(),
        mf_runtime::FlowSource::Builtin {
            builtin: "missing".into(),
        },
    );
    assert!(
        compile_definition(&unknown, &registry)
            .unwrap_err()
            .to_string()
            .contains("unknown built-in Flow")
    );
}

#[test]
fn source_resolution_rejects_unsupported_versions_names_depth_and_stream_modes() {
    let base = graph(json!([call("invoke", &child())]), json!([]));
    let mut old_sources = base.clone();
    old_sources.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_23;
    old_sources.flows.insert(
        "echo".into(),
        mf_runtime::FlowSource::Builtin {
            builtin: "mf-compiler.echo".into(),
        },
    );
    assert!(
        mf_compiler::resolve_flows(&old_sources, None)
            .err()
            .unwrap()
            .to_string()
            .contains("Flow sources require")
    );
    let mut old_call = base.clone();
    old_call.version = mf_runtime::WorkflowDefinitionVersion::V2026_10_23;
    assert!(
        mf_compiler::resolve_flows(&old_call, None)
            .err()
            .unwrap()
            .to_string()
            .contains("Flow calls require")
    );
    assert!(
        serde_json::to_value(&old_call)
            .unwrap_err()
            .to_string()
            .contains("nested flows require")
    );
    let mut old_wire = serde_json::to_value(&base).unwrap();
    old_wire["version"] = json!("2026-10-23");
    assert!(
        serde_json::from_value::<WorkflowDefinition>(old_wire)
            .unwrap_err()
            .to_string()
            .contains("nested flows require")
    );
    let mut blank_source = base.clone();
    blank_source.flows.insert(
        " ".into(),
        mf_runtime::FlowSource::Builtin {
            builtin: "mf-compiler.echo".into(),
        },
    );
    let mut blank_call = base.clone();
    blank_call.nodes[0].config["flow"] = json!("");
    for definition in [blank_source, blank_call] {
        assert!(
            mf_compiler::resolve_flows(&definition, None)
                .err()
                .unwrap()
                .to_string()
                .contains("must not be blank")
        );
    }
    let mut deep = child();
    for _ in 0..=mf_runtime::MAX_CALL_DEPTH {
        deep = graph(json!([call("invoke", &deep)]), json!([]));
    }
    assert!(
        mf_compiler::resolve_flows(&deep, None)
            .err()
            .unwrap()
            .to_string()
            .contains("exceeds sixteen")
    );
    let mut streaming_child = child();
    streaming_child.execution = Some(Default::default());
    let streaming = graph(json!([call("invoke", &streaming_child)]), json!([]));
    assert!(
        mf_compiler::resolve_flows(&streaming, None)
            .err()
            .unwrap()
            .to_string()
            .contains("terminal child interface")
    );
    let delta_child = graph(
        json!([{"id":"model","kind":"builtin.llm","config":{"model":"fixture-model","api_key_env":"MF_NESTED_TEST_KEY"}}]),
        json!([{"name":"delta","node":"model","port":"delta"}]),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let error = compile_definition(
        &graph(json!([call("invoke", &delta_child)]), json!([])),
        &registry,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("terminal child ports"),
        "{error}"
    );
}

#[test]
fn frozen_plans_and_manifests_reject_missing_corrupt_or_cyclic_call_bodies() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(
        &graph(json!([call("invoke", &child())]), json!([])),
        &registry,
    )
    .unwrap();
    for config in [json!({"flow":1}), json!({"flow":"child"})] {
        let mut corrupt = plan.clone();
        corrupt.definition.nodes[0].config = config;
        assert!(instantiate_compiled(&corrupt, &registry).is_err());
        assert!(corrupt.generate_runner_artifacts().is_err());
    }
    let mut manifest = plan.manifest(&registry).unwrap();
    manifest.definition.nodes[0]
        .config
        .as_object_mut()
        .unwrap()
        .remove("definition");
    assert!(manifest.validate().is_err());
    let mut cycle = child();
    cycle.edges.push(mf_runtime::EdgeDefinition {
        exclusive: false,
        from_node: "echo".into(),
        from_output: "value".into(),
        to_node: "input".into(),
        to_input: "value".into(),
    });
    let mut invalid = plan;
    invalid.definition.nodes[0].config["definition"] = serde_json::to_value(cycle).unwrap();
    assert!(
        invalid
            .generate_runner_artifacts()
            .unwrap_err()
            .to_string()
            .contains("Flow call")
    );
}

#[test]
fn compilation_cannot_replace_referenced_sources_with_outputs_or_locks() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent.json");
    let bytes = serde_json::to_vec(&child()).unwrap();
    for name in ["child.json", "parent.lock"] {
        let child_path = root.path().join(name);
        fs::write(&child_path, &bytes).unwrap();
        fs::write(&parent,json!({"version":"2026-10-25","dependencies":{},"flows":{"child":{"path":name}},"nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"child"}}]}).to_string()).unwrap();
        let output = if name == "child.json" {
            child_path.clone()
        } else {
            root.path().join("runner")
        };
        let request = mf_compiler::CompileRequest {
            definition: &parent,
            output: &output,
            locked: false,
            build_dir: None,
            support: &mf_compiler::SupportPackages::Registry,
        };
        let error = mf_compiler::compile_project(&request).unwrap_err();
        assert_eq!(error.stage, "output validation");
        assert_eq!(fs::read(&child_path).unwrap(), bytes);
        assert!(!root.path().join("parent.lock.guard").exists());
    }
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
fn exclusive_call_inputs_retain_child_sensitivity_after_branch_convergence() {
    let mut invocation = call("invoke", &child());
    invocation["inputs"] = json!({"input.value":{"exclusive":[{"node":"left","port":"value"},{"node":"right","port":"value"}]}});
    let mut definition = graph(
        json!([
            {"id":"which","kind":"builtin.constant","config":{"value":true}},
            {"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"yes","condition":{"source":{"output":"which.value","path":""},"operator":"eq","value":true}}]}},
            {"id":"left","kind":"builtin.constant","config":{"value":"credential"}},
            {"id":"right","kind":"builtin.constant","config":{"value":"other"}}, invocation
        ]),
        json!([{"name":"value","node":"invoke","port":"value"}]),
    );
    definition
        .order_edges
        .push(mf_runtime::OrderEdgeDefinition {
            from_node: "which".into(),
            to_node: "route".into(),
        });
    definition
        .control_edges
        .extend([("yes", "left"), ("else", "right")].map(|(port, node)| {
            mf_runtime::ControlEdgeDefinition {
                from_node: "route".into(),
                from_output: port.into(),
                to_node: node.into(),
            }
        }));
    let registry = NodeRegistry::from_inventory().unwrap();
    let result = instantiate_compiled(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap()
    .execute()
    .unwrap();
    assert_eq!(result["value"].as_str(), Some("credential"));
    assert!(result["value"].is_secret());
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
    let execution = plan.generate_runner_execution_plans(&registry).unwrap();
    let frozen = mf_runtime::WorkflowManifest::from_bytes(&execution.manifest_bytes).unwrap();
    assert_eq!(frozen.definition, plan.definition);
    let targets: Vec<mf_runtime::NodeTarget> =
        serde_json::from_str(&execution.node_targets_json).unwrap();
    assert_eq!(targets[0].id, "outer");
    let artifacts = plan.generate_runner_artifacts().unwrap();
    assert!(!artifacts.rust_source.is_empty());
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
