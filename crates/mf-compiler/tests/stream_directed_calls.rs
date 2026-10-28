extern crate mfn_code as _;
extern crate mfn_core as _;
#[path = "fixtures/multi-nodes/src/native_directed_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{PreparedStream, StreamError, StreamOptions};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::mpsc, time::Duration};

fn document() -> Value {
    json!({"version":"2026-10-28","dependencies":{},"execution":{"limits":{"workers":1,"max_pending_messages":3}},"nodes":[
        {"id":"a_source","kind":"fixture.directed_source"},
        {"id":"parent","kind":"fixture.directed_stream_caller","inputs":{"message":{"from":{"node":"a_source","port":"item"}}}},
        {"id":"z_config","kind":"fixture.directed_config"},
        {"id":"tool","kind":"builtin.code","config":{"language":"cel","inputs":{"query":"string","token":"string"},"code":{"text":"query + token"}},"inputs":{"token":{"from":{"node":"z_config","port":"token"}}}},
        {"id":"aaa_unused","kind":"builtin.code","config":{"language":"cel","inputs":{},"code":{"text":"1 / 0"}}}
    ],"calls":{"parent":{"search":{"target":"tool","ports":[{"output":"query","input":"query"}]},"unused":{"target":"aaa_unused"}}},
    "outputs":[{"name":"chunk","node":"parent","port":"increment"},{"name":"result","node":"parent","port":"value"}]})
}
fn parse(value: Value) -> WorkflowDefinition {
    serde_json::from_value(value).unwrap()
}
fn collect(prepared: PreparedStream) -> Vec<Value> {
    collect_with_options(prepared, StreamOptions::default())
}
fn collect_with_options(prepared: PreparedStream, options: StreamOptions) -> Vec<Value> {
    let instance = prepared.start_with_options(options).unwrap();
    let cancellation = instance.cancellation();
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| -> Result<Vec<Value>, StreamError> {
            let mut outputs = Vec::new();
            while let Some(output) = instance.recv()? {
                outputs.push(serde_json::to_value(output.outputs).unwrap());
            }
            instance.join()?;
            Ok(outputs)
        })();
        let _ = sender.send(result);
    });
    let result = receiver.recv_timeout(Duration::from_secs(5));
    if result.is_err() {
        cancellation.cancel(StreamError::Execution {
            message: "test deadline".into(),
        });
    }
    thread.join().unwrap();
    result.expect("directed stream must make progress").unwrap()
}

#[test]
fn startup_configuration_is_snapshotted_without_waiting_for_a_backpressured_producer() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&parse(document()), &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    assert!(
        prepared
            .plan()
            .program()
            .messages
            .iter()
            .any(|message| !message.startup_dependencies.is_empty())
    );
    assert!(!prepared.plan().input_schema().inputs.contains_key("tool"));
    let outputs = collect(prepared);
    let chunks: Vec<_> = outputs
        .iter()
        .filter_map(|output| output.get("chunk"))
        .collect();
    assert_eq!(chunks.len(), 8);
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.get("result").is_some())
            .count(),
        4
    );
    assert_eq!(
        chunks
            .iter()
            .map(|chunk| chunk["invocation"].as_u64().unwrap())
            .collect::<BTreeSet<_>>(),
        (0..8).collect()
    );
    let texts: Vec<_> = chunks
        .iter()
        .map(|chunk| chunk["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "oneconfig",
            "oneconfig",
            "twoconfig",
            "twoconfig",
            "threeconfig",
            "threeconfig",
            "fourconfig",
            "fourconfig"
        ]
    );
    plan.manifest(&registry).unwrap();
}

#[test]
fn startup_skips_are_call_outcomes_and_do_not_suppress_message_callers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"][2]["config"] = json!({"skip":true});
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let outputs = collect(instantiate_stream(&plan, &registry).unwrap());
    assert_eq!(outputs.len(), 12);
    assert!(outputs.iter().all(|output| {
        output
            .as_object()
            .unwrap()
            .values()
            .all(|value| value["skipped"] == true)
    }));
}

#[test]
fn caller_frame_and_startup_inputs_compose_without_a_latest_value_cache() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"].as_array_mut().unwrap().push(json!({"id":"query","kind":"builtin.identity","inputs":{"input":{"from":{"node":"a_source","port":"item"}}}}));
    value["nodes"][3]["inputs"]["query"] = json!({"from":{"node":"query","port":"value"}});
    value["calls"]["parent"]["search"]["ports"] = json!([]);
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    assert_eq!(
        prepared
            .plan()
            .program()
            .startup_visibility
            .keys()
            .collect::<Vec<_>>(),
        ["z_config.token"]
    );
    let outputs = collect(prepared);
    let texts: Vec<_> = outputs
        .iter()
        .filter_map(|output| output.get("result"))
        .map(|value| value["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        ["oneconfig", "twoconfig", "threeconfig", "fourconfig"]
    );
}

