mod common;
#[path = "fixtures/stream_driver.rs"]
mod driver;
extern crate mfn_code as _;
extern crate mfn_core as _;
use mf_compiler::{
    NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition, compile_definition,
    instantiate_stream, write_dependency_project_with_options,
};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn edge(source: &str, port: &str, target: &str, input: &str) -> Value {
    json!({"from_node":source, "from_output":port, "to_node":target, "to_input":input})
}
fn definition(scenario: &str) -> WorkflowDefinition {
    let mut nodes = vec![
        json!({"id":"left", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x * 2"}}}),
        json!({"id":"right", "kind":"builtin.identity"}),
        json!({"id":"join", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"a":"int", "b":"int"}, "code":{"value":"a + b"}}}),
        json!({"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"positive", "condition":{"source":{"output":"join.value", "path":""}, "operator":"gt", "value":0}}]}}),
        json!({"id":"collect", "kind":"builtin.batch", "config":{"max_items":3, "max_wait_ms":100}}),
        json!({"id":"consume", "kind":"builtin.identity"}),
    ];
    let mut edges = vec![
        edge("%input", "item", "left", "x"),
        edge("%input", "item", "right", "input"),
        edge("left", "value", "join", "a"),
        edge("right", "value", "join", "b"),
        edge("join", "value", "collect", "item"),
        edge("collect", "items", "consume", "input"),
    ];
    let mut controls = vec![
        json!({"from_node":"join", "from_output":"value", "to_node":"route"}),
        json!({"from_node":"route", "from_output":"positive", "to_node":"collect"}),
    ];
    let mut result_port = "value";
    match scenario {
        "windows" => {}
        "chain" => {
            nodes[4]["config"]["max_items"] = json!(2);
            nodes[5] = json!({"id":"consume", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}});
            edges[5]["to_input"] = json!("item");
            result_port = "items";
        }
        "failure" => {
            nodes[4]["config"]["max_items"] = json!(1);
            nodes[5] = json!({"id":"consume", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"items":{"list":"int"}}, "code":{"value":"items", "check":"1 / (items[0] - 6)"}}});
            edges[5]["to_input"] = json!("items");
        }
        "nested" => {
            nodes = vec![
                json!({"id":"repeat", "kind":"workflow.loop", "loop":{"max_iterations":2, "variables":[{"name":"x", "type":"int"}], "body":{
                    "nodes":[{"id":"increment", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x + 1"}}},
                        {"id":"assign", "kind":"workflow.loop_assign", "config":{"variable":"x"}}],
                    "edges":[edge("%loop", "x", "increment", "x"), edge("increment", "value", "assign", "value")]
                }}}),
                json!({"id":"collect", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}}),
                json!({"id":"consume", "kind":"builtin.iteration", "config":{"body":{
                    "nodes":[{"id":"double", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x * 2"}}}],
                    "edges":[edge("%iteration", "item", "double", "x")], "result":{"node":"double", "port":"value"}
                }}}),
            ];
            edges = vec![
                edge("%input", "item", "repeat", "x"),
                edge("repeat", "x", "collect", "item"),
                edge("collect", "items", "consume", "items"),
            ];
            controls.clear();
            result_port = "results";
        }
        _ => unreachable!(),
    }
    serde_json::from_value(json!({"version":"2026-10-02", "execution":{"mode":"stream", "input_type":"int"},
        "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}, "code":{"package":"mfn-code", "path":common::crates_dir().join("builtin-nodes/code")}},
        "nodes":nodes, "edges":edges, "control_edges":controls, "outputs":[{"name":"value", "node":"consume", "port":result_port}]
    })).unwrap()
}

#[test]
fn generated_preparation_matches_memory_under_the_same_clock_and_inputs() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let registry = NodeRegistry::from_inventory().unwrap();
    for scenario in ["windows", "chain", "failure", "nested"] {
        let definition = definition(scenario);
        let plan = compile_definition(&definition, &registry).unwrap();
        let expected = driver::drive(instantiate_stream(&plan, &registry).unwrap(), scenario);
        match scenario {
            "windows" => assert_eq!(
                expected,
                json!({"outputs":[{"value":[3,6]}, {"value":[9,12,15]}, {"value":[18,21]}], "error":null})
            ),
            "chain" => assert_eq!(
                expected,
                json!({"outputs":[{"value":[[3,6],[9,12]]}, {"value":[[15]]}], "error":null})
            ),
            "nested" => assert_eq!(
                expected,
                json!({"outputs":[{"value":[6,8]}, {"value":[10]}], "error":null})
            ),
            _ => {
                assert_eq!(expected["outputs"], json!([{"value":[3]}]));
                assert!(expected["error"].as_str().unwrap().contains("consume"));
            }
        }
        write_dependency_project_with_options(
            &project,
            &plan,
            &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
            &RunnerOptions { telemetry: false },
        )
        .unwrap();
        fs::write(
            project.join("src/driver.rs"),
            include_str!("fixtures/stream_driver.rs"),
        )
        .unwrap();
        fs::write(
            project.join("src/main.rs"),
            r#"extern crate node_0 as _;
extern crate node_1 as _;
mod workflow;
mod driver;
fn main() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let prepared = workflow::prepare_stream(&registry).unwrap();
    println!("{}", driver::drive(prepared, &std::env::args().nth(1).unwrap()));
}
"#,
        )
        .unwrap();
        let build = mf_compiler::cargo_command(&project)
            .args(["build", "--offline", "--release"])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let actual = Command::new(common::runner_executable(&project, "release"))
            .arg(scenario)
            .output()
            .unwrap();
        assert!(
            actual.status.success(),
            "{}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&actual.stdout).unwrap(),
            expected,
            "{scenario}"
        );
    }
}
