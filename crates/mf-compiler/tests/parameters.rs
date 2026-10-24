#[path = "common/snapshots.rs"]
mod snapshots;
use snapshots::Snapshots;
extern crate mfn_core as _;
mod common;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{ExecutionContext, WorkflowArguments};
use serde_json::{Value, json};
use std::{error::Error, process::Command};

const TOKEN: &str = "MINIFLOW_PARAMETERS_TEST_TOKEN";
const EMPTY: &str = "MINIFLOW_PARAMETERS_TEST_EMPTY";
const MISSING: &str = "MINIFLOW_PARAMETERS_TEST_MISSING";
const INVALID: &str = "MINIFLOW_PARAMETERS_TEST_INVALID";
const NUMBER: &str = "MINIFLOW_PARAMETERS_TEST_NUMBER";
const DUPLICATE: &str = "MINIFLOW_PARAMETERS_TEST_DUPLICATE";
const SENTINEL: &str = "synthetic-parameter-credential";

fn definition(parameters: Value) -> WorkflowDefinition {
    let outputs: Vec<_> = parameters
        .as_object()
        .unwrap()
        .keys()
        .map(|name| json!({"name": name, "node": "params", "port": name}))
        .collect();
    serde_json::from_value(json!({
        "version": "2026-10-03", "dependencies": {},
        "nodes": [{"id": "params", "kind": "builtin.parameters", "config": {"parameters": parameters}}],
        "outputs": outputs
    })).unwrap()
}

fn parameters() -> Value {
    json!({
        "token": {"type": "string", "env": TOKEN},
        "endpoint": {"type": "string", "env": MISSING, "default": "https://example.test", "secret": false},
        "count": {"type": "uint", "env": NUMBER, "default": 30, "secret": false},
        "empty": {"type": "string", "env": EMPTY, "default": "unused", "secret": false},
        "label": {"type": {"nullable": "string"}, "default": null},
        "items": {"type": {"list": "int"}, "default": [1, 2]}
    })
}

#[test]
fn configuration_and_graph_placement_are_validated_without_reading_sources() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let registration = registry.get(mfn_core::PARAMETERS_KIND).unwrap();
    for config in [
        json!({}),
        json!({"parameters": {}}),
        json!({"parameters": {"": {"type": "string"}}}),
        json!({"parameters": {"x": {}}}),
        json!({"parameters": {"x": {"type": "invalid"}}}),
        json!({"parameters": {"x": {"type": "string", "env": ""}}}),
        json!({"parameters": {"x": {"type": "string", "env": "A=B"}}}),
        json!({"parameters": {"x": {"type": "string", "env": "A\u{0}B"}}}),
        json!({"parameters": {"x": {"type": "string", "secret": "true"}}}),
        json!({"parameters": {"x": {"type": "int", "default": "42"}}}),
        json!({"parameters": {"x": {"type": "string", "default": null}}}),
        json!({"parameters": {"x": {"type": "string", "unknown": true}}}),
        json!({"parameters": {"x": {"type": "string"}}, "unknown": true}),
    ] {
        assert!(
            registration.instantiate(config.clone()).is_err(),
            "{config}"
        );
    }
    let plan = compile_definition(&definition(parameters()), &registry).unwrap();
    let interface = mf_compiler::describe_interface(&plan, &registry).unwrap();
    assert_eq!(
        interface.version,
        mf_runtime::WorkflowInterfaceVersion::V2026_10_16
    );
    let json = serde_json::to_vec(&interface).unwrap();
    assert_eq!(
        mf_runtime::WorkflowInterface::from_json(&json).unwrap(),
        interface
    );
    assert_eq!(
        interface.schema.inputs["params"]["label"].binding.default,
        Some(Value::Null)
    );
    assert!(!String::from_utf8(json).unwrap().contains(SENTINEL));
    let mut old = interface.clone();
    old.version = mf_runtime::WorkflowInterfaceVersion::V2026_10_11;
    assert!(
        old.validate_for_description(&mf_compiler::describe_compiled(&plan).unwrap())
            .is_err()
    );

    let mut value = serde_json::to_value(definition(json!({"x": {"type": "string"}}))).unwrap();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"before", "kind":"builtin.constant", "config":{"value":"text"}}));
    value["edges"] =
        json!([{"from_node":"before", "from_output":"value", "to_node":"params", "to_input":"x"}]);
    let error = compile_definition(&serde_json::from_value(value).unwrap(), &registry).unwrap_err();
    assert!(error.to_string().contains("top-level initial node"));
}

