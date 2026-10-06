extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{PreparedStream, StreamOptions, TextInput, WorkflowArguments};
use serde_json::{Value, json};
use std::{fs, fs::File};

fn prepare(nodes: Value, edges: Value) -> PreparedStream {
    let definition = WorkflowDefinition::from_json(
        &json!({
            "version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
            "nodes":nodes, "edges":edges,
            "outputs":[{"name":"text", "node":"read", "port":"line"}]
        })
        .to_string(),
    )
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    instantiate_stream(
        &compile_definition(&definition, &registry).unwrap(),
        &registry,
    )
    .unwrap()
}

fn arguments(value: Value) -> WorkflowArguments {
    WorkflowArguments::try_from(value).unwrap()
}

#[test]
fn readline_uses_initial_or_upstream_paths_and_stdin_as_text() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lines.txt");
    fs::write(&path, "[1,2]\r\n\n  padded  \n\u{4f60}\u{597d}\nlast\r").unwrap();
    let modes = ["initial", "upstream", "stdin"].into_iter();
    #[cfg(unix)]
    let modes = modes.chain(["symlink"]);
    for mode in modes {
        let mut nodes = json!([{"id":"read", "kind":"builtin.readline"}]);
        let mut edges = json!([]);
        let mut options = StreamOptions::default();
        match mode {
            "initial" => options.arguments = arguments(json!({"read":{"path":path}})),
            #[cfg(unix)]
            "symlink" => {
                let link = root.path().join("lines.link");
                std::os::unix::fs::symlink(&path, &link).unwrap();
                options.arguments = arguments(json!({"read":{"path":link}}));
            }
            "upstream" => {
                nodes
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"path", "kind":"builtin.constant", "config":{"value":path}}));
                edges = json!([{"from_node":"path", "from_output":"value", "to_node":"read", "to_input":"path"}]);
            }
            _ => options.stdin = Some(TextInput::new(File::open(&path).unwrap())),
        }
        let prepared = prepare(nodes, edges);
        assert_eq!(
            prepared
                .plan()
                .input_schema()
                .stdin_owner(&options.arguments)
                .unwrap()
                .is_some(),
            mode == "stdin"
        );
        let instance = prepared.start_with_options(options).unwrap();
        for expected in ["[1,2]", "", "  padded  ", "\u{4f60}\u{597d}", "last\r"] {
            assert_eq!(
                instance.recv().unwrap().unwrap().outputs["text"],
                json!(expected)
            );
        }
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
}

#[test]
fn readline_validates_conditional_ownership_and_reports_read_failures() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lines.txt");
    fs::write(&path, "").unwrap();
    let prepared = prepare(
        json!([
            {"id":"read", "kind":"builtin.readline"}, {"id":"other", "kind":"builtin.readline"}
        ]),
        json!([]),
    );
    let schema = prepared.plan().input_schema();
    assert!(schema.stdin_owner(&WorkflowArguments::default()).is_err());
    let files = arguments(json!({"read":{"path":path}, "other":{"path":path}}));
    assert_eq!(schema.stdin_owner(&files).unwrap(), None);
    assert_eq!(
        schema
            .stdin_owner(&arguments(json!({"other":{"path":path}})))
            .unwrap(),
        Some("read")
    );
    let instance = prepared
        .start_with_options(StreamOptions {
            arguments: files,
            ..Default::default()
        })
        .unwrap();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();

    let single = || prepare(json!([{"id":"read", "kind":"builtin.readline"}]), json!([]));
    for values in [
        json!({}),
        json!({"read":{"path":null}}),
        json!({"read":{"path":42}}),
    ] {
        assert!(
            single()
                .start_with_options(StreamOptions {
                    arguments: arguments(values),
                    ..Default::default()
                })
                .is_err()
        );
    }
    let failures = [
        (root.path().join("missing"), "missing"),
        (path, "line 2"),
        (root.path().to_path_buf(), "requires a regular file"),
    ]
    .into_iter();
    #[cfg(unix)]
    let failures = failures.chain([(
        std::path::PathBuf::from("/dev/null"),
        "requires a regular file",
    )]);
    for (path, expected) in failures {
        if expected == "line 2" {
            fs::write(&path, b"first\n\xff\n").unwrap();
        }
        let instance = single()
            .start_with_options(StreamOptions {
                arguments: arguments(json!({"read":{"path":path}})),
                ..Default::default()
            })
            .unwrap();
        while let Ok(Some(_)) = instance.recv() {}
        assert!(instance.join().unwrap_err().to_string().contains(expected));
    }
    let registry = NodeRegistry::from_inventory().unwrap();
    assert!(registry.get("builtin.channel").is_none());
    assert!(registry.get("builtin.stdin").is_none());
}

#[cfg(unix)]
#[test]
fn readline_rejects_fifos_without_waiting_for_a_writer() {
    use nix::{sys::stat::Mode, unistd::mkfifo};
    use std::{
        os::unix::fs::symlink,
        path::PathBuf,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    if let Some(path) = std::env::var_os("MF_TEST_READLINE_PATH") {
        let path = PathBuf::from(path);
        let instance = prepare(json!([{"id":"read", "kind":"builtin.readline"}]), json!([]))
            .start_with_options(StreamOptions {
                arguments: arguments(json!({"read":{"path":path}})),
                ..Default::default()
            })
            .unwrap();
        let error = instance.recv().unwrap_err().to_string();
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains("requires a regular file"), "{error}");
        assert!(instance.join().is_err());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("input.fifo");
    let link = root.path().join("input.link");
    mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    symlink(&fifo, &link).unwrap();
    for path in [fifo, link] {
        // Bound the entire invocation, including worker teardown, if FIFO opening regresses.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "readline_rejects_fifos_without_waiting_for_a_writer",
                "--nocapture",
            ])
            .env("MF_TEST_READLINE_PATH", &path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("readline rejection timed out for {path:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
