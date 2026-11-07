//! Nested executable leaves follow the receiving Rust type, including concrete collection layouts.
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/flow-arguments-provider/src/lib.rs"]
mod provider;

use mf_compiler::{NodeRegistry, WorkflowDefinition};
use mf_runtime::NodeConfig;
use mf_telemetry::{
    event::{Event, ScopePathEntry},
    identity::RunId,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn definition(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

fn leaf(content: &str) -> Value {
    serde_json::to_value(definition(json!({
        "version":"2026-10-28", "dependencies":{}, "edges":[],
        "nodes":[{"id":"leaf", "kind":"builtin.constant", "config":{"value":content}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    })))
    .unwrap()
}

fn skip() -> Value {
    json!({"kind":"skip", "data":{"path":"must-not-read.json"}})
}

fn write_fixture(root: &Path) -> std::path::PathBuf {
    fs::write(root.join("inner.json"), leaf("external").to_string()).unwrap();
    // The nested body's own version and alias table are authoritative.
    fs::write(
        root.join("body.json"),
        json!({
            "version":"2026-10-25", "dependencies":{},
            "flows":{"local":{"path":"inner.json"}},
            "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{"flow":"local"}}],
            "outputs":[{"name":"content", "node":"invoke", "port":"content"}]
        })
        .to_string(),
    )
    .unwrap();
    let scoped = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"/handlers/deep"}},
            {"id":"large", "kind":provider::NESTED_KIND, "config":{
                "handlers":{"deep":leaf("short")}, "policy":skip()
            }, "inputs":{"select":{"from":{"node":"select", "port":"value"}}}}
        ],
        "outputs":[{"name":"content", "node":"large", "port":"content"}]
    });
    let source = root.join("root.json");
    let mut workflow = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                "small":{"type":"string", "default":"/steps/0/body", "secret":false},
                "large":{"type":"string", "default":"/policy/body", "secret":false}
            }}},
            {"id":"small", "kind":provider::NESTED_KIND, "config":{
                "steps":[{"body":scoped}],
                "handlers":{"a/b~":{"definition":leaf("escaped")}},
                "policy":skip(),
                "opaque":{"path":"must-not-read.json"},
                "ordinary":{"payload":{"definition":{"path":"must-not-read.json"}}}
            }, "inputs":{"select":{"from":{"node":"params", "port":"small"}}}},
            {"id":"large", "kind":provider::NESTED_KIND, "config":{
                "steps":[{"body":{"path":"body.json"}}, {"body":{"builtin":"fixture-flow-arguments-provider.nested"}}],
                "policy":{"kind":"run", "body":{"definition":leaf("active")}}
            }, "inputs":{"select":{"from":{"node":"params", "port":"large"}}}}
        ],
        "outputs":[
            {"name":"short", "node":"small", "port":"content"},
            {"name":"long", "node":"large", "port":"content"}
        ]
    });
    // Keep the authored root in canonical producer-first order so the full raw frozen
    // definition (not merely a re-planned projection) must survive preparation exactly.
    workflow["nodes"].as_array_mut().unwrap().swap(1, 2);
    workflow["outputs"].as_array_mut().unwrap().swap(0, 1);
    fs::write(&source, workflow.to_string()).unwrap();
    source
}

