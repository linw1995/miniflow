use std::{fs, path::Path, process::Command};

struct Case {
    source: String,
    diagnostic: Option<String>,
}

#[derive(Default)]
struct Cases(Vec<Case>);

impl Cases {
    fn add(&mut self, source: &str, outputs: bool, diagnostic: Option<&str>) {
        self.0.push(Case {
            source: direction(source, outputs),
            diagnostic: diagnostic.map(|text| direction(text, outputs)),
        });
    }

    fn check(self, root: &Path) {
        for (index, case) in self.0.iter().enumerate() {
            fs::write(
                root.join(format!("src/bin/case_{index}.rs")),
                format!("{}\nfn main() {{}}\n", case.source),
            )
            .unwrap();
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let metadata = Command::new(&cargo)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["metadata", "--offline", "--no-deps", "--format-version=1"])
            .output()
            .unwrap();
        assert!(
            metadata.status.success(),
            "{}",
            String::from_utf8_lossy(&metadata.stderr),
        );
        let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
        fs::copy(
            Path::new(metadata["workspace_root"].as_str().unwrap()).join("Cargo.lock"),
            root.join("Cargo.lock"),
        )
        .unwrap();
        // Checks reuse workspace dependencies without replacing any executable under test.
        // Separate targets preserve diagnostic attribution within one Cargo invocation.
        let output = Command::new(cargo)
            .current_dir(root)
            .args([
                "check",
                "--offline",
                "--quiet",
                "--bins",
                "--keep-going",
                "--message-format=json",
            ])
            .env(
                "CARGO_TARGET_DIR",
                metadata["target_directory"].as_str().unwrap(),
            )
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let messages: Vec<serde_json::Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            output.status.code(),
            Some(101),
            "expected compile failures, got {}:\n{stdout}\n{stderr}",
            output.status,
        );
        for (index, case) in self.0.iter().enumerate() {
            let name = format!("case_{index}");
            let mut compiled = false;
            let mut errors = Vec::new();
            for message in &messages {
                if message["target"]["name"] != name {
                    continue;
                }
                if message["reason"] == "compiler-artifact" {
                    compiled = true;
                }
                if message["reason"] == "compiler-message" && message["message"]["level"] == "error"
                {
                    errors.push(message["message"]["rendered"].as_str().unwrap());
                }
            }
            match &case.diagnostic {
                None => assert!(
                    compiled && errors.is_empty(),
                    "{}: expected success, got {errors:?}\n{stderr}",
                    case.source,
                ),
                Some(diagnostic) => assert!(
                    !compiled && errors.iter().any(|error| error.contains(diagnostic)),
                    "{}: expected {diagnostic}, got {errors:?}\n{stderr}",
                    case.source,
                ),
            }
        }
    }
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
    fs::create_dir_all(root.path().join("src/bin")).unwrap();
    let mut cases = Cases::default();
    let runtime = serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap();
    fs::write(root.path().join("Cargo.toml"), format!(
        "[package]\nname = \"input-derive-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nruntime_alias = {{ package = \"mf-runtime\", path = {runtime} }}\n"
    )).unwrap();
    for outputs in [false, true] {
        let prefix = "use runtime_alias::NodeInputs;\n#[derive(NodeInputs)]\n#[input(runtime = \"::runtime_alias\")]\n";
        let valid = format!(
            "{prefix}pub struct Inputs<T> where T: runtime_alias::InputValue {{ pub data: T, pub path: Option<String>, pub items: Vec<T> }}"
        );
        cases.add(&valid, outputs, None);

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
            cases.add(&format!("{prefix}{declaration}"), outputs, Some(diagnostic));
        }
        let default_source = format!("{prefix}struct Inputs<T> {{ #[input(default)] value: T }}");
        cases.add(
            &default_source,
            outputs,
            outputs.then_some("expected `rename"),
        );
        for (attribute, diagnostic) in [
            (
                "#[input(runtime = \"::runtime_alias\", runtime = \"crate\")]",
                "duplicate runtime path",
            ),
            ("#[input(rename = \"x\")]", "expected `runtime"),
            ("#[input(runtime = \"bad path\")]", "unexpected token"),
        ] {
            cases.add(
                &format!(
                    "use runtime_alias::NodeInputs;\n#[derive(NodeInputs)]\n{attribute}\nstruct Inputs {{ value: i64 }}"
                ),
                outputs,
                Some(diagnostic),
            );
        }
    }
    let prefix = "use runtime_alias::NodeValue;\n#[derive(NodeValue)]\n#[value(runtime = \"::runtime_alias\")]\n";
    let valid =
        format!("{prefix}pub struct Values<T> {{ data: T, label: Option<String>, items: Vec<T> }}");
    cases.add(&valid, false, None);
    let typed = valid.replace(
        "#[derive(NodeValue)]",
        "#[derive(NodeValue)]\n#[value(typed)]",
    );
    cases.add(&typed, false, None);
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
        cases.add(&format!("{prefix}{declaration}"), false, Some(diagnostic));
    }
    let enum_prefix = "use runtime_alias::NodeEnum;\n#[derive(NodeEnum)]\n#[value(runtime = \"::runtime_alias\")]\n";
    for declaration in [
        "enum Mode { #[value(rename = \"fast\")] Fast, Safe }",
        "#[value(tag = \"kind\")] enum Payload<T> { Empty, Item { data: T, #[value(default)] limit: usize } }",
    ] {
        cases.add(&format!("{enum_prefix}{declaration}"), false, None);
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
        cases.add(
            &format!("{enum_prefix}{declaration}"),
            false,
            Some(diagnostic),
        );
    }
    cases.check(root.path());
}
