mod common;
use mf_compiler::{BuildDirectory, default_build_directory};
use std::fs;

#[test]
fn reuses_owned_directories_and_rejects_foreign_or_incompatible_entries() {
    let root = common::Directory::new();
    let definition = root.0.join("flow.json");
    let directory = root.0.join("build");
    let first = BuildDirectory::open(&definition, Some(&directory)).unwrap();
    assert!(!first.reused);
    let key = default_build_directory(&root.0, &definition);
    assert_eq!(key, default_build_directory(&root.0, &definition));
    assert_ne!(
        key,
        default_build_directory(&root.0, &root.0.join("other.json"))
    );
    drop(first);
    assert!(
        BuildDirectory::open(&definition, Some(&directory))
            .unwrap()
            .reused
    );
    assert!(BuildDirectory::open(&root.0.join("other.json"), Some(&directory)).is_err());
    let marker = directory.join(".mf-owner.json");
    let mut owner: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    owner["layout_version"] = serde_json::json!(999);
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
    let root = common::Directory::new();
    let definition = root.0.join("flow.json");
    let directory = root.0.join("build");
    let guard = BuildDirectory::open(&definition, Some(&directory)).unwrap();
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_err());
    let other = BuildDirectory::open(
        &root.0.join("other.json"),
        Some(&root.0.join("other-build")),
    )
    .unwrap();
    drop(other);
    drop(guard);
    assert!(BuildDirectory::open(&definition, Some(&directory)).is_ok());
    let protected = fs::canonicalize(&directory).unwrap().join("output");
    assert!(BuildDirectory::open_protected(&definition, Some(&directory), &[&protected]).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&directory, root.0.join("alias")).unwrap();
        assert!(
            BuildDirectory::open_protected(&definition, Some(&root.0.join("alias")), &[&protected])
                .is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn reports_unencodable_definition_paths_without_panicking_or_writing_partial_metadata() {
    use std::os::unix::ffi::OsStringExt;
    let root = common::Directory::new();
    let definition = root.0.join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
    let directory = root.0.join("build");
    let error = BuildDirectory::open(&definition, Some(&directory))
        .err()
        .unwrap();
    assert!(error.to_string().contains("metadata"));
    assert!(!directory.join(".mf-owner.json").exists());
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
}
