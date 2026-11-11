//! Regenerate the ordinary calculator chat Flow from the provider-owned Agent.
extern crate mfn_code as _;
extern crate mfn_core as _;
use mf_compiler::WorkflowDefinition;
use mfn_llm::{
    flows::{FlowTool, agent_definition},
    tools::ParameterDescription,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error};

pub fn calculator_definition() -> Result<WorkflowDefinition, Box<dyn Error>> {
    let body = WorkflowDefinition::from_json(include_str!(
        "../../../examples/agent-calculator/calculator.json"
    ))?;
    let tool = FlowTool {
        description:"Perform add, subtract, multiply, or divide on two numeric operands. Division by zero returns an error message.".into(),
        parameters:BTreeMap::from([
            ("calculate.operation".into(),ParameterDescription{description:"Operation to perform: add, subtract, multiply, or divide.".into()}),
            ("calculate.left".into(),ParameterDescription{description:"Left operand.".into()}),
            ("calculate.right".into(),ParameterDescription{description:"Right operand; must be nonzero for division.".into()}),
        ]), body,
    };
    let mut value = serde_json::to_value(agent_definition(BTreeMap::from([(
        "calculator".into(),
        tool,
    )]))?)?;
    value["dependencies"] = json!({
        "core":{"package":"mfn-core","path":"../crates/builtin-nodes/core"},
        "code":{"package":"mfn-code","path":"../crates/builtin-nodes/code"},
        "llm":{"package":"mfn-llm","path":"../crates/builtin-nodes/llm"}
    });
    let producer = value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "tool_0")
        .unwrap();
    producer["config"]["value"] = json!({"path":"agent-calculator/calculator.json"});
    Ok(WorkflowDefinition::from_json(&value.to_string())?)
}

#[cfg(not(test))]
fn main() -> Result<(), Box<dyn Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    std::fs::write(
        root.join("examples/chat-calculator-flow.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&calculator_definition()?)?
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn calculator_example_is_generated_from_the_provider_agent_without_special_nodes() {
        let source: Value =
            serde_json::from_str(include_str!("../../../examples/chat-calculator-flow.json"))
                .unwrap();
        assert_eq!(
            serde_json::to_value(calculator_definition().unwrap()).unwrap(),
            source
        );
        let source = source.to_string();
        for kind in [
            "llm.tool\"",
            "llm.tool_set",
            "llm.tool_calls",
            "llm.tool_call\"",
        ] {
            assert!(!source.contains(kind));
        }
        assert!(source.contains("workflow.loop"));
        assert!(source.contains("mfn-llm.agent-turn"));
    }
}
