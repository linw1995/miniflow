//! Frozen arguments retain ordinary source resolution without provider expansion hooks.
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;

use mf_compiler::{CallConfig, WorkflowDefinition, load_workflow, plan_definition, resolve_flows};
use mf_runtime::{Flow, NodeConfig};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};

fn definition(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

#[derive(NodeConfig)]
struct Config {
    local: Flow,
}

fn owner(child: Value) -> Value {
    json!({"id":"custom", "kind":"test.mixed_version_owner", "config":{
        "local":{"definition":child}
    }})
}

fn plain_owner(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    mf_runtime::NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(json!({"value": 0}))
}

fn prepared_owner(
    _: &str,
    config: Value,
    arguments: BTreeMap<String, mf_runtime::PreparedSubgraph>,
) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let config = Config::from_config(config, arguments)?;
    assert!(!config.local.nodes().is_empty());
    plain_owner(json!({}))
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "test.mixed_version_owner",
        factory: mf_runtime::NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain: plain_owner,
            prepared: prepared_owner,
        },
    }
}

fn leaf() -> Value {
    json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "value", "kind": "builtin.constant", "config": {"value": 42}}],
        "outputs": [{"name": "value", "node": "value", "port": "value"}]
    })
}

fn caller() -> Value {
    json!({
        "version": "2026-10-28", "dependencies": {},
        "flows": {"local": {"path": "leaf.json"}},
        "nodes": [{"id": "invoke", "kind": "workflow.call", "config": {"flow": "local"}}],
        "outputs": [{"name": "value", "node": "invoke", "port": "value"}]
    })
}

#[test]
fn frozen_arguments_resolve_relative_files_and_survive_without_source_files() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("provider")).unwrap();
    let mut child = leaf();
    child.as_object_mut().unwrap().remove("version");
    child.as_object_mut().unwrap().remove("dependencies");
    fs::write(directory.path().join("leaf.json"), child.to_string()).unwrap();
    let mut argument = caller();
    argument.as_object_mut().unwrap().remove("version");
    argument["dependencies"] =
        json!({"provider": {"package": "test-provider", "path": "provider"}});
    let authored = json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": [owner(argument)]
    });
    let source = directory.path().join("flow.json");
    fs::write(&source, authored.to_string()).unwrap();
    let resolved = load_workflow(&source).unwrap();
    assert_eq!(resolved.sources.len(), 2);
    let arguments = resolved.definition.nodes[0]
        .flow_parameters(&["/local"])
        .unwrap();
    let argument = &arguments["/local"];
    assert_eq!(
        argument.version,
        mf_compiler::WorkflowDefinitionVersion::CURRENT
    );
    assert!(argument.flows.is_empty());
    assert_eq!(
        argument.dependencies["provider"].path.as_deref(),
        Some(
            fs::canonicalize(directory.path().join("provider"))
                .unwrap()
                .as_path()
        )
    );
    let call: CallConfig = serde_json::from_value(argument.nodes[0].config.clone()).unwrap();
    assert_eq!(*call.body.unwrap(), definition(leaf()));
    let selected_plan = mf_compiler::plan_resolved_workflow(&resolved).unwrap();
    let frozen = serde_json::to_string(&resolved.definition).unwrap();
    directory.close().unwrap();
    let frozen = WorkflowDefinition::from_json(&frozen).unwrap();
    let offline = resolve_flows(&frozen, None).unwrap();
    assert_eq!(offline.definition, frozen);
    assert!(offline.sources.is_empty());
    let plan = plan_definition(&frozen).unwrap();
    assert_eq!(plan, selected_plan);
    assert_eq!(plan_definition(&plan.graph.definition).unwrap(), plan);
}

