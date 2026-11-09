extern crate mfn_core as _;

mod common;

use mf_compiler::{
    CompiledWorkflow, Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry,
    Outputs, PortSpec, TaskNode, ValueType, WorkflowCompileError, compile_definition,
    deserialize_config, instantiate_compiled, plan_resolved_workflow,
};
use mf_compiler::{DefinitionId, WorkflowDefinition};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct ConstantConfig {
    value: i64,
}

struct ConstantNode {
    value: i64,
}

impl TaskNode for ConstantNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), json!(self.value).into())])).into())
    }
}

struct IncrementNode;

impl TaskNode for IncrementNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let value = inputs["input"].as_i64().unwrap() + 1;
        Ok((Outputs::from([("value".to_owned(), json!(value).into())])).into())
    }
}

fn constant_factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value,
    };
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

fn increment_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = IncrementNode;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![PortSpec::new("input", ValueType::Number, true)],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

inventory::submit! {
    NodeRegistration { kind: "example.constant", factory: mf_runtime::NodeFactory::Plain(constant_factory) }
}

inventory::submit! {
    NodeRegistration { kind: "example.increment", factory: mf_runtime::NodeFactory::Plain(increment_factory) }
}

fn definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-26", "dependencies": {},
            "nodes":[
                {"id":"increment-two","kind":"example.increment"},
                {"id":"increment","kind":"example.increment"},
                {"id":"source","kind":"example.constant","config":{"value":41}}
            ],
            "edges":[
                {"from_node":"source","from_output":"value","to_node":"increment-two","to_input":"input"},
                {"from_node":"source","from_output":"value","to_node":"increment","to_input":"input"}
            ],
            "outputs":[
                {"name":"other","node":"increment-two","port":"value"},
                {"name":"answer","node":"increment","port":"value"}
            ]
        }"#,
    )
    .unwrap()
}

#[test]
fn generated_plan_round_trips_and_preserves_definition_semantics() {
    let definition = definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let generated = plan.generate_artifacts().unwrap();
    let reparsed = CompiledWorkflow::from_json(&generated.plan_json).unwrap();

    assert_eq!(reparsed, plan);
    let encoded: Value = serde_json::from_str(&generated.plan_json).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 1);
    assert!(encoded.get("graph").is_some());
    assert!(encoded["graph"].get("definition").is_some());
    assert!(encoded["graph"].get("execution_order").is_some());
    assert!(encoded["graph"].get("parameter_types").is_none());
    assert!(encoded["graph"].get("loop_orders").is_none());
    assert!(generated.rust_source.contains("pub fn run_workflow("));

    let original_nodes: BTreeMap<DefinitionId, (String, Value)> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), (node.kind.clone(), node.config.clone())))
        .collect();
    let generated_nodes: BTreeMap<DefinitionId, (String, Value)> = reparsed
        .graph
        .definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), (node.kind.clone(), node.config.clone())))
        .collect();
    assert_eq!(generated_nodes, original_nodes);

    let original_edges: BTreeSet<_> = definition
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from_node.clone(),
                edge.from_output.clone(),
                edge.to_node.clone(),
                edge.to_input.clone(),
            )
        })
        .collect();
    let generated_edges: BTreeSet<_> = reparsed
        .graph
        .definition
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from_node.clone(),
                edge.from_output.clone(),
                edge.to_node.clone(),
                edge.to_input.clone(),
            )
        })
        .collect();
    assert_eq!(generated_edges, original_edges);

    let original_outputs: BTreeSet<_> = definition
        .outputs
        .iter()
        .map(|output| {
            (
                output.name.clone(),
                output.node.clone(),
                output.port.clone(),
            )
        })
        .collect();
    let generated_outputs: BTreeSet<_> = reparsed
        .graph
        .definition
        .outputs
        .iter()
        .map(|output| {
            (
                output.name.clone(),
                output.node.clone(),
                output.port.clone(),
            )
        })
        .collect();
    assert_eq!(generated_outputs, original_outputs);

    let flow = instantiate_compiled(&reparsed, &registry).unwrap();
    let outputs = flow.execute().unwrap();
    assert_eq!(outputs["answer"], json!(42));
    assert_eq!(outputs["other"], json!(42));

    let mut reordered = definition;
    reordered.nodes.reverse();
    reordered.edges.reverse();
    reordered.outputs.reverse();
    let reordered_plan = compile_definition(&reordered, &registry).unwrap();
    let reordered_generated = reordered_plan.generate_artifacts().unwrap();
    assert_eq!(reordered_generated.plan_json, generated.plan_json);
    assert_eq!(reordered_generated.rust_source, generated.rust_source);
}

