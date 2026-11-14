//! An external provider: no compiler hooks, built-in call executor, or LLM required.
extern crate mfn_core as _;

#[path = "fixtures/observation_capture.rs"]
mod capture;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePortContract, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    PreparedSubgraph, TaskNode, ValueType, WorkflowArguments,
};
use mf_runtime::{Flow, NodeConfig};
use mf_telemetry::{
    description::WorkflowDescription,
    event::{Event, ScopePathEntry},
    identity::RunId,
};
use serde_json::{Value, json};
use snafu::{OptionExt, ensure};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

const KIND: &str = "test.custom_flow_arguments";

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct Config {
    // Each field owns its source; there is no ambient parameter namespace.
    #[serde(default)]
    left: Option<Flow>,
    #[serde(default)]
    right: Option<Flow>,
    // Deliberately provider-specific: generic expansion must not parse a built-in's config.
    label: String,
}

struct CustomNode {
    label: String,
    arguments: BTreeMap<String, (Flow, Vec<mf_telemetry::event::NodeIdentity>, String)>,
}

impl NodePortContract for CustomNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: if !self.arguments.is_empty() {
                vec![PortSpec::new("select", ValueType::String, true)]
            } else {
                vec![]
            },
            outputs: vec![PortSpec::new("content", ValueType::String, true)],
        }
    }
}

impl TaskNode for CustomNode {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let outputs = if let Some(selector) = inputs.get("select") {
            let name = selector.as_str().expect("validated string port");
            let (body, nodes, scope_key) =
                self.arguments
                    .get(name)
                    .with_context(|| mf_runtime::NodeExecutionFailedSnafu {
                        message: format!("unknown local custom body `{name}`"),
                    })?;
            // An external provider can create the same binding-aware scope as a built-in.
            context.run_call(scope_key, WorkflowArguments::default(), nodes, |child| {
                body.execute_in_context(child)
            })?
        } else {
            Outputs::from([("content".into(), self.label.clone().into())])
        };
        Ok(NodeResult {
            outputs,
            ..Default::default()
        })
    }
}

fn plain(config: Value) -> Result<PreparedNode, NodeBuildError> {
    prepared("", config, BTreeMap::new())
}

fn prepared(
    id: &str,
    config: Value,
    arguments: BTreeMap<String, PreparedSubgraph>,
) -> Result<PreparedNode, NodeBuildError> {
    // Identities are observation metadata, not an alternate source of executable bodies.
    let mut identities: BTreeMap<_, _> = arguments
        .iter()
        .map(|(name, body)| (name.clone(), (body.nodes.clone(), body.scope_key(id, name))))
        .collect();
    let config = Config::from_config(config, arguments)?;
    let mut arguments = BTreeMap::new();
    for (name, body) in [("left", config.left), ("right", config.right)] {
        if let Some(body) = body {
            let outputs: Vec<_> = body
                .plan()
                .outputs
                .iter()
                .map(|output| {
                    let port = body
                        .node_metadata(&output.node_id)
                        .unwrap()
                        .ports
                        .outputs
                        .iter()
                        .find(|port| port.name == output.port)
                        .unwrap();
                    PortSpec::owned(
                        output.name.as_ref(),
                        port.value_type.clone(),
                        !output.optional,
                    )
                })
                .collect();
            ensure!(
                body.input_schema().inputs.values().all(BTreeMap::is_empty)
                    && body.input_schema().stdin.is_empty()
                    && body.stream_output_ports().is_empty()
                    && outputs == vec![PortSpec::new("content", ValueType::String, true)],
                mf_runtime::NodeInvalidSubgraphSnafu {
                    message: "custom body must be a terminal string-content producer without inputs"
                }
            );
            arguments.insert(name.into(), {
                let (nodes, scope_key) = identities.remove(name).unwrap();
                (body, nodes, scope_key)
            });
        }
    }
    PreparedNode::new(
        CustomNode {
            label: config.label,
            arguments,
        },
        NodeMetadata::default(),
    )
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        factory: NodeFactory::Subgraph { scope: None, flow_parameters: Config::flow_parameters, plain, prepared },
    }
}

fn child(content: &str) -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"leaf", "kind":"builtin.constant", "config":{"value":content}}],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    })
}

