mod common;
#[path = "fixtures/multi-nodes/src/context_fixture.rs"]
mod fixture;
extern crate mfn_core as _;
use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled, plan_definition,
};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn graph(amount: Value) -> Value {
    let mut dependencies = serde_json::to_value(common::fixture_definition().dependencies).unwrap();
    dependencies["core"] =
        json!({"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")});
    json!({"version":"2026-09-26","dependencies":dependencies,
    "nodes":[
        {"id":"load_order","kind":"builtin.constant","config":{"value":{"amount":amount}}},
        {"id":"audit","kind":"fixture.context","config":{"ports":["done"],"outputs":{"done":false}}},
        {"id":"route","kind":"builtin.if_else","config":{"branches":[
            {"id":"large","condition":{"source":{"output":"load_order.value","path":"/amount"},"operator":"gte","value":1000}},
            {"id":"medium","condition":{"source":{"output":"load_order.value","path":"/amount"},"operator":"gte","value":100}}
        ]}},
        {"id":"large","kind":"fixture.context","config":{"ports":["value"],"read":"load_order.value"}},
        {"id":"medium","kind":"fixture.context","config":{"ports":["value"],"read":"load_order.value"}},
        {"id":"fallback","kind":"fixture.context","config":{"ports":["value"],"read":"load_order.value"}}
    ],
    "control_edges":[
        {"from_node":"load_order","from_output":"value","to_node":"audit"},
        {"from_node":"audit","from_output":"done","to_node":"route"},
        {"from_node":"route","from_output":"large","to_node":"large"},
        {"from_node":"route","from_output":"medium","to_node":"medium"},
        {"from_node":"route","from_output":"else","to_node":"fallback"}
    ],
    "outputs":[
        {"name":"large","node":"large","port":"value","optional":true},
        {"name":"medium","node":"medium","port":"value","optional":true},
        {"name":"fallback","node":"fallback","port":"value","optional":true}
    ]})
}
fn prepare(value: Value) -> Result<mf_compiler::Flow, mf_compiler::WorkflowCompileError> {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry)?;
    instantiate_compiled(&plan, &registry)
}
#[test]
fn selects_one_branch_with_short_circuiting_and_context_diagnostics() {
    for (amount, name) in [(1500, "large"), (500, "medium"), (50, "fallback")] {
        let result = prepare(graph(json!(amount))).unwrap().execute().unwrap();
        assert_eq!(json!(result), json!({name:{"amount":amount}}));
    }
    let mut value = graph(json!(1500));
    value["nodes"][2]["config"]["branches"][1]["condition"]["source"]["path"] = json!("/missing");
    assert!(prepare(value.clone()).unwrap().execute().is_ok());
    value["nodes"][0]["config"]["value"]["amount"] = json!(500);
    let error = prepare(value.clone())
        .unwrap()
        .execute()
        .unwrap_err()
        .to_string();
    for part in ["route", "medium", "load_order.value", "/missing", "Gte"] {
        assert!(error.contains(part), "{error}");
    }
    value["nodes"][0]["config"]["value"]["amount"] = json!(1500);
    value["nodes"][2]["config"]["branches"][1]["condition"]["operator"] = json!("invalid");
    assert!(prepare(value).is_err());
    assert!(prepare(graph(json!("1500"))).unwrap().execute().is_err());
}
#[test]
fn nested_instances_skip_unselected_failure_nodes() {
    let mut value = graph(json!(1500));
    value["nodes"][3]["config"]["fail"] = json!(true);
    let nested = json!({"id":"nested","kind":"builtin.if_else","config":{"branches":[
        {"id":"never","condition":{"source":{"output":"load_order.value","path":"/amount"},"operator":"lt","value":0}}
    ]}});
    value["nodes"].as_array_mut().unwrap().push(nested);
    value["control_edges"][2] =
        json!({"from_node":"route","from_output":"large","to_node":"nested"});
    value["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"nested","from_output":"never","to_node":"large"}));
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"nested","node":"nested","port":"else"}));
    assert_eq!(
        json!(prepare(value).unwrap().execute().unwrap()),
        json!({"nested":true})
    );
}
#[test]
fn compiled_branches_match_memory_and_rebuild_when_precedence_changes() {
    let root = tempfile::tempdir().unwrap();
    let trace = root.path().join("trace");
    let project = root.path().join("build");
    for (amount, reverse) in [
        (json!(1500), false),
        (json!(500), false),
        (json!(50), false),
        (json!(1500), true),
        (Value::Null, false),
    ] {
        let mut value = graph(amount.clone());
        if amount.is_null() {
            value["nodes"][0]["config"]["value"] = Value::Null;
            value["nodes"][2]["config"]["branches"][0]["condition"] = json!({
                "source":{"output":"load_order.value","path":""}, "operator":"eq", "value":null
            });
        }
        value["nodes"][1]["config"]["ports"] = json!(["done", "unused"]);
        value["nodes"][1]["config"]["skipped"] = json!(["unused"]);
        value["nodes"].as_array_mut().unwrap().push(json!({
            "id":"inactive", "kind":"fixture.context", "config":{"ports":[], "fail":true}
        }));
        value["control_edges"].as_array_mut().unwrap().push(json!({
            "from_node":"audit", "from_output":"unused", "to_node":"inactive"
        }));
        if reverse {
            value["nodes"][2]["config"]["branches"]
                .as_array_mut()
                .unwrap()
                .reverse();
        }
        for node in value["nodes"].as_array_mut().unwrap() {
            if node["kind"] == "fixture.context" {
                node["config"]["trace"] = json!(trace);
                node["config"]["name"] = node["id"].clone();
            }
        }
        let _ = fs::remove_file(&trace);
        let expected = prepare(value.clone()).unwrap().execute().unwrap();
        let expected_trace = fs::read_to_string(&trace).unwrap();
        let selected = if reverse {
            "medium"
        } else if amount.is_null() || amount.as_i64().unwrap() >= 1000 {
            "large"
        } else if amount.as_i64().unwrap() >= 100 {
            "medium"
        } else {
            "fallback"
        };
        assert_eq!(expected_trace, format!("audit\n{selected}\n"));
        if amount.is_null() {
            assert_eq!(json!(expected), json!({"large":null}));
        }
        fs::remove_file(&trace).unwrap();
        let definition = serde_json::from_value(value).unwrap();
        let plan = plan_definition(&definition).unwrap();
        mf_compiler::write_dependency_project(
            &project,
            &plan,
            &mf_compiler::SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        )
        .unwrap();
        mf_compiler::resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let build = mf_compiler::pipeline::cargo_command(&project)
            .args(["build", "--offline", "--locked"])
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let executable = project.join("target/debug/mf-generated-workflow");
        let validation = Command::new(&executable)
            .arg("--validate")
            .output()
            .unwrap();
        assert!(
            validation.status.success(),
            "{}",
            String::from_utf8_lossy(&validation.stderr)
        );
        assert!(!trace.exists());
        let result = Command::new(executable).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!(expected)
        );
        assert_eq!(fs::read_to_string(&trace).unwrap(), expected_trace);
    }
}

