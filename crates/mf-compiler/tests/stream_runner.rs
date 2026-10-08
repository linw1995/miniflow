#![cfg(unix)]
mod common;
use mf_compiler::{
    CompileRequest, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_project_with_options, plan_definition,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Process(Option<Child>);
impl Process {
    fn spawn(path: &Path, args: &[&str]) -> Self {
        Self(Some(
            Command::new(path)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    fn finish(mut self) -> Output {
        let until = Instant::now() + Duration::from_secs(10);
        while self.child().try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "runner did not terminate");
            thread::sleep(Duration::from_millis(5));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run(path: &Path, input: &[u8]) -> Output {
    let mut child = Process::spawn(path, &[]);
    child
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(input)
        .unwrap();
    child.finish()
}
fn records(output: &Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

#[test]
fn generated_streams_preserve_protocol_boundaries_and_installation_guarantees() {
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("flow.json");
    let executable = root.path().join("flow");
    let build = root.path().join("build");
    let trace = root.path().join("calls");
    let mut definition: Value =
        serde_json::from_str(include_str!("../../../examples/stream-batch.json")).unwrap();
    definition["dependencies"]["core"]["path"] =
        json!(common::crates_dir().join("builtin-nodes/core"));
    definition["dependencies"]
        .as_object_mut()
        .unwrap()
        .remove("code");
    definition["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "convert");
    {
        let edges = definition["edges"].as_array_mut().unwrap();
        let input = edges
            .iter_mut()
            .find(|edge| edge["to_node"] == "convert")
            .unwrap();
        input["to_node"] = json!("collect");
        input["to_input"] = json!("item");
        edges.retain(|edge| edge["from_node"] != "convert");
    }
    definition["dependencies"]["fixture"] = json!({"package":"fixture-multi-nodes", "path":Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes")});
    definition["nodes"][0]["config"]["max_wait_ms"] = json!(3_600_000);
    definition["nodes"][1] =
        json!({"id":"consume", "kind":"fixture.stream_echo", "config":{"trace":trace}});
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    let compile = |definition: &Value, telemetry| {
        fs::write(&definition_path, definition.to_string()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &definition_path,
                output: &executable,
                locked: false,
                build_dir: Some(&build),
                support: &support,
            },
            &RunnerOptions { telemetry },
        )
    };
    compile(&definition, false).unwrap();
    let planned: WorkflowDefinition = serde_json::from_value(definition.clone()).unwrap();
    assert!(
        plan_definition(&planned)
            .unwrap()
            .generate_artifacts()
            .unwrap()
            .rust_source
            .contains("builtin.readline")
    );

    fs::write(&trace, "").unwrap();
    assert_eq!(fs::read_to_string(&trace).unwrap(), "");

    fs::write(&trace, "").unwrap();
    let capture = Command::new(&executable)
        .env("MF_CAPTURE_SNAPSHOTS", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!capture.status.success());
    assert!(String::from_utf8_lossy(&capture.stderr).contains("snapshot capture"));
    assert_eq!(fs::read_to_string(&trace).unwrap(), "");

    fs::write(&trace, "").unwrap();
    let output = run(&executable, b"1\n2\r\n3\n4\n5");
    assert_eq!(
        records(&output),
        [json!({"batch":["1","2","3"]}), json!({"batch":["4","5"]})]
    );
    assert_eq!(
        fs::read_to_string(&trace).unwrap(),
        "prepare\nexecute\nexecute\n"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("factory diagnostic") && diagnostic.contains("execution diagnostic")
    );
    assert!(records(&run(&executable, b"")).is_empty());
    let invalid = run(&executable, b"\xff\n");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("line 1"));

    definition["nodes"][0]["config"]["max_wait_ms"] = json!(50);
    compile(&definition, true).unwrap();
    let mut idle = Process::spawn(&executable, &[]);
    let mut input = idle.child().stdin.take().unwrap();
    let stdout = idle.child().stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        sender.send(first).unwrap();
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        rest
    });
    input.write_all(b"7\n").unwrap();
    input.flush().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&receiver.recv_timeout(Duration::from_secs(5)).unwrap())
            .unwrap(),
        json!({"batch":["7"]})
    );
    drop(input);
    assert!(idle.finish().status.success());
    assert_eq!(reader.join().unwrap(), "");

    let mut broken = Process::spawn(&executable, &[]);
    drop(broken.child().stdout.take());
    let mut open_input = broken.child().stdin.take().unwrap();
    open_input.write_all(b"1\n2\n3\n").unwrap();
    open_input.flush().unwrap();
    let broken = broken.finish();
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stderr).contains("output failed"));
    drop(open_input);

    definition["nodes"][0]["config"]["max_wait_ms"] = json!(3_600_000);
    compile(&definition, true).unwrap();
    assert_eq!(
        records(&run(&executable, b"[1,2]\n[3]\n")),
        [json!({"batch":["[1,2]","[3]"]})]
    );
    let generated_time = fs::metadata(build.join("src/workflow.rs"))
        .unwrap()
        .modified()
        .unwrap();
    let unchanged = fs::read(&executable).unwrap();
    compile(&definition, true).unwrap();
    assert_eq!(
        fs::metadata(build.join("src/workflow.rs"))
            .unwrap()
            .modified()
            .unwrap(),
        generated_time
    );
    assert_eq!(fs::read(&executable).unwrap(), unchanged);

    let mut stalled_definition = definition.clone();
    stalled_definition["nodes"][0]["config"]["max_items"] = json!(1);
    compile(&stalled_definition, true).unwrap();
    let mut stalled = Process::spawn(&executable, &[]);
    let mut stalled_input = stalled.child().stdin.take().unwrap();
    let mut stalled_output = stalled.child().stdout.take().unwrap();
    let (started, ready) = mpsc::channel();
    let first_byte = thread::spawn(move || {
        let mut byte = [0];
        stalled_output.read_exact(&mut byte).unwrap();
        started.send(stalled_output).unwrap();
    });
    writeln!(stalled_input, "{}", "x".repeat(200000)).unwrap();
    stalled_input.flush().unwrap();
    let blocked_output = ready.recv_timeout(Duration::from_secs(5)).unwrap();
    first_byte.join().unwrap();
    stalled_input.write_all(b"\xff\n").unwrap();
    stalled_input.flush().unwrap();
    let stalled_result = stalled.finish();
    assert!(!stalled_result.status.success());
    assert!(String::from_utf8_lossy(&stalled_result.stderr).contains("line 2"));
    drop(blocked_output);
    drop(stalled_input);
    compile(&definition, true).unwrap();

    let binary = fs::read(&executable).unwrap();
    let lock = fs::read(definition_path.with_extension("lock")).unwrap();
    definition["nodes"][0]["config"]["max_items"] = json!(0);
    assert!(compile(&definition, true).is_err());
    assert_eq!(fs::read(&executable).unwrap(), binary);
    assert_eq!(
        fs::read(definition_path.with_extension("lock")).unwrap(),
        lock
    );
    let standalone = root.path().join("standalone");
    fs::copy(&executable, &standalone).unwrap();
    fs::remove_file(&definition_path).unwrap();
    fs::remove_file(definition_path.with_extension("lock")).unwrap();
    assert_eq!(
        records(&run(&standalone, b"[9]\n")),
        [json!({"batch":["[9]"]})]
    );
}
