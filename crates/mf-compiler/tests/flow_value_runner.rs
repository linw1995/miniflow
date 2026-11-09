//! A reusable Flow receives callable values through ordinary input edges.
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;

use mf_compiler::{NodeRegistry, WorkflowDefinition};
use mf_telemetry::{
    description::WorkflowDescription,
    event::{Event, FlowOrigin, ScopePathEntry},
    identity::RunId,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn apply_definition() -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{
                "parameters":{"policy":{"type":"flow", "secret":false}}
            }},
            {"id":"apply", "kind":"workflow.call", "config":{
                "inputs":[],
                "outputs":[{"name":"answer", "value_type":"string", "required":true}]
            }, "inputs":{"flow":{"from":{"node":"params", "port":"policy"}}}}
        ],
        "outputs":[{"name":"answer", "node":"apply", "port":"answer"}]
    })
}

fn fixture(root: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("callee")).unwrap();
    fs::create_dir_all(root.join("bodies")).unwrap();
    write_json(&root.join("callee/A.json"), &apply_definition());
    for (name, answer) in [("one", "first body"), ("two", "second body")] {
        write_json(
            &root.join(format!("bodies/{name}.json")),
            &json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[{"id":"inner", "kind":"workflow.call", "config":{
                    "flow":"leaf", "body":{
                        "version":"2026-10-28", "dependencies":{},
                        "nodes":[{"id":"value", "kind":"builtin.constant", "config":{"value":answer}}],
                        "outputs":[{"name":"answer", "node":"value", "port":"value"}]
                    }
                }}],
                "outputs":[{"name":"answer", "node":"inner", "port":"answer"}]
            }),
        );
    }
    let source = root.join("root.json");
    write_json(
        &source,
        &json!({
            "version":"2026-10-28",
            "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}},
            "flows":{"A":{"path":"callee/A.json"}},
            "nodes":[
                {"id":"makeOne", "kind":"builtin.constant", "config":{"type":"flow", "value":{"path":"bodies/one.json"}}},
                {"id":"makeTwo", "kind":"builtin.constant", "config":{"type":"flow", "value":{"path":"bodies/two.json"}}},
                {"id":"forward", "kind":"builtin.identity", "inputs":{
                    "input":{"from":{"node":"makeOne", "port":"value"}}
                }},
                {"id":"first", "kind":"workflow.call", "config":{"flow":"A"}, "inputs":{
                    "params.policy":{"from":{"node":"forward", "port":"value"}}
                }},
                {"id":"second", "kind":"workflow.call", "config":{"flow":"A"}, "inputs":{
                    "params.policy":{"from":{"node":"makeTwo", "port":"value"}}
                }}
            ],
            "outputs":[
                {"name":"first", "node":"first", "port":"answer"},
                {"name":"second", "node":"second", "port":"answer"}
            ]
        }),
    );
    source
}

fn expected() -> Value {
    json!({"first":"first body", "second":"second body"})
}

fn origin(description: &WorkflowDescription, parent: &[String], producer: &str) -> FlowOrigin {
    let paths = description.body_paths_for_node(parent, producer);
    assert_eq!(paths.len(), 1);
    FlowOrigin {
        workflow_id: description.workflow_id.clone(),
        path: paths[0].clone(),
    }
}

fn assert_description(description: &WorkflowDescription, parent: &[String]) {
    // Passing B does not invent a B child under A's invocation node.
    for consumer in ["first", "second"] {
        let paths = description.body_paths_for_node(parent, consumer);
        assert_eq!(paths.len(), 1);
        assert!(
            description
                .body_paths_for_node(paths[0], "apply")
                .is_empty()
        );
    }
    for (consumer, producer) in [("first", "makeOne"), ("second", "makeTwo")] {
        let mut path: Vec<_> = parent
            .iter()
            .map(|call_id| ScopePathEntry::Call {
                call_id: call_id.clone(),
            })
            .collect();
        path.extend([
            ScopePathEntry::Call {
                call_id: consumer.into(),
            },
            ScopePathEntry::Flow {
                call_id: "apply".into(),
                definition: origin(description, parent, producer),
            },
            ScopePathEntry::Call {
                call_id: "inner".into(),
            },
        ]);
        description.validate_scope_path(&path).unwrap();
    }
}

