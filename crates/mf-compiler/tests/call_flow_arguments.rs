//! A workflow.call specializes a reusable Flow with provider-declared child Flows.
extern crate embedded_flow_provider as _;
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;

#[cfg(feature = "codegen")]
use mf_compiler::WorkflowDefinition;
use mf_compiler::{NodeRegistry, compile_resolved_definition};
use mf_telemetry::{
    description::WorkflowDescription,
    event::{Event, ScopePathEntry},
    identity::RunId,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn fixture(root: &Path, provider: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("callee")).unwrap();
    fs::create_dir_all(root.join("bodies")).unwrap();
    let a = json!({
        "version":"2026-10-28",
        "dependencies":{"host":{"package":"embedded-flow-provider", "path":provider}},
        "nodes":[{"id":"apply", "kind":"example.apply", "config":{"policy":{}}}],
        "outputs":[{"name":"answer", "node":"apply", "port":"answer"}]
    });
    write_json(&root.join("callee/flow-a.json"), &a);
    for (name, answer) in [("one", "first body"), ("two", "second body")] {
        write_json(
            &root.join(format!("bodies/{name}.json")),
            &json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[
                    {"id":"echo", "kind":"builtin.identity"},
                    {"id":"result", "kind":"builtin.constant", "config":{"value":answer}}
                ],
                "outputs":[{"name":"answer", "node":"result", "port":"value"}]
            }),
        );
    }
    // If B is incorrectly resolved relative to A, the test must fail, not find a fallback.
    fs::create_dir_all(root.join("callee/bodies")).unwrap();
    fs::write(root.join("callee/bodies/one.json"), "not a workflow").unwrap();
    fs::write(root.join("callee/bodies/two.json"), "not a workflow").unwrap();
    let source = root.join("root.json");
    write_json(
        &source,
        &json!({
            "version":"2026-10-28",
            "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}},
            "flows":{"flowA":{"path":"callee/flow-a.json"}},
            "nodes":[
                {"id":"params", "kind":"builtin.parameters", "config":{
                    "parameters":{"value":{"type":"int", "default":42, "secret":false}}
                }},
                {"id":"callA", "kind":"workflow.call", "config":{
                    "flow":"flowA", "arguments":{"apply":{"/policy/body":{"path":"bodies/one.json"}}}
                }, "inputs":{"apply.value":{"from":{"node":"params", "port":"value"}}}},
                {"id":"callOther", "kind":"workflow.call", "config":{
                    "flow":"flowA", "arguments":{"apply":{"/policy/body":{"path":"bodies/two.json"}}}
                }, "inputs":{"apply.value":{"from":{"node":"params", "port":"value"}}}}
            ],
            "outputs":[
                {"name":"first", "node":"callA", "port":"answer"},
                {"name":"second", "node":"callOther", "port":"answer"}
            ]
        }),
    );
    source
}

fn example_provider() -> std::path::PathBuf {
    common::crates_dir()
        .parent()
        .unwrap()
        .join("examples/embedded-workflows/provider")
}

fn expected() -> Value {
    json!({"first":"first body", "second":"second body"})
}

fn assert_description(graph: &WorkflowDescription) {
    assert_eq!(
        graph.nodes.len(),
        3,
        "B must not become a root wrapper task"
    );
    assert_eq!(graph.loop_bodies.len(), 4);
    for id in ["callA", "callOther"] {
        let call = vec![id.into()];
        let binding = vec![
            id.into(),
            ScopePathEntry::binding_key("apply", "/policy/body"),
        ];
        assert_eq!(graph.body_paths_for_node(&[], id), [&call]);
        assert_eq!(graph.body_paths_for_node(&call, "apply"), [&binding]);
        assert_eq!(graph.body(&call).unwrap().nodes.len(), 1);
        assert_eq!(graph.body(&call).unwrap().nodes[0].id, "apply");
        assert_eq!(graph.body(&binding).unwrap().nodes.len(), 2);
        graph
            .validate_scope_path(&[
                ScopePathEntry::Call { call_id: id.into() },
                ScopePathEntry::Call {
                    call_id: ScopePathEntry::binding_key("apply", "/policy/body"),
                },
            ])
            .unwrap();
    }
    assert!(graph.body(&["flowA".into()]).is_none());
    assert!(
        graph
            .body(&[ScopePathEntry::binding_key("apply", "/policy/body")])
            .is_none()
    );
}