fn caller() -> Value {
    json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{
                "parameters":{"select":{"type":"string", "secret":false}}
            }},
            {"id":"custom", "kind":KIND, "config":{
                "label":"provider-specific config survives expansion",
                "left":{"definition":child("one")}, "right":{"definition":child("two")}
            }, "inputs":{"select":{"from":{"node":"params", "port":"select"}}}}
        ],
        "outputs":[{"name":"content", "node":"custom", "port":"content"}]
    })
}

fn definition(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

fn selection(value: Value) -> WorkflowArguments {
    WorkflowArguments::try_from(json!({"params.select":value})).unwrap()
}

fn assert_binding_bodies(graph: &WorkflowDescription, aliases: &[&str]) {
    let expected: Vec<_> = aliases
        .iter()
        .map(|alias| {
            let scope = ScopePathEntry::Call {
                call_id: ScopePathEntry::binding_key("custom", alias),
            };
            graph
                .validate_scope_path(std::slice::from_ref(&scope))
                .unwrap();
            let path = vec![scope.node_id().to_owned()];
            let body = graph.body(&path).unwrap();
            assert_eq!(body.nodes.len(), 1);
            assert_eq!(body.nodes[0].id, "leaf");
            path
        })
        .collect();
    assert_eq!(graph.loop_bodies.len(), aliases.len());
    assert_eq!(
        graph.body_paths_for_node(&[], "custom"),
        expected.iter().collect::<Vec<_>>()
    );
    assert!(graph.body_paths_for_node(&[], "params").is_empty());
    assert!(graph.body(&["custom".into()]).is_none());
}

#[test]
fn external_registration_supports_both_plain_and_prepared_factories() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get(KIND).unwrap();
    let NodeFactory::Subgraph {
        scope: None,
        flow_parameters,
        ..
    } = registration.factory
    else {
        panic!("custom provider must declare its Flow parameter contract");
    };
    assert!(flow_parameters(&json!({"label":"plain"})).is_empty());
    let node = registration.instantiate(json!({"label":"plain"})).unwrap();
    assert!(node.metadata.ports.inputs.is_empty());
    assert_eq!(
        node.metadata.ports.outputs,
        vec![PortSpec::new("content", ValueType::String, true)]
    );
    let outputs = node
        .execution
        .as_task_node()
        .unwrap()
        .execute(Inputs::default(), &mut ExecutionContext::default())
        .unwrap()
        .outputs;
    assert_eq!(outputs["content"].as_str(), Some("plain"));

    let plan = compile_definition(&definition(caller()), &registry).unwrap();
    assert_eq!(
        plan.graph.definition.nodes[1]
            .flow_parameters(
                &Config::flow_parameters(&plan.graph.definition.nodes[1].config)
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            )
            .unwrap()
            .len(),
        2
    );
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(
        graph.nodes.len(),
        2,
        "passed bodies must not become wrapper tasks"
    );
    assert_binding_bodies(&graph, &["left", "right"]);
    let frozen =
        WorkflowDefinition::from_json(&serde_json::to_string(&plan.graph.definition).unwrap())
            .unwrap();
    let rebuilt = compile_definition(&frozen, &registry).unwrap();
    assert_eq!(rebuilt.graph.definition, plan.graph.definition);
    let flow = instantiate_compiled(&rebuilt, &registry).unwrap();
    for (name, expected) in [("left", "one"), ("right", "two"), ("left", "one")] {
        let outputs = flow.execute_with_inputs(selection(json!(name))).unwrap();
        assert_eq!(outputs["content"].as_str(), Some(expected));
    }
}

#[cfg(feature = "codegen")]
#[test]
fn generated_constructor_preserves_the_custom_kind_and_local_binding_bodies() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(caller()), &registry).unwrap();
    let generated = plan.generate_artifacts().unwrap();
    syn::parse_file(&generated.rust_source).unwrap();
    assert!(
        generated
            .rust_source
            .contains("instantiate_subgraphs_with_metadata")
    );
    assert!(generated.rust_source.contains(KIND));
    assert!(
        generated
            .rust_source
            .contains("PreparedSubgraph::from_flow")
    );
    let execution = plan.generate_execution_plans(&registry).unwrap();
    let manifest = mf_runtime::WorkflowManifest::from_bytes(&execution.manifest_bytes).unwrap();
    let custom = manifest.node_definition("custom").unwrap();
    assert_eq!(custom.kind, KIND);
    assert_eq!(custom.config, plan.graph.definition.nodes[1].config);
    assert_eq!(
        manifest.description().unwrap(),
        mf_compiler::describe_compiled(&plan).unwrap()
    );
}

