#[test]
fn compiler_has_no_implicit_node_registrations() {
    assert_eq!(
        mf_compiler::NodeRegistry::from_inventory()
            .unwrap()
            .iter()
            .count(),
        0
    );
}
