//! Receiver declarations, not JSON shape or schema version, authorize executable children.
extern crate mfn_core as _;
#[cfg(feature = "codegen")]
mod common;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{
    NodeBuildError, NodeConfig, NodeFactory, NodeRegistration, PreparedNode, PreparedSubgraph,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn constant() -> Result<PreparedNode, NodeBuildError> {
    NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(json!({"value": 42}))
}

fn execute(plan: &mf_compiler::CompiledWorkflow, registry: &NodeRegistry) {
    assert_eq!(
        instantiate_compiled(plan, registry)
            .unwrap()
            .execute()
            .unwrap()["value"]
            .as_i64(),
        Some(42)
    );
}

#[cfg(feature = "codegen")]
fn manifest(
    plan: &mf_compiler::CompiledWorkflow,
    registry: &NodeRegistry,
) -> mf_runtime::WorkflowManifest {
    let artifacts = plan
        .generate_artifacts(registry, Default::default())
        .unwrap();
    artifacts.manifest
}

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    assert_ne!(
        config["panic_if_constructed"],
        json!(true),
        "latent declaration drift must be rejected before earlier factories"
    );
    assert_eq!(
        config["subgraphs"],
        json!({"provider_specific": "opaque data"})
    );
    constant()
}

inventory::submit! {
    NodeRegistration { kind: "test.versioned_plain_config", factory: NodeFactory::Plain(plain) }
}

fn definition(version: &str) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": version, "dependencies": {},
        "nodes": [{"id": "provider", "kind": "test.versioned_plain_config",
            "config": {"subgraphs": {"provider_specific": "opaque data"}}}],
        "outputs": [{"name": "value", "node": "provider", "port": "value"}]
    }))
    .unwrap()
}

#[test]
fn plain_provider_configuration_remains_opaque() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let authored = definition("2026-10-28");
    let resolved = mf_compiler::resolve_flows(&authored, None).unwrap();
    assert_eq!(resolved.definition, authored);
    let plan = compile_definition(&authored, &registry).unwrap();
    assert_eq!(
        plan.graph.definition.nodes[0].config,
        authored.nodes[0].config
    );
    assert!(
        mf_compiler::describe_compiled(&plan)
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    execute(&plan, &registry);
    #[cfg(feature = "codegen")]
    {
        let generated = plan
            .generate_artifacts(
                &registry,
                mf_compiler::CodegenOptions {
                    preserve_intermediate_outputs: true,
                },
            )
            .unwrap();
        syn::parse_file(&generated.rust_source).unwrap();
        assert!(
            !generated
                .rust_source
                .contains("PreparedSubgraph::from_plan")
        );
        let manifest = manifest(&plan, &registry);
        assert_eq!(
            manifest.node_definition("provider").unwrap().config,
            authored.nodes[0].config
        );
        assert!(manifest.description().unwrap().loop_bodies.is_empty());
    }
}

#[test]
fn removed_root_flow_arguments_are_rejected_instead_of_ignored() {
    let mut value = serde_json::to_value(definition("2026-10-28")).unwrap();
    value["flow_arguments"] = json!({});
    let error = WorkflowDefinition::from_json(&value.to_string()).unwrap_err();
    assert!(error.to_string().contains("flow_arguments"), "{error}");
}

fn optional_flow_plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    assert_ne!(
        config["panic_if_constructed"],
        json!(true),
        "declaration validation must reject drift before invoking factories"
    );
    constant()
}

fn unexpected_prepared(
    _: &str,
    _: Value,
    _: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    panic!("a provider without selected children must use its plain factory")
}

#[derive(mf_runtime::NodeConfig)]
struct OptionalFlowConfig {
    body: Option<mf_runtime::Flow>,
    note: Option<String>,
}

fn optional_typed_plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let decoded = OptionalFlowConfig::from_config(config.clone(), BTreeMap::new())?;
    assert!(decoded.body.is_none());
    assert_eq!(decoded.note.as_deref(), config["note"].as_str());
    optional_flow_plain(config)
}