#[test]
fn selected_binding_observation_preserves_child_lifecycle_and_static_body() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(caller()), &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_binding_bodies(&graph, &["left", "right"]);
    for (name, expected) in [("left", "one"), ("right", "two")] {
        let harness = capture::Harness::new(true);
        let observation = harness
            .observer()
            .start_with_description(graph.clone(), RunId::new())
            .unwrap();
        let outputs = mf_compiler::execute_compiled_with_inputs(
            &plan,
            &registry,
            selection(json!(name)),
            Some(observation),
        )
        .unwrap();
        assert_eq!(outputs["content"].as_str(), Some(expected));
        let mut started = Vec::new();
        let mut finished = Vec::new();
        for record in harness.records() {
            let event = record.decode().unwrap();
            event.validate_for(&graph).unwrap();
            match event.event {
                Event::NodeStarted { node, .. } if node.id == "leaf" => started.push(node.path),
                Event::NodeFinished { node, .. } if node.id == "leaf" => finished.push(node.path),
                _ => {}
            }
        }
        assert_eq!(
            finished, started,
            "the selected child must complete in its binding scope"
        );
        assert_eq!(
            started,
            vec![vec![ScopePathEntry::Call {
                call_id: ScopePathEntry::binding_key("custom", name)
            }]],
            "only the selected alias may run"
        );
        assert!(!harness.spans.get_finished_spans().unwrap().is_empty());
    }
}

#[test]
fn custom_handles_are_local_fields_not_global_names_or_executable_json() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    registry
        .register_flow(
            "host-only",
            definition(child("host")),
            mf_runtime::PreparedSubgraph::new(vec![], vec![], |_| {
                panic!("unpassed host executor ran")
            }),
        )
        .unwrap();
    let plan = compile_definition(&definition(caller()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for name in ["first", "second", "host-only", "unknown"] {
        let error = flow
            .execute_with_inputs(selection(json!(name)))
            .unwrap_err();
        assert!(
            error.to_string().contains("unknown local custom body"),
            "{error}"
        );
    }
    for forged in [
        child("forged"),
        json!({"definition":child("forged")}),
        json!(null),
    ] {
        assert!(flow.execute_with_inputs(selection(forged)).is_err());
    }

    // Static calls remain supported, and each nested consumer owns its direct Flow fields.
    let mut nested = caller();
    nested["nodes"][0]["config"]["parameters"]["select"]["default"] = json!("left");
    let mut outer = caller();
    outer["nodes"] = json!([{"id":"invoke", "kind":"workflow.call", "config":{
        "flow":"nested", "body":nested
    }}]);
    outer["outputs"] = json!([{"name":"content", "node":"invoke", "port":"content"}]);
    let plan = compile_definition(&definition(outer.clone()), &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::default())
            .unwrap()["content"]
            .as_str(),
        Some("one")
    );
    outer["nodes"][0]["config"]["body"]["nodes"][1]["config"]["left"] =
        json!({"parameter":"first"});
    assert!(compile_definition(&definition(outer), &registry).is_err());
}