#[test]
fn parameter_execution_uses_an_isolated_environment() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "parameters_probe", "--ignored", "--nocapture"])
        .env(TOKEN, SENTINEL)
        .env(EMPTY, "")
        .env(NUMBER, "41")
        .env(DUPLICATE, format!("{{\"{SENTINEL}\":1,\"{SENTINEL}\":2}}"))
        .env_remove(MISSING);
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        command.env(
            INVALID,
            OsString::from_vec([SENTINEL.as_bytes(), &[0xff]].concat()),
        );
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked with an isolated synthetic environment"]
fn parameters_probe() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut graph = definition(parameters());
    graph
        .nodes
        .push(serde_json::from_value(json!({"id":"copy", "kind":"builtin.identity"})).unwrap());
    graph.edges.push(serde_json::from_value(json!({"from_node":"params", "from_output":"token", "to_node":"copy", "to_input":"input"})).unwrap());
    let plan = compile_definition(&graph, &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let recording = Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(recording.recorder());
    let output = flow.execute_in_context(&mut context).unwrap();
    assert_eq!(output["token"].as_str(), Some(SENTINEL));
    assert_eq!(output["endpoint"], json!("https://example.test"));
    assert_eq!(output["count"], json!(41));
    assert_eq!(output["empty"], json!(""));
    assert!(output["label"].is_null());
    assert_eq!(output["items"], json!([1, 2]));
    for name in ["token", "label", "items"] {
        assert!(output[name].is_secret());
    }
    assert!(output["items"][0].is_secret());
    assert!(!output["count"].is_secret());
    let snapshot = recording.current();
    let params = snapshot.node(&[], "params").unwrap();
    for values in [&params.inputs, &params.outputs] {
        for name in ["token", "label", "items"] {
            assert_eq!(values[name], json!("[REDACTED]"));
        }
        assert_eq!(values["count"], json!(41));
    }
    assert_eq!(
        snapshot.node(&[], "copy").unwrap().inputs["input"],
        json!("[REDACTED]")
    );
    assert_eq!(
        snapshot.node(&[], "copy").unwrap().outputs["value"],
        json!("[REDACTED]")
    );

    context.set_workflow_arguments(
        WorkflowArguments::try_from(json!({"params.token":"explicit", "params.count":7})).unwrap(),
    );
    let output = flow.execute_in_context(&mut context).unwrap();
    assert_eq!(output["token"].as_str(), Some("explicit"));
    assert!(output["token"].is_secret());
    assert_eq!(output["count"], json!(7));
    assert_eq!(
        recording.current().node(&[], "params").unwrap().inputs["token"],
        json!("[REDACTED]")
    );

    let marked = WorkflowArguments::parse(
        ["--params.token=explicit"].map(Into::into),
        flow.input_schema(),
    )
    .unwrap();
    assert!(marked.0["params.token"].is_secret());
    assert!(!format!("{marked:?}").contains("explicit"));

    #[cfg(feature = "codegen")]
    {
        let generated = plan.generate_execution_plans(&registry).unwrap();
        let targets: Vec<mf_runtime::NodeTarget> =
            serde_json::from_str(&generated.node_targets_json).unwrap();
        let target = mf_runtime::NodeTarget::find(&targets, "params").unwrap();
        let bound = target.bind_flow(&flow).unwrap();
        let output = mf_runtime::FlowRuntime::default()
            .execute_target_in_context(
                &bound,
                &mut context,
                WorkflowArguments::default(),
                WorkflowArguments::default(),
            )
            .unwrap();
        assert_eq!(output["token"].as_str(), Some(SENTINEL));
        assert!(output["token"].is_secret());
        let mut changed = target.clone();
        changed
            .inputs
            .inputs
            .get_mut("params")
            .unwrap()
            .get_mut("token")
            .unwrap()
            .binding
            .secret = false;
        assert!(changed.bind_flow(&flow).is_err());
    }

    let mut stream_graph = graph.clone();
    stream_graph.execution = Some(serde_json::from_value(json!({})).unwrap());
    let stream_plan = compile_definition(&stream_graph, &registry).unwrap();
    let stream = mf_compiler::instantiate_stream(&stream_plan, &registry)
        .unwrap()
        .start_with_options(mf_runtime::StreamOptions {
            snapshots: Some(recording.recorder()),
            ..Default::default()
        })
        .unwrap();
    let item = stream.recv().unwrap().unwrap();
    assert_eq!(item.outputs["token"].as_str(), Some(SENTINEL));
    assert!(item.outputs["token"].is_secret());
    assert!(stream.recv().unwrap().is_none());
    stream.join().unwrap();

    assert_failure(
        &registry,
        json!({"type":"string", "env":MISSING}),
        json!({}),
        "required input",
    );
    assert_failure(
        &registry,
        json!({"type":"uint", "env":TOKEN, "default":1}),
        json!({}),
        "valid JSON",
    );
    assert_failure(
        &registry,
        json!({"type":"uint", "env":NUMBER, "default":1}),
        json!({"params.value":"invalid"}),
        "expected",
    );
    assert_failure(
        &registry,
        json!({"type":"uint", "env":EMPTY, "default":1}),
        json!({}),
        "valid JSON",
    );
    assert_failure(
        &registry,
        json!({"type":"object", "env":DUPLICATE}),
        json!({}),
        "valid JSON",
    );
    #[cfg(unix)]
    assert_failure(
        &registry,
        json!({"type":"string", "env":INVALID, "default":"unused"}),
        json!({}),
        "not valid Unicode",
    );
}

