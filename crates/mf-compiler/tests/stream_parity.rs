mod common;
#[path = "fixtures/controlled_source.rs"]
mod controlled;
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

fn definition(scenario: &str) -> WorkflowDefinition {
    let mut nodes = vec![
        json!({"id":"left", "kind":"builtin.code", "inputs":{"x":{"$ref":"feed.item"}}, "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x * 2"}}}),
        json!({"id":"right", "kind":"builtin.identity", "inputs":{"input":{"$ref":"feed.item"}}}),
        json!({"id":"join", "kind":"builtin.code", "inputs":{"a":{"$ref":"left.value"}, "b":{"$ref":"right.value"}}, "config":{"language":"cel", "inputs":{"a":"int", "b":"int"}, "code":{"value":"a + b"}}}),
        json!({"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"positive", "condition":{"source":{"output":"join.value", "path":""}, "operator":"gt", "value":0}}]}}),
        json!({"id":"collect", "kind":"builtin.batch", "inputs":{"item":{"$ref":"join.value"}}, "config":{"max_items":3, "max_wait_ms":100}}),
        json!({"id":"consume", "kind":"builtin.identity", "inputs":{"input":{"$ref":"collect.items"}}}),
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
            nodes[5]["inputs"]["item"] = json!({"$ref":"collect.items"});
            result_port = "items";
        }
        "failure" => {
            nodes[4]["config"]["max_items"] = json!(1);
            nodes[5] = json!({"id":"consume", "kind":"builtin.code", "config":{"language":"cel", "inputs":{"items":{"list":"int"}}, "code":{"value":"items", "check":"1 / (items[0] - 6)"}}});
            nodes[5]["inputs"]["items"] = json!({"$ref":"collect.items"});
        }
        "nested" => {
            nodes = vec![
                json!({"id":"repeat", "kind":"loop.run", "inputs":{"x":{"$ref":"feed.item"}}, "config":{"max_iterations":2, "variables":[{"name":"x", "type":"int"}], "body":{"version":"2026-10-03", "dependencies":{},
                    "nodes":[{"id":"increment", "kind":"builtin.code", "inputs":{"x":{"$ref":"%loop.x"}}, "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x + 1"}}},
                        {"id":"assign", "kind":"workflow.assign", "inputs":{"value":{"$ref":"increment.value"}}, "config":{"variable":"x"}}]
                }}}),
                json!({"id":"collect", "kind":"builtin.batch", "inputs":{"item":{"$ref":"repeat.x"}}, "config":{"max_items":2, "max_wait_ms":100}}),
                json!({"id":"consume", "kind":"builtin.iteration", "inputs":{"items":{"$ref":"collect.items"}}, "config":{"body":{"version":"2026-10-03", "dependencies":{},
                    "nodes":[{"id":"double", "kind":"builtin.code", "inputs":{"x":{"$ref":"%iteration.item"}}, "config":{"language":"cel", "inputs":{"x":"int"}, "code":{"value":"x * 2"}}}],
                    "outputs": [{"name":"result", "node":"double", "port":"value"}]
                }}}),
            ];
            controls.clear();
            result_port = "results";
        }
        _ => unreachable!(),
    }
    if scenario == "nested" {
        add_body_join(
            &mut nodes[0]["config"]["body"],
            ("%loop", "x"),
            ("increment", "value"),
            ("assign", "value"),
        );
        nodes[2]["config"]["body"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"join", "kind":"builtin.identity"}));
        add_body_join(
            &mut nodes[2]["config"]["body"],
            ("%iteration", "item"),
            ("double", "value"),
            ("join", "input"),
        );
        nodes[2]["config"]["body"]["outputs"][0] =
            json!({"name":"result", "node":"join", "port":"value"});
    }
    if scenario == "windows" {
        nodes.push(json!({"id":"alternative", "kind":"builtin.identity", "inputs":{"input":{"$ref":"join.value"}}}));
        controls.retain(|edge| edge["to_node"] != "collect");
        nodes[3]["config"]["branches"].as_array_mut().unwrap().insert(0, json!({"id":"large", "condition":{"source":{"output":"join.value", "path":""}, "operator":"gt", "value":10}}));
        controls.push(json!({"from_node":"route", "from_output":"large", "to_node":"alternative"}));
        nodes[4]["inputs"]["item"] =
            json!({"exclusive":[{"$ref":"primary.value"}, {"$ref":"alternative.value"}]});
        // Negative messages skip both candidates; larger values switch to the alternative.
        nodes.push(json!({"id":"primary", "kind":"builtin.identity", "inputs":{"input":{"$ref":"join.value"}}}));
        controls.push(json!({"from_node":"route", "from_output":"positive", "to_node":"primary"}));
    }
    nodes.push(json!({"id":"feed", "kind":"test.controlled_source", "config":{"item_type":"int"}}));
    serde_json::from_value(json!({"version":"2026-10-03", "execution":{},
        "dependencies":{"core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")}, "code":{"package":"mfn-code", "path":common::crates_dir().join("builtin-nodes/code")}},
        "nodes":nodes, "edges":controls, "outputs":[{"name":"value", "node":"consume", "port":result_port}]
    })).unwrap()
}

fn add_body_join(
    body: &mut Value,
    source: (&str, &str),
    winner: (&str, &str),
    target: (&str, &str),
) {
    let input = json!({"$ref":format!("{}.{}", winner.0, winner.1)});
    let nodes = body["nodes"].as_array_mut().unwrap();
    nodes
        .iter_mut()
        .find(|node| node["id"] == target.0)
        .unwrap()["inputs"][target.1] =
        json!({"exclusive":[{"$ref":"selected.value"}, {"$ref":"unused.value"}]});
    nodes.extend([
        json!({"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"present", "condition":{"source":{"output":format!("{}.{}", source.0, source.1), "path":""}, "operator":"exists"}}]}}),
        json!({"id":"unused", "kind":"builtin.identity", "inputs":{"input":input}}),
        json!({"id":"selected", "kind":"builtin.identity", "inputs":{"input":input}})
    ]);
    body["edges"] = json!([
        {"from_node":source.0, "from_output":source.1, "to_node":"route"},
        {"from_node":"route", "from_output":"else", "to_node":"unused"},
        {"from_node":"route", "from_output":"present", "to_node":"selected"}
    ]);
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
        let workflow_source = fs::read_to_string(project.join("src/workflow.rs")).unwrap();
        assert!(!workflow_source.contains("STREAM_DEPENDENCIES"));
        assert!(!workflow_source.contains("STREAM_OUTPUTS"));
        fs::write(
            project.join("src/controlled.rs"),
            include_str!("fixtures/controlled_source.rs"),
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
mod controlled;
mod driver;
fn main() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let prepared = workflow::prepare_stream(&registry).unwrap();
    println!("{}", driver::drive(prepared, &std::env::args().nth(1).unwrap()));
}
"#,
        )
        .unwrap();
        let build_script = project.join("build.rs");
        let build_source = fs::read_to_string(&build_script).unwrap();
        fs::write(
            &build_script,
            format!("#[path = \"src/controlled.rs\"] mod controlled;\n{build_source}"),
        )
        .unwrap();
        let manifest = project.join("Cargo.toml");
        let mut text = fs::read_to_string(&manifest).unwrap();
        text.push('\n');
        text = text.replace("[dependencies]", "[dependencies]\ninventory = \"0.3.24\"");
        text = text.replace(
            "[build-dependencies]",
            "[build-dependencies]\ninventory = \"0.3.24\"",
        );
        fs::write(&manifest, text).unwrap();
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