#[test]
fn all_direct_bodies_are_prepared_and_validated_before_runtime_selection() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for name in ["left", "right"] {
        let mut invalid_provider = caller();
        invalid_provider["nodes"][1]["config"][name]["definition"]["nodes"][0]["kind"] =
            json!("missing.provider");
        let error = compile_definition(&definition(invalid_provider), &registry).unwrap_err();
        assert!(error.to_string().contains("missing.provider"), "{error}");

        let mut invalid_interface = caller();
        invalid_interface["nodes"][1]["config"][name]["definition"]["nodes"][0]["config"]["value"] =
            json!(42);
        let error = compile_definition(&definition(invalid_interface), &registry).unwrap_err();
        assert!(error.to_string().contains("custom body must be"), "{error}");

        // Empty per-node input maps are harmless, but an actual startup port is not.
        let mut startup_input = caller();
        startup_input["nodes"][1]["config"][name]["definition"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(
                json!({"id":"startup", "kind":"builtin.parameters", "config":{
                    "parameters":{"required":{"type":"string", "secret":false}}
                }}),
            );
        let error = compile_definition(&definition(startup_input), &registry).unwrap_err();
        assert!(error.to_string().contains("custom body must be"), "{error}");
    }
    let mut unknown = caller();
    unknown["nodes"][1]["config"]["right"] = json!({"parameter":"not-passed"});
    assert!(compile_definition(&definition(unknown), &registry).is_err());

    // Removing a local field exposes no handle for that binding.
    let mut limited = caller();
    limited["nodes"][1]["config"]
        .as_object_mut()
        .unwrap()
        .remove("right");
    let plan = compile_definition(&definition(limited), &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_binding_bodies(&graph, &["left"]);
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(
        flow.execute_with_inputs(selection(json!("left"))).unwrap()["content"].as_str(),
        Some("one")
    );
    assert!(flow.execute_with_inputs(selection(json!("right"))).is_err());
}

#[test]
fn ambient_registry_alias_cannot_capture_a_direct_parameter() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    for alias in ["first", "second", "left", "right"] {
        let count = executions.clone();
        registry
            .register_flow(
                alias,
                definition(child("one")),
                PreparedSubgraph::new(
                    vec![],
                    vec![PortSpec::new("content", ValueType::String, true)],
                    move |_| {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok([("content".into(), "captured".into())].into())
                    },
                ),
            )
            .unwrap();
    }
    let plan = compile_definition(&definition(caller()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (binding, expected) in [("left", "one"), ("right", "two")] {
        assert_eq!(
            flow.execute_with_inputs(selection(json!(binding))).unwrap()["content"].as_str(),
            Some(expected)
        );
    }
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let mut unavailable = caller();
    unavailable["nodes"][1]["config"]["left"]["definition"]["nodes"][0]["kind"] =
        json!("unlinked.provider");
    let error = compile_definition(&definition(unavailable), &registry).unwrap_err();
    assert!(error.to_string().contains("unlinked.provider"), "{error}");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn generated_flow_argument_constructor_reports_missing_provider_and_invalid_config() {
    use mf_compiler::{WorkflowBuildError, instantiate_subgraphs_with_metadata};

    let registry = NodeRegistry::from_inventory().unwrap();
    let construct = |kind, config| {
        instantiate_subgraphs_with_metadata(&registry, "custom", kind, config, BTreeMap::new())
            .err()
            .unwrap()
    };
    assert!(matches!(
        construct("missing.provider", "{}"),
        WorkflowBuildError::UnknownKind { definition_id, kind }
            if definition_id.as_str() == "custom" && kind == "missing.provider"
    ));
    assert!(matches!(
        construct(KIND, "{"),
        WorkflowBuildError::InvalidEmbeddedConfig { definition_id, .. }
            if definition_id.as_str() == "custom"
    ));
}

#[test]
fn selected_body_validation_failure_propagates_through_the_external_call_binding() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = caller();
    value["nodes"][1]["config"]["left"]["definition"] = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[
            {"id":"leaf", "kind":"builtin.constant", "config":{"value":"unapproved"}},
            {"id":"validate", "kind":"builtin.assert", "config":{
                "message":"custom content is not approved",
                "condition":{"source":{"output":"leaf.value", "path":""}, "operator":"eq", "value":"approved"}
            }},
            {"id":"publish", "kind":"builtin.identity", "inputs":{
                "input":{"from":{"node":"leaf", "port":"value"}}
            }}
        ],
        "edges":[{"from_node":"leaf", "to_node":"validate"}, {"from_node":"validate", "to_node":"publish"}],
        "outputs":[{"name":"content", "node":"publish", "port":"value"}]
    });
    let plan = compile_definition(&definition(value.clone()), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    // A failed mapped body must fail its caller, not silently return the label or another body.
    let error = flow
        .execute_with_inputs(selection(json!("left")))
        .unwrap_err();
    assert!(
        error.to_string().contains("custom content is not approved"),
        "{error}"
    );
    assert_eq!(
        flow.execute_with_inputs(selection(json!("right"))).unwrap()["content"].as_str(),
        Some("two")
    );
    value["nodes"][1]["config"]["left"]["definition"]["nodes"][0]["config"]["value"] =
        json!("approved");
    let plan = compile_definition(&definition(value), &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(selection(json!("left")))
            .unwrap()["content"]
            .as_str(),
        Some("approved")
    );
}

#[test]
fn all_task_body_accepts_explicit_execution_configuration() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = caller();
    value["nodes"][1]["config"]["right"]["definition"]["execution"] = json!({});
    let plan = compile_definition(&definition(value), &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(selection(json!("right")))
            .unwrap()["content"]
            .as_str(),
        Some("two")
    );
}

