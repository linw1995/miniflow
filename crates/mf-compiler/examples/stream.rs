extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{ExecutionResources, StreamOptions, TextInput, WorkflowArguments};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[{"id":"read", "kind":"builtin.readline"},{"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"read", "from_output":"line", "to_node":"copy", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"copy", "port":"value"}]
    }))?;
    let registry = NodeRegistry::from_inventory()?;
    let plan = compile_definition(&definition, &registry)?;
    let prepared = instantiate_stream(&plan, &registry)?;
    let arguments = std::env::args()
        .nth(1)
        .map(|path| WorkflowArguments::try_from(json!({"read":{"path":path}})))
        .transpose()?
        .unwrap_or_default();
    let resources = if prepared
        .plan()
        .input_schema()
        .stdin_owner(&arguments)?
        .is_some()
    {
        ExecutionResources::default().with_stdin(TextInput::claim()?)
    } else {
        ExecutionResources::default()
    };
    let instance = prepared.start_with_options(StreamOptions {
        arguments,
        resources,
        ..Default::default()
    })?;
    while let Some(output) = instance.recv()? {
        println!("{}", serde_json::to_string(&output.outputs)?);
    }
    instance.join()?;
    Ok(())
}
