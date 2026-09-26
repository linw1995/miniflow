use mf_runtime::{NodeRegistry, NodeRegistryError};

pub fn registered_kinds() -> Vec<&'static str> {
    vec![mfn_constant::kind(), mfn_identity::kind()]
}

pub fn registry() -> Result<NodeRegistry, NodeRegistryError> {
    let registry = NodeRegistry::from_inventory()?;
    let kinds = registered_kinds();
    for kind in &kinds {
        if registry.get(kind).is_none() {
            return Err(NodeRegistryError::MissingKind {
                kind: (*kind).to_owned(),
            });
        }
    }
    for registration in registry.iter() {
        if !kinds.contains(&registration.kind) {
            return Err(NodeRegistryError::UnexpectedKind {
                kind: registration.kind.to_owned(),
            });
        }
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_target_links_bundle_registrations() {
        let kinds: Vec<_> = registry().unwrap().iter().map(|entry| entry.kind).collect();
        assert_eq!(kinds, registered_kinds());
    }
}
