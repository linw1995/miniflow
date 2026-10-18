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
            {"id":"consume", "kind":"builtin.identity"}
        ],
        "control_edges":[
            {"from_node":"condition", "from_output":"value", "to_node":"route"},
            {"from_node":"route", "from_output":"left", "to_node":"left"},
            {"from_node":"route", "from_output":"else", "to_node":"right"}
        ],
        "edges":[
            {"from_node":"left", "from_output":"value", "to_node":"consume", "to_input":"input", "exclusive":true},
            {"from_node":"right", "from_output":"value", "to_node":"consume", "to_input":"input", "exclusive":true}
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
fn ordinary_gates_remain_mandatory_and_ambiguity_is_not_hidden() {
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
    let plan = compile_definition(&definition, &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap_err();
    assert!(error.to_string().contains("multiple produced candidates"));
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
        body["control_edges"][0]["from_node"] = json!(source);
        body["control_edges"][0]["from_output"] = json!(port);
        let definition = if mode == "loop" {
            body["nodes"][3] = json!({"id":"consume", "kind":"workflow.loop_assign", "config":{"variable":"state"}});
            for edge in body["edges"].as_array_mut().unwrap() {
                edge["to_input"] = json!("value");
            }
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":false}},
                {"id":"loop", "kind":"workflow.loop", "loop":{"max_iterations":3, "variables":[{"name":"state", "type":"bool"}], "body":{
                    "nodes":body["nodes"], "control_edges":body["control_edges"], "edges":body["edges"]
                }}}
            ], "edges":[{"from_node":"start", "from_output":"value", "to_node":"loop", "to_input":"state"}],
            "outputs":[{"name":"result", "node":"loop", "port":"state"}]})
        } else {
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":[true,false,true,false]}},
                {"id":"iterate", "kind":"builtin.iteration", "config":{"mode":mode, "body":{
                    "nodes":body["nodes"], "control_edges":body["control_edges"], "edges":body["edges"], "result":{"node":"consume", "port":"value"}
                }}}
            ], "edges":[{"from_node":"start", "from_output":"value", "to_node":"iterate", "to_input":"items"}],
            "outputs":[{"name":"result", "node":"iterate", "port":"results"}]})
        };
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
        json!({"id":"unrelated_b", "kind":"builtin.identity"}),
    ]);
    value["edges"].as_array_mut().unwrap().push(json!({"from_node":"unrelated_a", "from_output":"value", "to_node":"unrelated_b", "to_input":"input"}));
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
    let report: mf_compiler::TypedPlan = serde_json::from_str(&artifacts.typed_plan_json).unwrap();
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
    let targets: Vec<mf_runtime::NodeTarget> =
        serde_json::from_str(&artifacts.node_targets_json).unwrap();
    let target = mf_runtime::NodeTarget::find(&targets, "consume").unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let result = mf_runtime::FlowRuntime::default()
        .execute_target(
            &target.bind_flow(&flow).unwrap(),
            mf_runtime::WorkflowArguments::try_from(json!({"consume.input":7})).unwrap(),
            Default::default(),
        )
        .unwrap();
    assert_eq!(result["value"], json!(7));
}

