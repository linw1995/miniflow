extern crate mfn_code as _;
extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::*;
use serde_json::{Value, json};
use std::num::NonZeroUsize;

#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod caller;

fn document() -> Value {
    json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"a_parent","kind":"test.directed_caller","config":{"repeat":2}},
        {"id":"z_config","kind":"builtin.constant","config":{"value":"credential"}},
        {"id":"tool","kind":"builtin.code","config":{"language":"cel","inputs":{"query":"string","token":"string"},"code":{"text":"query + token"}},
            "inputs":{"token":{"from":{"node":"z_config","port":"value"}}}},
        {"id":"unused","kind":"builtin.code","config":{"language":"cel","inputs":{},"code":{"text":"1 / 0"}}}
    ],"calls":{"a_parent":{
        "search":{"target":"tool","ports":[{"output":"query","input":"query"}]},
        "unused":{"target":"unused"}
    }},"outputs":[{"name":"result","node":"a_parent","port":"value"}]})
}
fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}

#[test]
fn schedules_external_readiness_without_activating_tools_or_exposing_startup_inputs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(document()), &registry).unwrap();
    let position = |id: &str| {
        plan.execution_order
            .iter()
            .position(|node| node.as_str() == id)
            .unwrap()
    };
    assert!(position("z_config") < position("a_parent"));
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let scheduled: Vec<_> = flow
        .execution_domains()
        .domains()
        .iter()
        .flat_map(|domain| domain.nodes.iter())
        .map(|id| flow.definition_node_id(id).unwrap())
        .collect();
    assert_eq!(scheduled, ["z_config", "a_parent"]);
    assert!(!flow.input_schema().inputs.contains_key("tool"));
    for workers in [1, 4] {
        let outputs = flow
            .execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(workers).unwrap(),
            })
            .unwrap();
        assert_eq!(outputs["result"], ValueRef::from("query:credential"));
    }
    let description = plan.manifest(&registry).unwrap().description().unwrap();
    assert_eq!(description.loop_bodies.len(), 1);
    assert_eq!(description.loop_bodies[0].call_targets.len(), 2);
    let recovered = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(
        instantiate_compiled(&recovered, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"],
        ValueRef::from("query:credential")
    );
}

#[test]
fn nested_call_readiness_and_snapshots_include_transitive_external_producers() {
    let mut value = document();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"middle","kind":"test.directed_caller"}));
    value["nodes"][0]["config"]["result"] = "value".into();
    value["calls"]["middle"] = value["calls"]["a_parent"].clone();
    value["calls"]["a_parent"] = json!({"search":{"target":"middle"}});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(
        flow.execute().unwrap()["result"],
        ValueRef::from("query:credential")
    );
    let description = plan.manifest(&registry).unwrap().description().unwrap();
    assert_eq!(description.loop_bodies.len(), 2);
}

#[test]
fn rejects_global_wait_cycles_between_otherwise_acyclic_callers() {
    let definition = parse(json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"left","kind":"test.directed_caller"},{"id":"right","kind":"test.directed_caller"},
        {"id":"left_tool","kind":"builtin.identity","inputs":{"input":{"from":{"node":"right","port":"value"}}}},
        {"id":"right_tool","kind":"builtin.identity","inputs":{"input":{"from":{"node":"left","port":"value"}}}}
    ],"calls":{"left":{"search":{"target":"left_tool"}},"right":{"search":{"target":"right_tool"}}}}));
    let registry = NodeRegistry::from_inventory().unwrap();
    let error = compile_definition(&definition, &registry).unwrap_err();
    assert!(
        matches!(error, mf_compiler::WorkflowCompileError::Cycle { .. }),
        "{error}"
    );
}

#[test]
fn static_flow_bodies_own_their_call_tables_and_observation_templates() {
    let definition = parse(json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"outer","kind":"workflow.call","config":{"flow":"child","definition":document()}}
    ],"outputs":[{"name":"result","node":"outer","port":"result"}]}));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"],
        ValueRef::from("query:credential")
    );
    let description = plan.manifest(&registry).unwrap().description().unwrap();
    assert!(
        description
            .loop_bodies
            .iter()
            .any(|body| body.path == ["outer", "a_parent"] && !body.call_targets.is_empty())
    );
}