#[test]
fn same_callee_receives_different_flow_values_over_edges_and_saved_plans_keep_origins() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture(root.path());
    let registry = NodeRegistry::from_inventory().unwrap();
    // A is also independently compilable; its parameter does not identify B or C.
    let authored = WorkflowDefinition::from_json(&apply_definition().to_string()).unwrap();
    mf_compiler::compile_definition(&authored, &registry).unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    assert_eq!(resolved.sources.len(), 4);
    let plan = mf_compiler::plan_resolved_workflow(&resolved).unwrap();
    assert_eq!(
        plan.graph.children[&"first".into()]["body"].definition,
        plan.graph.children[&"second".into()]["body"].definition,
    );
    let plan = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    root.close().unwrap();

    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_description(&description, &[]);
    assert_eq!(
        plan.manifest(&registry).unwrap().description().unwrap(),
        description
    );
    let harness = capture::Harness::new(true);
    let observation = harness
        .observer()
        .start_with_description(description.clone(), RunId::new())
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
    assert_eq!(
        spans
            .iter()
            .filter(|span| span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "mf.node.id" && attribute.value.as_str() == "value"
            }))
            .count(),
        2,
        "each received Flow executes once, not while producing the handle",
    );
    let records: Vec<_> = harness
        .records()
        .into_iter()
        .map(|record| record.decode().unwrap())
        .collect();
    for record in &records {
        record.validate_for(&description).unwrap();
    }
    for (consumer, producer) in [("first", "makeOne"), ("second", "makeTwo")] {
        let expected_path = vec![
            ScopePathEntry::Call {
                call_id: consumer.into(),
            },
            ScopePathEntry::Flow {
                call_id: "apply".into(),
                definition: origin(&description, &[], producer),
            },
            ScopePathEntry::Call {
                call_id: "inner".into(),
            },
        ];
        assert!(records.iter().any(|record| matches!(&record.event, Event::NodeStarted {node, ..} if node.id == "value" && node.path == expected_path)));
    }
}

#[test]
fn code_supplies_real_callable_values_as_native_workflow_inputs() {
    use mf_runtime::{
        FlowValue, Outputs, PortSpec, PreparedSubgraph, ValueType, WorkflowArguments,
    };
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = WorkflowDefinition::from_json(&apply_definition().to_string()).unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    for answer in ["native B", "native C"] {
        let body = PreparedSubgraph::new(
            Vec::new(),
            vec![PortSpec::new("answer", ValueType::String, true)],
            move |_| Ok(Outputs::from([("answer".into(), answer.into())])),
        );
        let policy = FlowValue::new(body).unwrap();
        let inputs = WorkflowArguments(std::collections::BTreeMap::from([(
            "params.policy".into(),
            policy.into(),
        )]));
        assert_eq!(
            flow.execute_with_inputs(inputs).unwrap()["answer"].as_str(),
            Some(answer)
        );
    }
}

#[cfg(feature = "codegen")]
#[test]
fn generated_edge_passed_flows_execute_without_sources_or_build_tools() {
    use mf_compiler::{SupportPackages, write_dependency_project};
    use std::process::Command;

    let root = tempfile::tempdir().unwrap();
    let source = fixture(root.path());
    // Keep both producers inside one isolated Task as well. Verbose --node outer
    // must map original definition origins to the isolated operation identity.
    let mut body: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    let dependencies = body["dependencies"].take();
    body["dependencies"] = json!({});
    write_json(
        &source,
        &json!({
            "version":"2026-10-28", "dependencies":dependencies,
            "nodes":[{"id":"outer", "kind":"workflow.call", "config":{"flow":"outer", "body":body}}],
            "outputs":[
                {"name":"first", "node":"outer", "port":"first"},
                {"name":"second", "node":"outer", "port":"second"}
            ]
        }),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let plan = mf_compiler::plan_resolved_workflow(&resolved).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    let bytes = manifest.to_bytes().unwrap();
    let output = root.path().join("runner");
    let build = root.path().join("build");
    write_dependency_project(
        &build,
        &plan,
        &SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let target = std::env::var_os("MF_TEST_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| build.join("target"));
    let status = mf_compiler::cargo_command(&build)
        .args(["build", "--offline", "--target-dir"])
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success(), "generated runner must compile");
    fs::copy(common::runner_executable(&build, "debug"), &output).unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(root.path().join("callee")).unwrap();
    fs::remove_dir_all(root.path().join("bodies")).unwrap();
    fs::remove_dir_all(&build).unwrap();
    let restored = mf_runtime::WorkflowManifest::from_bytes(&bytes).unwrap();
    assert_description(&restored.description().unwrap(), &["outer".into()]);
    assert_eq!(
        restored.description().unwrap(),
        manifest.description().unwrap()
    );
    for args in [
        vec![],
        vec!["--node", "outer"],
        vec!["-v", "--node", "outer"],
    ] {
        let result = Command::new(&output)
            .args(&args)
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "args {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            expected()
        );
    }
}
