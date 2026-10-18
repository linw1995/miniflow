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
        "exclusive_inputs":[{"to_node":"consume", "to_input":"input", "sources":[
            {"from_node":"left", "from_output":"value"}, {"from_node":"right", "from_output":"value"}
        ]}],
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
            body["exclusive_inputs"][0]["to_input"] = json!("value");
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":false}},
                {"id":"loop", "kind":"workflow.loop", "loop":{"max_iterations":3, "variables":[{"name":"state", "type":"bool"}], "body":{
                    "nodes":body["nodes"], "control_edges":body["control_edges"], "exclusive_inputs":body["exclusive_inputs"]
                }}}
            ], "edges":[{"from_node":"start", "from_output":"value", "to_node":"loop", "to_input":"state"}],
            "outputs":[{"name":"result", "node":"loop", "port":"state"}]})
        } else {
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
                {"id":"start", "kind":"builtin.constant", "config":{"value":[true,false,true,false]}},
                {"id":"iterate", "kind":"builtin.iteration", "config":{"mode":mode, "body":{
                    "nodes":body["nodes"], "control_edges":body["control_edges"], "exclusive_inputs":body["exclusive_inputs"], "result":{"node":"consume", "port":"value"}
                }}}
            ], "edges":[{"from_node":"start", "from_output":"value", "to_node":"iterate", "to_input":"items"}],
            "outputs":[{"name":"result", "node":"iterate", "port":"results"}]})
        };
        let plan =
            compile_definition(&serde_json::from_value(definition).unwrap(), &registry).unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        for _ in 0..2 {
            let result = flow.execute().unwrap();
            assert_eq!(
                result["result"],
                if mode == "loop" {
                    json!(true)
                } else {
                    json!([false, true, false, true])
                }
            );
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
    value["edges"] = json!([{"from_node":"unrelated_a", "from_output":"value", "to_node":"unrelated_b", "to_input":"input"}]);
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
        let groups = serde_json::from_value(json!([{"to_node":"consume", "to_input":"input", "sources":[
            {"from_node":"left", "from_output":"value"}, {"from_node":"right", "from_output":"value"}
        ]}])).unwrap();
        let outputs =
            serde_json::from_value(json!([{"name":"result", "node":"consume", "port":"value"}]))
                .unwrap();
        let flow = mf_compiler::FlowBuilder::prepare_with_exclusive_inputs(
            nodes,
            vec![],
            groups,
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
