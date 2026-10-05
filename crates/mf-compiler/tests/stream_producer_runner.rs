#![cfg(unix)]
mod common;
#[path = "fixtures/controlled_source.rs"]
mod controlled_source;
#[path = "fixtures/multi-nodes/src/line_producer.rs"]
mod line_producer;

extern crate mfn_core as _;
use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_stream,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
    thread,
};

fn run(executable: &Path, input: &[u8]) -> Output {
    let mut child = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
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
fn external_line_producer_matches_memory_and_drains_after_stdin_closes() {
    let root = tempfile::tempdir().unwrap();
    let definition_path = root.path().join("flow.json");
    let executable = root.path().join("flow");
    let build = root.path().join("build");
    let document = root.path().join("document.txt");
    let empty = root.path().join("empty.txt");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    let value = json!({
        "version":"2026-10-03",
        "execution":{"mode":"stream", "limits":{"max_pending_messages":4, "workers":1}},
        "dependencies":{
            "fixture":{"package":"fixture-multi-nodes", "path":fixture},
            "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}
        },
        "nodes":[
            {"id":"read", "kind":"fixture.read_lines"},
            {"id":"batch", "kind":"builtin.batch", "config":{"max_items":3, "max_wait_ms":3_600_000}},
            {"id":"copy", "kind":"builtin.identity"},
            {"id":"feed", "kind":"builtin.readline"}
        ],
        "edges":[
            {"from_node":"feed", "from_output":"line", "to_node":"read", "to_input":"path"},
            {"from_node":"read", "from_output":"line", "to_node":"batch", "to_input":"item"},
            {"from_node":"batch", "from_output":"items", "to_node":"copy", "to_input":"input"}
        ],
        "outputs":[{"name":"lines", "node":"copy", "port":"value"}]
    });
    fs::write(&definition_path, value.to_string()).unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    compile_project_with_options(
        &CompileRequest {
            definition: &definition_path,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &support,
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    for argument in ["--validate", "--describe"] {
        let output = Command::new(&executable)
            .arg(argument)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!document.exists());
    let mut expected_lines = vec![
        String::from("first"),
        String::new(),
        String::from("  padded  "),
        String::from("caf\u{e9}"),
    ];
    expected_lines.extend((0..200).map(|value| format!("line {value}")));
    expected_lines.push("last without newline".into());
    fs::write(&document, expected_lines.join("\r\n")).unwrap();
    fs::write(&empty, "").unwrap();
    let mut definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let feed = definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "feed")
        .unwrap();
    feed.kind = "test.controlled_source".into();
    feed.config = json!({"item_type":"string"});
    definition
        .edges
        .iter_mut()
        .find(|edge| edge.from_node.as_str() == "feed")
        .unwrap()
        .from_output = "item".into();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&plan, &registry).unwrap();
    let running =
        controlled_source::SourceRun::start(prepared, "feed", Default::default()).unwrap();
    let input = running.source.clone();
    let instance = running.instance;
    let empty_input = format!("{}\n", empty.display());
    let paths = [empty.clone(), document.clone(), empty];
    let input_bytes = paths
        .iter()
        .map(|path| format!("{}\n", path.display()))
        .collect::<String>();
    let sender = thread::spawn(move || {
        for path in paths {
            input.send(json!(path)).unwrap();
        }
        input.close();
    });
    let mut memory = Vec::new();
    while let Some(output) = instance.recv().unwrap() {
        memory.push(serde_json::to_value(output.outputs).unwrap());
    }
    sender.join().unwrap();
    instance.join().unwrap();
    let expected: Vec<_> = expected_lines
        .chunks(3)
        .map(|lines| json!({"lines":lines}))
        .collect();
    assert_eq!(memory, expected);
    assert_eq!(records(&run(&executable, input_bytes.as_bytes())), memory);
    assert!(records(&run(&executable, b"")).is_empty());
    assert!(records(&run(&executable, empty_input.as_bytes())).is_empty());
    let missing = format!("{}\n", root.path().join("missing.txt").display());
    let failed = run(&executable, missing.as_bytes());
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("`read`"));
    let invalid = root.path().join("invalid.txt");
    fs::write(&invalid, b"\xff\n").unwrap();
    assert!(
        !run(&executable, format!("{}\n", invalid.display()).as_bytes())
            .status
            .success()
    );
}
