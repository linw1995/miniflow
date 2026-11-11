//! Build a configured tool-use Flow, then execute it or save it for standalone compilation.
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::WorkflowArguments;
use mfn_llm::{
    flows::{FlowTool, tool_uses_definition},
    tools::ParameterDescription,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, path::Path};

#[cfg(not(test))]
fn main() -> Result<(), Box<dyn Error>> {
    run(std::env::args_os().nth(1))
}

fn run(output: Option<std::ffi::OsString>) -> Result<(), Box<dyn Error>> {
    let body = WorkflowDefinition::from_json(
        &json!({
            "version":"2026-10-28",
            "nodes":[{"id":"calculate", "kind":"builtin.code", "config":{
                "language":"cel", "inputs":{"left":"int", "right":"int"},
                "code":{"sum":"left + right"}
            }}],
            "outputs":[{"name":"sum", "node":"calculate", "port":"sum"}]
        })
        .to_string(),
    )?;
    let tools = BTreeMap::from([(
        "add".into(),
        FlowTool {
            description: "Add two integers.".into(),
            parameters: BTreeMap::from([
                (
                    "calculate.left".into(),
                    ParameterDescription {
                        description: "Left operand.".into(),
                    },
                ),
                (
                    "calculate.right".into(),
                    ParameterDescription {
                        description: "Right operand.".into(),
                    },
                ),
            ]),
            body,
        },
    )]);
    let mut definition = tool_uses_definition(tools)?;
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    definition.dependencies = serde_json::from_value(json!({
        "core":{"package":"mfn-core", "path":crates.join("builtin-nodes/core")},
        "code":{"package":"mfn-code", "path":crates.join("builtin-nodes/code")},
        "llm":{"package":"mfn-llm", "path":crates.join("builtin-nodes/llm")}
    }))?;
    if let Some(path) = output {
        std::fs::write(path, serde_json::to_vec_pretty(&definition)?)?;
    } else {
        let registry = NodeRegistry::from_inventory()?;
        let plan = compile_definition(&definition, &registry)?;
        let outputs = instantiate_compiled(&plan, &registry)?.execute_with_inputs(
            WorkflowArguments::try_from(json!({"params.calls":[
                {"id":"first", "name":"add", "arguments":{"left":2,"right":3}},
                {"id":"second", "name":"unknown", "arguments":{}}
            ]}))?,
        )?;
        println!("{}", serde_json::to_string(&outputs)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_example_executes_and_exports_an_ordinary_workflow() {
        run(None).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tools.json");
        run(Some(path.clone().into_os_string())).unwrap();
        let definition =
            WorkflowDefinition::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            definition
                .nodes
                .iter()
                .filter(|node| node.kind == mf_runtime::ITERATION_KIND)
                .count(),
            1
        );
        assert!(
            definition
                .nodes
                .iter()
                .any(|node| node.kind == "llm.tool_definition")
        );
        assert_eq!(
            definition
                .outputs
                .iter()
                .map(|output| output.name.as_ref())
                .collect::<Vec<_>>(),
            ["catalog", "results"]
        );
    }
}