fn execute(
    plan: &mf_compiler::CompiledWorkflow,
    registry: &NodeRegistry,
    small: &str,
    large: &str,
) -> Value {
    let inputs = mf_runtime::WorkflowArguments::try_from(json!({
        "params.small":small, "params.large":large
    }))
    .unwrap();
    serde_json::to_value(
        mf_compiler::instantiate_compiled(plan, registry)
            .unwrap()
            .execute_with_inputs(inputs)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn nested_sources_freeze_exactly_and_contracts_are_per_static_node_and_scope() {
    let root = tempfile::tempdir().unwrap();
    let source = write_fixture(root.path());
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    assert_eq!(resolved.sources.len(), 3);
    let frozen = serde_json::to_value(&resolved.definition).unwrap();
    let large = resolved
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "large")
        .unwrap();
    let small = resolved
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "small")
        .unwrap();
    let small_children = &resolved.children[&small.id];
    let large_children = &resolved.children[&large.id];
    assert_eq!(
        small_children
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["/handlers/a~1b~0", "/steps/0/body"]
    );
    assert_eq!(
        large_children
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["/policy/body", "/steps/0/body", "/steps/1/body"]
    );
    let nested_scope = &small_children["/steps/0/body"];
    let nested = nested_scope
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "large")
        .unwrap();
    assert_eq!(
        nested_scope.children[&nested.id]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["/handlers/deep"]
    );
    assert_ne!(nested_scope.children[&nested.id], *large_children);
    assert_eq!(small.config["opaque"], json!({"path":"must-not-read.json"}));
    assert_eq!(
        small.config["ordinary"],
        json!({"payload":{"definition":{"path":"must-not-read.json"}}})
    );
    assert_eq!(small.config["policy"], skip());
    assert_eq!(large.config["steps"][0]["body"]["version"], "2026-10-25");
    assert!(large.config["steps"][0]["body"].get("flows").is_none());
    assert_eq!(
        large.config["steps"][0]["body"]["nodes"][0]["config"]["body"],
        leaf("external")
    );
    assert_eq!(large.config["steps"][1]["body"], leaf("builtin"));
    assert_eq!(large.config["policy"]["body"], leaf("active"));
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    assert_eq!(
        serde_json::to_value(&plan.graph.definition).unwrap(),
        frozen,
        "preparation must preserve canonical raw frozen bodies"
    );
    assert_eq!(
        execute(&plan, &registry, "/steps/0/body", "/policy/body"),
        json!({"short":"short", "long":"active"})
    );
    assert_eq!(
        execute(&plan, &registry, "/handlers/a~1b~0", "/steps/0/body"),
        json!({"short":"escaped", "long":"external"})
    );
    assert_eq!(
        execute(&plan, &registry, "/handlers/a~1b~0", "/steps/1/body"),
        json!({"short":"escaped", "long":"builtin"})
    );
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(
        manifest.description().unwrap(),
        mf_compiler::describe_compiled(&plan).unwrap()
    );
    fs::remove_file(source).unwrap();
    fs::remove_file(root.path().join("body.json")).unwrap();
    fs::remove_file(root.path().join("inner.json")).unwrap();
    let frozen = WorkflowDefinition::from_json(&frozen.to_string()).unwrap();
    let rebuilt = mf_compiler::compile_definition(&frozen, &registry).unwrap();
    assert_eq!(rebuilt, plan);
    assert_eq!(
        execute(&rebuilt, &registry, "/steps/0/body", "/policy/body"),
        json!({"short":"short", "long":"active"})
    );
}

#[test]
fn nested_binding_observation_retains_escaped_pointer_and_scoped_node_identity() {
    let root = tempfile::tempdir().unwrap();
    let source = write_fixture(root.path());
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    let nested_path = vec![
        ScopePathEntry::Call {
            call_id: ScopePathEntry::binding_key("small", "/steps/0/body"),
        },
        ScopePathEntry::Call {
            call_id: ScopePathEntry::binding_key("large", "/handlers/deep"),
        },
    ];
    let escaped_path = vec![ScopePathEntry::Call {
        call_id: ScopePathEntry::binding_key("small", "/handlers/a~1b~0"),
    }];
    for (selection, expected_path) in [
        ("/steps/0/body", nested_path),
        ("/handlers/a~1b~0", escaped_path),
    ] {
        graph.validate_scope_path(&expected_path).unwrap();
        let static_path: Vec<_> = expected_path
            .iter()
            .map(|scope| scope.node_id().to_owned())
            .collect();
        assert_eq!(graph.body(&static_path).unwrap().nodes[0].id, "leaf");
        let harness = capture::Harness::new(true);
        let observation = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let inputs =
            mf_runtime::WorkflowArguments::try_from(json!({"params.small":selection})).unwrap();
        mf_compiler::execute_compiled_with_inputs(&plan, &registry, inputs, Some(observation))
            .unwrap();
        harness.traces.force_flush().unwrap();
        assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
        let records: Vec<_> = harness
            .records()
            .into_iter()
            .map(|record| record.decode().unwrap())
            .collect();
        for record in &records {
            record.validate_for(&graph).unwrap();
        }
        for finished in [false, true] {
            assert!(records.iter().any(|record| match &record.event {
                Event::NodeStarted { node, .. } if !finished =>
                    node.id == "leaf" && node.path == expected_path,
                Event::NodeFinished { node, .. } if finished =>
                    node.id == "leaf" && node.path == expected_path,
                _ => false,
            }));
        }
    }
}

