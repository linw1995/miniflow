#[path = "fixtures/observation_capture.rs"]
mod capture;
mod common;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, compile_definition, instantiate_compiled, instantiate_stream};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::{Value, json};

fn definition(mode: &str, config: Value) -> Value {
    json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"each","kind":"builtin.iteration","config":{
            "mode":mode,"stream_outputs":{"increment":{"node":"step","port":"delta"}},
            "body":{"nodes":[{"id":"step","kind":"fixture.iteration_stream","config":config,
                "inputs":{"count":{"from":{"node":"%iteration","port":"item"}}}}],
                "result":{"node":"step","port":"total"}}
        }}],
        "outputs":[{"name":"increment","node":"each","port":"increment"},
                   {"name":"results","node":"each","port":"results"}]
    })
}

fn start(mut value: Value, items: Value, workers: usize) -> mf_runtime::StreamInstance {
    value["execution"] = json!({"limits":{"workers":workers,"max_pending_messages":2}});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(json!({"each.items":items})).unwrap(),
            ..Default::default()
        })
        .unwrap()
}

fn drain(instance: mf_runtime::StreamInstance) -> Vec<Value> {
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    values
}

fn increments(values: &[Value]) -> Vec<u64> {
    values
        .iter()
        .filter_map(|value| value.get("increment"))
        .map(|value| value.as_u64().unwrap())
        .collect()
}

fn results(values: &[Value]) -> Vec<Value> {
    values
        .iter()
        .filter_map(|value| value.get("results"))
        .cloned()
        .collect()
}

#[test]
fn item_streams_keep_terminal_collection_and_support_single_worker_and_empty_inputs() {
    for mode in ["sequential", "parallel"] {
        for workers in [1, 2] {
            for (items, expected) in [
                (json!([1, 2]), json!([1, 2])),
                (json!({"b":2,"a":1}), json!([1, 2])),
                (json!([]), json!([])),
            ] {
                let values = drain(start(definition(mode, json!({})), items, workers));
                assert_eq!(results(&values), vec![expected.clone()]);
                let mut emitted = increments(&values);
                if mode == "sequential" || workers == 1 {
                    assert_eq!(json!(emitted), expected);
                } else {
                    emitted.sort_unstable();
                    assert_eq!(json!(emitted), expected);
                }
                assert_eq!(values.len(), expected.as_array().unwrap().len() + 1);
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut terminal = definition(
        "parallel",
        json!({"wait_first":root.path().join("unreleased")}),
    );
    terminal["outputs"]
        .as_array_mut()
        .unwrap()
        .retain(|output| output["name"] != "increment");
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(terminal).unwrap(), &registry).unwrap();
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(WorkflowArguments::try_from(json!({"each.items":[1,2]})).unwrap())
            .unwrap()["results"],
        json!([1, 2])
    );
}

#[test]
fn parallel_streams_arrive_before_earlier_items_and_cancellation_stops_the_scope() {
    for cancel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let release = root.path().join("release");
        let instance = start(
            definition("parallel", json!({"wait_first":release})),
            json!([1, 2]),
            2,
        );
        let first = instance.receive().unwrap().unwrap();
        assert_eq!(first.output().outputs["increment"], json!(2));
        first.acknowledge().unwrap();
        if cancel {
            instance
                .cancellation()
                .cancel(mf_runtime::StreamError::Execution {
                    message: "cancel iteration after prefix".into(),
                });
            assert!(
                instance
                    .join()
                    .unwrap_err()
                    .to_string()
                    .contains("cancel iteration after prefix")
            );
        } else {
            std::fs::write(release, "continue").unwrap();
            let remaining = drain(instance);
            assert_eq!(increments(&remaining), vec![1]);
            assert_eq!(results(&remaining), vec![json!([1, 2])]);
        }
    }
}

#[test]
fn emitted_prefixes_do_not_change_item_error_policies() {
    for mode in ["sequential", "parallel"] {
        for (policy, expected) in [
            ("continue_on_error", json!([1, null])),
            ("remove_failed", json!([1])),
        ] {
            let mut value = definition(mode, json!({"fail_count":2}));
            value["nodes"][0]["config"]["on_error"] = json!(policy);
            let values = drain(start(value, json!([1, 2]), 2));
            let mut emitted = increments(&values);
            emitted.sort_unstable();
            assert_eq!(emitted, vec![1, 2]);
            assert_eq!(results(&values), vec![expected]);
        }
    }
}

