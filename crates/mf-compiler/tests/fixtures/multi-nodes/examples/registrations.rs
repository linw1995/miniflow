extern crate fixture_multi_nodes as _;

fn main() {
    let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
    let kinds: Vec<_> = registry.iter().map(|entry| entry.kind).collect();
    assert_eq!(kinds, ["fixture.echo", "fixture.source"]);
}