#[test]
fn nested_call_arguments_authorize_existing_typed_leaves_before_source_io() {
    let root = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let callee = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"/steps/0/body"}},
            {"id":"receive", "kind":provider::NESTED_KIND, "config":{
                "steps":[{"body":null}], "handlers":{"a/b~":null},
                "policy":{"kind":"run", "body":null}, "opaque":{"body":null}
            }, "inputs":{"select":{"from":{"node":"select", "port":"value"}}}}
        ],
        "outputs":[{"name":"content", "node":"receive", "port":"content"}]
    });
    let authored = |arguments: Value| {
        definition(json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{
                "flow":"callee", "body":callee, "arguments":{"receive":arguments}
            }}],
            "outputs":[{"name":"content", "node":"invoke", "port":"content"}]
        }))
    };
    let arguments = json!({
        "/steps/0/body":{"definition":leaf("bound")},
        "/handlers/a~1b~0":{"definition":leaf("escaped")},
        "/policy/body":{"definition":leaf("active")}
    });
    let resolved = mf_compiler::resolve_flows_with_registry(
        &authored(arguments.clone()),
        Some(root.path()),
        &registry,
    )
    .unwrap();
    let child = &resolved.definition.nodes[0].config["body"]["nodes"][1]["config"];
    assert_eq!(child["steps"][0]["body"], leaf("bound"));
    assert_eq!(child["handlers"]["a/b~"], leaf("escaped"));
    assert_eq!(child["policy"]["body"], leaf("active"));
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    assert_eq!(
        mf_compiler::instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["content"]
            .as_str(),
        Some("bound")
    );
    for pointer in [
        "/opaque/body",
        "/steps/1/body",
        "/steps/-/body",
        "/missing/body",
        "/policy/other",
        "steps/0/body",
    ] {
        let mut invalid = json!({
            "/steps/0/body":{"path":"must-not-read.json"},
            "/handlers/a~1b~0":{"path":"must-not-read.json"},
            "/policy/body":{"path":"must-not-read.json"}
        });
        invalid[pointer] = json!({"path":"must-not-read.json"});
        let error = mf_compiler::resolve_flows_with_registry(
            &authored(invalid),
            Some(root.path()),
            &registry,
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("not a declared Flow parameter")
                || error.to_string().contains("JSON pointer"),
            "{pointer}: {error}"
        );
        assert!(
            !error.to_string().contains("must-not-read.json"),
            "source read before authority validation: {error}"
        );
    }
    let mut wrong_shape = authored(json!({
        "/steps/0/body":{"path":"must-not-read.json"},
        "/handlers/a~1b~0":{"path":"must-not-read.json"},
        "/policy/body":{"path":"must-not-read.json"}
    }));
    wrong_shape.nodes[0].config["body"]["nodes"][1]["config"]["steps"] = json!({"body":null});
    let error =
        mf_compiler::resolve_flows_with_registry(&wrong_shape, Some(root.path()), &registry)
            .err()
            .unwrap();
    assert!(error.to_string().contains("array"), "{error}");
    assert!(
        !error.to_string().contains("must-not-read.json"),
        "source read before receiving shape validation: {error}"
    );

    // A discriminator change removes authority, even though an ordinary payload contains body-shaped JSON.
    let mut drift = authored(arguments);
    drift.nodes[0].config["body"]["nodes"][1]["config"]["policy"] = skip();
    let error = mf_compiler::resolve_flows_with_registry(&drift, Some(root.path()), &registry)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("not a declared Flow parameter"),
        "{error}"
    );
}

#[test]
fn nested_collection_leaves_share_the_existing_static_call_depth_budget() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for depth in [mf_compiler::MAX_CALL_DEPTH, mf_compiler::MAX_CALL_DEPTH + 1] {
        let mut body = leaf("terminal");
        for level in 0..depth {
            body = if level % 2 == 0 {
                json!({
                    "version":"2026-10-28", "dependencies":{},
                    "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{"flow":"local", "body":body}}],
                    "outputs":[{"name":"content", "node":"invoke", "port":"content"}]
                })
            } else {
                json!({
                    "version":"2026-10-28", "dependencies":{},
                    "nodes":[
                        {"id":"select", "kind":"builtin.constant", "config":{"value":"/steps/0/body"}},
                        {"id":"owner", "kind":provider::NESTED_KIND, "config":{
                            "steps":[{"body":body}], "policy":skip()
                        }, "inputs":{"select":{"from":{"node":"select", "port":"value"}}}}
                    ],
                    "outputs":[{"name":"content", "node":"owner", "port":"content"}]
                })
            };
        }
        let result = mf_compiler::resolve_flows_with_registry(&definition(body), None, &registry);
        if depth == mf_compiler::MAX_CALL_DEPTH {
            assert!(
                result.is_ok(),
                "the existing sixteen-call boundary must remain usable"
            );
        } else {
            let error = result.err().unwrap();
            assert!(error.to_string().contains("sixteen calls"), "{error}");
        }
    }
}