#[test]
fn unresolved_candidates_never_trigger_early_execution_or_fallback() {
    use mf_runtime::{
        FlowNode, Inputs, NodeExecutionError, NodePorts, NodeResult, Outputs, PortSpec,
        PreparedNode, TaskNode, ValueType,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    struct Probe {
        name: &'static str,
        outcome: &'static str,
        ready: mpsc::Sender<&'static str>,
        release: Option<Mutex<mpsc::Receiver<()>>>,
        calls: Arc<AtomicUsize>,
    }
    impl TaskNode for Probe {
        fn execute(
            &self,
            inputs: Inputs,
            context: &mut mf_runtime::ExecutionContext,
        ) -> Result<NodeResult, NodeExecutionError> {
            if self.name == "consume" {
                self.calls.fetch_add(1, Ordering::SeqCst);
                return Ok(Outputs::from([("value".into(), inputs["input"].clone())]).into());
            }
            self.ready.send(self.name).unwrap();
            if let Some(release) = &self.release {
                release.lock().unwrap().recv().unwrap();
            }
            if let Some(failure) = context.cancellation().failure() {
                return Err(failure.into());
            }
            match self.outcome {
                "skip" => Ok(NodeResult {
                    skipped: ["value".into()].into(),
                    ..Default::default()
                }),
                "missing" => Ok(NodeResult::default()),
                "fail" => mf_runtime::NodeExecutionFailedSnafu {
                    message: "candidate failed",
                }
                .fail(),
                _ => Ok(Outputs::from([("value".into(), 1.into())]).into()),
            }
        }
    }
    for outcome in ["skip", "produce", "missing", "fail", "cancel"] {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let probes = [
            Probe {
                name: "left",
                outcome: "produce",
                ready: ready_tx.clone(),
                release: None,
                calls: calls.clone(),
            },
            Probe {
                name: "right",
                outcome,
                ready: ready_tx.clone(),
                release: Some(Mutex::new(release_rx)),
                calls: calls.clone(),
            },
            Probe {
                name: "consume",
                outcome: "produce",
                ready: ready_tx,
                release: None,
                calls: calls.clone(),
            },
        ];
        let nodes = probes
            .into_iter()
            .map(|probe| {
                let ports = NodePorts {
                    inputs: if probe.name == "consume" {
                        vec![PortSpec::new("input", ValueType::Int64, true)]
                    } else {
                        vec![]
                    },
                    outputs: vec![PortSpec::new("value", ValueType::Int64, false)],
                };
                FlowNode::new(
                    probe.name,
                    PreparedNode::from_parts(
                        mf_runtime::NodeExecution::Task(Box::new(probe)),
                        ports,
                    ),
                )
            })
            .collect();
        let edges = serde_json::from_value(json!([
            {"from_node":"left", "from_output":"value", "to_node":"consume", "to_input":"input", "exclusive":true},
            {"from_node":"right", "from_output":"value", "to_node":"consume", "to_input":"input", "exclusive":true}
        ])).unwrap();
        let outputs =
            serde_json::from_value(json!([{"name":"result", "node":"consume", "port":"value"}]))
                .unwrap();
        let flow = mf_compiler::FlowBuilder::prepare(
            nodes,
            edges,
            vec!["left".into(), "right".into(), "consume".into()],
            outputs,
        )
        .unwrap()
        .into_tasks()
        .unwrap();
        let mut context = mf_runtime::ExecutionContext::default();
        let cancellation = context.cancellation().clone();
        let thread = std::thread::spawn(move || flow.execute_in_context(&mut context));
        for _ in 0..2 {
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        if outcome == "cancel" {
            cancellation.cancel(mf_runtime::StreamError::Execution {
                message: "cancelled candidate".into(),
            });
        }
        release_tx.send(()).unwrap();
        let result = thread.join().unwrap();
        if outcome == "skip" {
            assert_eq!(result.unwrap()["result"], json!(1));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        } else {
            assert!(result.is_err(), "{outcome}");
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn observed_selections_and_snapshots_preserve_provenance_without_secret_values() {
    use mf_runtime::{ExecutionContext, SnapshotRecording, WorkflowArguments};
    use mf_telemetry::{event::Event, identity::RunId};
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/exclusive-inputs.json")).unwrap();
    value["nodes"][0]["config"]["parameters"]["payload"] =
        json!({"type":"object", "default":{"token":"selection-secret-sentinel"}, "secret":true});
    for index in [2, 3] {
        value["nodes"][index]["kind"] = json!("builtin.identity");
        value["nodes"][index]["config"] = json!({});
    }
    value["edges"].as_array_mut().unwrap().extend([
        json!({"from_node":"params", "from_output":"payload", "to_node":"local", "to_input":"input"}),
        json!({"from_node":"params", "from_output":"payload", "to_node":"ci", "to_input":"input"})
    ]);
    let registry = NodeRegistry::from_inventory().unwrap();
    for ci in [false, true] {
        let plan =
            compile_definition(&serde_json::from_value(value.clone()).unwrap(), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let harness = capture::Harness::new(false);
        let recording = SnapshotRecording::memory();
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
        "version":"2026-10-03", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[
            controlled::source(json!("int")),
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"positive", "condition":{"source":{"output":"feed.item", "path":""}, "operator":"gt", "value":0}}]}},
            {"id":"active", "kind":"builtin.identity"},
            {"id":"inactive", "kind":"builtin.identity"},
            {"id":"collect", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}}
        ],
        "edges":[
            {"from_node":"feed", "from_output":"item", "to_node":"active", "to_input":"input"},
            {"from_node":"feed", "from_output":"item", "to_node":"inactive", "to_input":"input"},
            {"from_node":"active", "from_output":"value", "to_node":"collect", "to_input":"item", "exclusive":true},
            {"from_node":"inactive", "from_output":"value", "to_node":"collect", "to_input":"item", "exclusive":true}
        ],
        "control_edges":[
            {"from_node":"feed", "from_output":"item", "to_node":"route"},
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
    assert_eq!(skipped.0[0].source_node, "route");
    assert_eq!(skipped.1.len(), 1);
    assert_eq!(skipped.1[0].source_node, "active");
}

#[test]
fn exclusive_startup_input_activates_a_stream_producer() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for ci in [false, true] {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-10-03", "dependencies":{}, "execution":{"mode":"stream"},
            "nodes":[
                {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
                    "control":{"type":"string"}, "ci":{"type":"bool", "default":ci, "secret":false}
                }}},
                {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"ci", "condition":{"source":{"output":"params.ci", "path":""}, "operator":"eq", "value":true}}]}},
                {"id":"local", "kind":"builtin.identity"},
                {"id":"ci", "kind":"builtin.identity"},
                controlled::source(json!("int")),
                {"id":"consume", "kind":"builtin.identity"}
            ],
            "edges":[
                {"from_node":"params", "from_output":"control", "to_node":"local", "to_input":"input"},
                {"from_node":"params", "from_output":"control", "to_node":"ci", "to_input":"input"},
                {"from_node":"feed", "from_output":"item", "to_node":"consume", "to_input":"input"},
                {"from_node":"local", "from_output":"value", "to_node":"feed", "to_input":"control", "exclusive":true},
                {"from_node":"ci", "from_output":"value", "to_node":"feed", "to_input":"control", "exclusive":true}
            ],
            "control_edges":[
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