inventory::submit! {
    NodeRegistration {
        kind: "test.versioned_optional_flow",
        factory: NodeFactory::Subgraph {
            scope: None,
            flow_parameters: OptionalFlowConfig::flow_parameters,
            plain: optional_typed_plain,
            prepared: unexpected_prepared,
        },
    }
}

#[test]
fn subgraph_capable_providers_without_selected_children_keep_plain_factory_semantics() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (version, config) in [
        ("2026-10-25", json!({"body": null})),
        ("2026-10-28", json!({"note": "optional body omitted"})),
    ] {
        let authored: WorkflowDefinition = serde_json::from_value(json!({
            "version": version, "dependencies": {},
            "nodes": [
                {"id": "before", "kind": "test.versioned_plain_config", "config": {
                    "subgraphs": {"provider_specific": "opaque data"}
                }},
                {"id": "provider", "kind": "test.versioned_optional_flow", "config": config}
            ],
            "outputs": [{"name": "value", "node": "provider", "port": "value"}]
        }))
        .unwrap();
        let plan = compile_definition(&authored, &registry).unwrap();
        assert_eq!(
            plan.graph.definition.nodes[1].config,
            authored.nodes[1].config
        );
        assert_eq!(plan.graph.execution_order[0].as_str(), "before");
        assert!(plan.graph.children.is_empty());
        assert!(OptionalFlowConfig::flow_parameters(&config).is_empty());
        execute(&plan, &registry);
        #[cfg(feature = "codegen")]
        {
            let generated = plan
                .generate_artifacts(&registry, Default::default())
                .unwrap();
            let source = &generated.rust_source;
            let check = source.find("mf_runtime::generated::configuration").unwrap();
            let constructor = source.find("mf_runtime::generated::instantiate").unwrap();
            assert!(check < constructor);
            assert!(!source.contains("PreparedSubgraph::from_plan"));
            assert_eq!(generated.manifest.definition, plan.graph.definition);
        }
    }
}

static CHILD_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);
static CHILD_CONSTRUCTION_LOCK: Mutex<()> = Mutex::new(());

fn counted_child(config: Value) -> Result<PreparedNode, NodeBuildError> {
    CHILD_CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
    optional_flow_plain(config)
}

inventory::submit! {
    NodeRegistration {
        kind: "test.undeclared_flow_child",
        factory: NodeFactory::Plain(counted_child),
    }
}

#[derive(mf_runtime::NodeConfig)]
struct TypedConfig {
    body: mf_runtime::Flow,
}

fn typed_prepared(
    _: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    let config = TypedConfig::from_config(config, arguments)?;
    assert_eq!(config.body.execute().unwrap()["value"].as_i64(), Some(42));
    optional_flow_plain(Value::Null)
}

inventory::submit! {
    NodeRegistration {
        kind: "test.declared_flow_owner",
        factory: NodeFactory::Subgraph {
            scope: None,
            flow_parameters: TypedConfig::flow_parameters,
            plain: optional_flow_plain, prepared: typed_prepared,
        },
    }
}

