//! Node state belongs to providers; assignments retain only target relationships.
extern crate mfn_core as _;

#[path = "fixtures/flow-arguments-provider/src/lib.rs"]
mod provider;

use mf_compiler::{
    CompiledWorkflow, NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled,
};
use mf_runtime::{
    ExecutionContext, ExecutionScope, Outputs, ValueRef, ValueType, WorkflowArguments,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};

fn definition(target: &str) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-11-13",
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                "value":{"type":"int", "default":7, "secret":false}
            }}},
            {"id":"write", "kind":"workflow.assign", "config":{"variable":target},
                "inputs":{"value":{"from":{"node":"params", "port":"value"}}}},
            {"id":"counter", "kind":provider::STATE_KIND, "config":{"initial":1, "limit":10}}
        ],
        "edges":[{"from_node":"write", "from_output":"done", "to_node":"counter"}],
        "outputs":[{"name":"count", "node":"counter", "port":"count"}]
    }))
    .unwrap()
}

#[test]
fn external_state_contracts_survive_saved_plans_and_instruction_relabeling() {
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(registry.flow_bindings().contains_key(provider::STATE_KIND));
    let mut plan = compile_definition(&definition("%counter.count"), &registry).unwrap();
    let writer = plan
        .graph
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "write")
        .unwrap();
    writer.kind = "fixture.unregistered-instruction".into();
    assert_eq!(writer.config, json!({}));
    assert!(matches!(
        writer.operation.as_ref(),
        Some(mf_runtime::NodeOperation::Assign { .. })
    ));
    let saved = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    let flow = instantiate_compiled(&saved, &registry).unwrap();
    for value in [3, 9, 2] {
        let arguments = WorkflowArguments(BTreeMap::from([(
            "params.value".into(),
            ValueRef::from(value).into_secret(),
        )]));
        let result = flow.execute_with_inputs(arguments).unwrap();
        assert_eq!(result["count"], json!(value));
        assert!(result["count"].is_secret());
    }
    #[cfg(feature = "codegen")]
    saved.generate_runner_execution_plans(&registry).unwrap();
}

#[test]
fn compiled_assignment_has_one_target_and_authored_input_cannot_supply_operations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let source = definition("%counter.count");
    let plan = compile_definition(&source, &registry).unwrap();
    let encoded = serde_json::to_value(&plan).unwrap();
    let writer = encoded["graph"]["definition"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "write")
        .unwrap();
    assert_eq!(writer["config"], json!({}));
    assert_eq!(
        writer["operation"],
        json!({"op":"assign", "target":{"scope":"node", "node":"counter", "field":"count"}})
    );
    assert!(encoded["graph"]["relations"].get("writes").is_none());
    let mut forged = source;
    let writer = forged
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "write")
        .unwrap();
    writer.operation = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "write")
        .unwrap()
        .operation
        .clone();
    writer.config = json!({});
    let error = compile_definition(&forged, &registry).unwrap_err();
    assert!(
        matches!(error, mf_compiler::WorkflowCompileError::InvalidScope { path, message }
        if path == "[\"write\"]" && message == "compiled node operations are not authoring syntax")
    );
}

#[test]
fn rejected_provider_writes_preserve_values_and_typed_error_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition("%counter.count"), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    context.node_state("%counter", Outputs::from([("count".into(), 4.into())]));
    context
        .set_workflow_arguments(WorkflowArguments::try_from(json!({"params.value":11})).unwrap());
    let error = flow.execute_in_context(&mut context).unwrap_err();
    let mut source: &(dyn Error + 'static) = &error;
    loop {
        if let Some(provider::StateError::Limit { limit }) =
            source.downcast_ref::<provider::StateError>()
        {
            assert_eq!(*limit, 10);
            break;
        }
        source = source
            .source()
            .expect("provider error must retain its typed source");
    }
    assert_eq!(
        context.node_state("%counter", Outputs::new())["count"],
        json!(4)
    );
}