#[test]
fn mixed_version_child_iterations_retain_body_paths_for_static_and_mapped_calls() {
    let child = json!({
        "version": "2026-10-03", "dependencies": {},
        "nodes": [
            {"id": "items", "kind": "builtin.constant", "config": {"value": [1]}},
            {"id": "iteration", "kind": "builtin.iteration", "config": {
                "mode": "sequential", "on_error": "terminate",
                "body": {"version":"2026-10-03", "dependencies":{},
                    "nodes": [{"id": "value", "kind": "builtin.constant", "config": {"value": 7}}],
                    "outputs": [{"name":"result", "node": "value", "port": "value"}]
                }
            }}
        ],
        "edges": [{"from_node": "items", "from_output": "value", "to_node": "iteration", "to_input": "items"}],
        "outputs": [{"name": "results", "node": "iteration", "port": "results"}]
    });
    let parent = definition(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [
            {"id": "invoke", "kind": "workflow.call", "config": {"flow": "static", "body": child}},
            owner(child.clone())
        ]
    }));
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&parent, &registry).unwrap();
    let expected_paths = vec![
        vec!["invoke".to_owned(), "iteration".to_owned()],
        vec![
            mf_telemetry::event::ScopePathEntry::binding_key("custom", "/local"),
            "iteration".to_owned(),
        ],
    ];
    let assert_iteration_paths = |description: mf_telemetry::description::WorkflowDescription| {
        for path in &expected_paths {
            let body = description
                .loop_bodies
                .iter()
                .find(|body| &body.path == path)
                .unwrap_or_else(|| panic!("missing mixed-version Iteration body {path:?}"));
            assert!(body.nodes.iter().any(|node| node.id == "value"));
        }
    };
    assert_iteration_paths(mf_compiler::describe_compiled(&plan).unwrap());
    let replanned =
        mf_compiler::plan_resolved_definition(&plan.graph.definition, &registry).unwrap();
    assert_eq!(
        plan.graph.children, replanned.graph.children,
        "mixed-version scope orders must remain canonical after preparation"
    );
    assert_iteration_paths(plan.manifest(&registry).unwrap().description().unwrap());
    #[cfg(feature = "codegen")]
    {
        let generated = plan.generate_runner_execution_plans(&registry).unwrap();
        let manifest = mf_runtime::WorkflowManifest::from_bytes(&generated.manifest_bytes).unwrap();
        assert_iteration_paths(manifest.description().unwrap());
    }
    // Historical projection omits this Iteration body, but its actual compiled
    // graph must still identify the run independently of the duplicate raw JSON.
    let mut legacy = definition(child);
    legacy.version = mf_compiler::WorkflowDefinitionVersion::V2026_09_26;
    let mut legacy = mf_compiler::compile_definition(&legacy, &registry).unwrap();
    let original = legacy.graph.definition.clone();
    let mut identities = Vec::new();
    for value in [7, 8] {
        legacy
            .graph
            .children
            .get_mut(&"iteration".into())
            .unwrap()
            .get_mut("/body")
            .unwrap()
            .definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "value")
            .unwrap()
            .config["value"] = json!(value);
        assert_eq!(legacy.graph.definition, original);
        assert!(
            mf_compiler::describe_compiled(&legacy)
                .unwrap()
                .loop_bodies
                .is_empty()
        );
        let expected = legacy.graph.workflow_id().unwrap();
        let harness = capture::Harness::new(true);
        let observation = legacy
            .start_observation(&harness.observer(), mf_telemetry::identity::RunId::new())
            .unwrap();
        assert!(!observation.supports_loops());
        assert_eq!(harness.records()[0].decode().unwrap().workflow_id, expected);
        identities.push(expected);
    }
    assert_ne!(identities[0], identities[1]);
}

#[test]
fn old_root_preserves_new_child_features_in_every_observation_projection() {
    use mf_telemetry::{description::WorkflowDescriptionVersion, identity::RunId};
    let registry = mf_compiler::NodeRegistry::from_inventory().unwrap();
    for clustered in [false, true] {
        let mut child = leaf();
        child["nodes"].as_array_mut().unwrap().push(json!({
            "id": "before", "kind": "builtin.constant", "config": {"value": 1}
        }));
        child["edges"] = json!([{"from_node": "before", "to_node": "value"}]);
        if clustered {
            child["clusters"] = json!([{"label": "Child steps", "annotation": "Newer child metadata", "nodes": ["before", "value"]}]);
        }
        let child_plan =
            mf_compiler::compile_definition(&definition(child.clone()), &registry).unwrap();
        assert_eq!(
            mf_compiler::instantiate_compiled(&child_plan, &registry)
                .unwrap()
                .execute()
                .unwrap()["value"]
                .as_i64(),
            Some(42)
        );
        let root = definition(json!({
            "version": "2026-09-26", "dependencies": {}, "nodes": [owner(child)]
        }));
        let plan = mf_compiler::compile_definition(&root, &registry).unwrap();
        assert_eq!(
            plan.graph.definition.version,
            mf_compiler::WorkflowDefinitionVersion::V2026_09_26
        );
        let graph = mf_compiler::describe_compiled(&plan).unwrap();
        assert_eq!(
            graph.version,
            if clustered {
                WorkflowDescriptionVersion::V2026_10_23
            } else {
                WorkflowDescriptionVersion::V2026_10_19
            }
        );
        let path = vec![mf_telemetry::event::ScopePathEntry::binding_key(
            "custom", "/local",
        )];
        let body = graph.body(&path).unwrap();
        assert_eq!(body.order_edges.len(), 1);
        assert_eq!(body.order_edges[0].from_node, "before");
        assert_eq!(body.order_edges[0].to_node, "value");
        assert_eq!(body.clusters.len(), usize::from(clustered));
        if clustered {
            assert_eq!(body.clusters[0].nodes, ["before", "value"]);
        }
        let manifest = plan.manifest(&registry).unwrap();
        assert_eq!(manifest.graph.definition.version, root.version);
        assert_eq!(manifest.description().unwrap(), graph);
        let harness = capture::Harness::new(true);
        let observation = plan
            .start_observation(&harness.observer(), RunId::new())
            .unwrap();
        mf_compiler::execute_compiled_with_inputs(
            &plan,
            &registry,
            Default::default(),
            Some(observation),
        )
        .unwrap();
        harness.traces.force_flush().unwrap();
        assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
        let records = harness.records();
        assert!(!records.is_empty());
        for record in records {
            record.decode().unwrap().validate_for(&graph).unwrap();
        }
        #[cfg(feature = "codegen")]
        {
            let generated = plan.generate_execution_plans(&registry).unwrap();
            let embedded =
                mf_runtime::WorkflowManifest::from_bytes(&generated.manifest_bytes).unwrap();
            assert_eq!(embedded.graph.definition.version, root.version);
            assert_eq!(embedded.description().unwrap(), graph);
        }
    }
}

