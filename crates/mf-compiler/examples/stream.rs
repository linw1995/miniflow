extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{StreamOptions, TextInput, WorkflowArguments};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(std::env::args().nth(1), |outputs| {
        println!("{}", serde_json::to_string(&outputs)?);
        Ok(())
    })
}

pub fn run(
    path: Option<String>,
    mut publish: impl FnMut(mf_runtime::FlowOutputs) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "nodes":[{"id":"read", "kind":"builtin.readline"},{"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"read", "from_output":"line", "to_node":"copy", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"copy", "port":"value"}]
    }))?;
    let registry = NodeRegistry::from_inventory()?;
    let plan = compile_definition(&definition, &registry)?;
    let prepared = instantiate_stream(&plan, &registry)?;
    let arguments = path
        .map(|path| WorkflowArguments::try_from(json!({"read.path":path})))
        .transpose()?
        .unwrap_or_default();
    let stdin = if prepared
        .plan()
        .input_schema()
        .stdin_owner(&arguments)?
        .is_some()
    {
        Some(TextInput::claim()?)
    } else {
        None
    };
    let instance = prepared.start_with_options(StreamOptions {
        arguments,
        stdin,
        ..Default::default()
    })?;
    while let Some(output) = instance.recv()? {
        publish(output.outputs)?;
    }
    instance.join()?;
    Ok(())
}
