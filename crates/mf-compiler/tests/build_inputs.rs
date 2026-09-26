use mf_compiler::{BuildInputs, WorkflowDefinition};
use std::{fs, path::PathBuf};

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temporary() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mf-inputs-{}-{}-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    dir
}

#[test]
fn resolves_paths_from_canonical_definition_and_names_locks_per_flow() {
    let root = temporary();
    fs::create_dir(root.join("nodes")).unwrap();
    let definition = WorkflowDefinition::from_json(r#"{"version":"2026-09-26","dependencies":{"local":{"package":"local","path":"nodes"}},"nodes":[]}"#).unwrap();
    for name in [
        "order.json",
        "refund.json",
        "order.flow.json",
        "extensionless",
    ] {
        fs::write(root.join(name), "{}").unwrap();
        let inputs = BuildInputs::new(&root.join(name)).unwrap();
        assert_eq!(
            inputs.lock,
            fs::canonicalize(&root)
                .unwrap()
                .join(name)
                .with_extension("lock")
        );
        assert_eq!(
            inputs
                .resolve_dependencies(&definition)
                .unwrap()
                .dependencies["local"]
                .path,
            Some(fs::canonicalize(root.join("nodes")).unwrap())
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("order.json"), root.join("alias.json")).unwrap();
        assert_eq!(
            BuildInputs::new(&root.join("alias.json")).unwrap().lock,
            BuildInputs::new(&root.join("order.json")).unwrap().lock
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_input_and_lock_aliases_without_mutation() {
    let root = temporary();
    let source = root.join("flow.json");
    fs::write(&source, "original definition").unwrap();
    let inputs = BuildInputs::new(&source).unwrap();
    assert!(inputs.check_output(&source).is_err());
    assert!(inputs.check_output(&root.join("flow.lock")).is_err());
    fs::write(root.join("flow.lock"), "original lock").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("flow.lock"), root.join("alias")).unwrap();
        assert!(inputs.check_output(&root.join("alias")).is_err());
    }
    assert!(BuildInputs::new(&root.join("flow.lock")).is_err());
    assert_eq!(fs::read_to_string(&source).unwrap(), "original definition");
    assert_eq!(
        fs::read_to_string(root.join("flow.lock")).unwrap(),
        "original lock"
    );
    fs::remove_dir_all(root).unwrap();
}