#[test]
fn reusing_a_context_rebinds_defaults_and_preserves_existing_secret_values() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut context = ExecutionContext::default();
    for default in [1, 2] {
        let plan = compile_definition(
            &definition(json!({"value":{"type":"int", "default":default, "secret":false}})),
            &registry,
        )
        .unwrap();
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        assert_eq!(
            flow.execute_in_context(&mut context).unwrap()["value"],
            json!(default)
        );
        let secret = mf_runtime::ValueRef::from(json!(42)).into_secret();
        let outputs = flow
            .execute_with_inputs(WorkflowArguments(
                [("params.value".into(), secret.clone())].into(),
            ))
            .unwrap();
        assert!(outputs["value"].ptr_eq(&secret));
    }
}

fn assert_failure(registry: &NodeRegistry, descriptor: Value, inputs: Value, message: &str) {
    let plan = compile_definition(&definition(json!({"value": descriptor})), registry).unwrap();
    let flow = instantiate_compiled(&plan, registry).unwrap();
    let recording = Snapshots::memory();
    let mut context = ExecutionContext::default();
    context.set_workflow_arguments(WorkflowArguments::try_from(inputs).unwrap());
    context.set_snapshot_recorder(recording.recorder());
    let error = flow.execute_in_context(&mut context).unwrap_err();
    assert!(error.to_string().contains(message), "{error}");
    assert!(recording.history().is_empty());
    let mut source: &dyn Error = &error;
    loop {
        assert!(!source.to_string().contains(SENTINEL));
        assert!(!format!("{source:?}").contains(SENTINEL));
        match source.source() {
            Some(next) => source = next,
            None => break,
        }
    }
}

#[test]
#[cfg(all(unix, feature = "codegen"))]
fn standalone_parameters_share_transports_and_binding_rules() {
    use mf_compiler::{
        CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options,
    };
    use std::{fs, process::Stdio};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("flow.json");
    let binary = root.path().join("flow");
    let build = root.path().join("build");
    let mut graph = serde_json::to_value(definition(json!({
        "token":{"type":"string", "env":TOKEN},
        "count":{"type":"uint", "default":30, "secret":false}
    })))
    .unwrap();
    graph["dependencies"] = json!({"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}});
    fs::write(&path, graph.to_string()).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &path,
            output: &binary,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    fs::remove_dir_all(&build).unwrap();
    let run = |args: &[&str], token: Option<&str>| {
        let mut command = Command::new(&binary);
        command
            .args(args)
            .stdin(Stdio::null())
            .env_remove(TOKEN)
            .env_remove("MF_CAPTURE_SNAPSHOTS");
        if let Some(token) = token {
            command.env(TOKEN, token);
        }
        command.output().unwrap()
    };
    let help = run(&["--help"], None);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("env: MINIFLOW_PARAMETERS_TEST_TOKEN"));
    let missing = run(&[], None);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    let file = root.path().join("inputs.json");
    fs::write(&file, r#"{"params.token":"explicit","params.count":7}"#).unwrap();
    for args in [
        vec!["--params.token=explicit", "--params.count=7"],
        vec![
            "--inputs",
            r#"{"params.token":"explicit","params.count":7}"#,
        ],
        vec!["--inputs-file", file.to_str().unwrap()],
        vec![
            "--node=params",
            "--params.token=explicit",
            "--params.count=7",
        ],
    ] {
        let output = run(&args, Some(SENTINEL));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"token":"explicit", "count":7})
        );
    }
    for args in [vec![], vec!["--node=params"]] {
        let output = run(&args, Some(SENTINEL));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({"token":SENTINEL, "count":30})
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
    }
    let invalid = run(&["--params.count=\"wrong\""], Some(SENTINEL));
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
}
