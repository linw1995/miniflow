#![allow(dead_code)]
//! Caller-owned native tool values for integration tests, not production graph generators.

use mf_compiler::WorkflowDefinition;
use mfn_llm::tools::ParameterDescription;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct FlowTool {
    pub description: String,
    pub parameters: BTreeMap<String, ParameterDescription>,
    pub body: WorkflowDefinition,
}

fn edge(node: &str, port: &str) -> Value {
    json!({"from":{"node":node,"port":port}})
}

fn local_dependencies() -> Value {
    json!({
        "core":{"package":"mfn-core", "path":crate::common::crates_dir().join("builtin-nodes/core")},
        "code":{"package":"mfn-code", "path":crate::common::crates_dir().join("builtin-nodes/code")},
        "llm":{"package":"mfn-llm", "path":crate::common::crates_dir().join("builtin-nodes/llm")}
    })
}

// Tool bodies may themselves contain inline Flows. Keep every source declaration
// consistent with the caller's local packages, including fixture example bodies.
fn select_local_sources(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if fields.contains_key("version") && fields.contains_key("nodes") {
                fields.insert("dependencies".into(), local_dependencies());
            }
            for value in fields.values_mut() {
                select_local_sources(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                select_local_sources(value);
            }
        }
        _ => {}
    }
}

fn caller(tools: BTreeMap<String, FlowTool>) -> Value {
    let mut nodes = Vec::new();
    let mut fields = BTreeMap::new();
    let mut inputs = BTreeMap::new();
    let mut descriptions = BTreeMap::new();
    for (name, tool) in tools {
        let producer = format!("tool_{name}");
        let mut body = serde_json::to_value(tool.body).unwrap();
        select_local_sources(&mut body);
        nodes.push(json!({
            "id":producer,"kind":"builtin.constant", "config":{"type":"flow","value":body}
        }));
        fields.insert(name.clone(), "flow");
        inputs.insert(name.clone(), edge(&producer, "value"));
        descriptions.insert(
            name,
            json!({
                "description":tool.description,"parameters":tool.parameters
            }),
        );
    }
    nodes.extend([
        // This node constructs a native map; Flow values never enter JSON/CEL.
        json!({"id":"tools","kind":"builtin.object","config":{"fields":fields},"inputs":inputs}),
        json!({"id":"descriptions","kind":"builtin.constant","config":{"value":descriptions}}),
    ]);
    json!({
        "version":"2026-10-28", "dependencies":local_dependencies(),
        "nodes":nodes,"outputs":[]
    })
}

pub fn configured_tools(tools: BTreeMap<String, FlowTool>) -> WorkflowDefinition {
    let mut value = caller(tools);
    value["flows"] = json!({
        "tool-catalog":{"builtin":"mfn-llm.tool-catalog"},
        "tool-uses":{"builtin":"mfn-llm.tool-uses"}
    });
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"params","kind":"builtin.parameters","config":{"parameters":{
            "calls":{"type":{"list":"object"},"secret":false}
        }}}),
        json!({"id":"catalog","kind":"workflow.call","config":{"flow":"tool-catalog"},"inputs":{
            "params.tools":edge("tools","value"),
            "params.descriptions":edge("descriptions","value")
        }}),
        json!({"id":"dispatch","kind":"workflow.call","config":{"flow":"tool-uses"},"inputs":{
            "params.calls":edge("params","calls"),
            "params.flows":edge("tools","value")
        }}),
    ]);
    // ToolUses reads the same native Flows directly. Its execution still waits
    // for the catalog's real projection/metadata validation to succeed.
    value["edges"] = json!([
        {"from_node":"catalog","from_output":"tools","to_node":"dispatch"}
    ]);
    value["outputs"] = json!([
        {"name":"catalog","node":"catalog","port":"tools"},
        {"name":"results","node":"dispatch","port":"results"}
    ]);
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}

pub fn configured_agent(tools: BTreeMap<String, FlowTool>) -> WorkflowDefinition {
    let mut value = caller(tools);
    value["flows"] = json!({"agent":{"builtin":"mfn-llm.agent"}});
    let parameters = json!({
        "prompt":{"type":"string"},
        "system":{"type":"string","default":""},
        "history":{"type":{"list":"object"},"default":[]},
        "model":{"type":"string","default":"your-model","secret":false},
        "base_url":{"type":"string","default":"https://api.openai.com/v1","secret":false},
        "api_key":{"type":"string","env":"OPENAI_API_KEY"}
    });
    let mut inputs = serde_json::Map::new();
    for name in [
        "prompt", "system", "history", "model", "base_url", "api_key",
    ] {
        inputs.insert(format!("params.{name}"), edge("params", name));
    }
    inputs.insert("params.tools".into(), edge("tools", "value"));
    inputs.insert("params.descriptions".into(), edge("descriptions", "value"));
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"params","kind":"builtin.parameters","config":{"parameters":parameters}}),
        json!({"id":"agent","kind":"workflow.call","config":{"flow":"agent"},"inputs":inputs}),
    ]);
    value["outputs"] = json!([
        {"name":"answer","node":"agent","port":"answer","optional":true},
        {"name":"status","node":"agent","port":"status"},
        {"name":"text","node":"agent","port":"text"},
        {"name":"usages","node":"agent","port":"usages"},
        {"name":"error","node":"agent","port":"error","optional":true}
    ]);
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}
