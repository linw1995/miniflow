#[test]
fn compiler_target_links_the_shared_plugin_bundle() {
    let kinds: Vec<_> = mf_compiler::plugin_registry()
        .unwrap()
        .iter()
        .map(|entry| entry.kind)
        .collect();
    assert_eq!(kinds, mf_bundle::registered_kinds());
}