#[test]
fn documented_examples_produce_the_documented_results() {
    for (source, expected) in [
        (
            include_str!("../../../examples/if-else.json"),
            json!({"accepted":{"amount":150}}),
        ),
        (
            include_str!("../../../examples/else-if.json"),
            json!({"medium":{"amount":500}}),
        ),
    ] {
        assert_eq!(
            json!(
                prepare(serde_json::from_str(source).unwrap())
                    .unwrap()
                    .execute()
                    .unwrap()
            ),
            expected
        );
    }
}

#[test]
fn presence_checks_distinguish_skipped_outputs_and_plugin_omissions() {
    let mut value = graph(json!(1500));
    value["nodes"][1]["config"]["ports"] = json!(["done", "absent"]);
    value["nodes"][1]["config"]["skipped"] = json!(["absent"]);
    value["nodes"][2]["config"]["branches"][0]["condition"] = json!({
        "source":{"output":"audit.absent","path":""},"operator":"not_exists"
    });
    assert!(
        prepare(value.clone())
            .unwrap()
            .execute()
            .unwrap()
            .contains_key("large")
    );
    value["nodes"][2]["config"]["branches"][0]["condition"]["operator"] = json!("exists");
    assert!(
        prepare(value.clone())
            .unwrap()
            .execute()
            .unwrap()
            .contains_key("medium")
    );
    value["nodes"][1]["config"]["skipped"] = json!([]);
    let error = prepare(value).unwrap().execute().unwrap_err().to_string();
    assert!(error.contains("audit.absent") && error.contains("missing"));
}

#[test]
fn selected_branch_fanout_runs_all_consumers_and_keeps_aliases() {
    let mut value = graph(json!(1500));
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"copy","kind":"builtin.identity"}));
    value["edges"] = json!([{"from_node":"load_order","from_output":"value","to_node":"copy","to_input":"input"}]);
    value["control_edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"route","from_output":"large","to_node":"copy"}));
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"other","node":"copy","port":"value"}));
    assert_eq!(
        json!(prepare(value).unwrap().execute().unwrap()),
        json!({"large":{"amount":1500},"other":{"amount":1500}})
    );
}
