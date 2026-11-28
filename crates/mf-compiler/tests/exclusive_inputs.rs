#[path = "common/snapshots.rs"]
mod snapshots;
use snapshots::Snapshots;
#[path = "fixtures/observation_capture.rs"]
mod capture;
#[path = "fixtures/controlled_source.rs"]
mod controlled;
extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};

fn definition(selected: bool, left: Value, right: Value) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[
            {"id":"condition", "kind":"builtin.constant", "config":{"value":selected}},
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"left", "condition":{"source":{"output":"condition.value", "path":""}, "operator":"eq", "value":true}}]}},
            {"id":"left", "kind":"builtin.constant", "config":{"value":left}},
            {"id":"right", "kind":"builtin.constant", "config":{"value":right}},
            {"id":"consume", "kind":"builtin.identity", "inputs":{"input":{"exclusive":[{"$ref":"left.value"}, {"$ref":"right.value"}]}}}
        ],
        "edges":[
            {"from_node":"condition", "from_output":"value", "to_node":"route"},
            {"from_node":"route", "from_output":"left", "to_node":"left"},
            {"from_node":"route", "from_output":"else", "to_node":"right"}
        ],
        "outputs":[{"name":"result", "node":"consume", "port":"value", "optional":true}]
    })).unwrap()
}

#[test]
fn converges_both_branches_without_arbitrary_type_or_constant_inference() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (left, right) in [
        (json!(1), json!(2)),
        (json!(false), json!(null)),
        (json!([1]), json!({"text":"two"})),
    ] {
        for selected in [true, false] {
            let definition = definition(selected, left.clone(), right.clone());
            let plan = compile_definition(&definition, &registry).unwrap();
            let flow = instantiate_compiled(&plan, &registry).unwrap();
            let result = flow.execute().unwrap();
            assert_eq!(
                result["result"],
                if selected {
                    left.clone()
                } else {
                    right.clone()
                }
            );
        }
    }
}

#[test]
fn ordinary_gates_remain_mandatory_and_unproven_groups_fail_compilation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition(true, json!(1), json!(1));
    definition.control_edges.push(
        serde_json::from_value(
            json!({"from_node":"route", "from_output":"else", "to_node":"consume"}),
        )
        .unwrap(),
    );
    let plan = compile_definition(&definition, &registry).unwrap();
    assert!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()
            .is_empty()
    );
    definition
        .control_edges
        .retain(|edge| edge.to_node.as_str() != "left" && edge.to_node.as_str() != "right");
    let error = compile_definition(&definition, &registry).unwrap_err();
    assert!(error.to_string().contains("not proven mutually exclusive"));
}