#[test]
fn rejects_cross_message_inputs_and_terminal_producer_backpressure_cycles() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for cycle in [false, true] {
        let mut value = document();
        if cycle {
            value["edges"] = json!([{"from_node":"a_source","to_node":"z_config"}]);
        } else {
            value["nodes"].as_array_mut().unwrap().extend([
                json!({"id":"other","kind":"fixture.directed_source"}),
                json!({"id":"other_config","kind":"builtin.identity","inputs":{"input":{"from":{"node":"other","port":"item"}}}}),
            ]);
            value["nodes"][3]["inputs"]["token"] =
                json!({"from":{"node":"other_config","port":"value"}});
        }
        let error = compile_definition(&parse(value), &registry).unwrap_err();
        assert!(
            error.to_string().contains(if cycle {
                "connected streaming producer"
            } else {
                "different messages"
            }),
            "{error}"
        );
    }
}

#[test]
fn zero_output_startup_success_dependencies_still_gate_callers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"][2]["config"] = json!({"empty":true});
    value["nodes"][3]["inputs"] = json!({});
    value["nodes"][3]["config"]["inputs"] = json!({"query":"string"});
    value["nodes"][3]["config"]["code"]["text"] = "query".into();
    value["edges"] = json!([{"from_node":"z_config","to_node":"tool"}]);
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    assert_eq!(
        prepared.plan().program().startup_visibility.keys().count(),
        0
    );
    assert!(
        prepared
            .plan()
            .program()
            .messages
            .iter()
            .any(|message| !message.startup_dependencies.is_empty())
    );
    assert_eq!(collect(prepared).len(), 12);
}

#[test]
fn task_stream_tools_return_terminal_results_without_activating_emission_channels() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"][3]["kind"] = "fixture.directed_stream_tool".into();
    value["nodes"][3]["config"] = json!({});
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    assert_eq!(prepared.plan().program().messages.len(), 3);
    let outputs = collect(prepared);
    assert_eq!(outputs.len(), 12);
    assert_eq!(
        outputs
            .iter()
            .filter_map(|output| output.get("result"))
            .map(|value| value["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["oneconfig", "twoconfig", "threeconfig", "fourconfig"]
    );
}

#[path = "fixtures/observation_capture.rs"]
mod capture;
#[test]
fn invocation_ids_and_secret_snapshots_remain_scoped_across_native_frames() {
    use mf_telemetry::{
        event::Event,
        stream::{StreamPayload, StreamRecord},
    };
    use std::sync::{Arc, Mutex};
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["nodes"][2]["config"] = json!({"secret":true});
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let graph = plan.manifest(&registry).unwrap().description().unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), Default::default())
        .unwrap();
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let captured = snapshots.clone();
    collect_with_options(
        instantiate_stream(&plan, &registry).unwrap(),
        StreamOptions {
            observation: Some(observation),
            snapshots: Some(mf_runtime::SnapshotRecorder::new(move |event| {
                captured.lock().unwrap().push(event)
            })),
            ..Default::default()
        },
    );
    let mut starts = BTreeSet::new();
    for record in harness
        .records()
        .iter()
        .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
    {
        let record = StreamRecord::decode(record).unwrap();
        if let StreamPayload::Execution(event) = &record.payload {
            if let Event::NodeStarted { node, .. } = event
                && node.id == "tool"
            {
                let identity = record.identity.as_ref().unwrap();
                let mf_runtime::ScopePathEntry::Invocation {
                    target_id,
                    invocation,
                    ..
                } = node.path.last().unwrap()
                else {
                    panic!("missing directed call scope")
                };
                assert_eq!(&node.id, target_id);
                assert_eq!(
                    invocation.get() as u64 / 2,
                    identity.message.unwrap().sequence
                );
                starts.insert(node.path.clone());
            }
            if let Some((node, position)) = event.node() {
                graph.validate_scope_path(&node.path).unwrap();
                assert_eq!(
                    graph.node_in_scope(&node.path, &node.id).unwrap().kind,
                    node.kind
                );
                let path: Vec<_> = node
                    .path
                    .iter()
                    .map(|entry| entry.node_id().to_owned())
                    .collect();
                let order = if path.is_empty() {
                    &graph.execution_order
                } else {
                    &graph.body(&path).unwrap().execution_order
                };
                assert_eq!(order[position.get() as usize], node.id);
            }
            if let Event::CallStarted { path, .. } | Event::CallFinished { path, .. } = event {
                graph.validate_scope_path(path).unwrap();
            }
        }
    }
    assert_eq!(starts.len(), 8);
    assert!(
        harness
            .spans
            .get_finished_spans()
            .unwrap()
            .iter()
            .any(|span| span
                .attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "mf.node.id"
                    && attribute.value.as_str() == "tool"))
    );
    let snapshots = snapshots.lock().unwrap();
    let tools: Vec<_> = snapshots
        .iter()
        .filter(|event| event.node.as_ref() == "tool")
        .collect();
    assert_eq!(tools.len(), 16);
    assert!(
        tools
            .iter()
            .all(|event| event.snapshot.inputs["token"].as_str() == Some("[REDACTED]"))
    );
}

