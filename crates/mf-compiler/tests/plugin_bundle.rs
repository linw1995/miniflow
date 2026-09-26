#[test]
fn caller_selects_the_plugin_registry() {
    let registry = mf_bundle::registry().unwrap();
    let kinds: Vec<_> = registry.iter().map(|entry| entry.kind).collect();
    assert_eq!(kinds, mf_bundle::registered_kinds());
}
