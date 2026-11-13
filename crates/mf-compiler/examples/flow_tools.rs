//! A caller supplies native tools to fixed, reusable catalog and dispatch Flows.
extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::WorkflowArguments;
use serde_json::json;
use std::{error::Error, path::Path};

fn definition() -> Result<WorkflowDefinition, Box<dyn Error>> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    Ok(WorkflowDefinition::from_json(&json!({
        "version":"2026-10-28",
        "dependencies":{
            "core":{"package":"mfn-core","path":crates.join("builtin-nodes/core")},
            "code":{"package":"mfn-code","path":crates.join("builtin-nodes/code")},
            "llm":{"package":"mfn-llm","path":crates.join("builtin-nodes/llm")}
        },
        "flows":{"catalog":{"builtin":"mfn-llm.tool-catalog"},"dispatch":{"builtin":"mfn-llm.tool-uses"}},
        "nodes":[
            {"id":"params","kind":"builtin.parameters","config":{"parameters":{
                "calls":{"type":{"list":"object"},"secret":false},
                "add_description":{"type":"string","secret":false,"default":"Add two integers."},
                "add_parameters":{"type":"object","secret":false,"default":{
                    "left":{"description":"Left operand."},"right":{"description":"Right operand."}
                }}
            }}},
            {"id":"add","kind":"builtin.constant","config":{"type":"flow","value":{
                "version":"2026-10-28","nodes":[{"id":"calculate","kind":"builtin.code","config":{
                    "language":"cel","inputs":{"left":"int","right":"int"},"code":{"sum":"left + right"}
                }}],"outputs":[{"name":"sum","node":"calculate","port":"sum"}]
            }}},
            {"id":"add_properties","kind":"builtin.object","config":{"fields":{
                "description":"string","parameters":"object","interface":"object"
            }},"inputs":{
                "description":{"from":{"node":"params","port":"add_description"}},
                "parameters":{"from":{"node":"params","port":"add_parameters"}},
                "interface":{"from":{"node":"add","port":"properties"}}
            }},
            {"id":"tools","kind":"builtin.object","config":{"fields":{"add":"flow"}},"inputs":{
                "add":{"from":{"node":"add","port":"value"}}
            }},
            {"id":"tool_properties","kind":"builtin.object","config":{"fields":{"add":"object"}},"inputs":{
                "add":{"from":{"node":"add_properties","port":"value"}}
            }},
            {"id":"catalog","kind":"workflow.call","config":{"flow":"catalog"},"inputs":{
                "params.properties":{"from":{"node":"tool_properties","port":"value"}}
            }},
            {"id":"dispatch","kind":"workflow.call","config":{"flow":"dispatch"},"inputs":{
                "params.calls":{"from":{"node":"params","port":"calls"}},
                "params.tools":{"from":{"node":"tools","port":"value"}},
                "params.tool_properties":{"from":{"node":"tool_properties","port":"value"}}
            }}
        ],
        "edges":[{"from_node":"catalog","from_output":"tools","to_node":"dispatch"}],
        "outputs":[{"name":"catalog","node":"catalog","port":"tools"},{"name":"results","node":"dispatch","port":"results"}]
    }).to_string())?)
}

#[cfg(not(test))]
fn main() -> Result<(), Box<dyn Error>> {
    let definition = definition()?;
    if let Some(path) = std::env::args_os().nth(1) {
        std::fs::write(path, serde_json::to_vec_pretty(&definition)?)?;
    } else {
        let registry = NodeRegistry::from_inventory()?;
        let plan = compile_definition(&definition, &registry)?;
        let output = instantiate_compiled(&plan, &registry)?.execute_with_inputs(WorkflowArguments::try_from(json!({
            "params.calls":[{"id":"one","name":"add","arguments":{"left":2,"right":3}},{"id":"unknown","name":"missing","arguments":{}}]
        }))?)?;
        println!("{}", serde_json::to_string(&output)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caller_supplies_a_native_map_without_rewriting_the_catalog() {
        let definition = definition().unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let result = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(
                WorkflowArguments::try_from(json!({
                    "params.calls":[{"id":"one","name":"add","arguments":{"left":2,"right":3}}]
                }))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            result["results"][0]["content"].as_str(),
            Some("{\"sum\":5}")
        );
        assert!(
            definition
                .nodes
                .iter()
                .any(|node| node.kind == "builtin.object")
        );
        assert!(
            definition
                .nodes
                .iter()
                .filter(|node| node.kind == "workflow.call")
                .all(|node| node.config.get("body").is_none())
        );
    }
}
