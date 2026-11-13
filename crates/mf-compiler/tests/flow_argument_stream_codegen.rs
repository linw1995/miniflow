//! Single/Task callable children retain both TaskStream interfaces in generated runners.
mod common;
#[path = "fixtures/flow-argument-stream-provider/src/lib.rs"]
mod provider;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, instantiate_stream,
};
use mf_runtime::StreamOptions;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn definition(stream: bool, optional: bool) -> WorkflowDefinition {
    let child = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"step", "kind":provider::SOURCE_KIND}],
        "outputs":[
            {"name":"increment", "node":"step", "port":"delta", "optional":optional},
            {"name":"count", "node":"step", "port":"count"},
            {"name":"subscribed", "node":"step", "port":"subscribed"}
        ]
    });
    let mut value = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"custom", "kind":provider::KIND, "config":{
            "optional":optional, "local":{"definition":child}
        }}],
        "outputs":[
            {"name":"count", "node":"custom", "port":"count"},
            {"name":"subscribed", "node":"custom", "port":"subscribed"}
        ]
    });
    if stream {
        value["execution"] = json!({});
        value["outputs"].as_array_mut().unwrap().push(json!({
            "name":"increment", "node":"custom", "port":"increment", "optional":optional
        }));
    }
    serde_json::from_value(value).unwrap()
}

fn assert_outputs(values: &[Value], stream: bool) {
    if stream {
        assert_eq!(values.len(), 4);
        assert_eq!(
            values
                .iter()
                .filter_map(|value| value.get("increment"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!(0), json!(1), json!(2)]
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| value.get("count").is_some())
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!({"count":3, "subscribed":true})]
        );
    } else {
        assert_eq!(values, &[json!({"count":3, "subscribed":false})]);
    }
}

#[test]
fn callable_task_children_preserve_stream_exports_without_promoting_or_flattening_the_body() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (stream, optional) in [(true, false), (false, true)] {
        let plan = compile_definition(&definition(stream, optional), &registry).unwrap();
        assert_eq!(plan.graph.definition.execution.is_some(), stream);
        assert!(
            plan.graph.definition.nodes[0]
                .flow_parameters(&["local"])
                .unwrap()["local"]
                .execution
                .is_none()
        );
        let description = mf_compiler::describe_compiled(&plan).unwrap();
        assert_eq!(description.nodes.len(), 1);
        assert_eq!(description.loop_bodies.len(), 1);
        assert_eq!(description.loop_bodies[0].nodes.len(), 1);
        let values = if stream {
            let instance = instantiate_stream(&plan, &registry)
                .unwrap()
                .start_with_options(StreamOptions::default())
                .unwrap();
            let mut values = vec![];
            while let Some(output) = instance.recv().unwrap() {
                values.push(serde_json::to_value(output.outputs).unwrap());
            }
            instance.join().unwrap();
            values
        } else {
            vec![
                serde_json::to_value(
                    instantiate_compiled(&plan, &registry)
                        .unwrap()
                        .execute()
                        .unwrap(),
                )
                .unwrap(),
            ]
        };
        assert_outputs(&values, stream);
    }
}

// This target intentionally does not link mfn-core, so changed receiving
// contracts can be tested without duplicate inventory kinds or registry mutation.
mod contract_drift {
    use super::*;
    use mf_runtime::{NodeBuildError, NodeFactory, NodeRegistration, PreparedNode};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CHILD_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

    fn unexpected(_: Value) -> Result<PreparedNode, NodeBuildError> {
        panic!("receiving declaration drift must fail before either factory runs")
    }

    inventory::submit! { NodeRegistration { kind: "workflow.call", factory: NodeFactory::Subgraph {
        flow_parameters: |config| BTreeMap::from([("unrelated".into(), config["body"].clone())]), plain: unexpected, prepared: |_, config, _| unexpected(config),
    } } }
    inventory::submit! { NodeRegistration { kind: "loop.run", factory: NodeFactory::Subgraph {
        flow_parameters: |config| BTreeMap::from([("unrelated".into(), config["body"].clone())]), plain: unexpected, prepared: |_, config, _| unexpected(config),
    } } }
    inventory::submit! { NodeRegistration { kind: "builtin.iteration", factory: NodeFactory::Subgraph {
        flow_parameters: |config| BTreeMap::from([("unrelated".into(), config["body"].clone())]), plain: unexpected, prepared: |_, config, _| unexpected(config),
    } } }
    inventory::submit! { NodeRegistration { kind: "test.contract_child", factory: NodeFactory::Plain(|config| {
        CHILD_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
        NodeRegistry::from_inventory().unwrap().get(provider::SOURCE_KIND).unwrap().instantiate(config)
    }) } }