#[test]
fn same_json_only_prepares_children_for_the_declared_receiver_on_the_same_version() {
    let _lock = CHILD_CONSTRUCTION_LOCK.lock().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let child: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "child", "kind": "test.undeclared_flow_child"}],
        "outputs": [{"name": "value", "node": "child", "port": "value"}]
    }))
    .unwrap();
    let config = json!({
        "subgraphs": {"provider_specific": "opaque data"},
        "body": child,
        "opaque": {"definition": {"path": "not-a-file.json"}}
    });
    for (kind, executable) in [
        ("test.versioned_plain_config", false),
        ("test.declared_flow_owner", true),
    ] {
        CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
        let authored: WorkflowDefinition = serde_json::from_value(json!({
            "version": "2026-10-28", "dependencies": {},
            "nodes": [{"id": "owner", "kind": kind, "config": config}],
            "outputs": [{"name": "value", "node": "owner", "port": "value"}]
        }))
        .unwrap();
        let resolved =
            mf_compiler::resolve_flows_with_registry(&authored, None, &registry).unwrap();
        assert_eq!(resolved.definition.nodes[0].config, config);
        let plan =
            mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
        assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst) > 0, executable);
        assert_eq!(plan.graph.definition.nodes[0].config, config);
        let graph = mf_compiler::describe_compiled(&plan).unwrap();
        assert_eq!(!graph.loop_bodies.is_empty(), executable);
        execute(&plan, &registry);
        #[cfg(feature = "codegen")]
        {
            let generated = plan
                .generate_artifacts(
                    &registry,
                    mf_compiler::CodegenOptions {
                        preserve_intermediate_outputs: true,
                    },
                )
                .unwrap();
            syn::parse_file(&generated.rust_source).unwrap();
            assert_eq!(
                generated
                    .rust_source
                    .contains("PreparedSubgraph::from_plan"),
                executable
            );
            let manifest = manifest(&plan, &registry);
            assert_eq!(manifest.node_definition("owner").unwrap().config, config);
            assert_eq!(manifest.description().unwrap(), graph);
        }
    }
}

#[test]
fn resolved_planning_uses_selected_contracts_instead_of_ambient_host_metadata() {
    use std::error::Error;

    let registry = NodeRegistry::from_inventory().unwrap();
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "owner", "kind": "test.declared_flow_owner", "config": {
            "body": {"version": "2026-10-28", "dependencies": {}, "nodes": [
                {"id": "child", "kind": "builtin.constant", "config": {"value": 42}}
            ], "outputs": [{"name": "value", "node": "child", "port": "value"}]}
        }}]
    }))
    .unwrap();
    // The selected process confirmed no executable children. A linked host
    // registration of the same kind must not reinterpret this handoff's JSON.
    let ordinary = mf_compiler::plan_resolved_workflow(&mf_compiler::ResolvedWorkflow {
        definition: authored.clone(),
        relations: Default::default(),
        sources: Default::default(),
        children: Default::default(),
    })
    .unwrap();
    let typed = mf_compiler::plan_resolved_definition(&authored, &registry).unwrap();
    assert!(ordinary.graph.children.is_empty());
    assert_eq!(
        typed.graph.children[&mf_runtime::DefinitionId::from("owner")].len(),
        1
    );
    for (plan, children) in [(ordinary, 0), (typed, 1)] {
        let saved = plan.clone();
        assert_eq!(saved.graph.children.len(), children);
        let mut expected = authored.nodes[0].config.clone();
        if children != 0 {
            let body: WorkflowDefinition = serde_json::from_value(expected["body"].take()).unwrap();
            expected["body"] = json!(body);
        }
        assert_eq!(saved.graph.definition.nodes[0].config, expected);
        assert_eq!(
            mf_compiler::describe_compiled(&saved)
                .unwrap()
                .loop_bodies
                .len(),
            children
        );
        #[cfg(feature = "codegen")]
        {
            let generated = saved.generate_artifacts(&registry, Default::default());
            if children == 0 {
                let mf_compiler::PlanError::Compilation { source } = generated.unwrap_err() else {
                    panic!("expected a receiver-binding error");
                };
                common::assert_receiver_binding_preflight(&source, "owner");
            } else {
                assert!(
                    generated
                        .unwrap()
                        .rust_source
                        .contains("PreparedSubgraph::from_plan")
                );
            }
        }
    }
    let mut malformed = authored;
    malformed.nodes[0].config["body"] = json!("not a workflow");
    let error = mf_compiler::compile_resolved_definition(&malformed, &registry).unwrap_err();
    assert!(matches!(
        &error,
        mf_compiler::WorkflowCompileError::FlowParameters {
            definition_id,
            source: mf_runtime::FlowParameterError::Json { parameter, source },
        } if definition_id.as_str() == "owner" && parameter == "body" && source.is_data()
    ));
    // The exact chain excludes description/telemetry contract wrappers.
    let parameter_error = error.source().unwrap();
    assert!(parameter_error.is::<mf_runtime::FlowParameterError>());
    let json_error = parameter_error.source().unwrap();
    assert!(json_error.is::<serde_json::Error>());
    assert!(json_error.source().is_none());
}

