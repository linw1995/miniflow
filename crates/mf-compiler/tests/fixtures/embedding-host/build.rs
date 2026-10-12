use code_nodes as _;
use core_nodes as _;
use fixture_provider as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let registry = runtime::NodeRegistry::from_inventory()?;
    compiler::embed_workflows(
        &[
            compiler::EmbedRequest::new("answer", "answer.json")
                .with_provider("core", "core_nodes"),
            compiler::EmbedRequest::new("events", "events.json")
                .with_provider("core", "core_nodes"),
            compiler::EmbedRequest::new("contract", "contract.json")
                .with_provider("fixture", "fixture_provider"),
            compiler::EmbedRequest::new("contract_stream", "contract-stream.json")
                .with_provider("fixture", "fixture_provider"),
            compiler::EmbedRequest::new("typed", "typed.json")
                .with_provider("fixture", "fixture_provider"),
            compiler::EmbedRequest::new("counter", "counter.json")
                .with_provider("fixture", "fixture_provider"),
            compiler::EmbedRequest::new("repeat", "repeat.json")
                .with_provider("core", "core_nodes")
                .with_provider("code", "code_nodes"),
            compiler::EmbedRequest::new("iteration", "iteration.json")
                .with_provider("core", "core_nodes")
                .with_provider("code", "code_nodes"),
        ],
        &registry,
    )?;
    Ok(())
}