#[test]
fn state_writes_are_isolated_across_calls_and_container_frames() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition("%counter.count");
    let input = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "params")
        .unwrap();
    input.kind = "builtin.constant".into();
    input.config = json!({"value":7});
    let plan = compile_definition(&source, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    context.node_state("%counter", Outputs::from([("count".into(), 4.into())]));
    let result = context
        .run_call("invoke", Default::default(), &[], |child| {
            flow.execute_in_context(child)
        })
        .unwrap();
    assert_eq!(result["count"], json!(7));
    assert_eq!(
        context.node_state("%counter", Outputs::new())["count"],
        json!(4)
    );
    let scope =
        ExecutionScope::new("each", "%iteration", 0, Outputs::new(), BTreeMap::new()).unwrap();
    let (result, _, _) = context
        .run_scope(scope, |child| flow.execute_in_context(child))
        .unwrap();
    assert_eq!(result["count"], json!(7));
    assert_eq!(
        context.node_state("%counter", Outputs::new())["count"],
        json!(4)
    );
}

fn frame_program(
    types: BTreeMap<String, ValueType>,
    state_outputs: BTreeMap<String, String>,
    field: &str,
    output: &str,
    exit: bool,
) -> mf_runtime::PreparedSubgraph {
    let source = mf_runtime::prepared_scope_source("input", &types);
    let update = if exit {
        mf_runtime::prepared_loop_exit("update")
    } else {
        mf_runtime::prepared_assign(
            "update",
            mf_runtime::StateTarget::Frame {
                field: field.into(),
            },
            types[field].clone(),
            None,
        )
        .unwrap()
    };
    let edges = if exit {
        vec![]
    } else {
        vec![mf_runtime::EdgeDefinition {
            exclusive: false,
            from_node: "input".into(),
            from_output: field.into(),
            to_node: "update".into(),
            to_input: "value".into(),
        }]
    };
    let flow = mf_compiler::build_flow(
        vec![source, update],
        edges,
        vec!["input".into(), "update".into()],
        vec![mf_runtime::WorkflowOutputDefinition {
            name: output.to_owned().into(),
            node: "input".into(),
            port: field.to_owned().into(),
            optional: false,
        }],
    )
    .unwrap();
    mf_runtime::PreparedSubgraph::from_flow(
        flow,
        vec![
            mf_runtime::NodeIdentity {
                id: "input".into(),
                kind: "scope.source".into(),
                path: vec![],
            },
            mf_runtime::NodeIdentity {
                id: "update".into(),
                kind: "scope.update".into(),
                path: vec![],
            },
        ],
    )
    .with_relations(mf_runtime::GraphRelations {
        scope: Some(mf_runtime::ScopeRelations {
            input: "input".into(),
            values: types,
            state_outputs,
        }),
        ..Default::default()
    })
}

#[test]
fn scoped_receivers_validate_operations_from_actual_flow_executors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let body = frame_program(
        BTreeMap::from([("count".into(), ValueType::Int64)]),
        BTreeMap::from([("count".into(), "count".into())]),
        "count",
        "count",
        false,
    );
    let error = registry
        .get("loop.run")
        .unwrap()
        .instantiate_subgraphs(
            "repeat",
            json!({"max_iterations":1,"variables":[{"name":"count","type":"int"}],"body":{}}),
            BTreeMap::from([("body".into(), body)]),
        )
        .err()
        .unwrap();
    let mf_runtime::NodeBuildError::FactoryFailed { source } = error else {
        panic!("expected an owning-node error")
    };
    assert!(
        matches!(source.downcast_ref::<mfn_core::LoopBuildError>(), Some(mfn_core::LoopBuildError::ConflictingBodyOutput { variable }) if variable == "count")
    );
    for exit in [false, true] {
        let body = frame_program(
            BTreeMap::from([
                ("item".into(), ValueType::Any),
                ("key".into(), ValueType::Any),
            ]),
            BTreeMap::new(),
            "item",
            "result",
            exit,
        );
        let error = registry
            .get("builtin.iteration")
            .unwrap()
            .instantiate_subgraphs(
                "each",
                json!({"body":{}}),
                BTreeMap::from([("body".into(), body)]),
            )
            .err()
            .unwrap();
        let mf_runtime::NodeBuildError::FactoryFailed { source } = error else {
            panic!("expected an owning-node error")
        };
        assert!(matches!(
            source.downcast_ref::<mfn_core::IterationValidationError>(),
            Some(mfn_core::IterationValidationError::InvalidScopeBindings)
        ));
    }
}

