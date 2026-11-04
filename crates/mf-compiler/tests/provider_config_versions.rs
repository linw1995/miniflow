//! Receiver declarations, not JSON shape or schema version, authorize executable children.
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{
    NodeBuildError, NodeConfig, NodeFactory, NodeRegistration, PreparedNode, PreparedSubgraph,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    assert_eq!(
        config["subgraphs"],
        json!({"provider_specific": "opaque data"})
    );
    NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(json!({"value": 42}))
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
    for marker_shaped in [false, true] {
        let mut authored = definition("2026-10-28");
        if marker_shaped {
            authored.nodes[0].config["body"] = json!({"$flow":{
                "version":"2026-10-28", "dependencies":{}, "nodes":[]
            }});
        }
        let resolved = mf_compiler::resolve_flows(&authored, None).unwrap();
        assert_eq!(resolved.definition, authored);
        let plan = compile_definition(&authored, &registry).unwrap();
        assert_eq!(plan.definition.nodes[0].config, authored.nodes[0].config);
        assert!(
            mf_compiler::describe_compiled(&plan)
                .unwrap()
                .loop_bodies
                .is_empty()
        );
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(outputs["value"].as_i64(), Some(42));
        #[cfg(feature = "codegen")]
        {
            let generated = plan.generate_artifacts().unwrap();
            syn::parse_file(&generated.rust_source).unwrap();
            assert!(
                !generated
                    .rust_source
                    .contains("instantiate_subgraphs_with_metadata")
            );
            let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
            let manifest =
                mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
            assert_eq!(
                manifest.node_definition("provider").unwrap().config,
                authored.nodes[0].config
            );
            assert!(manifest.description().unwrap().loop_bodies.is_empty());
        }
    }
}

#[test]
fn removed_root_flow_arguments_are_rejected_instead_of_ignored() {
    let mut value = serde_json::to_value(definition("2026-10-28")).unwrap();
    value["flow_arguments"] = json!({});
    let error = WorkflowDefinition::from_json(&value.to_string()).unwrap_err();
    assert!(error.to_string().contains("flow_arguments"), "{error}");
}

fn optional_flow_plain(_: Value) -> Result<PreparedNode, NodeBuildError> {
    NodeRegistry::from_inventory()
        .unwrap()
        .get("builtin.constant")
        .unwrap()
        .instantiate(json!({"value": 42}))
}

fn unexpected_prepared(
    _: &str,
    _: Value,
    _: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    panic!("a provider without selected children must use its plain factory")
}

inventory::submit! {
    NodeRegistration {
        kind: "test.versioned_optional_flow",
        factory: NodeFactory::Subgraph {
            parameters: &["body"],
            plain: optional_flow_plain,
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
            "nodes": [{"id": "provider", "kind": "test.versioned_optional_flow", "config": config}],
            "outputs": [{"name": "value", "node": "provider", "port": "value"}]
        }))
        .unwrap();
        let plan = compile_definition(&authored, &registry).unwrap();
        assert_eq!(plan.definition.nodes[0].config, authored.nodes[0].config);
        let outputs = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(outputs["value"].as_i64(), Some(42));
        #[cfg(feature = "codegen")]
        {
            let generated = plan.generate_artifacts().unwrap();
            assert!(
                generated
                    .rust_source
                    .contains("instantiate_node_with_metadata")
            );
            assert!(
                !generated
                    .rust_source
                    .contains("instantiate_subgraphs_with_metadata")
            );
            let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
            let manifest =
                mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
            assert_eq!(manifest.definition, plan.definition);
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
            parameters: TypedConfig::FLOW_PARAMETERS,
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
        "opaque": {"$flow": null, "definition": {"path": "not-a-file.json"}}
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
        assert_eq!(plan.definition.nodes[0].config, config);
        let graph = mf_compiler::describe_compiled(&plan).unwrap();
        assert_eq!(!graph.loop_bodies.is_empty(), executable);
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()["value"]
                .as_i64(),
            Some(42)
        );
        #[cfg(feature = "codegen")]
        {
            let generated = plan.generate_artifacts().unwrap();
            syn::parse_file(&generated.rust_source).unwrap();
            assert_eq!(
                generated
                    .rust_source
                    .contains("instantiate_subgraphs_with_metadata"),
                executable
            );
            let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
            let manifest =
                mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes).unwrap();
            assert_eq!(manifest.node_definition("owner").unwrap().config, config);
            assert_eq!(manifest.description().unwrap(), graph);
        }
    }
}