#[test]
fn external_skips_and_success_guards_do_not_skip_the_parent() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for success in [false, true] {
        let mut value = json!({"version":"2026-10-28","dependencies":{},"nodes":[
            {"id":"a_parent","kind":"test.directed_caller","config":{"result":"value"}},
            {"id":"value","kind":"builtin.constant","config":{"value":-1}},
            {"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"positive","condition":{"source":{"output":"value.value","path":""},"operator":"gt","value":0}}]}},
            {"id":"producer","kind":"builtin.constant","config":{"value":"credential"}},
            {"id":"tool","kind":"builtin.identity","inputs":{"input":{"from":{"node":"producer","port":"value"}}}}
        ],"edges":[{"from_node":"value","to_node":"route"},{"from_node":"route","from_output":"positive","to_node":"producer"}],
        "calls":{"a_parent":{"search":{"target":"tool"}}},"outputs":[{"name":"result","node":"a_parent","port":"value"}]});
        if success {
            value["nodes"][4] =
                json!({"id":"tool","kind":"builtin.constant","config":{"value":"unreachable"}});
            value["edges"]
                .as_array_mut()
                .unwrap()
                .push(json!({"from_node":"producer","to_node":"tool"}));
        }
        let plan = compile_definition(&parse(value), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        assert_eq!(flow.execute().unwrap()["result"], ValueRef::from("skipped"));
    }
}

#[test]
fn explicit_arguments_defaults_and_eager_external_producers_keep_distinct_ownership() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"][2]["config"]["inputs"]["extra"] = "string".into();
    value["nodes"][2]["config"]["code"]["text"] = "query + token + extra".into();
    value["calls"]["a_parent"]["search"]["defaults"] = "remaining_inputs".into();
    let plan = compile_definition(&parse(value.clone()), &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"],
        ValueRef::from("query:credentialdefault")
    );
    value["nodes"][0]["config"]["repeat"] = 0.into();
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let mut context = ExecutionContext::default();
    assert_eq!(
        flow.execute_in_context(&mut context).unwrap()["result"],
        ValueRef::from("unselected")
    );
    assert!(matches!(
        context.output("z_config.value").unwrap(),
        ContextValue::Value(_)
    ));
    assert!(context.output("tool.text").is_err());
}

#[test]
fn isolated_descriptions_keep_discovery_separate_from_executable_templates() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(document()), &registry).unwrap();
    let nodes = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    let targets = plan.node_targets(&nodes).unwrap();
    assert_eq!(targets.len(), plan.definition.nodes.len());
    for target in targets {
        target.description.validate().unwrap();
    }
}

#[path = "fixtures/observation_capture.rs"]
mod capture;

#[test]
fn callable_flow_tools_reenter_their_bodies_and_emit_valid_nested_invocations() {
    let definition = parse(json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"parent","kind":"test.directed_caller","config":{"result":"result","repeat":2}},
        {"id":"delegate","kind":"workflow.call","config":{"flow":"child","definition":document()}}
    ],"calls":{"parent":{"search":{"target":"delegate"}}},"outputs":[{"name":"result","node":"parent","port":"value"}]}));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let description = plan.manifest(&registry).unwrap().description().unwrap();
    let harness = capture::Harness::new(true);
    let observation = harness
        .observer()
        .start_with_description(description.clone(), mf_telemetry::identity::RunId::new())
        .unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(
        flow.execute_with_observation(Some(observation)).unwrap()["result"],
        ValueRef::from("query:credential")
    );
    let events: Vec<_> = harness
        .records()
        .iter()
        .map(|record| record.decode().unwrap())
        .collect();
    for event in &events {
        event.validate_for(&description).unwrap();
    }
    assert_eq!(events.iter().filter(|event| matches!(&event.event, mf_telemetry::event::Event::NodeStarted { node, .. } if node.id == "z_config")).count(), 2);
    assert_eq!(events.iter().filter(|event| matches!(&event.event, mf_telemetry::event::Event::NodeStarted { node, .. } if node.id == "tool" && node.path.len() == 3)).count(), 4);
    let spans = harness.spans.get_finished_spans().unwrap();
    for span in spans.iter().filter(|span| {
        span.attributes.iter().any(|attribute| {
            attribute.key.as_str() == "mf.node.id" && attribute.value.as_str() == "tool"
        })
    }) {
        let parent = spans
            .iter()
            .find(|candidate| candidate.span_context.span_id() == span.parent_span_id)
            .unwrap();
        assert!(
            parent
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "mf.node.id"
                    && attribute.value.as_str() == "a_parent")
        );
    }
    let prepared = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    plan.node_targets(&prepared).unwrap();
}

#[test]
fn parallel_iteration_items_share_their_own_prepared_call_scope() {
    let child = document();
    let definition = parse(json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"source","kind":"builtin.constant","config":{"value":[1,2]}},
        {"id":"items","kind":"builtin.iteration","config":{"mode":"parallel","body":{
            "nodes":child["nodes"],"calls":child["calls"],"result":{"node":"a_parent","port":"value"}
        }},"inputs":{"items":{"from":{"node":"source","port":"value"}}}}
    ],"outputs":[{"name":"results","node":"items","port":"results"}]}));
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&output["results"]).unwrap(),
        json!(["query:credential", "query:credential"])
    );
    plan.manifest(&registry).unwrap().description().unwrap();
}

#[test]
fn builder_call_binding_rejects_stale_scopes_and_missing_readiness_order() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(document()), &registry).unwrap();
    let build = |order| {
        mf_compiler::FlowBuilder::prepare(
            mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap(),
            plan.definition.edges.clone(),
            order,
            plan.definition.outputs.clone(),
        )
        .unwrap()
    };
    let mut changed = plan.definition.clone();
    changed.nodes.push(changed.nodes[0].clone());
    changed.nodes.last_mut().unwrap().id = "extra".into();
    assert!(
        build(plan.execution_order.clone())
            .with_declared_calls(&changed)
            .is_err()
    );
    let mut order = plan.execution_order.clone();
    order.retain(|id| id.as_str() != "a_parent");
    order.insert(0, "a_parent".into());
    assert!(build(order).with_declared_calls(&plan.definition).is_err());
}