#[test]
fn strict_pointer_setters_do_not_fabricate_parent_objects_or_append_arrays() {
    let mut node: mf_runtime::NodeDefinition = serde_json::from_value(json!({
        "id":"receive", "kind":provider::NESTED_KIND,
        "config":{"steps":[{"body":null}], "handlers":{"a/b~":null}, "policy":{"kind":"run", "body":null}}
    })).unwrap();
    let body = definition(leaf("set"));
    for pointer in [
        "/missing/body",
        "/steps/1/body",
        "/steps/-/body",
        "body",
        "/handlers/a~2b",
    ] {
        let before = node.clone();
        assert!(
            node.set_node_flow(pointer, body.clone()).is_err(),
            "{pointer}"
        );
        assert_eq!(
            node, before,
            "a failed setter must not partially mutate configuration"
        );
    }
    node.set_node_flow("/steps/0/body", body.clone()).unwrap();
    node.set_node_flow("/handlers/a~1b~0", body.clone())
        .unwrap();
    assert_eq!(node.config["steps"][0]["body"], leaf("set"));
    assert_eq!(node.config["handlers"]["a/b~"], leaf("set"));
    node.set_node_flow("", body.clone()).unwrap();
    assert_eq!(node.config, leaf("set"));
    assert_eq!(
        node.flow_parameters(&[""]).unwrap(),
        BTreeMap::from([("".into(), body)])
    );
}

#[derive(mf_runtime::NodeConfig)]
#[expect(dead_code, reason = "metadata-only receiver must not be constructed")]
struct FixedConfig {
    optional: Option<FixedStep>,
    steps: [FixedStep; 1],
}

#[derive(mf_runtime::NodeConfig)]
#[expect(dead_code, reason = "metadata-only receiver must not be constructed")]
struct FixedStep {
    body: mf_runtime::Flow,
}

fn fixed_plain(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    panic!("metadata validation must not construct the receiving provider")
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind: "test.fixed_nested_flow",
        factory: mf_runtime::NodeFactory::Subgraph {
            flow_parameters: FixedConfig::flow_parameters,
            plain: fixed_plain,
            prepared: |_, config, _| fixed_plain(config),
        },
    }
}

#[test]
fn fixed_array_shape_and_absent_optional_parent_are_checked_before_source_io() {
    for optional in [None, Some(Value::Null)] {
        let mut config = json!({"steps":[{"body":null}]});
        if let Some(optional) = optional {
            config["optional"] = optional;
        }
        assert_eq!(
            FixedConfig::flow_parameters(&config).unwrap(),
            BTreeMap::from([("/steps/0/body".into(), Value::Null)])
        );
        let caller = |config: Value, arguments: Value| {
            definition(json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{
                    "flow":"local", "body":{"version":"2026-10-28", "dependencies":{},
                        "nodes":[{"id":"receive", "kind":"test.fixed_nested_flow", "config":config}]},
                    "arguments":{"receive":arguments}
                }}]
            }))
        };
        let registry = NodeRegistry::from_inventory().unwrap();
        let error = mf_compiler::resolve_flows_with_registry(
            &caller(
                config.clone(),
                json!({
                    "/optional/body":{"path":"must-not-read.json"}
                }),
            ),
            None,
            &registry,
        )
        .err()
        .unwrap();
        assert!(
            error.to_string().contains("not a declared Flow parameter"),
            "{error}"
        );
        assert!(!error.to_string().contains("must-not-read.json"), "{error}");
        config["steps"] = json!([{"body":{"path":"must-not-read.json"}}, {"body":null}]);
        let error =
            mf_compiler::resolve_flows_with_registry(&caller(config, json!({})), None, &registry)
                .err()
                .unwrap();
        assert!(error.to_string().contains("array"), "{error}");
        assert!(!error.to_string().contains("must-not-read.json"), "{error}");
    }
}

