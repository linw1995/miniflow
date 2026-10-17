use mfn_test_affected::git;
use std::{fs, path::Path};

pub const SOURCE: &str = "pub fn value() -> u32 { 1 }\n#[cfg(test)] mod tests { #[test] fn changed() { assert_eq!(1, 1); } #[test] fn stable() { assert!(true); } }\n";

pub fn write(root: &Path, path: &str, contents: &str) {
    fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
    fs::write(root.join(path), contents).unwrap();
}
pub fn commit(root: &Path) -> String {
    git::command(root, "git", &["add", "."]).unwrap();
    git::command(root, "git", &["commit", "-qm", "test(ci): fixture"]).unwrap();
    git::revision(root, "HEAD").unwrap()
}
pub fn fixture() -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    git::command(root, "git", &["init", "-q", "-b", "main"]).unwrap();
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git::command(root, "git", &["config", key, value]).unwrap();
    }
    write(root, ".gitignore", "/target/\n");
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"core\", \"app\", \"other\"]\nresolver = \"3\"\n",
    );
    for name in ["core", "app", "other"] {
        let deps = if name == "app" {
            "[dependencies]\nsample-core = { path = \"../core\" }\n"
        } else {
            ""
        };
        write(
            root,
            &format!("{name}/Cargo.toml"),
            &format!(
                "[package]\nname = \"sample-{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{deps}"
            ),
        );
        write(root, &format!("{name}/src/lib.rs"), SOURCE);
    }
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let base = commit(root);
    (directory, base)
}
