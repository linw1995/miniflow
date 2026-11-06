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
        sources: BTreeSet::new(),
        children: BTreeMap::new(),
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

fn assert_invalid_call(error: WorkflowCompileError, owner: &str, expected: &str) {
    let WorkflowCompileError::InvalidCall {
        definition_id,
        message,
    } = error
    else {
        panic!("expected InvalidCall, got {error:?}");
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
    ] {
        let mut authored = definition();
        let (owner, field) = match case {
            "absent owner" => ("missing", "local"),
            "blank field" => ("source", " \t "),
            _ => ("source", "local"),
        };
        if case == "scope collision" {
            let mut collision = authored.nodes[2].clone();
            collision.id = collision.flow_scope_key(field).into();
            authored.nodes.push(collision);
        }
        let mut plan = compile_definition(&authored, &registry).unwrap();
        let mut handoff = resolved(authored);
        let bindings = if case == "empty bindings" {
            BTreeMap::new()
        } else {
            BTreeMap::from([(field.to_owned(), leaf.graph.clone())])
        };
        plan.graph.children.insert(owner.into(), bindings);
        handoff.children.insert(
            owner.into(),
            if case == "empty bindings" {
                BTreeMap::new()
            } else {
                BTreeMap::from([(field.to_owned(), resolved(definition()))])
            },
        );
        let expected = match case {
            "absent owner" | "empty bindings" => {
                "child graphs require a known owner and nonempty binding set"
            }
            "blank field" => "blank receiving Flow parameter declaration",
            _ => "Flow parameter body key collides with an authored node ID",
        };
        assert_invalid_call(
            plan_resolved_workflow(&handoff).unwrap_err(),
            owner,
            expected,
        );
        let saved_error = saved_structure_error(&plan);
        if case == "blank field" {
            // Projection writes actual children through the public field setter,
            // which rejects blank names before the later compiled-binding guard.
            assert!(matches!(
                saved_error,
                WorkflowCompileError::FlowParameters {
                    definition_id,
                    source: mf_runtime::FlowParameterError::Invalid { message },
                } if definition_id.as_str() == owner
                    && message == "Flow parameter field names must be nonblank"
            ));
        } else {
            assert_invalid_call(saved_error, owner, expected);
        }
    }
}

#[test]
fn resolved_handoffs_reject_noncanonical_loop_sources_before_freezing() {
    for (id, kind, config) in [
        ("source", "%loop", json!({})),
        ("%loop", "example.constant", json!({"value": 41})),
        ("%loop", "%loop", json!({})),
        ("%loop", "%loop", json!({"unexpected": true})),
    ] {
        let mut authored = definition();
        authored.nodes.retain(|node| node.id.as_str() == "source");
        authored.edges.clear();
        authored.outputs.clear();
        authored.nodes[0].id = id.into();
        authored.nodes[0].kind = kind.into();
        authored.nodes[0].config = config;
        let expected_path = format!("{:?}", authored.nodes[0].id);
        let error = plan_resolved_workflow(&resolved(authored)).unwrap_err();
        assert!(matches!(
            error,
            WorkflowCompileError::InvalidLoop { path, message }
                if path == expected_path
                    && message == "`%loop` is reserved for the canonical Loop body source"
        ));
    }
}

#[test]
fn saved_containers_require_their_actual_body_and_canonical_source() {
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
        let mut handoff = resolved(authored);
        handoff.children.insert(
            "container".into(),
            BTreeMap::from([("body".to_owned(), resolved(body))]),
        );
        let plan = plan_resolved_workflow(&handoff).unwrap();
        assert_eq!(
            CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap(),
            plan
        );

        let mut extra = plan.clone();
        let bindings = extra.graph.children.get_mut(&"container".into()).unwrap();
        bindings.insert("extra".to_owned(), bindings["body"].clone());
        assert_invalid_call(
            saved_structure_error(&extra),
            "container",
            "built-in container requires its compiled body binding",
        );

        let mut missing = plan.clone();
        missing.graph.children.clear();
        // The still-present authored body is not an executable fallback.
        let error = saved_structure_error(&missing);
        if kind == mf_runtime::LOOP_KIND {
            assert!(matches!(
                error,
                WorkflowCompileError::LoopConfiguration { path, .. }
                    if path == r#"["container"]"#
            ));
        } else {
            assert!(matches!(
                error,
                WorkflowCompileError::IterationConfiguration { definition_id, .. }
                    if definition_id.as_str() == "container"
            ));
        }

        let mut missing_source = plan;
        let body = missing_source
            .graph
            .children
            .get_mut(&"container".into())
            .unwrap()
            .get_mut("body")
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
        assert_invalid_call(
            saved_structure_error(&missing_source),
            "container",
            "compiled container body is missing its canonical source",
        );
    }
}

#[test]
fn actual_child_trees_enforce_normalization_projection_and_call_depth_limits() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let leaf = compile_definition(&definition(), &registry).unwrap();
    let mut handoff = resolved(definition());
    let mut plan = leaf.clone();
    for depth in 1..=mf_telemetry::MAX_SCOPE_DEPTH + 1 {
        let mut parent = resolved(definition());
        parent.children.insert(
            "source".into(),
            BTreeMap::from([("local".to_owned(), handoff)]),
        );
        handoff = parent;
        let mut parent = leaf.clone();
        parent.graph.children.insert(
            "source".into(),
            BTreeMap::from([("local".to_owned(), plan.graph)]),
        );
        plan = parent;
        if depth == mf_runtime::MAX_CALL_DEPTH {
            assert!(plan_resolved_workflow(&handoff).is_ok());
            assert!(CompiledWorkflow::from_json(&plan.to_json().unwrap()).is_ok());
        }
        if depth == mf_runtime::MAX_CALL_DEPTH + 1 {
            let mut error = saved_structure_error(&plan);
            for _ in 0..depth {
                let WorkflowCompileError::CallBody {
                    definition_id,
                    source,
                } = error
                else {
                    panic!("expected nested CallBody, got {error:?}");
                };
                assert_eq!(definition_id.as_str(), "source");
                error = *source;
            }
            assert_invalid_call(error, "workflow", "Flow nesting exceeds sixteen calls");
        }
    }
    let message = "combined container nesting exceeds twenty scopes";
    let mut error = plan_resolved_workflow(&handoff).unwrap_err();
    for _ in 0..=mf_telemetry::MAX_SCOPE_DEPTH {
        let WorkflowCompileError::CallBody {
            definition_id,
            source,
        } = error
        else {
            panic!("expected nested CallBody, got {error:?}");
        };
        assert_eq!(definition_id.as_str(), "source");
        error = *source;
    }
    assert_invalid_call(error, "workflow", message);
    // Preparation projects the actual tree before node construction; no private
    // depth counter is injected to bypass the public traversal.
    assert_invalid_call(
        instantiate_compiled(&plan, &registry).err().unwrap(),
        "workflow",
        message,
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