#[test]
fn plain_configuration_stays_opaque_in_saved_graphs_and_offline_descriptions() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition();
    let opaque = json!({
        "definition": {"version": "not-a-workflow", "nodes": []},
        "path": "missing-child.json",
        "body": {"builtin": "unlinked.child"}
    });
    definition.nodes[2].config["ordinary"] = opaque.clone();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert!(plan.graph.children.is_empty());
    let saved = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    let source = saved
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "source")
        .unwrap();
    assert_eq!(source.config["ordinary"], opaque);
    assert!(
        mf_compiler::describe_compiled(&saved)
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    assert_eq!(
        instantiate_compiled(&saved, &registry)
            .unwrap()
            .execute()
            .unwrap()["answer"],
        json!(42)
    );
}

#[test]
fn saved_plans_require_explicit_graph_and_reject_parameter_metadata() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let encoded = serde_json::to_value(&plan).unwrap();
    assert!(CompiledWorkflow::from_json(&encoded["graph"].to_string()).is_err());
    let mut missing_relations = encoded.clone();
    missing_relations["graph"]
        .as_object_mut()
        .unwrap()
        .remove("relations");
    assert!(matches!(
        CompiledWorkflow::from_json(&missing_relations.to_string()).unwrap_err(),
        mf_compiler::PlanError::Parse { source } if source.is_data()
    ));
    let mut missing_relations = serde_json::to_value(resolved(definition())).unwrap();
    missing_relations
        .as_object_mut()
        .unwrap()
        .remove("relations");
    assert!(serde_json::from_value::<mf_compiler::ResolvedWorkflow>(missing_relations).is_err());
    for field in ["parameter_types", "loop_orders"] {
        let mut legacy = encoded.clone();
        legacy[field] = json!({});
        assert!(CompiledWorkflow::from_json(&legacy.to_string()).is_err());
        let mut legacy = encoded.clone();
        legacy["graph"][field] = json!({});
        assert!(CompiledWorkflow::from_json(&legacy.to_string()).is_err());
    }
}

#[test]
fn rejects_a_plan_with_a_modified_execution_order() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&definition(), &registry).unwrap();
    plan.graph.execution_order.reverse();

    let error = instantiate_compiled(&plan, &registry).err().unwrap();
    assert!(matches!(error, WorkflowCompileError::NonCanonicalPlanOrder));
    #[cfg(feature = "codegen")]
    assert!(matches!(
        plan.generate_embedded_artifacts().unwrap_err(),
        mf_compiler::PlanError::InvalidExecutionOrder
    ));
}

fn resolved(definition: WorkflowDefinition) -> mf_compiler::ResolvedWorkflow {
    mf_compiler::ResolvedWorkflow {
        definition,
        relations: Default::default(),
        sources: BTreeSet::new(),
        children: BTreeMap::new(),
    }
}

