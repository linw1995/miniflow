#![cfg(unix)]
mod common;
extern crate mfn_core as _;

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn records(output: &Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn generated_streams_preserve_protocol_boundaries_and_installation_guarantees() {
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("flow.json");
    let executable = root.path().join("flow");
    let build = root.path().join("build");
    let trace = root.path().join("calls");
    let lines = root.path().join("lines");
    let mut definition = json!({
        "version":"2026-10-23", "dependencies":{
            "core":{"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")},
            "fixture":{"package":"fixture-multi-nodes","path":Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")}
        }, "nodes":[{"id":"feed","kind":"builtin.readline"},
            {"id":"collect","kind":"builtin.batch","config":{"max_items":3,"max_wait_ms":3600000},"inputs":{"item":{"$ref":"feed.line"}}},
            {"id":"consume","kind":"fixture.stream_echo","config":{"trace":trace},"inputs":{"input":{"$ref":"collect.items"}}}],
        "outputs":[{"name":"batch","node":"consume","port":"value"}]
    });
    let compile = |definition: &Value, telemetry| {
        fs::write(&definition_path, definition.to_string()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &definition_path,
                output: &executable,
                locked: false,
                build_dir: Some(&build),
                support: &SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                },
            },
            &RunnerOptions { telemetry },
        )
    };
    compile(&definition, false).unwrap();
    fs::write(&trace, "").unwrap();
    let missing = Command::new(&executable).output().unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("required stdin resource is unavailable")
    );
    assert_eq!(fs::read(&trace).unwrap(), b"");
    fs::write(&lines, b"1\n2\r\n3\n4\n5").unwrap();
    let run = |path: &Path| {
        Command::new(path)
            .arg("--feed.path")
            .arg(&lines)
            .env(mf_telemetry::SNAPSHOT_CAPTURE_ENV, "1")
            .output()
            .unwrap()
    };
    let output = run(&executable);
    assert_eq!(
        records(&output),
        [json!({"batch":["1","2","3"]}), json!({"batch":["4","5"]})]
    );
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "prepare\nexecute\nexecute\n"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    for text in [
        "telemetry-enabled build required",
        "factory diagnostic",
        "execution diagnostic",
    ] {
        assert!(diagnostic.contains(text), "{diagnostic}");
    }
    fs::write(&lines, b"").unwrap();
    assert!(records(&run(&executable)).is_empty());
    fs::write(&lines, b"\xff\n").unwrap();
    let invalid = run(&executable);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("line 1"));

    definition["nodes"][1]["config"]["max_items"] = json!(1);
    compile(&definition, true).unwrap();
    // A stalled consumer must not prevent a source failure from cancelling the writer.
    fs::write(&lines, [vec![b'x'; 400_000], b"\n\xff\n".to_vec()].concat()).unwrap();
    let mut child = Command::new(&executable)
        .arg("--feed.path")
        .arg(&lines)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let blocked_output = child.stdout.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("runner did not cancel blocked output");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let failed = child.wait_with_output().unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("line 2"));
    drop(blocked_output);

    fs::write(&lines, vec![b'x'; 400_000]).unwrap();
    let mut broken = Command::new(&executable)
        .arg("--feed.path")
        .arg(&lines)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = broken.stdout.take().unwrap();
    pipe.read_exact(&mut [0u8; 1]).unwrap();
    drop(pipe);
    let broken = broken.wait_with_output().unwrap();
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stderr).contains("output failed"));

    let generated_time = fs::metadata(build.join("src/workflow.rs"))
        .unwrap()
        .modified()
        .unwrap();
    let binary = fs::read(&executable).unwrap();
    compile(&definition, true).unwrap();
    assert_eq!(
        fs::metadata(build.join("src/workflow.rs"))
            .unwrap()
            .modified()
            .unwrap(),
        generated_time
    );
    assert_eq!(fs::read(&executable).unwrap(), binary);
    let lock = fs::read(definition_path.with_extension("lock")).unwrap();
    definition["nodes"][1]["config"]["max_items"] = json!(0);
    assert!(compile(&definition, true).is_err());
    assert_eq!(fs::read(&executable).unwrap(), binary);
    assert_eq!(
        fs::read(definition_path.with_extension("lock")).unwrap(),
        lock
    );
    let standalone = root.path().join("standalone");
    fs::rename(&executable, &standalone).unwrap();
    fs::remove_file(&definition_path).unwrap();
    fs::remove_file(definition_path.with_extension("lock")).unwrap();
    fs::remove_dir_all(build).unwrap();
    fs::write(&lines, b"[9]\n").unwrap();
    assert_eq!(records(&run(&standalone)), [json!({"batch":["[9]"]})]);
}