static CHILD_CONSTRUCTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn counted_child(config: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    CHILD_CONSTRUCTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(config)
}

inventory::submit! {
    mf_runtime::NodeRegistration {
        kind:"test.nested_counted_child", factory:mf_runtime::NodeFactory::Plain(counted_child),
    }
}

#[test]
fn concrete_nested_metadata_drift_is_rejected_before_any_child_factory() {
    use std::sync::atomic::Ordering;
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = leaf("counted");
    body["nodes"][0]["kind"] = json!("test.nested_counted_child");
    let authored = definition(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"select", "kind":"builtin.constant", "config":{"value":"/steps/0/body"}},
            {"id":"owner", "kind":provider::NESTED_KIND, "config":{
                "steps":[{"body":body}], "handlers":{"a/b~":body}, "policy":skip()
            }, "inputs":{"select":{"from":{"node":"select", "port":"value"}}}}
        ],
        "outputs":[{"name":"content", "node":"owner", "port":"content"}]
    }));
    let plan = mf_compiler::compile_definition(&authored, &registry).unwrap();
    assert!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst) > 0);
    assert_eq!(
        mf_compiler::instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["content"]
            .as_str(),
        Some("counted")
    );
    for change in ["array", "variant", "map"] {
        let mut damaged = plan.clone();
        let config = &mut damaged
            .graph
            .definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "owner")
            .unwrap()
            .config;
        match change {
            "array" => {
                config["steps"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"body":body}));
            }
            "variant" => {
                config["policy"] = json!({"kind":"run", "body":body});
            }
            "map" => {
                config["handlers"] = json!({"different/~":body});
            }
            _ => unreachable!(),
        }
        CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
        let error = mf_compiler::instantiate_compiled(&damaged, &registry)
            .err()
            .unwrap();
        common::assert_receiver_binding_preflight(&error, "owner");
        assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst), 0, "{change}");
        #[cfg(feature = "codegen")]
        {
            assert!(
                damaged.generate_runner_execution_plans(&registry).is_err(),
                "{change}"
            );
            assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst), 0, "{change}");
        }
    }
}

#[cfg(feature = "codegen")]
#[test]
fn standalone_nested_provider_runner_is_source_free_and_keeps_concrete_metadata() {
    use mf_compiler::{CompileRequest, SupportPackages, compile_project};
    use std::process::Command;
    let root = tempfile::tempdir().unwrap();
    let provider_dir = root.path().join("provider");
    fs::create_dir_all(provider_dir.join("src")).unwrap();
    fs::write(
        provider_dir.join("Cargo.toml"),
        include_str!("fixtures/flow-arguments-provider/Cargo.toml").replace(
            "\"../../../../mf-runtime\"",
            &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        provider_dir.join("src/lib.rs"),
        include_str!("fixtures/flow-arguments-provider/src/lib.rs"),
    )
    .unwrap();
    let source = write_fixture(root.path());
    let mut value: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    value["dependencies"] = json!({
        "provider":{"package":"fixture-flow-arguments-provider", "path":provider_dir},
        "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}
    });
    fs::write(&source, value.to_string()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    let bytes = manifest.to_bytes().unwrap();
    let build = root.path().join("build");
    let output = root.path().join("runner");
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
    for path in [
        &source,
        &root.path().join("body.json"),
        &root.path().join("inner.json"),
    ] {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir_all(&build).unwrap();
    fs::remove_dir_all(&provider_dir).unwrap();
    assert_eq!(
        mf_runtime::WorkflowManifest::from_bytes(&bytes)
            .unwrap()
            .description()
            .unwrap(),
        manifest.description().unwrap()
    );
    for (small, large, expected) in [
        (
            "/steps/0/body",
            "/policy/body",
            json!({"short":"short", "long":"active"}),
        ),
        (
            "/handlers/a~1b~0",
            "/steps/0/body",
            json!({"short":"escaped", "long":"external"}),
        ),
        (
            "/handlers/a~1b~0",
            "/steps/1/body",
            json!({"short":"escaped", "long":"builtin"}),
        ),
    ] {
        let result = Command::new(&output)
            .arg(format!("--params.small={small}"))
            .arg(format!("--params.large={large}"))
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
            expected
        );
    }
}
