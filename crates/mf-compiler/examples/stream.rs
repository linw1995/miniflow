extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "dependencies":{},
        "execution":{"mode":"stream"},
        "nodes":[{"id":"feed", "kind":"builtin.channel", "config":{"item_type":"int"}},{"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"feed", "from_output":"item", "to_node":"copy", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"copy", "port":"value"}]
    }))?;
    let registry = NodeRegistry::from_inventory()?;
    let plan = compile_definition(&definition, &registry)?;
    let mut prepared = instantiate_stream(&plan, &registry)?;
    let input = prepared.channel("feed")?;
    let instance = prepared.start()?;
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
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