#[test]
fn loop_rounds_and_parallel_iterations_select_within_each_scope() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let template = serde_json::to_value(definition(true, json!(false), json!(true))).unwrap();
    for mode in ["loop", "sequential", "parallel"] {
        let source = if mode == "loop" {
            "%loop"
        } else {
            "%iteration"
        };
        let port = if mode == "loop" { "state" } else { "item" };
        let mut body = template.clone();
        body["nodes"].as_array_mut().unwrap().remove(0);
        body["nodes"][0]["config"]["branches"][0]["condition"]["source"]["output"] =
            json!(format!("{source}.{port}"));
        body["edges"][0]["from_node"] = json!(source);
        body["edges"][0]["from_output"] = json!(port);
        let definition = if mode == "loop" {
            let selected = body["nodes"][3]["inputs"]["input"].take();
            body["nodes"][3] = json!({"id":"consume", "kind":"workflow.assign", "config":{"variable":"state"}, "inputs":{"value":selected}});
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":false}},
                {"id":"loop", "kind":"loop.run", "config":{"max_iterations":3, "variables":[{"name":"state", "type":"bool"}], "body":{"version":"2026-10-03", "dependencies":{},
                    "nodes":body["nodes"], "edges":body["edges"]
                }},
                "inputs":{"state":{"$ref":"start.value"}}}
            ],
            "outputs":[{"name":"result", "node":"loop", "port":"state"}]})
        } else {
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":[true,false,true,false]}},
                {"id":"iterate", "kind":"builtin.iteration", "config":{"mode":mode, "body":{"version":"2026-10-03", "dependencies":{},
                    "nodes":body["nodes"], "edges":body["edges"], "outputs": [{"name":"result", "node":"consume", "port":"value"}]
                }},
                "inputs":{"items":{"$ref":"start.value"}}}
            ],
            "outputs":[{"name":"result", "node":"iterate", "port":"results"}]})
        };
        let mut invalid = definition.clone();
        let body = &mut invalid["nodes"][1]["config"]["body"];
        body["edges"].as_array_mut().unwrap().truncate(1);
        let error =
            compile_definition(&serde_json::from_value(invalid).unwrap(), &registry).unwrap_err();
        assert!(
            error.to_string().contains("not proven mutually exclusive"),
            "{error}"
        );
        let plan =
            compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        for observe in [false, true] {
            let harness = capture::Harness::new(false);
            let observation = observe.then(|| {
                plan.start_observation(&harness.observer(), mf_telemetry::identity::RunId::new())
                    .unwrap()
            });
            let result = mf_runtime::FlowRuntime::default()
                .execute_with_observation(&flow, observation)
                .unwrap();
            assert_eq!(
                result["result"],
                if mode == "loop" {
                    json!(true)
                } else {
                    json!([false, true, false, true])
                }
            );
            if observe && mode != "loop" {
                let records = harness.records();
                let mut selected: Vec<_> = records
                    .iter()
                    .filter(|record| {
                        record.event_name == "mf.iteration.node.started"
                            && record.attributes["mf.node.id"] == "consume"
                    })
                    .collect();
                selected.sort_by_key(|record| {
                    record.attributes["mf.iteration.index"].as_i64().unwrap()
                });
                assert_eq!(selected.len(), 4);
                for (index, record) in selected.iter().enumerate() {
                    assert_eq!(record.attributes["mf.iteration.index"], json!(index));
                    assert_eq!(
                        record.attributes["mf.input.selections"],
                        json!([{
                            "input":"input", "source_node":if index % 2 == 0 { "left" } else { "right" }, "source_output":"value"
                        }])
                    );
                }
            }
        }
    }
}

#[cfg(feature = "codegen")]
#[test]
fn typed_plans_fall_back_at_groups_and_isolated_nodes_accept_resolved_inputs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition(true, json!(1), json!(2))).unwrap();
    value["dependencies"] = json!({"core":{"package":"mfn-core", "path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../builtin-nodes/core")}});
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"unrelated_a", "kind":"builtin.identity"}),
        json!({"id":"unrelated_b", "kind":"builtin.identity", "inputs":{"input":{"$ref":"unrelated_a.value"}}}),
    ]);
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let artifacts = plan
        .generate_artifacts(&registry, Default::default())
        .unwrap();
    let report = artifacts.typed_plan;
    assert!(
        report
            .connections
            .iter()
            .filter(|edge| edge.target == "consume")
            .all(|edge| edge.fallback == Some(mf_compiler::TypedFallbackReason::ExclusiveInput))
    );
    assert!(
        report
            .connections
            .iter()
            .any(|edge| edge.target == "unrelated_b" && edge.fallback.is_none())
    );
    let targets = artifacts.manifest.descriptor.node_targets.clone();
    let target = mf_runtime::NodeTarget::find(&targets, "consume").unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let result = mf_runtime::FlowRuntime::default()
        .execute_target(
            &target
                .bind_flow(
                    &flow,
                    plan.graph
                        .execution_order
                        .iter()
                        .position(|id| id.as_str() == target.id)
                        .unwrap(),
                )
                .unwrap(),
            mf_runtime::WorkflowArguments::try_from(json!({"consume.input":7})).unwrap(),
            Default::default(),
        )
        .unwrap();
    assert_eq!(result["value"], json!(7));
}

