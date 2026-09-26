mod common;
use mf_compiler::{BuildGuard, atomic_write};
use std::{fs, process::Command};

#[test]
fn lock_child() {
    if let Some(path) = std::env::var_os("MF_TEST_STATE_LOCK") {
        let guard = BuildGuard::acquire(std::path::Path::new(&path));
        assert_eq!(
            guard.is_ok(),
            std::env::var_os("MF_TEST_EXPECT_LOCK").is_some()
        );
    }
}

#[test]
fn locks_are_released_by_process_exit_and_stale_files_do_not_block() {
    let root = common::Directory::new();
    let path = root.0.join("flow.lock.guard");
    let guard = BuildGuard::acquire(&path).unwrap();
    let child = || {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "lock_child"])
            .env("MF_TEST_STATE_LOCK", &path);
        cmd
    };
    assert!(child().status().unwrap().success());
    drop(guard);
    assert!(
        child()
            .env("MF_TEST_EXPECT_LOCK", "1")
            .status()
            .unwrap()
            .success()
    );
    assert!(BuildGuard::acquire(&path).is_ok());
}

#[test]
fn atomically_replaces_state_and_cleans_up_failed_staging() {
    let root = common::Directory::new();
    let lock = root.0.join("flow.lock");
    fs::write(&lock, "old").unwrap();
    atomic_write(&lock, b"new").unwrap();
    assert_eq!(fs::read(&lock).unwrap(), b"new");
    fs::create_dir(root.0.join("directory")).unwrap();
    assert!(atomic_write(&root.0.join("directory"), b"invalid").is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"new");
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 2);
}