#[test]
fn resolved_planning_uses_selected_contracts_instead_of_ambient_host_metadata() {
    use std::error::Error;

    let registry = NodeRegistry::from_inventory().unwrap();
    assert_eq!(
        registry
            .get("test.declared_flow_owner")
            .unwrap()
            .flow_parameters(),
        ["body"]
    );
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "owner", "kind": "test.declared_flow_owner", "config": {
            "body": {"version": "2026-10-28", "dependencies": {}, "nodes": [
                {"id": "child", "kind": "builtin.constant", "config": {"value": 42}}
            ]}
        }}]
    }))
    .unwrap();
    // A selected external provider can have no Flow fields even though a linked
    // host registration of the same kind declares one. Never infer its contract.
    let ordinary = mf_compiler::plan_resolved_definition(&authored, &BTreeMap::new()).unwrap();
    let typed =
        mf_compiler::plan_resolved_definition(&authored, &registry.flow_parameter_contracts())
            .unwrap();
    assert!(ordinary.loop_orders.is_empty());
    assert_eq!(typed.loop_orders.len(), 1);
    assert_eq!(
        ordinary.definition.nodes[0].config,
        authored.nodes[0].config
    );
    assert!(
        mf_compiler::describe_compiled(&ordinary)
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    assert_eq!(
        mf_compiler::describe_compiled(&typed)
            .unwrap()
            .loop_bodies
            .len(),
        1
    );
    #[cfg(feature = "codegen")]
    {
        assert!(
            !ordinary
                .generate_artifacts()
                .unwrap()
                .rust_source
                .contains("instantiate_subgraphs_with_metadata")
        );
        assert!(
            typed
                .generate_artifacts()
                .unwrap()
                .rust_source
                .contains("instantiate_subgraphs_with_metadata")
        );
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
fn undeclared_fields_stay_opaque_even_when_the_factory_accepts_other_flow_fields() {
    let _lock = CHILD_CONSTRUCTION_LOCK.lock().unwrap();
    CHILD_CONSTRUCTIONS.store(0, Ordering::SeqCst);
    let registry = NodeRegistry::from_inventory().unwrap();
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "owner", "kind": "test.versioned_optional_flow", "config": {
            "undeclared": {"$flow": {
                "version": "2026-10-28", "dependencies": {},
                "nodes": [{"id": "child", "kind": "test.undeclared_flow_child"}]
            }}
        }}]
    }))
    .unwrap();
    let plan = mf_compiler::compile_resolved_definition(&authored, &registry).unwrap();
    assert_eq!(plan.definition, authored);
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
    use mf_compiler::{
        WorkflowBuildError, instantiate_subgraphs_with_metadata, validate_node_flow_parameters,
    };
    use std::error::Error;

    let registry = NodeRegistry::from_inventory().unwrap();
    let kind = "test.versioned_optional_flow";
    let config = json!({"body": {
        "version": "2026-10-28", "dependencies": {}, "nodes": []
    }})
    .to_string();
    // This registration's prepared factory panics: declaration validation must not invoke it.
    validate_node_flow_parameters(&registry, "owner", kind, &config, &["body"]).unwrap();
    for (config, expected) in [(config.as_str(), vec![]), ("{}", vec!["body"])] {
        let error =
            validate_node_flow_parameters(&registry, "owner", kind, config, &expected).unwrap_err();
        assert!(
            matches!(error, WorkflowBuildError::InvalidDefinition { definition_id, message }
            if definition_id.as_str() == "owner" && message.contains("compiled binding set"))
        );
    }
    for construct in [false, true] {
        let error = if construct {
            instantiate_subgraphs_with_metadata(
                &registry,
                "owner",
                "missing.receiver",
                "{}",
                BTreeMap::new(),
            )
            .err()
            .unwrap()
        } else {
            validate_node_flow_parameters(&registry, "owner", "missing.receiver", "{}", &[])
                .unwrap_err()
        };
        assert!(
            matches!(error, WorkflowBuildError::UnknownKind { definition_id, kind }
            if definition_id.as_str() == "owner" && kind == "missing.receiver")
        );
    }
    let error = instantiate_subgraphs_with_metadata(&registry, "owner", kind, "{", BTreeMap::new())
        .err()
        .unwrap();
    assert!(matches!(
        error,
        WorkflowBuildError::InvalidEmbeddedConfig { .. }
    ));
    assert!(error.source().unwrap().is::<serde_json::Error>());
    let error =
        instantiate_subgraphs_with_metadata(&registry, "owner", kind, &config, BTreeMap::new())
            .err()
            .unwrap();
    assert!(matches!(error, WorkflowBuildError::NodeConstruction { .. }));
    assert!(error.source().unwrap().is::<NodeBuildError>());
}