#[test]
fn saved_loop_preparation_and_codegen_use_the_compiled_child_not_its_raw_copy() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": [
            {"id":"seed", "kind":"builtin.constant", "config":{"value":0}}, {
            "id": "repeat", "kind": "loop.run", "inputs":{"count":{"$ref":"seed.value"}}, "config": {
                "max_iterations": 1, "variables": [{"name":"count","type":"int"}], "body": {
                    "version": "2026-10-28", "dependencies": {}, "nodes": [{
                        "id": "step", "kind": "builtin.constant", "config": {"value": 42}
                    }]
                }
            }
        }]
    }))
    .unwrap();
    let mut plan = compile_definition(&definition, &registry).unwrap();
    let children = plan.graph.children.clone();
    plan.graph
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "repeat")
        .unwrap()
        .config["body"] = json!({"path": "must-not-be-opened.json"});
    let saved = plan.clone();
    assert_eq!(saved.graph.children, children);
    assert_eq!(
        mf_compiler::describe_compiled(&saved)
            .unwrap()
            .loop_bodies
            .len(),
        1
    );
    assert!(
        instantiate_compiled(&saved, &registry)
            .unwrap()
            .execute()
            .unwrap()
            .is_empty()
    );
    #[cfg(feature = "codegen")]
    {
        syn::parse_file(
            &saved
                .generate_artifacts(
                    &registry,
                    mf_compiler::CodegenOptions {
                        preserve_intermediate_outputs: true,
                    },
                )
                .unwrap()
                .rust_source,
        )
        .unwrap();
        assert_eq!(
            manifest(&saved, &registry).description().unwrap(),
            mf_runtime::describe_graph(&saved.graph).unwrap()
        );
        assert_eq!(
            saved
                .generate_artifacts(
                    &registry,
                    mf_compiler::CodegenOptions {
                        preserve_intermediate_outputs: true
                    }
                )
                .unwrap()
                .manifest,
            saved
                .generate_artifacts(&registry, Default::default())
                .unwrap()
                .manifest
        );
        let mut streaming = saved.clone();
        streaming.graph.definition.execution = Some(Default::default());
        syn::parse_file(
            &streaming
                .generate_artifacts(
                    &registry,
                    mf_compiler::CodegenOptions {
                        preserve_intermediate_outputs: true,
                    },
                )
                .unwrap()
                .rust_source,
        )
        .unwrap();
    }
}

#[test]
fn undeclared_fields_stay_opaque_even_when_the_factory_accepts_other_flow_fields() {
    let _lock = CHILD_CONSTRUCTION_LOCK.lock().unwrap();
    CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
    let registry = NodeRegistry::from_inventory().unwrap();
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "owner", "kind": "test.versioned_optional_flow", "config": {
            "undeclared": {
                "version": "2026-10-28", "dependencies": {},
                "nodes": [{"id": "child", "kind": "test.undeclared_flow_child"}]
            }
        }}]
    }))
    .unwrap();
    let plan = mf_compiler::compile_resolved_definition(&authored, &registry).unwrap();
    assert_eq!(plan.graph.definition, authored);
    assert!(
        mf_compiler::describe_compiled(&plan)
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    assert_eq!(CHILD_CONSTRUCTIONS.load(Ordering::SeqCst), 0);
}