#[test]
fn observed_selections_and_snapshots_preserve_provenance_without_secret_values() {
    use mf_runtime::{ExecutionContext, WorkflowArguments};
    use mf_telemetry::{event::Event, identity::RunId};
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/exclusive-inputs.json")).unwrap();
    value["nodes"][0]["config"]["parameters"]["payload"] =
        json!({"type":"object", "default":{"token":"selection-secret-sentinel"}, "secret":true});
    for index in [2, 3] {
        value["nodes"][index]["kind"] = json!("builtin.identity");
        value["nodes"][index]["config"] = json!({});
    }
    for index in [2, 3] {
        value["nodes"][index]["inputs"] = json!({"input":{"$ref":"params.payload"}});
    }
    let registry = NodeRegistry::from_inventory().unwrap();
    for ci in [false, true] {
        let plan =
            compile_definition(&serde_json::from_value(value.clone()).unwrap(), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let harness = capture::Harness::new(false);
        let recording = Snapshots::memory();
        let observation = plan
            .start_observation(&harness.observer(), RunId::new())
            .unwrap();
        let outputs = ExecutionContext::run(Some(observation), |context| {
            context.set_snapshot_recorder(recording.recorder());
            context.set_workflow_arguments(
                WorkflowArguments::try_from(json!({"params.ci":ci})).unwrap(),
            );
            flow.execute_in_context(context)
        })
        .unwrap();
        assert!(outputs["result"].is_secret());
        assert_eq!(
            recording.current().node(&[], "consume").unwrap().inputs["input"],
            json!("[REDACTED]")
        );
        assert!(harness.spans.get_finished_spans().unwrap().is_empty());
        let records = harness.records();
        assert!(
            !serde_json::to_string(&records)
                .unwrap()
                .contains("selection-secret-sentinel")
        );
        let selected = records
            .iter()
            .map(|record| record.decode().unwrap())
            .find_map(|record| match record.event {
                Event::NodeStarted {
                    node,
                    selected_inputs,
                    ..
                } if node.id == "consume" => Some(selected_inputs),
                _ => None,
            })
            .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].input, "input");
        assert_eq!(selected[0].source_node, if ci { "ci" } else { "local" });
        assert_eq!(selected[0].source_output, "value");
    }
}

#[test]
fn stream_skip_observations_use_logical_inputs_and_keep_selected_sources() {
    use mf_telemetry::{
        event::Event,
        identity::RunId,
        stream::{StreamPayload, StreamRecord},
    };
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{}, "execution":{},
        "nodes":[
            controlled::source(json!("int")),
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"positive", "condition":{"source":{"output":"feed.item", "path":""}, "operator":"gt", "value":0}}]}},
            {"id":"active", "kind":"builtin.identity", "inputs":{"input":{"$ref":"feed.item"}}},
            {"id":"inactive", "kind":"builtin.identity", "inputs":{"input":{"$ref":"feed.item"}}},
            {"id":"collect", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}, "inputs":{"item":{"exclusive":[{"$ref":"active.value"}, {"$ref":"inactive.value"}]}}}
        ],
        "edges":[
            {"from_node":"feed", "from_output":"item", "to_node":"route"},
            {"from_node":"route", "from_output":"positive", "to_node":"active"},
            {"from_node":"route", "from_output":"else", "to_node":"inactive"},
            {"from_node":"route", "from_output":"else", "to_node":"collect"}
        ],
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let harness = capture::Harness::new(false);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let run = controlled::SourceRun::start(
        mf_compiler::instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        mf_runtime::StreamOptions {
            observation: Some(observation),
            ..Default::default()
        },
    )
    .unwrap();
    run.source.send(json!(1)).unwrap();
    run.source.close();
    assert!(run.recv().unwrap().is_none());
    run.join().unwrap();
    let skipped = harness
        .records()
        .iter()
        .filter_map(|wire| StreamRecord::decode(wire).ok())
        .find_map(|record| match record.payload {
            StreamPayload::Execution(Event::NodeSkipped {
                node,
                causes,
                selected_inputs,
                ..
            }) if node.id == "collect" => Some((causes, selected_inputs)),
            _ => None,
        })
        .unwrap();
    assert_eq!(skipped.0.len(), 1);
    assert_eq!(skipped.0[0].source_node(), "route");
    assert_eq!(skipped.1.len(), 1);
    assert_eq!(skipped.1[0].source_node, "active");
}

