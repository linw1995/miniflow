mf_compiler::embed_workflow!(pub mod answer);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let flow = answer::prepare_workflow(&registry, None)?;
    let outputs = mf_runtime::FlowRuntime::default().execute(&flow)?;
    println!("{}", serde_json::to_string(&outputs)?);
    Ok(())
}
