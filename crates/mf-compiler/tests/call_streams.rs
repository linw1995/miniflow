mod common;
#[path = "fixtures/multi-nodes/src/task_stream.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, compile_definition, instantiate_compiled, instantiate_stream};
use mf_runtime::{StreamOptions, WorkflowArguments};
use serde_json::{Value, json};

fn child(config: Value) -> Value {
    json!({"version":"2026-10-03","dependencies":{},
        "nodes":[{"id":"step","kind":"fixture.task_stream","config":config}],
        "outputs":[{"name":"increment","node":"step","port":"delta"},
                   {"name":"count","node":"step","port":"total"}]
    })
}

fn caller(child: Value) -> Value {
    json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"invoke","kind":"workflow.call","config":{"flow":"child","definition":child}}],
        "outputs":[{"name":"increment","node":"invoke","port":"increment"},
                   {"name":"count","node":"invoke","port":"count"}]
    })
}

fn arguments(input: &str) -> WorkflowArguments {
    WorkflowArguments::try_from(json!({(input):2})).unwrap()
}

fn drain(value: Value, input: &str) -> Vec<Value> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: arguments(input),
            ..Default::default()
        })
        .unwrap();
    let mut values = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        values.push(serde_json::to_value(output.outputs).unwrap());
    }
    instance.join().unwrap();
    values
}

#[test]
fn nested_calls_project_public_streams_and_keep_terminal_adaptation_unsubscribed() {
    let root = tempfile::tempdir().unwrap();
    for nested in [false, true] {
        let input = if nested {
            "invoke.invoke.step.count"
        } else {
            "invoke.step.count"
        };
        let definition = if nested {
            caller(caller(child(json!({}))))
        } else {
            caller(child(json!({})))
        };
        let values = drain(definition.clone(), input);
        assert_eq!(values.len(), 3);
        assert_eq!(
            values
                .iter()
                .filter_map(|value| value.get("increment"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!(0), json!(1)]
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| value.get("count").is_some())
                .cloned()
                .collect::<Vec<_>>(),
            vec![json!({"count":2})]
        );

        let leaf = child(json!({"wait_path":root.path().join("unreleased")}));
        let mut terminal = if nested {
            caller(caller(leaf))
        } else {
            caller(leaf)
        };
        terminal["outputs"]
            .as_array_mut()
            .unwrap()
            .retain(|output| output["name"] != "increment");
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan =
            compile_definition(&serde_json::from_value(terminal).unwrap(), &registry).unwrap();
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute_with_inputs(arguments(input))
                .unwrap()["count"],
            json!(2)
        );
    }
}

#[test]
fn calls_export_loop_streams_without_exposing_loop_state() {
    let body = json!({"version":"2026-10-28","dependencies":{},
        "nodes":[{"id":"repeat","kind":"workflow.loop","loop":{
            "max_iterations":2,"variables":[{"name":"count","type":"usize"}],
            "stream_outputs":{"delta":{"node":"step","port":"delta"}},
            "body":{"nodes":[{"id":"step","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%loop","port":"count"}}}}]}
        }}],
        "outputs":[{"name":"increment","node":"repeat","port":"delta"}, {"name":"count","node":"repeat","port":"count"}]
    });
    let values = drain(caller(body), "invoke.repeat.count");
    assert_eq!(values.len(), 5);
    assert_eq!(
        values
            .iter()
            .filter_map(|value| value.get("increment"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!(0), json!(1), json!(0), json!(1)]
    );
    assert_eq!(
        values
            .iter()
            .filter(|value| value.get("count").is_some())
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!({"count":2})]
    );
}

#[test]
fn call_prefixes_arrive_before_completion_and_cancellation_remains_fatal() {
    let root = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let value = caller(child(json!({"wait_path":root.path().join("unreleased")})));
    let plan = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap();
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            arguments: arguments("invoke.step.count"),
            ..Default::default()
        })
        .unwrap();
    let first = instance.receive().unwrap().unwrap();
    assert_eq!(first.output().outputs["increment"], json!(0));
    first.acknowledge().unwrap();
    instance
        .cancellation()
        .cancel(mf_runtime::StreamError::Execution {
            message: "cancel call after prefix".into(),
        });
    assert!(
        instance
            .join()
            .unwrap_err()
            .to_string()
            .contains("cancel call after prefix")
    );
}

#[cfg(unix)]
#[test]
fn standalone_calls_forward_frozen_streams_without_sources() {
    use std::{fs, process::Command};
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let source = root.path().join("parent.json");
    let child_path = root.path().join("child.json");
    let output = root.path().join("runner");
    let build = root.path().join("build");
    let mut leaf = child(json!({"marker":marker}));
    leaf["execution"] = json!({});
    leaf["dependencies"] = json!({"fixture":{"package":"fixture-multi-nodes","path":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}});
    let mut parent = caller(leaf.clone());
    parent["nodes"][0]["config"]
        .as_object_mut()
        .unwrap()
        .remove("definition");
    parent["flows"] = json!({"child":{"path":"child.json"}});
    parent["dependencies"] = json!({"core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")}});
    fs::write(&source, parent.to_string()).unwrap();
    fs::write(&child_path, leaf.to_string()).unwrap();
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
    assert!(!marker.exists());
    fs::remove_file(source).unwrap();
    fs::remove_file(child_path).unwrap();
    fs::remove_dir_all(build).unwrap();
    let invoke = |drift: bool| {
        let mut command = Command::new(&output);
        command
            .args(["--inputs", r#"{"invoke.step.count":2}"#])
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
    assert!(!marker.exists());
    let result = invoke(false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(marker.exists());
    let values: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(
        values
            .iter()
            .filter_map(|value| value.get("increment"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!(0), json!(1)]
    );
    assert_eq!(
        values
            .iter()
            .filter(|value| value.get("count").is_some())
            .cloned()
            .collect::<Vec<_>>(),
        vec![json!({"count":2})]
    );
}
