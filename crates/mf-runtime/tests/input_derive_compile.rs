use std::{fs, path::Path, process::Command};

fn check(root: &Path, source: &str) -> std::process::Output {
    fs::write(root.join("src/lib.rs"), source).unwrap();
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .current_dir(root)
        .args(["check", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", root.join("target"));
    if let Some(config) = std::env::var_os("MF_TEST_SOURCE_CONFIG") {
        command.arg("--config").arg(config);
    }
    command.output().unwrap()
}

#[test]
fn derive_compiles_with_aliases_and_reports_invalid_declarations() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    let runtime = serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap();
    fs::write(root.path().join("Cargo.toml"), format!(
        "[package]\nname = \"input-derive-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nruntime_alias = {{ package = \"mf-runtime\", path = {runtime} }}\n"
    )).unwrap();
    let prefix = "use runtime_alias::NodeInputs;\n#[derive(NodeInputs)]\n#[input(runtime = \"::runtime_alias\")]\n";
    let valid = format!(
        "{prefix}pub struct Inputs<T> where T: runtime_alias::InputValue {{ pub data: T, pub path: Option<String>, pub items: Vec<T> }}"
    );
    let output = check(root.path(), &valid);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (declaration, diagnostic) in [
        ("enum Inputs { Value }", "requires a named-field struct"),
        ("struct Inputs(i64);", "requires a named-field struct"),
        ("struct Inputs;", "requires a named-field struct"),
        ("struct Inputs { value: u32 }", "InputField"),
        ("struct Inputs<'a> { value: &'a str }", "owned input fields"),
        (
            "struct Inputs<'a> { value: Vec<&'a str> }",
            "owned input fields",
        ),
        ("struct Inputs { value: Vec<Option<i64>> }", "InputValue"),
        ("struct Inputs { value: Option<Option<i64>> }", "InputValue"),
        (
            "struct Inputs { #[input(rename = \"\")] value: i64 }",
            "must not be empty",
        ),
        (
            "struct Inputs { #[input(rename = \"x\")] first: i64, #[input(rename = \"x\")] second: i64 }",
            "duplicate input port name",
        ),
        (
            "struct Inputs { #[input(rename = \"x\", rename = \"y\")] value: i64 }",
            "duplicate input rename",
        ),
        (
            "struct Inputs { #[input(default)] value: i64 }",
            "expected `rename",
        ),
    ] {
        let output = check(root.path(), &format!("{prefix}{declaration}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted {declaration}");
        assert!(
            stderr.contains(diagnostic),
            "{declaration}: expected {diagnostic}, got {stderr}"
        );
    }
    for (attribute, diagnostic) in [
        (
            "#[input(runtime = \"::runtime_alias\", runtime = \"crate\")]",
            "duplicate runtime path",
        ),
        ("#[input(rename = \"x\")]", "expected `runtime"),
        ("#[input(runtime = \"bad path\")]", "unexpected token"),
    ] {
        let output = check(
            root.path(),
            &format!(
                "use runtime_alias::NodeInputs;\n#[derive(NodeInputs)]\n{attribute}\nstruct Inputs {{ value: i64 }}"
            ),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted {attribute}");
        assert!(
            stderr.contains(diagnostic),
            "{attribute}: expected {diagnostic}, got {stderr}"
        );
    }
}