#[test]
fn generated_receiving_contracts_reject_stale_binding_sets_before_factories() {
    use mf_compiler::{WorkflowBuildError, validate_compiled_graph};
    use std::error::Error;

    let registry = NodeRegistry::from_inventory().unwrap();
    for kind in [
        mf_runtime::LOOP_SOURCE_ID,
        mf_runtime::ITERATION_INPUT_KIND,
        mf_runtime::EXIT_LOOP_KIND,
    ] {
        assert!(
            registry.get(kind).is_none(),
            "{kind} must be compiler-owned"
        );
    }
    // Preparing a saved graph rejects missing providers before invoking earlier
    // factories; host supplied child graphs remain a separate execution boundary.
    for nested in [false, true] {
        let missing = json!({"id": "z_missing", "kind": "test.missing_receiver"});
        let later = if nested {
            json!({"id": "z_owner", "kind": "test.declared_flow_owner", "config": {
                "body": {"version": "2026-10-28", "dependencies": {}, "nodes": [missing]}
            }})
        } else {
            missing
        };
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version": "2026-10-28", "dependencies": {}, "nodes": [
                {"id": "a_before", "kind": "test.versioned_optional_flow", "config": {
                    "panic_if_constructed": true
                }},
                later
            ]
        }))
        .unwrap();
        let plan = mf_compiler::plan_resolved_definition(&definition, &registry).unwrap();
        let saved = plan.clone();
        assert_eq!(saved.graph.execution_order[0].as_str(), "a_before");
        let error = validate_compiled_graph(&registry, &saved.graph).unwrap_err();
        let missing = if nested {
            match &error {
                WorkflowBuildError::Subgraph {
                    definition_id,
                    source,
                } => {
                    assert_eq!(definition_id.as_str(), "z_owner");
                    source.as_ref()
                }
                _ => panic!("expected receiving-scope attribution: {error:?}"),
            }
        } else {
            &error
        };
        assert!(
            matches!(missing, WorkflowBuildError::UnknownKind { definition_id, kind }
            if definition_id.as_str() == "z_missing" && kind == "test.missing_receiver")
        );
        assert!(instantiate_compiled(&saved, &registry).is_err());
    }
    let kind = "test.versioned_optional_flow";
    let config = json!({"body": {
        "version": "2026-10-28", "dependencies": {}, "nodes": []
    }})
    .to_string();
    // This registration's prepared factory panics: declaration validation must not invoke it.
    mf_runtime::generated::configuration(&registry, "owner", kind, &config, &["body"]).unwrap();
    for (config, expected) in [(config.as_str(), vec![]), ("{}", vec!["body"])] {
        let error =
            mf_runtime::generated::configuration(&registry, "owner", kind, config, &expected)
                .unwrap_err();
        assert!(
            matches!(error, mf_runtime::generated::BindError::Bindings { definition_id } if definition_id.as_str() == "owner")
        );
    }
    for construct in [false, true] {
        let error = if construct {
            let (registration, config) =
                mf_runtime::generated::configuration(&registry, "owner", kind, &config, &["body"])
                    .unwrap();
            mf_runtime::generated::instantiate(registration, "owner", config, BTreeMap::new())
                .err()
                .unwrap()
        } else {
            mf_runtime::generated::configuration(&registry, "owner", "missing.receiver", "{}", &[])
                .err()
                .unwrap()
        };
        if construct {
            assert!(
                matches!(error, mf_runtime::generated::BindError::Node { definition_id, .. } if definition_id.as_str() == "owner")
            );
        } else {
            assert!(
                matches!(error, mf_runtime::generated::BindError::Unknown { definition_id, kind } if definition_id.as_str() == "owner" && kind == "missing.receiver")
            );
        }
    }
    let error =
        mf_runtime::generated::configuration(&registry, "owner", kind, "{", &["body"]).unwrap_err();
    assert!(matches!(
        error,
        mf_runtime::generated::BindError::Configuration { .. }
    ));
    assert!(error.source().unwrap().is::<serde_json::Error>());
}