#[test]
fn concurrent_message_owners_share_unique_call_ids_and_one_startup_snapshot() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["execution"]["limits"]["max_pending_messages"] = 5.into();
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"other_source","kind":"fixture.directed_source","config":{"values":["other1","other2","other3","other4"]}}),
        json!({"id":"other_parent","kind":"fixture.directed_stream_caller","inputs":{"message":{"from":{"node":"other_source","port":"item"}}}}),
    ]);
    value["calls"]["other_parent"] = value["calls"]["parent"].clone();
    value["outputs"].as_array_mut().unwrap().extend([
        json!({"name":"other_chunk","node":"other_parent","port":"increment"}),
        json!({"name":"other_result","node":"other_parent","port":"value"}),
    ]);
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let outputs = collect(instantiate_stream(&plan, &registry).unwrap());
    assert_eq!(outputs.len(), 24);
    let ids: BTreeSet<_> = outputs
        .iter()
        .filter_map(|output| output.get("chunk").or_else(|| output.get("other_chunk")))
        .map(|value| value["invocation"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, (0..16).collect());
}

#[cfg(feature = "codegen")]
mod common;
#[cfg(feature = "codegen")]
#[test]
fn generated_native_calls_keep_startup_snapshots_and_invocation_scopes() {
    use std::{
        fs,
        process::{Command, Stdio},
        time::Instant,
    };
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = document();
    value["dependencies"] = json!({
        "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
        "code":{"package":"mfn-code","path":common::crates_dir().join("builtin-nodes/code")},
        "custom":{"package":"fixture-multi-nodes","path":common::crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")}
    });
    value["nodes"][1]["config"] = json!({"allow_drift":true});
    let plan = compile_definition(&parse(value), &registry).unwrap();
    let expected = collect(instantiate_stream(&plan, &registry).unwrap());
    let artifacts = plan.generate_runner_execution_plans(&registry).unwrap();
    assert!(artifacts.rust_source.contains("bind_calls_stream"));
    assert!(artifacts.rust_source.contains("startup_dependencies"));
    assert!(artifacts.rust_source.contains("z_config.token"));
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    mf_compiler::write_dependency_project(
        &project,
        &plan,
        &mf_compiler::SupportPackages::Local {
            crates_dir: common::crates_dir(),
        },
    )
    .unwrap();
    let main = project.join("src/main.rs");
    let mut source = fs::read_to_string(&main).unwrap();
    source.push_str(
        r#"
#[test]
fn native_scope_identity_mismatch_is_rejected() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let prepared = workflow::prepare_stream(&registry).unwrap();
    let error = workflow::bind_calls_stream(prepared, "mismatched").err().unwrap();
    assert!(error.to_string().contains("scope identity differs"));
}
"#,
    );
    fs::write(main, source).unwrap();
    let built = mf_compiler::cargo_command(&project)
        .args(["build", "--offline"])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let checked = mf_compiler::cargo_command(&project)
        .args([
            "test",
            "--offline",
            "--bin",
            "mf-generated-workflow",
            "native_scope_identity_mismatch_is_rejected",
        ])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let binary = common::runner_executable(&project, "debug");
    let drift = Command::new(&binary)
        .env("MINIFLOW_TEST_CALL_PORT_DRIFT", "1")
        .output()
        .unwrap();
    assert!(!drift.status.success());
    assert!(String::from_utf8_lossy(&drift.stderr).contains("metadata differs"));
    let installed = root.path().join("stream");
    fs::copy(binary, &installed).unwrap();
    fs::remove_dir_all(project).unwrap();
    let mut child = Command::new(installed)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // Different message channels may deliver in a different order; per-call identities remain exact.
    let canonical = |values: Vec<Value>| {
        let mut values: Vec<_> = values.iter().map(Value::to_string).collect();
        values.sort();
        values
    };
    assert_eq!(canonical(actual), canonical(expected));
}