#[test]
fn preparation_checks_schema_ownership_and_unavailable_state_declarations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let target = mf_runtime::StateTarget::Node {
        node: "counter".into(),
        field: "count".into(),
    };
    let typed = mf_compiler::AssignmentSchema::Typed {
        value_type: ValueType::Int64,
    };
    assert!(mf_compiler::prepared_assign(&registry, "write", &target, &typed).is_err());
    let mut owner = definition("%counter.count")
        .nodes
        .into_iter()
        .find(|node| node.id.as_str() == "counter")
        .unwrap();
    owner.id = "other".into();
    let schema = mf_compiler::AssignmentSchema::Provider {
        node: owner.clone(),
    };
    assert!(mf_compiler::prepared_assign(&registry, "write", &target, &schema).is_err());
    owner.id = "counter".into();
    owner.operation = Some(mf_runtime::NodeOperation::RootInput);
    let schema = mf_compiler::AssignmentSchema::Provider {
        node: owner.clone(),
    };
    assert!(mf_compiler::prepared_assign(&registry, "write", &target, &schema).is_err());
    owner.operation = None;
    owner.kind = "fixture.unavailable".into();
    let schema = mf_compiler::AssignmentSchema::Provider { node: owner };
    let error = mf_compiler::prepared_assign(&registry, "write", &target, &schema)
        .err()
        .unwrap();
    assert!(matches!(
        error,
        mf_compiler::WorkflowBuildError::UnknownKind { .. }
    ));
    let plan = compile_definition(&definition("%counter.count"), &registry).unwrap();
    for target in [
        mf_runtime::StateTarget::Root {
            field: "missing".into(),
        },
        mf_runtime::StateTarget::Frame {
            field: "missing".into(),
        },
        mf_runtime::StateTarget::Node {
            node: "missing".into(),
            field: "count".into(),
        },
    ] {
        assert!(mf_compiler::assignment_schema(&plan.graph, &target).is_err());
    }
    let prepared = registry
        .get(provider::STATE_KIND)
        .unwrap()
        .instantiate(json!({"initial":3,"limit":10}))
        .unwrap();
    let output = prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(Default::default(), &mut ExecutionContext::default())
        .unwrap();
    assert_eq!(output.outputs["count"], json!(3));
}

#[test]
fn assignment_targets_require_declared_writable_state() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for target in [
        "%counter.limit",
        "%counter.missing",
        "%params.value",
        "%missing.count",
        "%counter.",
        "%root.count",
    ] {
        assert!(
            compile_definition(&definition(target), &registry).is_err(),
            "{target}"
        );
    }
}

#[test]
fn saved_node_operations_reject_unknown_targets_and_duplicate_executable_config() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition("%counter.count"), &registry).unwrap();
    for mutation in ["config", "target", "field", "instruction", "collision"] {
        let mut changed = plan.clone();
        let writer = changed
            .graph
            .definition
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "write")
            .unwrap();
        match mutation {
            "config" => writer.config = json!({"variable":"%other.count"}),
            "target" | "instruction" => {
                let Some(mf_runtime::NodeOperation::Assign {
                    target: mf_runtime::StateTarget::Node { node, .. },
                }) = writer.operation.as_mut()
                else {
                    unreachable!()
                };
                *node = if mutation == "target" {
                    "missing"
                } else {
                    "write"
                }
                .into();
            }
            "field" => {
                let Some(mf_runtime::NodeOperation::Assign {
                    target: mf_runtime::StateTarget::Node { field, .. },
                }) = writer.operation.as_mut()
                else {
                    unreachable!()
                };
                field.clear();
            }
            "collision" => {
                changed.graph.relations.scope = Some(mf_runtime::ScopeRelations {
                    input: "write".into(),
                    values: BTreeMap::from([("count".into(), ValueType::Int64)]),
                    state_outputs: BTreeMap::from([("count".into(), "count".into())]),
                })
            }
            _ => unreachable!(),
        }
        let error = CompiledWorkflow::from_json(&changed.to_json().unwrap()).unwrap_err();
        assert!(
            error.to_string().contains(match mutation {
                "collision" => "frame source",
                "config" => "compiled operations cannot retain executable config",
                "field" => "assignment targets",
                _ => "node state writes",
            }),
            "{mutation}: {error}"
        );
        assert!(instantiate_compiled(&changed, &registry).is_err());
    }
}