#[test]
fn exclusive_startup_input_activates_a_stream_producer() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for ci in [false, true] {
        let mut feed = controlled::source(json!("int"));
        feed["inputs"]["control"] =
            json!({"exclusive":[{"$ref":"local.value"}, {"$ref":"ci.value"}]});
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-03", "dependencies":{}, "execution":{},
            "nodes":[
                {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                    "control":{"type":"string"}, "ci":{"type":"bool", "default":ci, "secret":false}
                }}},
                {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"ci", "condition":{"source":{"output":"params.ci", "path":""}, "operator":"eq", "value":true}}]}},
                {"id":"local", "kind":"builtin.identity", "inputs":{"input":{"$ref":"params.control"}}},
                {"id":"ci", "kind":"builtin.identity", "inputs":{"input":{"$ref":"params.control"}}},
                feed,
                {"id":"consume", "kind":"builtin.identity", "inputs":{"input":{"$ref":"feed.item"}}}
            ],
            "edges":[
                {"from_node":"params", "from_output":"ci", "to_node":"route"},
                {"from_node":"route", "from_output":"ci", "to_node":"ci"},
                {"from_node":"route", "from_output":"else", "to_node":"local"}
            ],
            "outputs":[{"name":"result", "node":"consume", "port":"value"}]
        })).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let mut options = mf_runtime::StreamOptions::default();
        let source = controlled::prepare_control("params", &mut options);
        let instance = mf_compiler::instantiate_stream(&plan, &registry)
            .unwrap()
            .start_with_options(options)
            .unwrap();
        source.send(json!(42)).unwrap();
        source.close();
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["result"],
            json!(42)
        );
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
}

#[test]
fn rejects_unproven_groups_before_execution_and_code_generation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for case in [
        "unguarded",
        "same_branch",
        "unrelated_router",
        "reconverged_left",
        "reconverged_right",
    ] {
        let mut value = serde_json::to_value(definition(true, json!(1), json!(2))).unwrap();
        match case {
            "unguarded" => value["edges"].as_array_mut().unwrap().truncate(1),
            "same_branch" => value["edges"][2]["from_output"] = json!("left"),
            "unrelated_router" => {
                let mut route = value["nodes"][1].clone();
                route["id"] = json!("other_route");
                value["nodes"].as_array_mut().unwrap().push(route);
                value["edges"][2]["from_node"] = json!("other_route");
                value["edges"].as_array_mut().unwrap().push(json!({
                    "from_node":"condition", "from_output":"value", "to_node":"other_route"
                }));
            }
            "reconverged_left" | "reconverged_right" => {
                value["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"after", "kind":"builtin.identity", "inputs":{"input":{"exclusive":[
                        {"$ref":"consume.value"}, {"$ref":if case == "reconverged_left" { "left.value" } else { "right.value" }}
                    ]}}}));
            }
            _ => unreachable!(),
        }
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        for reverse in [false, true] {
            let mut definition: WorkflowDefinition = definition.clone();
            if reverse {
                definition.edges.reverse();
                definition.control_edges.reverse();
            }
            let error = compile_definition(&definition, &registry)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("not proven mutually exclusive"),
                "{case}: {error}"
            );
            assert!(error.contains("`input`"), "{error}");
            let plan = mf_compiler::plan_definition(&definition).unwrap();
            assert!(instantiate_compiled(&plan, &registry).is_err(), "{case}");
            #[cfg(feature = "codegen")]
            assert!(
                plan.generate_artifacts(&registry, Default::default())
                    .is_err(),
                "{case}"
            );
        }
    }
}

#[test]
fn propagates_data_and_control_facts_and_keeps_common_outer_branches() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition(true, json!(1), json!(2))).unwrap();
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"outer", "kind":"builtin.if_else", "config":{"branches":[{"id":"inside", "condition":{"source":{"output":"condition.value", "path":""}, "operator":"eq", "value":true}}]}}),
        json!({"id":"outside", "kind":"builtin.constant", "config":{"value":3}}),
        json!({"id":"forward", "kind":"builtin.identity", "inputs":{"input":{"$ref":"consume.value"}}}),
        json!({"id":"final", "kind":"builtin.identity", "inputs":{"input":{"exclusive":[{"$ref":"forward.value"},{"$ref":"outside.value"}]}}})
    ]);
    value["edges"].as_array_mut().unwrap().extend([
        json!({"from_node":"condition", "from_output":"value", "to_node":"outer"}),
        json!({"from_node":"outer", "from_output":"inside", "to_node":"route"}),
        json!({"from_node":"outer", "from_output":"else", "to_node":"outside"}),
    ]);
    value["outputs"] = json!([{"name":"result", "node":"final", "port":"value"}]);
    for selected in [true, false] {
        value["nodes"][0]["config"]["value"] = json!(selected);
        let definition = serde_json::from_value(value.clone()).unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let result = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(result["result"], json!(if selected { 1 } else { 3 }));
    }
    // Direct data edges from a router also carry its output contract.
    let mut direct = definition(true, json!(1), json!(2));
    for (edge, port) in direct.edges.iter_mut().zip(["left", "else"]) {
        edge.from_node = "route".into();
        edge.from_output = port.into();
    }
    compile_definition(&direct, &registry).unwrap();
}

