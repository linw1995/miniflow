use mf_compiler::{BuildDirectory, CacheError, default_build_directory};
use std::{error::Error, fs};

#[test]
fn invalid_ownership_metadata_preserves_json_error_sources() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let directory = root.path().join("build");
    drop(BuildDirectory::open(&definition, Some(&directory)).unwrap());
    let marker = fs::canonicalize(&directory).unwrap().join(".mf-owner.json");
    fs::write(&marker, "{ invalid }").unwrap();

    let error = BuildDirectory::open(&definition, Some(&directory))
        .err()
        .unwrap();
    assert!(error.source().unwrap().is::<serde_json::Error>());
    assert!(matches!(&error, CacheError::MetadataParse { path, .. } if path == &marker));
}

#[test]
fn reuses_owned_directories_and_rejects_foreign_or_incompatible_entries() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let directory = root.path().join("build");
    let first = BuildDirectory::open(&definition, Some(&directory)).unwrap();
    assert!(!first.reused);
    let key = default_build_directory(root.path(), &definition);
    assert_eq!(key, default_build_directory(root.path(), &definition));
    assert_ne!(
        key,
        default_build_directory(root.path(), &root.path().join("other.json"))
    );
    drop(first);
    assert!(
        BuildDirectory::open(&definition, Some(&directory))
            .unwrap()
            .reused
    );
    assert!(BuildDirectory::open(&root.path().join("other.json"), Some(&directory)).is_err());
    let marker = directory.join(".mf-owner.json");
    let mut owner: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    assert_eq!(owner["layout_version"], "2026-10-07");
    owner["layout_version"] = serde_json::json!("2026-09-26");
    fs::write(&marker, owner.to_string()).unwrap();
    assert!(matches!(
        BuildDirectory::open(&definition, Some(&directory)),
        Err(CacheError::Ownership { .. })
    ));
    owner["layout_version"] = serde_json::json!("2099-01-01");
    fs::write(&marker, owner.to_string()).unwrap();
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_err());
    fs::remove_dir_all(&directory).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("unrelated"), "keep").unwrap();
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_err());
    assert_eq!(fs::read(directory.join("unrelated")).unwrap(), b"keep");
}

#[test]
fn serializes_reuse_and_rejects_overlapping_inputs() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("flow.json");
    let directory = root.path().join("build");
    let guard = BuildDirectory::open(&definition, Some(&directory)).unwrap();
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_err());
    let other = BuildDirectory::open(
        &root.path().join("other.json"),
        Some(&root.path().join("other-build")),
    )
    .unwrap();
    drop(other);
    drop(guard);
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_ok());
    let protected = fs::canonicalize(&directory).unwrap().join("output");
    assert!(BuildDirectory::open_protected(&definition, Some(&directory), &[&protected]).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&directory, root.path().join("alias")).unwrap();
        assert!(
            BuildDirectory::open_protected(
                &definition,
                Some(&root.path().join("alias")),
                &[&protected]
            )
            .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn reports_unencodable_definition_paths_without_panicking_or_writing_partial_metadata() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let definition = root
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
    let directory = root.path().join("build");
    let error = BuildDirectory::open(&definition, Some(&directory))
        .err()
        .unwrap();
    assert!(error.to_string().contains("metadata"));
    assert!(!directory.join(".mf-owner.json").exists());
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
}