#[test]
fn binding_scope_keys_must_not_shadow_authored_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut collision = caller();
    let key = ScopePathEntry::binding_key("custom", "right");
    collision["nodes"].as_array_mut().unwrap().push(json!({
        "id":key, "kind":"builtin.constant", "config":{"value":"authored"}
    }));
    let error = compile_definition(&definition(collision), &registry).unwrap_err();
    assert!(matches!(
        &error,
        mf_compiler::WorkflowCompileError::InvalidGraph { definition_id, .. }
            if definition_id.as_str() == "custom"
    ));
    assert!(
        error
            .to_string()
            .contains("body key collides with an authored node ID"),
        "{error}"
    );
}

#[test]
fn iteration_checks_all_self_contained_flow_fields_for_transitive_containers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut nested = caller();
    nested["nodes"][0] =
        json!({"id":"params", "kind":"builtin.constant", "config":{"value":"left"}});
    nested["nodes"][1]["inputs"]["select"]["from"]["port"] = json!("value");
    let mut value = caller();
    value["nodes"] = json!([
        {"id":"items", "kind":"builtin.constant", "config":{"value":["left", "left"]}},
        {"id":"each", "kind":"builtin.iteration", "config":{"body":{"version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"custom", "kind":KIND, "config":{
                "label":"iteration caller", "left":{"definition":nested}
            }, "inputs":{"select":{"from":{"node":"%iteration", "port":"item"}}}}],
            "outputs": [{"name":"result", "node":"custom", "port":"content"}]
        }}, "inputs":{"items":{"from":{"node":"items", "port":"value"}}}}
    ]);
    value["outputs"] = json!([{"name":"results", "node":"each", "port":"results"}]);
    let plan = compile_definition(&definition(value.clone()), &registry).unwrap();
    assert_eq!(
        serde_json::to_value(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        json!({"results":["one", "one"]})
    );

    // The inner constant selects left, but right is also present and cannot hide a Loop.
    value["nodes"][1]["config"]["body"]["nodes"][0]["config"]["left"]["definition"]["nodes"][1]["config"]
        ["right"]["definition"] = json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"repeat", "kind":"loop.run", "config":{
            "max_iterations":1, "variables":[{"name":"content", "type":"string"}], "body":{"version":"2026-10-28", "dependencies":{}, "nodes":[]}
        }}],
        "outputs":[{"name":"content", "node":"repeat", "port":"content"}]
    });
    let error = compile_definition(&definition(value), &registry).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("child scopes inside an item scope cannot contain container scopes"),
        "{error}"
    );
}

#[test]
fn describing_a_damaged_nested_call_reports_the_owning_binding_path() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&definition(caller()), &registry).unwrap();
    // Compiled plans are public data: description must diagnose a damaged frozen child.
    plan.graph
        .children
        .get_mut(&"custom".into())
        .unwrap()
        .get_mut("left")
        .unwrap()
        .execution_order
        .push("missing-child".into());
    let error = mf_compiler::describe_compiled(&plan).unwrap_err();
    assert!(
        matches!(
            &error,
            mf_compiler::DescriptionError::DescriptionOrder { path, source }
                if path == &[ScopePathEntry::binding_key("custom", "left")]
                    && matches!(source.as_ref(), mf_compiler::WorkflowCompileError::ChildGraph { definition_id, source }
                        if definition_id.as_str() == "custom" && matches!(source.as_ref(), mf_compiler::WorkflowCompileError::NonCanonicalPlanOrder))
        ),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("execution order does not match its definition"),
        "{error}"
    );
}

#[cfg(feature = "codegen")]
#[test]
fn codegen_uses_the_actual_child_graph_instead_of_reparsing_duplicate_json() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&definition(caller()), &registry).unwrap();
    let mut body: WorkflowDefinition =
        serde_json::from_value(plan.graph.definition.nodes[1].config["left"].clone()).unwrap();
    body.nodes[0].kind = mf_runtime::LOOP_KIND.into();
    body.nodes[0].config = json!({});
    plan.graph.definition.nodes[1].config["left"] = serde_json::to_value(body).unwrap();
    let generated = plan.generate_artifacts().unwrap();
    syn::parse_file(&generated.rust_source).unwrap();
    let manifest = plan.manifest(&registry).unwrap();
    assert_eq!(manifest.graph.children, plan.graph.children);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(selection(json!("left")))
            .unwrap()["content"]
            .as_str(),
        Some("one")
    );
}
