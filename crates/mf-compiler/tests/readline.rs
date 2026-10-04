extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{ExecutionResources, PreparedStream, StreamOptions, TextInput, WorkflowArguments};
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
    for mode in ["initial", "upstream", "stdin"] {
        let mut nodes = json!([{"id":"read", "kind":"builtin.readline"}]);
        let mut edges = json!([]);
        let mut options = StreamOptions::default();
        match mode {
            "initial" => options.arguments = arguments(json!({"read":{"path":path}})),
            "upstream" => {
                nodes
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"path", "kind":"builtin.constant", "config":{"value":path}}));
                edges = json!([{"from_node":"path", "from_output":"value", "to_node":"read", "to_input":"path"}]);
            }
            _ => {
                options.resources = ExecutionResources::default()
                    .with_stdin(TextInput::new(File::open(&path).unwrap()))
            }
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
    for (path, expected) in [(root.path().join("missing"), "missing"), (path, "line 2")] {
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
