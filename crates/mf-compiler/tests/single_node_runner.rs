#![cfg(all(unix, feature = "codegen"))]
mod common;

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

fn run(binary: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(binary);
    command.args(args).stdin(Stdio::null());
    for name in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "MF_CAPTURE_SNAPSHOTS",
    ] {
        command.env_remove(name);
    }
    command.output().unwrap()
}
fn result(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn compile(definition: &Path, binary: &Path, build: &Path, telemetry: bool) {
    compile_project_with_options(
        &CompileRequest {
            definition,
            output: binary,
            locked: false,
            build_dir: Some(build),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry },
    )
    .unwrap();
}

#[test]
fn combined_nodes_preserve_terminal_results_in_generated_and_isolated_execution() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let binary = root.path().join("flow");
    let build = root.path().join("build");
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    for streaming in [false, true] {
        let mut document = json!({
            "version":"2026-10-19",
            "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":plugin}},
            "nodes":[{"id":"count", "kind":"fixture.task_stream"}],
            "outputs":[{"name":"result", "node":"count", "port":"total"}]
        });
        if streaming {
            document["execution"] = json!({"mode":"stream"});
        }
        fs::write(&definition, document.to_string()).unwrap();
        compile(&definition, &binary, &build, true);
        assert_eq!(
            result(run(&binary, &["--count.count=100"])),
            json!({"result":100})
        );
        assert_eq!(
            result(run(&binary, &["--node=count", "--count.count=200"])),
            json!({"total":200})
        );
    }
}

#[test]
fn finite_runner_isolates_noisy_plugins_and_survives_without_build_inputs() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let binary = root.path().join("flow");
    let build = root.path().join("build");
    let trace = root.path().join("execution.trace");
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    fs::write(&definition, json!({"version":"2026-09-26", "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":plugin, "features":["fail-execution"]}},
        "nodes":[{"id":"source", "kind":"fixture.source", "config":{"print":true}}, {"id":"copy.part", "kind":"fixture.echo"},
        {"id":"after", "kind":"fixture.context", "config":{"ports":["value"], "required":"value", "outputs":{"value":null}, "trace":trace, "name":"after", "fail":true}},
        {"id":"context", "kind":"fixture.context", "config":{"ports":["value"], "read":"source.value"}}],
        "edges":[{"from_node":"source", "from_output":"value", "to_node":"copy.part", "to_input":"input"}],
        "control_edges":[{"from_node":"copy.part", "from_output":"value", "to_node":"after"}, {"from_node":"source", "from_output":"value", "to_node":"context"}],
        "outputs":[{"name":"alias", "node":"after", "port":"value"}]
    }).to_string()).unwrap();
    for telemetry in [false, true] {
        compile(&definition, &binary, &build, telemetry);
        for args in [vec!["--help"], vec!["--node=copy.part", "--help"]] {
            let help = run(&binary, &args);
            assert!(help.status.success());
            assert!(help.stderr.is_empty(), "{:?}", help.stderr);
            let stdout = String::from_utf8_lossy(&help.stdout);
            assert!(stdout.contains("Usage: workflow"), "{stdout}");
            assert!(stdout.contains("--node <ID>"), "{stdout}");
            if args.len() > 1 {
                assert!(stdout.contains("--copy.part.input <VALUE>"), "{stdout}");
            }
            assert!(!trace.exists());
        }
        let invalid = run(&binary, &["--unexpected"]);
        assert_eq!(invalid.status.code(), Some(2));
        assert!(invalid.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&invalid.stderr);
        assert!(stderr.contains("unexpected argument"), "{stderr}");
        assert!(!stderr.contains("factory diagnostic"));
        assert!(!trace.exists());
        let output = run(&binary, &["--node", "copy.part", "--copy.part.input", "42"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("factory diagnostic"));
        assert_eq!(result(output), json!({"value":42}));
        assert!(!trace.exists());
        for flag in ["-v", "-vv", "-vvv"] {
            let output = run(&binary, &[flag, "--node=copy.part", "--copy.part.input=42"]);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if flag == "-vvv" {
                let records: Vec<Value> = stderr
                    .lines()
                    .filter_map(|line| line.strip_prefix("[verbose] "))
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(
                    records.first().unwrap()["event_name"],
                    "mf.workflow.started"
                );
                assert_eq!(records.first().unwrap()["body"]["node_count"], 1);
                assert_eq!(
                    records.last().unwrap()["event_name"],
                    "mf.workflow.finished"
                );
                assert_eq!(
                    records
                        .iter()
                        .filter(|record| record["event_name"] == "mf.node.started")
                        .count(),
                    1
                );
            } else {
                assert!(stderr.contains("workflow started (nodes: 1"));
                assert!(stderr.contains("node \"copy.part\""));
            }
            assert_eq!(result(output), json!({"value":42}));
            assert!(!trace.exists());
        }
        let input = root.path().join("inputs.json");
        fs::write(&input, "{\"copy.part.input\":43}").unwrap();
        assert_eq!(
            result(run(
                &binary,
                &["--node=copy.part", "--inputs-file", input.to_str().unwrap()]
            )),
            json!({"value":43})
        );
        assert_eq!(
            result(run(
                &binary,
                &["--node=context", "--context", "{\"source.value\":44}"]
            )),
            json!({"value":44})
        );
        assert!(!trace.exists());
        for args in [
            vec!["--node=unknown"],
            vec!["--node=copy.part"],
            vec!["--node=context"],
            vec!["--node=context", "--context", "{\"other.value\":1}"],
            vec!["--node=copy.part", "--copy.part.input=wrong"],
        ] {
            let output = run(&binary, &args);
            assert!(!output.status.success(), "{args:?}");
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("factory diagnostic"));
        }
        let failure = run(&binary, &["--node=after"]);
        assert!(!failure.status.success() && failure.stdout.is_empty());
        assert!(String::from_utf8_lossy(&failure.stderr).contains("execution sentinel"));
        assert_eq!(fs::read_to_string(&trace).unwrap(), "after\n");
        fs::remove_file(&trace).unwrap();
        let normal = run(&binary, &[]);
        assert!(!normal.status.success());
        assert!(String::from_utf8_lossy(&normal.stderr).contains("execution sentinel"));
        if telemetry {
            let unavailable = Command::new(&binary)
                .args(["--node=copy.part", "--copy.part.input=45"])
                .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(result(unavailable), json!({"value":45}));
        }
    }
    let portable = root.path().join("portable");
    fs::copy(&binary, &portable).unwrap();
    fs::remove_file(&definition).unwrap();
    fs::remove_file(definition.with_extension("lock")).unwrap();
    fs::remove_dir_all(&build).unwrap();
    assert_eq!(
        result(run(
            &portable,
            &["--node=copy.part", "--inputs", "{\"copy.part.input\":46}"]
        )),
        json!({"value":46})
    );
}

