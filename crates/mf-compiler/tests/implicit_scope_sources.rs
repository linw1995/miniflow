extern crate mfn_core as _;

use mf_compiler::{
    CompiledWorkflow, NodeRegistry, WorkflowDefinition, compile_definition,
    compile_resolved_definition, instantiate_compiled, plan_definition,
};
use mf_runtime::{ITERATION_INPUT_ID, ITERATION_INPUT_KIND, LOOP_SOURCE_ID, ROOT_SOURCE_ID};
use serde_json::{Value, json};

fn definition(source: &str) -> Value {
    let assignment = json!({
        "id":"assign", "kind":"workflow.assign", "config":{"variable":"count"},
        "inputs":{"value":{"from":{"node":source, "port":"count"}}}
    });
    match source {
        ROOT_SOURCE_ID => json!({
            "version":"2026-11-13",
            "variables":[{"name":"count", "type":"int", "initial":17}],
            "nodes":[assignment],
            "outputs":[{"name":"value", "node":source, "port":"count"}]
        }),
        LOOP_SOURCE_ID => json!({
            "version":"2026-11-13",
            "nodes":[{"id":"scope", "kind":"loop.run", "config":{
                "max_iterations":2,
                "variables":[{"name":"count", "type":"int", "initial":17}],
                "body":{"nodes":[assignment]}
            }}],
            "outputs":[{"name":"value", "node":"scope", "port":"count"}]
        }),
        ITERATION_INPUT_ID => json!({
            "version":"2026-11-13",
            "nodes":[
                {"id":"scope", "kind":"builtin.iteration", "config":{
                    "body":{
                        "nodes":[],
                        "outputs":[{"name":"result", "node":source, "port":"item"}]
                    }
                }, "inputs":{"items":{"from":{"node":"items", "port":"value"}}}},
                {"id":"items", "kind":"builtin.constant", "config":{"value":[2,4]}}
            ],
            "outputs":[{"name":"value", "node":"scope", "port":"results"}]
        }),
        _ => unreachable!("test scope source"),
    }
}

fn authored_scope<'a>(value: &'a mut Value, source: &str) -> &'a mut Value {
    if source == ROOT_SOURCE_ID {
        value
    } else {
        &mut value["nodes"][0]["config"]["body"]
    }
}

#[test]
fn scope_reads_and_assignments_need_no_source_node_declaration() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for source in [ROOT_SOURCE_ID, LOOP_SOURCE_ID, ITERATION_INPUT_ID] {
        let value = definition(source);
        let authored: WorkflowDefinition = serde_json::from_value(value).unwrap();
        let before = authored.clone();
        let expected = if source == ITERATION_INPUT_ID {
            json!({"value":[2,4]})
        } else {
            json!({"value":17})
        };
        let compiled = compile_definition(&authored, &registry).unwrap();
        let resolved = compile_resolved_definition(&compiled.graph.definition, &registry).unwrap();
        for plan in [
            compiled,
            resolved,
            plan_definition(&authored).unwrap(),
            compile_resolved_definition(&authored, &registry).unwrap(),
        ] {
            let graph = if source == ROOT_SOURCE_ID {
                &plan.graph
            } else {
                &plan.graph.children[&"scope".into()]["body"]
            };
            assert_eq!(
                graph
                    .definition
                    .nodes
                    .iter()
                    .filter(|node| node.id.as_str() == source)
                    .count(),
                1,
                "{source}: the compiler supplies exactly one internal source"
            );
            // Stored source instructions stay valid; artifact replay is not authoring.
            let restored = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
            let flow = instantiate_compiled(&restored, &registry).unwrap();
            assert_eq!(
                serde_json::to_value(flow.execute().unwrap()).unwrap(),
                expected
            );
        }
        assert_eq!(
            authored, before,
            "compilation must not mutate authored nodes"
        );
    }
}

#[test]
fn registered_flows_keep_internal_scope_sources_implicit_for_callers() {
    for source in [ROOT_SOURCE_ID, LOOP_SOURCE_ID, ITERATION_INPUT_ID] {
        let mut registry = NodeRegistry::from_inventory().unwrap();
        let authored: WorkflowDefinition = serde_json::from_value(definition(source)).unwrap();
        let compiled = compile_definition(&authored, &registry).unwrap();
        let flow = instantiate_compiled(&compiled, &registry).unwrap();
        let expected = serde_json::to_value(flow.execute().unwrap()).unwrap();
        let body = compiled.graph.definition;
        registry
            .register_flow(
                "host.sources",
                body.clone(),
                mf_runtime::PreparedSubgraph::from_flow(flow, vec![]),
            )
            .unwrap();
        let caller: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-11-13",
            "nodes":[{"id":"invoke", "kind":"workflow.call", "config":{
                "flow":"host.sources", "body":body
            }}],
            "outputs":[{"name":"value", "node":"invoke", "port":"value"}]
        }))
        .unwrap();
        let plan = compile_resolved_definition(&caller, &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        assert_eq!(
            serde_json::to_value(flow.execute().unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn authors_cannot_declare_implicit_sources_as_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (source, kind) in [
        (ROOT_SOURCE_ID, ROOT_SOURCE_ID),
        (LOOP_SOURCE_ID, LOOP_SOURCE_ID),
        (ITERATION_INPUT_ID, ITERATION_INPUT_KIND),
    ] {
        for (id, kind) in [
            (source, kind),
            ("named", kind),
            (source, "builtin.identity"),
        ] {
            let mut value = definition(source);
            authored_scope(&mut value, source)["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":id, "kind":kind}));
            let authored: WorkflowDefinition = serde_json::from_value(value).unwrap();
            for error in [
                compile_definition(&authored, &registry).unwrap_err(),
                plan_definition(&authored).unwrap_err(),
            ] {
                assert!(
                    error.to_string().contains("scope sources are implicit"),
                    "{source}, {id}, {kind}: {error}"
                );
            }
        }
    }
}

#[test]
fn resolved_definitions_cannot_forge_implicit_source_instructions() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (source, kind) in [
        (ROOT_SOURCE_ID, ROOT_SOURCE_ID),
        (LOOP_SOURCE_ID, LOOP_SOURCE_ID),
        (ITERATION_INPUT_ID, ITERATION_INPUT_KIND),
    ] {
        for node in [
            json!({"id":"named", "kind":kind}),
            json!({"id":source, "kind":"builtin.identity"}),
            json!({"id":source, "kind":kind, "config":{"unexpected":true}}),
        ] {
            let mut value = definition(source);
            authored_scope(&mut value, source)["nodes"]
                .as_array_mut()
                .unwrap()
                .push(node);
            let resolved: WorkflowDefinition = serde_json::from_value(value).unwrap();
            let error = compile_resolved_definition(&resolved, &registry).unwrap_err();
            assert!(
                error.to_string().contains("scope sources are implicit"),
                "{error}"
            );
        }
    }
}

#[test]
fn source_like_values_in_ordinary_node_configuration_remain_opaque() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let literal = json!({"nodes":[{"id":"%loop", "kind":"%loop"}]});
    let authored: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-11-13",
        "nodes":[{"id":"literal", "kind":"builtin.constant", "config":{"value":literal}}],
        "outputs":[{"name":"value", "node":"literal", "port":"value"}]
    }))
    .unwrap();
    let plan = compile_definition(&authored, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(
        serde_json::to_value(flow.execute().unwrap()).unwrap(),
        json!({"value":literal})
    );
}