#[test]
fn missing_grandchild_versions_inherit_the_root_across_explicit_older_call_frames() {
    use mf_compiler::WorkflowDefinitionVersion;
    let mut grandchild = leaf();
    grandchild.as_object_mut().unwrap().remove("version");
    grandchild.as_object_mut().unwrap().remove("dependencies");
    grandchild["clusters"] = json!([{
        "label":"Grandchild", "annotation":"Uses the root schema", "nodes":["value"]
    }]);
    let middle = json!({"version":"2026-09-26", "nodes":[owner(grandchild)]});
    let root = definition(json!({"version":"2026-10-28", "nodes":[
        {"id":"invoke", "kind":"workflow.call", "config":{"flow":"static", "body":middle}},
        owner(middle)
    ]}));
    let resolved = resolve_flows(&root, None).unwrap();
    let plan = plan_definition(&root).unwrap();
    for (owner, field) in [("invoke", "/body"), ("custom", "/local")] {
        let child = &resolved.children[&owner.into()][field];
        assert_eq!(
            child.definition.version,
            WorkflowDefinitionVersion::V2026_09_26
        );
        let grandchild = &child.children[&"custom".into()]["/local"];
        assert_eq!(
            grandchild.definition.version,
            WorkflowDefinitionVersion::CURRENT
        );
        assert_eq!(grandchild.definition.clusters.len(), 1);
        let compiled = &plan.graph.children[&owner.into()][field];
        assert_eq!(
            compiled.definition.version,
            WorkflowDefinitionVersion::V2026_09_26
        );
        assert_eq!(
            compiled.children[&"custom".into()]["/local"]
                .definition
                .version,
            WorkflowDefinitionVersion::CURRENT
        );
    }
    assert!(resolved.sources.is_empty());
    let canonical =
        WorkflowDefinition::from_json(&serde_json::to_string(&resolved.definition).unwrap())
            .unwrap();
    assert_eq!(plan_definition(&canonical).unwrap(), plan);
}

#[test]
fn inherited_old_child_versions_reject_new_features_but_explicit_versions_are_independent() {
    let mut child = leaf();
    child.as_object_mut().unwrap().remove("version");
    child.as_object_mut().unwrap().remove("dependencies");
    child["clusters"] = json!([{
        "label":"Child", "annotation":"Requires a newer schema", "nodes":["value"]
    }]);
    let mut root = definition(json!({"version":"2026-10-03", "nodes":[owner(child)]}));
    let error = resolve_flows(&root, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("node clusters require version 2026-10-23")
    );
    root.nodes[0].config["local"]["definition"]["version"] = json!("2026-10-28");
    let resolved = resolve_flows(&root, None).unwrap();
    assert_eq!(
        resolved.children[&"custom".into()]["/local"]
            .definition
            .version,
        mf_compiler::WorkflowDefinitionVersion::CURRENT
    );
}

#[test]
fn frozen_arguments_do_not_capture_enclosing_source_aliases() {
    let mut authored = caller();
    authored["flows"] = json!({"local": {"builtin": "not-linked"}});
    authored["nodes"] = json!([]);
    let mut argument = caller();
    argument.as_object_mut().unwrap().remove("flows");
    authored["nodes"] = json!([owner(argument)]);
    let error = resolve_flows(&definition(authored), None).err().unwrap();
    assert!(error.to_string().contains("unknown Flow alias"), "{error}");
}

#[test]
fn frozen_argument_resolution_shares_the_call_depth_budget() {
    let mut nested = leaf();
    for _ in 0..=mf_compiler::MAX_CALL_DEPTH {
        nested = json!({
            "version": "2026-10-28", "dependencies": {}, "nodes": [owner(nested)]
        });
    }
    let error = resolve_flows(&definition(nested), None).err().unwrap();
    assert!(error.to_string().contains("sixteen calls"), "{error}");
}

#[test]
fn frozen_argument_normalization_canonicalizes_every_body() {
    let mut argument = leaf();
    argument["nodes"] = json!([
        {"id": "z", "kind": "builtin.constant", "config": {"value": 1}},
        {"id": "a", "kind": "builtin.constant", "config": {"value": 2}}
    ]);
    argument["outputs"] = json!([]);
    let authored = definition(json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": [owner(argument)]
    }));
    let plan = plan_definition(&authored).unwrap();
    let child = &plan.graph.children[&mf_compiler::DefinitionId::from("custom")]["/local"];
    assert_eq!(
        child
            .execution_order
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(
        child
            .definition
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(plan_definition(&plan.graph.definition).unwrap(), plan);
}