#[test]
fn executable_builders_require_proof_and_valid_task_output_contracts() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition = definition(true, json!(1), json!(2));
    let order = mf_compiler::plan_definition(&definition)
        .unwrap()
        .graph
        .execution_order;
    for case in [
        "valid",
        "no_controls",
        "missing_contract",
        "singleton",
        "duplicate",
        "unknown",
        "event",
        "stream",
        "stream_valid",
        "stream_unproven",
    ] {
        let mut nodes = mf_compiler::resolve_nodes(&definition, &registry).unwrap();
        let route = nodes
            .iter_mut()
            .find(|node| node.definition_id.as_str() == "route")
            .unwrap();
        match case {
            "missing_contract" => route.metadata.exclusive_outputs.clear(),
            "singleton" => route.metadata.exclusive_outputs.truncate(1),
            "duplicate" => route.metadata.exclusive_outputs = vec!["left".into(), "left".into()],
            "unknown" => route.metadata.exclusive_outputs = vec!["left".into(), "missing".into()],
            "event" | "stream" => {
                let replacement = if case == "event" {
                    json!({"id":"replacement", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}})
                } else {
                    controlled::source(json!("int"))
                };
                let definition = serde_json::from_value(json!({
                    "version":"2026-10-03", "dependencies":{}, "execution":{}, "nodes":[replacement]
                }))
                .unwrap();
                route.node = mf_compiler::resolve_nodes(&definition, &registry)
                    .unwrap()
                    .pop()
                    .unwrap()
                    .node;
            }
            _ => {}
        }
        let mut builder = mf_compiler::FlowBuilder::prepare(
            nodes,
            definition.edges.clone(),
            order.clone(),
            definition.outputs.clone(),
        )
        .unwrap();
        if !matches!(case, "no_controls" | "stream_unproven") {
            builder = builder
                .with_control_edges(definition.control_edges.clone())
                .unwrap();
        }
        let result = if case.starts_with("stream_") {
            builder
                .into_stream(serde_json::from_value(json!({})).unwrap())
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            builder
                .into_tasks()
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        if matches!(case, "valid" | "stream_valid") {
            assert!(result.is_ok());
        } else {
            let error = result.err().unwrap().to_string();
            assert!(
                error.contains(
                    if matches!(case, "no_controls" | "missing_contract" | "stream_unproven") {
                        "not proven mutually exclusive"
                    } else {
                        "invalid exclusive outputs"
                    }
                ),
                "{case}: {error}"
            );
        }
    }
}

#[test]
fn stateful_boundaries_do_not_export_per_invocation_branch_facts() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for event in [false, true] {
        let mut value = serde_json::to_value(definition(true, json!(1), json!(2))).unwrap();
        value["execution"] = json!({});
        value["nodes"][2] = if event {
            json!({"id":"left", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}})
        } else {
            let mut source = controlled::source(json!("int"));
            source["id"] = json!("left");
            source
        };
        value["nodes"].as_array_mut().unwrap().push(
            json!({"id":"seed", "kind":"builtin.constant", "config":{"value":"source-control"}}),
        );
        value["nodes"][4]["inputs"]["input"]["exclusive"][0] =
            json!({"$ref":if event { "left.items" } else { "left.item" }});
        value["nodes"][2]["inputs"][if event { "item" } else { "control" }] =
            json!({"$ref":"seed.value"});
        let definition = serde_json::from_value(value).unwrap();
        let error = compile_definition(&definition, &registry)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("not proven mutually exclusive"), "{error}");
    }
}
