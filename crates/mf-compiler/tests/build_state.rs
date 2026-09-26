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
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("flow.lock.guard");
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
    let root = tempfile::tempdir().unwrap();
    let lock = root.path().join("flow.lock");
    fs::write(&lock, "old").unwrap();
    atomic_write(&lock, b"new").unwrap();
    assert_eq!(fs::read(&lock).unwrap(), b"new");
    fs::create_dir(root.path().join("directory")).unwrap();
    assert!(atomic_write(&root.path().join("directory"), b"invalid").is_err());
    assert_eq!(fs::read(&lock).unwrap(), b"new");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn materializes_working_locks_without_aliasing_the_authoritative_lock() {
    let root = tempfile::tempdir().unwrap();
    let authoritative = root.path().join("flow.lock");
    fs::write(&authoritative, "locked resolution").unwrap();
    for symbolic in [true, false] {
        let working = root.path().join(if symbolic {
            "symbolic.lock"
        } else {
            "hard.lock"
        });
        if symbolic {
            std::os::unix::fs::symlink(&authoritative, &working).unwrap();
        } else {
            fs::hard_link(&authoritative, &working).unwrap();
        }
        mf_compiler::write_if_changed(&working, b"locked resolution").unwrap();
        fs::write(&working, "failed build resolution").unwrap();
        assert_eq!(fs::read(&authoritative).unwrap(), b"locked resolution");
        assert!(
            !fs::symlink_metadata(&working)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(unix)]
#[test]
fn rejects_symbolic_guards_without_creating_their_targets() {
    let root = tempfile::tempdir().unwrap();
    let missing_lock = root.path().join("flow.lock");
    let guard = root.path().join("flow.lock.guard");
    std::os::unix::fs::symlink(&missing_lock, &guard).unwrap();
    assert!(BuildGuard::acquire(&guard).is_err());
    assert!(!missing_lock.exists());
}