fn resolved_graph(graph: &mf_runtime::CompiledGraph) -> mf_compiler::ResolvedWorkflow {
    mf_compiler::ResolvedWorkflow {
        definition: graph.definition.clone(),
        relations: graph.relations.clone(),
        sources: BTreeSet::new(),
        children: graph
            .children
            .iter()
            .map(|(owner, bindings)| {
                (
                    owner.clone(),
                    bindings
                        .iter()
                        .map(|(binding, child)| (binding.clone(), resolved_graph(child)))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn saved_structure_error(plan: &CompiledWorkflow) -> WorkflowCompileError {
    let error = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap_err();
    let mf_compiler::PlanError::Description {
        source: mf_compiler::DescriptionError::DescriptionOrder { path, source },
    } = error
    else {
        panic!("expected a saved-graph structure error, got {error:?}");
    };
    assert!(path.is_empty(), "unexpected description path: {path:?}");
    *source
}

fn assert_invalid_graph(error: WorkflowCompileError, owner: &str, expected: &str) {
    let WorkflowCompileError::InvalidGraph {
        definition_id,
        message,
    } = error
    else {
        panic!("expected InvalidGraph, got {error:?}");
    };
    assert_eq!(definition_id.as_str(), owner);
    assert_eq!(message, expected);
}

#[test]
fn resolved_handoffs_and_saved_graphs_validate_actual_binding_owners_and_keys() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let leaf = compile_definition(&definition(), &registry).unwrap();
    for case in [
        "absent owner",
        "empty bindings",
        "blank field",
        "scope collision",
        "foreign owner key",
        "foreign binding key",
        "unbound alias",
    ] {
        let mut authored = definition();
        let (owner, field) = match case {
            "absent owner" => ("missing", "/local"),
            "blank field" => ("source", " \t "),
            _ => ("source", "/local"),
        };
        if case == "scope collision" {
            let mut collision = authored.nodes[2].clone();
            collision.id =
                mf_runtime::ScopePathEntry::binding_key(collision.id.as_str(), field).into();
            authored.nodes.push(collision);
        }
        let mut plan = compile_definition(&authored, &registry).unwrap();
        let mut handoff = resolved(authored);
        let mut child = leaf.graph.clone();
        child.relations.key = match case {
            "foreign owner key" => Some(mf_runtime::ScopePathEntry::binding_key("other", field)),
            "foreign binding key" => Some(mf_runtime::ScopePathEntry::binding_key(owner, "other")),
            "unbound alias" => Some("unbound".into()),
            _ => None,
        };
        let bindings = if case == "empty bindings" {
            BTreeMap::new()
        } else {
            BTreeMap::from([(field.to_owned(), child.clone())])
        };
        plan.graph.children.insert(owner.into(), bindings);
        handoff.children.insert(
            owner.into(),
            if case == "empty bindings" {
                BTreeMap::new()
            } else {
                BTreeMap::from([(field.to_owned(), resolved_graph(&child))])
            },
        );
        let expected = match case {
            "absent owner" | "empty bindings" => {
                "child graphs require a known owner and nonempty binding set"
            }
            "blank field" => "receiving Flow parameters must be RFC 6901 pointers",
            "foreign owner key" | "foreign binding key" | "unbound alias" => {
                "scope keys must reference their actual receiving owner and binding"
            }
            _ => "Flow parameter body key collides with an authored node ID",
        };
        assert_invalid_graph(
            plan_resolved_workflow(&handoff).unwrap_err(),
            owner,
            expected,
        );
        // Offline inspection validates the binding itself, without projecting or
        // reparsing the owner's configuration through an authoring field setter.
        assert_invalid_graph(saved_structure_error(&plan), owner, expected);
    }
}

#[test]
fn authoring_rejects_noncanonical_loop_sources_before_lowering() {
    for (id, kind, config) in [
        ("source", "%loop", json!({})),
        ("%loop", "example.constant", json!({"value": 41})),
        ("%loop", "%loop", json!({})),
        ("%loop", "%loop", json!({"unexpected": true})),
    ] {
        let mut authored = definition();
        authored.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_28;
        authored.nodes.retain(|node| node.id.as_str() == "source");
        authored.edges.clear();
        authored.outputs.clear();
        authored.nodes[0].id = id.into();
        authored.nodes[0].kind = kind.into();
        authored.nodes[0].config = config;
        let expected_path = json!([id]).to_string();
        let expected = if kind == "%loop" {
            "reserved scope source requires its canonical enclosing scope"
        } else {
            "reserved scope source must use its canonical declaration"
        };
        let error = mf_compiler::plan_resolved_definition(
            &authored,
            &NodeRegistry::from_inventory().unwrap(),
        )
        .unwrap_err();
        assert!(
            matches!(
                &error,
                WorkflowCompileError::InvalidScope { path, message }
                    if path == &expected_path
                        && message == expected
            ),
            "unexpected source error for {id}/{kind}: {error:?}"
        );
    }
}

#[test]
fn saved_container_frames_require_known_instruction_endpoints_and_distinct_bindings() {
    for kind in [mf_runtime::LOOP_KIND, mf_runtime::ITERATION_KIND] {
        let mut body = definition();
        body.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_28;
        if kind == mf_runtime::LOOP_KIND {
            body.outputs.clear();
        } else {
            body.outputs.truncate(1);
            body.outputs[0].name = "result".into();
        }
        let mut authored = body.clone();
        authored.edges.clear();
        authored.outputs.clear();
        authored.nodes.truncate(1);
        authored.nodes[0].id = "container".into();
        authored.nodes[0].kind = kind.to_owned();
        authored.nodes[0].config = if kind == mf_runtime::LOOP_KIND {
            json!({"max_iterations": 1, "variables": [{"name": "count", "type": "int"}]})
        } else {
            json!({})
        };
        authored.nodes[0].set_node_flow("/body", body).unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = mf_compiler::plan_resolved_definition(&authored, &registry).unwrap();
        let mut handoff = resolved_graph(&plan.graph);
        assert_eq!(plan_resolved_workflow(&handoff).unwrap(), plan);
        assert_eq!(
            CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap(),
            plan
        );

        let mut old_streams = plan.clone();
        old_streams.graph.definition.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_25;
        old_streams
            .graph
            .children
            .get_mut(&"container".into())
            .unwrap()
            .get_mut("/body")
            .unwrap()
            .relations
            .streams
            .insert(
                "events".into(),
                mf_runtime::OutputReferenceDefinition {
                    node: "source".into(),
                    port: "events".into(),
                },
            );
        assert_invalid_graph(
            saved_structure_error(&old_streams),
            "container",
            "child stream exports require definition version 2026-10-28",
        );

        if kind == mf_runtime::LOOP_KIND {
            let template = handoff.clone();
            for depth in 2..=5 {
                // Reuse the actual lowered frame, not the container's authoring kind.
                let scope = &template.children[&"container".into()]["/body"];
                handoff.relations = scope.relations.clone();
                let input = &scope.relations.scope.as_ref().unwrap().input;
                handoff.definition.nodes.push(
                    scope
                        .definition
                        .nodes
                        .iter()
                        .find(|node| &node.id == input)
                        .unwrap()
                        .clone(),
                );
                let mut parent = template.clone();
                parent
                    .children
                    .get_mut(&"container".into())
                    .unwrap()
                    .insert("/body".to_owned(), handoff);
                handoff = parent;
                if depth <= 4 {
                    let nested = plan_resolved_workflow(&handoff).unwrap();
                    assert_eq!(
                        CompiledWorkflow::from_json(&nested.to_json().unwrap()).unwrap(),
                        nested
                    );
                } else {
                    let mut error = plan_resolved_workflow(&handoff).unwrap_err();
                    for _ in 0..depth {
                        let WorkflowCompileError::ChildGraph {
                            definition_id,
                            source,
                        } = error
                        else {
                            panic!("expected nested ChildGraph, got {error:?}");
                        };
                        assert_eq!(definition_id.as_str(), "container");
                        error = *source;
                    }
                    assert!(
                        matches!(
                            &error,
                            WorkflowCompileError::InvalidScope { path, message }
                                if path == &json!(vec!["container"; depth]).to_string()
                                    && message == "state-scope nesting exceeds four"
                        ),
                        "unexpected state-depth leaf: {error:?}"
                    );
                }
            }

            let mut inline = template.children[&"container".into()]["/body"]
                .definition
                .clone();
            for _ in 0..=mf_telemetry::MAX_SCOPE_DEPTH {
                let mut parent = template.definition.clone();
                parent.nodes[0].set_node_flow("/body", inline).unwrap();
                inline = parent;
            }
            let registry = NodeRegistry::from_inventory().unwrap();
            let mut error = mf_compiler::plan_resolved_definition(&inline, &registry).unwrap_err();
            for _ in 0..=mf_telemetry::MAX_SCOPE_DEPTH {
                let WorkflowCompileError::ChildGraph {
                    definition_id,
                    source,
                } = error
                else {
                    panic!("expected inline ChildGraph, got {error:?}");
                };
                assert_eq!(definition_id.as_str(), "container");
                error = *source;
            }
            assert_invalid_graph(
                error,
                "workflow",
                "combined container nesting exceeds twenty scopes",
            );
        } else {
            let mut unsupported = authored.clone();
            unsupported.nodes[0].config["body"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": "unsupported-loop", "kind": mf_runtime::LOOP_KIND}));
            // Raw kind restrictions belong to the authoring boundary, not saved graphs.
            let error = mf_compiler::plan_resolved_definition(&unsupported, &registry).unwrap_err();
            let WorkflowCompileError::ChildGraph {
                definition_id,
                source,
            } = &error
            else {
                panic!("expected the Iteration owner, got {error:?}");
            };
            assert_eq!(definition_id.as_str(), "container");
            assert!(matches!(
                source.as_ref(),
                WorkflowCompileError::ScopeStructure { source, .. }
                    if matches!(source, mf_compiler::ScopeStructureError::StateScope { node }
                        if node.as_str() == "unsupported-loop")
            ));
            assert!(matches!(
                std::error::Error::source(source.as_ref())
                    .unwrap()
                    .downcast_ref::<mf_compiler::ScopeStructureError>(),
                Some(mf_compiler::ScopeStructureError::StateScope { node })
                    if node.as_str() == "unsupported-loop"
            ));
        }

        let mut extra = plan.clone();
        let bindings = extra.graph.children.get_mut(&"container".into()).unwrap();
        bindings.insert("/extra".to_owned(), bindings["/body"].clone());
        assert_invalid_graph(
            saved_structure_error(&extra),
            "container",
            "Flow parameter body key collides with an authored node ID",
        );

        let mut missing = plan.clone();
        missing.graph.children.clear();
        // Provider-free inspection/generation cannot infer a required field from
        // kind/config. A selected receiving contract rejects the missing binding
        // before a factory is invoked, for both dynamic and generated execution.
        let saved = CompiledWorkflow::from_json(&missing.to_json().unwrap()).unwrap();
        assert!(saved.graph.children.is_empty());
        saved.generate_artifacts().unwrap();
        saved.generate_runner_artifacts().unwrap();
        let error = instantiate_compiled(&saved, &registry).err().unwrap();
        common::assert_receiver_binding_preflight(&error, "container");
        #[cfg(feature = "codegen")]
        for error in [
            saved.generate_execution_plans(&registry).unwrap_err(),
            saved
                .generate_runner_execution_plans(&registry)
                .unwrap_err(),
        ] {
            let mf_compiler::PlanError::Compilation { source } = error else {
                panic!("expected generated receiver-binding preflight, got {error:?}");
            };
            common::assert_receiver_binding_preflight(source.as_ref(), "container");
        }

        let mut missing_source = plan;
        let body = missing_source
            .graph
            .children
            .get_mut(&"container".into())
            .unwrap()
            .get_mut("/body")
            .unwrap();
        let source = if kind == mf_runtime::LOOP_KIND {
            mf_runtime::LOOP_SOURCE_ID
        } else {
            mf_runtime::ITERATION_INPUT_ID
        };
        body.definition
            .nodes
            .retain(|node| node.id.as_str() != source);
        body.execution_order.retain(|id| id.as_str() != source);
        let error = saved_structure_error(&missing_source);
        assert!(matches!(
            error,
            WorkflowCompileError::ChildGraph { definition_id, source }
                if definition_id.as_str() == "container"
                    && matches!(source.as_ref(), WorkflowCompileError::InvalidScope { path, message }
                        if path == "[\"container\"]"
                            && message == "scope instruction relationships must be disjoint and reference known nodes")
        ));
    }
}

#[test]
fn actual_child_trees_enforce_call_depth_before_the_combined_scope_limit() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let leaf = compile_definition(&definition(), &registry).unwrap();
    let mut handoff = resolved(definition());
    let mut plan = leaf.clone();
    for depth in 1..=mf_runtime::MAX_CALL_DEPTH + 1 {
        let mut parent = resolved(definition());
        parent.children.insert(
            "source".into(),
            BTreeMap::from([("/local".to_owned(), handoff)]),
        );
        handoff = parent;
        let mut parent = leaf.clone();
        parent.graph.children.insert(
            "source".into(),
            BTreeMap::from([("/local".to_owned(), plan.graph)]),
        );
        plan = parent;
        if depth == mf_runtime::MAX_CALL_DEPTH {
            assert!(plan_resolved_workflow(&handoff).is_ok());
            assert!(CompiledWorkflow::from_json(&plan.to_json().unwrap()).is_ok());
        }
    }
    // Callable trees exhaust their call budget before they can reach the larger
    // combined-scope budget. All entry points retain every actual owner wrapper.
    for mut error in [
        plan_resolved_workflow(&handoff).unwrap_err(),
        saved_structure_error(&plan),
        instantiate_compiled(&plan, &registry).err().unwrap(),
    ] {
        for _ in 0..=mf_runtime::MAX_CALL_DEPTH {
            let WorkflowCompileError::ChildGraph {
                definition_id,
                source,
            } = error
            else {
                panic!("expected nested ChildGraph, got {error:?}");
            };
            assert_eq!(definition_id.as_str(), "source");
            error = *source;
        }
        assert_invalid_graph(error, "workflow", "Flow nesting exceeds sixteen calls");
    }
}

#[test]
fn mixed_actual_frames_reach_the_combined_scope_limit_without_exhausting_other_budgets() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plain = compile_definition(&definition(), &registry).unwrap();
    let mut body = definition();
    body.version = mf_compiler::WorkflowDefinitionVersion::V2026_10_28;
    body.outputs.clear();
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {},
        "nodes": [{"id": "container", "kind": mf_runtime::LOOP_KIND, "config": {
            "max_iterations": 1, "variables": [{"name": "count", "type": "int"}],
            "body": body
        }}]
    }))
    .unwrap();
    let lowered = mf_compiler::plan_resolved_definition(&authored, &registry).unwrap();
    let state = &lowered.graph.children[&"container".into()]["/body"];
    let chain = |depth: usize| {
        // Four complete state frames, then a callable boundary that resets the
        // state-depth budget. At depth 21 there are only four calls in total.
        let at = |level: usize| {
            if level.is_multiple_of(5) {
                plain.graph.clone()
            } else {
                state.clone()
            }
        };
        let mut graph = at(depth);
        for level in (0..depth).rev() {
            let mut parent = at(level);
            // This attachment belongs to source/local, not the original template
            // owner. Use its generic binding key for every actual child frame.
            graph.relations.key = None;
            parent
                .children
                .insert("source".into(), BTreeMap::from([("/local".into(), graph)]));
            graph = parent;
        }
        CompiledWorkflow { graph }
    };
    let boundary = chain(mf_telemetry::MAX_SCOPE_DEPTH);
    let boundary = plan_resolved_workflow(&resolved_graph(&boundary.graph)).unwrap();
    assert_eq!(
        CompiledWorkflow::from_json(&boundary.to_json().unwrap()).unwrap(),
        boundary
    );

    let over = chain(mf_telemetry::MAX_SCOPE_DEPTH + 1);
    let mut error = plan_resolved_workflow(&resolved_graph(&over.graph)).unwrap_err();
    for _ in 0..=mf_telemetry::MAX_SCOPE_DEPTH {
        let WorkflowCompileError::ChildGraph {
            definition_id,
            source,
        } = error
        else {
            panic!("expected every mixed-scope owner, got {error:?}");
        };
        assert_eq!(definition_id.as_str(), "source");
        error = *source;
    }
    assert_invalid_graph(
        error,
        "workflow",
        "combined container nesting exceeds twenty scopes",
    );

    let mut expected_path = Vec::new();
    let mut graph = &over.graph;
    while let Some(bindings) = graph.children.get(&"source".into()) {
        let owner = graph
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "source")
            .unwrap();
        let child = &bindings["/local"];
        expected_path.push(child.child_scope_key(owner, "/local"));
        graph = child;
    }
    let error = CompiledWorkflow::from_json(&over.to_json().unwrap()).unwrap_err();
    let mf_compiler::PlanError::Description {
        source: mf_compiler::DescriptionError::DescriptionOrder { path, source },
    } = error
    else {
        panic!("expected the saved mixed-scope traversal error, got {error:?}");
    };
    assert_eq!(path, expected_path);
    let WorkflowCompileError::ChildGraph {
        definition_id,
        source,
    } = *source
    else {
        panic!("expected the saved boundary's typed owner");
    };
    assert_eq!(definition_id.as_str(), "source");
    assert_invalid_graph(
        *source,
        "workflow",
        "combined container nesting exceeds twenty scopes",
    );
}

#[test]
fn code_generation_escapes_definition_strings() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [{
            "id": "source\"quoted\n",
            "kind": "example.constant",
            "config": {"value": 3, "label": "quoted\"value\nnext line"}
        }],
        "outputs": [{
            "name": "answer\"quoted",
            "node": "source\"quoted\n",
            "port": "value"
        }]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let artifacts = plan.generate_artifacts().unwrap();
    let layouts = plan.generate_execution_plans(&registry).unwrap();

    syn::parse_file(&artifacts.rust_source).unwrap();
    syn::parse_file(&layouts.rust_source).unwrap();
    assert!(artifacts.rust_source.contains("source\\\"quoted\\n"));
    assert!(layouts.rust_source.contains("answer\\\"quoted"));
}
