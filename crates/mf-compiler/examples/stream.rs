extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-02", "dependencies":{},
        "execution":{"mode":"stream", "input_type":"int"},
        "nodes":[{"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"%input", "from_output":"item", "to_node":"copy", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"copy", "port":"value"}]
    }))?;
    let registry = NodeRegistry::from_inventory()?;
    let plan = compile_definition(&definition, &registry)?;
    let instance = instantiate_stream(&plan, &registry)?.start()?;
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let input = instance.input();
        let producer = scope.spawn(move || -> Result<(), mf_runtime::StreamError> {
            for value in 0..10 {
                input.send(json!(value))?;
            }
            input.close();
            Ok(())
        });
        while let Some(output) = instance.recv()? {
            println!("{}", serde_json::to_string(&output.outputs)?);
        }
        producer.join().expect("producer thread panicked")?;
        Ok(())
    })?;
    instance.join()?;
    Ok(())
}
