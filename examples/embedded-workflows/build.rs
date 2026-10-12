use core_nodes as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let answer =
        mf_compiler::EmbedRequest::new("answer", "answer.json").with_provider("core", "core_nodes");
    mf_compiler::embed_workflows(&[answer], &registry)?;
    Ok(())
}
