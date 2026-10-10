use std::{fs, path::Path, process::Command};

fn check(root: &Path, source: &str, outputs: bool) -> std::process::Output {
    let source = direction(source, outputs);
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

fn direction(text: &str, outputs: bool) -> String {
    if outputs {
        text.replace("Input", "Output").replace("input", "output")
    } else {
        text.to_owned()
    }
}

#[test]
fn struct_derives_compile_with_aliases_and_report_invalid_declarations() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    let runtime = serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap();
    fs::write(root.path().join("Cargo.toml"), format!(
        "[package]\nname = \"input-derive-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nruntime_alias = {{ package = \"mf-runtime\", path = {runtime} }}\n"
    )).unwrap();
    for outputs in [false, true] {
        let prefix = "use runtime_alias::NodeInputs;\n#[derive(NodeInputs)]\n#[input(runtime = \"::runtime_alias\")]\n";
        let valid = format!(
            "{prefix}pub struct Inputs<T> where T: runtime_alias::InputValue {{ pub data: T, pub path: Option<String>, pub items: Vec<T> }}"
        );
        let output = check(root.path(), &valid, outputs);
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
                "struct Inputs { #[input(unknown)] value: i64 }",
                "expected `rename",
            ),
        ] {
            let output = check(root.path(), &format!("{prefix}{declaration}"), outputs);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "accepted {declaration}");
            assert!(
                stderr.contains(&direction(diagnostic, outputs)),
                "{declaration}: expected {diagnostic}, got {stderr}"
            );
        }
        let default_source = format!("{prefix}struct Inputs<T> {{ #[input(default)] value: T }}");
        let output = check(root.path(), &default_source, outputs);
        assert_eq!(
            output.status.success(),
            !outputs,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
                outputs,
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "accepted {attribute}");
            assert!(
                stderr.contains(&direction(diagnostic, outputs)),
                "{attribute}: expected {diagnostic}, got {stderr}"
            );
        }
    }
    let prefix = "use runtime_alias::NodeValue;\n#[derive(NodeValue)]\n#[value(runtime = \"::runtime_alias\")]\n";
    let valid =
        format!("{prefix}pub struct Values<T> {{ data: T, label: Option<String>, items: Vec<T> }}");
    let output = check(root.path(), &valid, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let typed = valid.replace(
        "#[derive(NodeValue)]",
        "#[derive(NodeValue)]\n#[value(typed)]",
    );
    let output = check(root.path(), &typed, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (declaration, diagnostic) in [
        ("enum Values { Item }", "requires a named-field struct"),
        ("struct Values(i64);", "requires a named-field struct"),
        ("struct Values<'a> { item: &'a str }", "owned value fields"),
        (
            "struct Values { #[value(rename = \"\")] item: i64 }",
            "must not be empty",
        ),
        (
            "struct Values { #[value(rename = \"x\")] a: i64, #[value(rename = \"x\")] b: i64 }",
            "duplicate value port name",
        ),
        ("struct Values { item: Vec<Option<i64>> }", "InputValue"),
    ] {
        let output = check(root.path(), &format!("{prefix}{declaration}"), false);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(diagnostic),
            "{stderr}"
        );
    }
    let enum_prefix = "use runtime_alias::NodeEnum;\n#[derive(NodeEnum)]\n#[value(runtime = \"::runtime_alias\")]\n";
    for declaration in [
        "enum Mode { #[value(rename = \"fast\")] Fast, Safe }",
        "#[value(tag = \"kind\")] enum Payload<T> { Empty, Item { data: T, #[value(default)] limit: usize } }",
    ] {
        let output = check(root.path(), &format!("{enum_prefix}{declaration}"), false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (declaration, diagnostic) in [
        ("struct Mode {}", "NodeEnum requires an enum"),
        ("enum Mode {}", "at least one variant"),
        (
            "enum Mode { Item(String) }",
            "string NodeEnum requires unit variants",
        ),
        (
            "#[value(tag = \"kind\")] enum Mode { Item(String) }",
            "unit or named-field variants",
        ),
        (
            "#[value(tag = \"\")] enum Mode { Item }",
            "tag must not be empty",
        ),
        (
            "enum Mode { #[value(rename = \"x\")] A, #[value(rename = \"x\")] B }",
            "duplicate enum variant",
        ),
        (
            "#[value(tag = \"kind\")] enum Mode { Item { kind: String } }",
            "conflict with enum tag",
        ),
        (
            "#[value(tag = \"kind\")] enum Mode<'a> { Item { data: &'a str } }",
            "owned value fields",
        ),
        (
            "#[value(tag = \"kind\")] enum Mode { Item { #[value(default, default)] data: String } }",
            "duplicate field default",
        ),
    ] {
        let output = check(root.path(), &format!("{enum_prefix}{declaration}"), false);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(diagnostic),
            "{declaration}: {stderr}"
        );
    }
}