#[test]
fn streamed_parallel_items_keep_correlated_observation_and_failure_outcomes() {
    let mut value = definition("parallel", json!({"fail_count":2}));
    value["nodes"][0]["config"]["on_error"] = json!("continue_on_error");
    value["execution"] = json!({"limits":{"workers":2,"max_pending_messages":2}});
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let harness = capture::Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), Default::default())
        .unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(json!({"each.items":[1,2]})).unwrap(),
            observation: Some(observation),
            ..Default::default()
        })
        .unwrap();
    let values = drain(instance);
    let mut emitted = increments(&values);
    emitted.sort_unstable();
    assert_eq!(emitted, vec![1, 2]);
    assert_eq!(results(&values), vec![json!([1, null])]);

    let records = harness.records();
    let spans = harness.spans.get_finished_spans().unwrap();
    for index in 0..2 {
        for event in [
            "mf.iteration.item.started",
            "mf.iteration.item.finished",
            "mf.iteration.node.started",
            "mf.iteration.node.finished",
        ] {
            let matching: Vec<_> = records
                .iter()
                .filter(|record| {
                    record.event_name == event && record.attributes["mf.iteration.index"] == index
                })
                .collect();
            assert_eq!(matching.len(), 1, "{event} for item {index}");
            assert_eq!(matching[0].attributes["mf.iteration.id"], "each");
            assert!(matching[0].trace_context.is_some());
        }
        let finished = records
            .iter()
            .find(|record| {
                record.event_name == "mf.iteration.item.finished"
                    && record.attributes["mf.iteration.index"] == index
            })
            .unwrap();
        assert_eq!(
            finished.attributes["mf.outcome"],
            if index == 0 { "succeeded" } else { "failed" }
        );
        let node_finished = records
            .iter()
            .find(|record| {
                record.event_name == "mf.iteration.node.finished"
                    && record.attributes["mf.iteration.index"] == index
            })
            .unwrap();
        assert_eq!(
            node_finished.attributes["mf.outcome"],
            if index == 0 { "succeeded" } else { "failed" }
        );
        if index == 1 {
            assert_eq!(node_finished.attributes["mf.failure.phase"], "execution");
        }
        let started = |event| {
            records
                .iter()
                .find(|record| {
                    record.event_name == event && record.attributes["mf.iteration.index"] == index
                })
                .unwrap()
        };
        let item = started("mf.iteration.item.started")
            .trace_context
            .as_ref()
            .unwrap();
        let node = started("mf.iteration.node.started")
            .trace_context
            .as_ref()
            .unwrap();
        assert_eq!(item.trace_id, node.trace_id);
        let node_span = spans
            .iter()
            .find(|span| span.span_context.span_id().to_string() == node.span_id)
            .unwrap();
        assert_eq!(node_span.parent_span_id.to_string(), item.span_id);
    }
}

#[test]
fn delivery_failure_is_fatal_under_every_item_policy_and_borrowed_sinks_stay_local() {
    use std::{cell::Cell, rc::Rc};
    struct Reject {
        count: Rc<Cell<usize>>,
        caller: std::thread::ThreadId,
        panic: bool,
    }
    impl mf_runtime::EmissionSink for Reject {
        fn send(&mut self, _: mf_runtime::NodeResult) -> Result<(), mf_runtime::StreamError> {
            assert_eq!(std::thread::current().id(), self.caller);
            self.count.set(self.count.get() + 1);
            assert!(!self.panic, "sink panic sentinel");
            Err(mf_runtime::StreamError::Execution {
                message: "delivery sentinel".into(),
            })
        }
    }
    for mode in ["sequential", "parallel"] {
        for policy in ["terminate", "continue_on_error", "remove_failed"] {
            for panic in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let mut value = definition(mode, json!({"marker_dir":root.path()}));
                value["nodes"][0]["config"]["on_error"] = json!(policy);
                value["outputs"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|output| output["name"] != "increment");
                let registry = NodeRegistry::from_inventory().unwrap();
                let plan =
                    compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
                let flow = instantiate_compiled(&plan, &registry).unwrap();
                let mut context = mf_runtime::ExecutionContext::default();
                context.configure_worker_limit(std::num::NonZeroUsize::new(2).unwrap());
                let count = Rc::new(Cell::new(0));
                let mut sink = Reject {
                    count: count.clone(),
                    caller: std::thread::current().id(),
                    panic,
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    flow.nodes()[0].node.execute_with_sink(
                        [("items".into(), json!((1..=32).collect::<Vec<_>>()).into())].into(),
                        &mut context,
                        &mut sink,
                    )
                }));
                if panic {
                    assert!(result.is_err());
                } else {
                    let error = result.unwrap().unwrap_err();
                    assert!(error.to_string().contains("delivery sentinel"));
                }
                assert_eq!(count.get(), 1);
                assert!(
                    std::fs::read_dir(root.path()).unwrap().count()
                        <= if mode == "sequential" { 1 } else { 2 }
                );
            }
        }
    }
}