#[test]
fn calls_bind_missing_and_null_fields_independently_and_validate_authority_before_reading_b() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture(root.path(), &example_provider());
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    assert_eq!(resolved.sources.len(), 4);
    let frozen = serde_json::to_value(&resolved.definition).unwrap();
    assert!(frozen.get("flow_arguments").is_none());
    for i in [1, 2] {
        assert!(frozen["nodes"][i]["config"].get("arguments").is_none());
        assert!(
            frozen["nodes"][i]["config"]["body"]["nodes"][0]["config"]["policy"]["body"]
                .get("version")
                .is_some()
        );
    }
    assert_ne!(
        frozen["nodes"][1]["config"]["body"],
        frozen["nodes"][2]["config"]["body"]
    );
    let plan = compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_description(&graph);
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(manifest.description().unwrap(), graph);
    let harness = capture::Harness::new(true);
    let observation = harness
        .observer()
        .start_with_description(graph.clone(), RunId::new())
        .unwrap();
    let outputs = mf_compiler::execute_compiled_with_inputs(
        &plan,
        &registry,
        Default::default(),
        Some(observation),
    )
    .unwrap();
    assert_eq!(serde_json::to_value(outputs).unwrap(), expected());
    harness.traces.force_flush().unwrap();
    let spans = harness.spans.get_finished_spans().unwrap();
    for (id, expected_count) in [("callA", 1), ("callOther", 1), ("apply", 2), ("echo", 2)] {
        assert_eq!(
            spans
                .iter()
                .filter(|span| span
                    .attributes
                    .iter()
                    .any(|attribute| attribute.key.as_str() == "mf.node.id"
                        && attribute.value.as_str() == id))
                .count(),
            expected_count,
            "node spans must retain both specialized call paths for {id}",
        );
    }
    let records: Vec<_> = harness
        .records()
        .into_iter()
        .map(|record| record.decode().unwrap())
        .collect();
    for record in &records {
        record.validate_for(&graph).unwrap();
    }
    for id in ["callA", "callOther"] {
        let path = vec![
            ScopePathEntry::Call { call_id: id.into() },
            ScopePathEntry::Call {
                call_id: ScopePathEntry::binding_key("apply", "/policy/body"),
            },
        ];
        assert!(records.iter().any(|record| matches!(&record.event, Event::NodeStarted { node, .. } if node.id == "echo" && node.path == path)));
    }

    let a_source = root.path().join("callee/flow-a.json");
    let incomplete = mf_compiler::load_workflow_with_registry(&a_source, &registry).unwrap();
    assert!(
        compile_resolved_definition(&incomplete.definition, &registry).is_err(),
        "A alone still requires its provider's body"
    );
    let mut a: Value = serde_json::from_slice(&fs::read(&a_source).unwrap()).unwrap();
    a["nodes"][0]["config"]["policy"]["body"] = Value::Null;
    write_json(&a_source, &a);
    let null_bound = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    assert_eq!(null_bound.definition, resolved.definition);

    let original: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    for (arguments, message) in [
        (
            json!({"missing":{"/policy/body":{"path":"must-not-read.json"}}}),
            "unknown Flow argument target node",
        ),
        (
            json!({"apply":{
                "/policy/body":{"path":"must-not-read.json"},
                "/not_declared":{"path":"must-not-read.json"}
            }}),
            "not a declared Flow parameter",
        ),
        (
            json!({"apply.body":{"/policy/body":{"path":"must-not-read.json"}}}),
            "unknown Flow argument target node",
        ),
    ] {
        let mut invalid = original.clone();
        invalid["nodes"][1]["config"]["arguments"] = arguments;
        write_json(&source, &invalid);
        let error = mf_compiler::load_workflow_with_registry(&source, &registry)
            .err()
            .unwrap();
        assert!(error.to_string().contains(message), "{error}");
        assert!(
            !error.to_string().contains("must-not-read.json"),
            "argument source accessed before authority validation: {error}"
        );
    }
    // Call arguments are an API refactor on an existing schema, not a new schema gate.
    let mut existing_version = original.clone();
    existing_version["version"] = json!("2026-10-25");
    write_json(&source, &existing_version);
    let existing = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let existing_plan = compile_resolved_definition(&existing.definition, &registry).unwrap();
    assert_eq!(
        serde_json::to_value(
            mf_compiler::instantiate_compiled(&existing_plan, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        expected()
    );
    let mut unsupported = original.clone();
    unsupported["version"] = json!("2026-11-03");
    write_json(&source, &unsupported);
    assert!(mf_compiler::load_workflow_with_registry(&source, &registry).is_err());

    write_json(&source, &original);
    let mut spoofed = a.clone();
    spoofed["nodes"][0]["kind"] = json!("builtin.identity");
    write_json(&a_source, &spoofed);
    let error = mf_compiler::load_workflow_with_registry(&source, &registry)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("not a declared Flow parameter"),
        "{error}"
    );

    a["nodes"][0]["config"]["policy"]["body"] = json!({"path":"already-bound.json"});
    write_json(&a_source, &a);
    let error = mf_compiler::load_workflow_with_registry(&source, &registry)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("cannot override non-null field"),
        "{error}"
    );
}

#[cfg(feature = "codegen")]
#[test]
fn generated_calls_discover_the_receiving_provider_and_run_without_any_a_b_or_provider_sources() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    use std::process::Command;

    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    fs::create_dir_all(provider.join("src")).unwrap();
    let original = example_provider();
    let manifest = fs::read_to_string(original.join("Cargo.toml"))
        .unwrap()
        .replace(
            "\"../../../crates/mf-runtime\"",
            &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
        );
    fs::write(provider.join("Cargo.toml"), manifest).unwrap();
    fs::copy(original.join("src/lib.rs"), provider.join("src/lib.rs")).unwrap();
    let source = fixture(root.path(), &provider);
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let plan = compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    let bytes = manifest.to_bytes().unwrap();
    let output = root.path().join("runner");
    let build = root.path().join("build");
    compile_project(&CompileRequest {
        definition: &source,
        output: &output,
        locked: false,
        build_dir: Some(&build),
        support: &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    })
    .unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(root.path().join("callee")).unwrap();
    fs::remove_dir_all(root.path().join("bodies")).unwrap();
    fs::remove_dir_all(&provider).unwrap();
    fs::remove_dir_all(&build).unwrap();
    // Description generation is provider-free and source-free, using only manifest bytes.
    let restored = mf_runtime::WorkflowManifest::from_bytes(&bytes).unwrap();
    assert_eq!(
        restored.description().unwrap(),
        manifest.description().unwrap()
    );
    assert_description(&restored.description().unwrap());
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&restored.graph.definition).unwrap())
            .unwrap();
    let rebuilt = compile_resolved_definition(&frozen, &registry).unwrap();
    assert_eq!(
        serde_json::to_value(
            mf_compiler::instantiate_compiled(&rebuilt, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        expected()
    );
    for _ in 0..2 {
        let result = Command::new(&output)
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            expected()
        );
    }
}