#[test]
fn streaming_binary_selects_finite_typed_tasks_and_rejects_producers_and_events() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let binary = root.path().join("flow");
    let build = root.path().join("build");
    let core = common::crates_dir().join("builtin-nodes/core");
    fs::write(&definition, json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{"core":{"package":"mfn-core", "path":core}},
        "nodes":[{"id":"read", "kind":"builtin.readline"}, {"id":"copy", "kind":"builtin.identity"}, {"id":"batch", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":10}}],
        "edges":[{"from_node":"read", "from_output":"line", "to_node":"copy", "to_input":"input"}, {"from_node":"read", "from_output":"line", "to_node":"batch", "to_input":"item"}],
        "outputs":[{"name":"line", "node":"copy", "port":"value"}]
    }).to_string()).unwrap();
    for telemetry in [false, true] {
        compile(&definition, &binary, &build, telemetry);
        assert_eq!(
            result(run(&binary, &["--node=copy", "--copy.input=\"selected\""])),
            json!({"value":"selected"})
        );
        let verbose = run(
            &binary,
            &["--node=copy", "--copy.input=\"selected\"", "-vv"],
        );
        let stderr = String::from_utf8_lossy(&verbose.stderr);
        assert!(stderr.contains("workflow started (nodes: 1"));
        assert!(stderr.contains("node \"copy\""));
        assert!(!stderr.contains("node \"read\"") && !stderr.contains("node \"batch\""));
        assert_eq!(result(verbose), json!({"value":"selected"}));
        for id in ["read", "batch", "%loop", "copy.body"] {
            let output = run(&binary, &["--node", id]);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        let observed = Command::new(&binary)
            .args(["--node=copy", "--copy.input=\"finite\""])
            .env("MF_CAPTURE_SNAPSHOTS", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&observed.stderr).contains("unsupported for streaming"));
        assert_eq!(result(observed), json!({"value":"finite"}));
        let whole = Command::new(&binary)
            .env("MF_CAPTURE_SNAPSHOTS", "1")
            .stdin(Stdio::piped())
            .output()
            .unwrap();
        assert!(whole.status.success(), "{:?}", whole);
        assert!(String::from_utf8_lossy(&whole.stderr).contains("data capture unavailable"));
    }
    fs::remove_file(&definition).unwrap();
    fs::remove_dir_all(&build).unwrap();
    assert_eq!(
        result(run(&binary, &["--node=copy", "--copy.input=\"portable\""])),
        json!({"value":"portable"})
    );
}

#[test]
fn finite_typed_segments_keep_normal_execution_and_isolated_publication_contracts() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let binary = root.path().join("flow");
    let build = root.path().join("build");
    let mut value = json!({"version":"2026-10-03", "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}},
        "nodes":[{"id":"a", "kind":"builtin.identity"}, {"id":"b", "kind":"builtin.identity"}, {"id":"c", "kind":"builtin.identity"}],
        "edges":[{"from_node":"a", "from_output":"value", "to_node":"b", "to_input":"input"}, {"from_node":"b", "from_output":"value", "to_node":"c", "to_input":"input"}],
        "outputs":[{"name":"alias", "node":"c", "port":"value"}]
    });
    fs::write(&definition, value.to_string()).unwrap();
    compile(&definition, &binary, &build, false);
    assert_eq!(result(run(&binary, &["--a.input=7"])), json!({"alias":7}));
    assert_eq!(
        result(run(&binary, &["--node=b", "--b.input=42"])),
        json!({"value":42})
    );
    value["nodes"][0] = json!({"id":"a", "kind":"builtin.constant", "config":{"value":7}});
    fs::write(&definition, value.to_string()).unwrap();
    compile(&definition, &binary, &build, false);
    assert_eq!(
        result(run(&binary, &["--node=b", "--b.input=43"])),
        json!({"value":43})
    );
    let invalid = run(&binary, &["--node=b", "--b.input=\"wrong\""]);
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("output `value`"));
}