#[test]
fn iteration_stream_bindings_reject_invalid_versions_aliases_sources_and_stream_results() {
    let mut old = definition("sequential", json!({}));
    old["version"] = json!("2026-10-25");
    assert!(serde_json::from_value::<mf_runtime::WorkflowDefinition>(old).is_err());
    let registry = NodeRegistry::from_inventory().unwrap();
    for bindings in [
        json!({"results":{"node":"step","port":"delta"}}),
        json!({" ":{"node":"step","port":"delta"}}),
        json!({"increment":{"node":"missing","port":"delta"}}),
        json!({"increment":{"node":"step","port":"total"}}),
    ] {
        let mut value = definition("sequential", json!({}));
        value["nodes"][0]["config"]["stream_outputs"] = bindings;
        assert!(compile_definition(&serde_json::from_value(value).unwrap(), &registry).is_err());
    }
    let mut value = definition("sequential", json!({}));
    value["nodes"][0]["config"]["body"]["result"]["port"] = json!("delta");
    assert!(
        compile_definition(&serde_json::from_value(value).unwrap(), &registry)
            .unwrap_err()
            .to_string()
            .contains("terminal body output")
    );
}

#[cfg(unix)]
#[test]
fn standalone_iterations_keep_parallel_streams_and_frozen_contracts_without_sources() {
    use std::{fs, process::Command};
    for mode in ["sequential", "parallel"] {
        let root = tempfile::tempdir().unwrap();
        let markers = root.path().join("markers");
        fs::create_dir(&markers).unwrap();
        let source = root.path().join("flow.json");
        let output = root.path().join("runner");
        let build = root.path().join("build");
        let mut value = definition(mode, json!({"marker_dir":markers}));
        value["nodes"][0]["config"]["stream_outputs"] =
            json!({"result":{"node":"step","port":"delta"}});
        value["outputs"][0]["port"] = json!("result");
        value["execution"] = json!({"limits":{"workers":1}});
        value["dependencies"] = json!({
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes","path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        });
        fs::write(&source, value.to_string()).unwrap();
        mf_compiler::compile_project_with_options(
            &mf_compiler::CompileRequest {
                definition: &source,
                output: &output,
                locked: false,
                build_dir: Some(&build),
                support: &mf_compiler::SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                },
            },
            &mf_compiler::RunnerOptions { telemetry: false },
        )
        .unwrap();
        assert_eq!(fs::read_dir(&markers).unwrap().count(), 0);
        fs::remove_file(source).unwrap();
        fs::remove_dir_all(build).unwrap();
        let invoke = |drift: bool| {
            let mut command = Command::new(&output);
            command
                .args(["--inputs", r#"{"each.items":[1,2]}"#])
                .env_clear()
                .env("PATH", "");
            if drift {
                command.env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1");
            }
            command.output().unwrap()
        };
        let drift = invoke(true);
        assert!(!drift.status.success());
        assert!(String::from_utf8_lossy(&drift.stderr).contains("prepared port contract"));
        assert!(drift.stdout.is_empty());
        assert_eq!(fs::read_dir(&markers).unwrap().count(), 0);
        let result = invoke(false);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(increments(&values), vec![1, 2]);
        assert_eq!(results(&values), vec![json!([1, 2])]);
    }
}