    #[test]
    fn canonical_body_contract_drift_fails_before_any_child_constructor() {
        let registry = NodeRegistry::from_inventory().unwrap();
        let child = json!({"id": "child", "kind": "test.contract_child"});
        let body = json!({"version": "2026-10-28", "dependencies": {}, "nodes": [child],
            "outputs": [{"name": "count", "node": "child", "port": "count"}]});
        let positive: WorkflowDefinition = serde_json::from_value(body.clone()).unwrap();
        let plan = mf_compiler::compile_resolved_definition(&positive, &registry).unwrap();
        assert!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst) > 0);
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()["count"]
                .as_u64(),
            Some(3)
        );
        for node in [
            json!({"id": "owner", "kind": "workflow.call", "config": {"flow": "child", "body": body}}),
            json!({"id": "owner", "kind": "loop.run", "config": {"max_iterations": 1,
                "variables": [{"name": "count", "type": "int"}], "body": {"version":"2026-10-28", "dependencies":{}, "nodes": [child]}}}),
            json!({"id": "owner", "kind": "builtin.iteration", "config": {"body": {"version":"2026-10-28", "dependencies":{},
                "nodes": [child], "outputs": [{"name":"result", "node": "child", "port": "count"}]}}}),
        ] {
            CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
            let kind = node["kind"].as_str().unwrap();
            assert!(registry.get(kind).unwrap().accepts_subgraphs());
            assert!(
                !registry
                    .get(kind)
                    .unwrap()
                    .flow_parameters(&node["config"])
                    .contains_key("body")
            );
            let definition: WorkflowDefinition = serde_json::from_value(json!({
                "version": "2026-10-28", "dependencies": {}, "nodes": [node]
            }))
            .unwrap();
            // Positive structural control: the authored child is executable under
            // the original body contract, before checking the changed receiver.
            let child_definition = definition.nodes[0]
                .flow_parameters(&["body"])
                .unwrap()
                .remove("body")
                .unwrap();
            let child_plan =
                mf_compiler::compile_resolved_definition(&child_definition, &registry).unwrap();
            let plan = mf_compiler::CompiledWorkflow {
                graph: mf_runtime::CompiledGraph {
                    definition,
                    execution_order: vec!["owner".into()],
                    relations: Default::default(),
                    children: BTreeMap::from([(
                        "owner".into(),
                        BTreeMap::from([("body".into(), child_plan.graph)]),
                    )]),
                },
            };
            assert_eq!(plan.graph.children.len(), 1, "{kind}");
            // A frozen plan carries the original body receiving contract. The
            // current registry must reject drift before constructing any child.
            CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
            let error = instantiate_compiled(&plan, &registry).err().unwrap();
            assert!(
                matches!(
                    &error,
                    mf_compiler::WorkflowCompileError::CompiledGraph { .. }
                ),
                "{kind}: {error}"
            );
            assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst), 0, "{kind}");
            #[cfg(feature = "codegen")]
            {
                assert!(
                    plan.generate_runner_execution_plans(&registry).is_err(),
                    "{kind}"
                );
                assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst), 0, "{kind}");
            }
        }
    }
}

#[cfg(feature = "codegen")]
#[test]
fn source_free_generated_runner_preserves_stream_forwarding_and_unsubscribed_terminal_adaptation() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    use std::{fs, process::Command};

    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    fs::create_dir_all(provider.join("src")).unwrap();
    fs::write(
        provider.join("Cargo.toml"),
        include_str!("fixtures/flow-argument-stream-provider/Cargo.toml").replace(
            "\"../../../../mf-runtime\"",
            &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        provider.join("src/lib.rs"),
        include_str!("fixtures/flow-argument-stream-provider/src/lib.rs"),
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let source = root.path().join("flow.json");
    let build = root.path().join("build");
    let mut runners = vec![];
    // Both stream requirednesses must be filtered out of terminal layouts at preparation.
    for (stream, optional) in [(true, false), (false, true)] {
        let mut value = serde_json::to_value(definition(stream, optional)).unwrap();
        value["dependencies"] = json!({"provider":{
            "package":"fixture-flow-argument-stream-provider", "path":provider
        }});
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let generated = plan.generate_runner_execution_plans(&registry).unwrap();
        syn::parse_file(&generated.rust_source).unwrap();
        let output = root.path().join(format!("runner-{stream}"));
        fs::write(&source, serde_json::to_vec(&definition).unwrap()).unwrap();
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
        runners.push((output, stream));
    }
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(&build).unwrap();
    fs::remove_dir_all(&provider).unwrap();
    for (output, stream) in runners {
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
        let values = result
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_outputs(&values, stream);
    }
}
