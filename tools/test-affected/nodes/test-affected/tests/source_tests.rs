use mfn_test_affected::{PlanOptions, TestCase, git, make_plan};

mod common;
use common::*;

#[test]
fn selects_changed_and_added_cases_offline_and_preserves_package_precision() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "core/src/lib.rs",
        &format!(
            "{}\n#[test] fn added() {{ assert!(true); }}\n",
            SOURCE.replace("1, 1", "2, 2")
        ),
    );
    let head = commit(root);
    let comparison = git::compare(root, &base, &head, None).unwrap();
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.baseline_packages, ["sample-app", "sample-core"]);
    assert!(plan.packages.is_empty());
    assert_eq!(
        plan.tests,
        ["added", "tests::changed"].map(|name| TestCase {
            package: "sample-core".into(),
            binary: "sample-core".into(),
            name: name.into(),
        })
    );
    assert!(
        plan.reasons["sample-core"]
            .iter()
            .any(|reason| reason.contains("without coverage history"))
    );
    let plan = make_plan(
        &comparison,
        &PlanOptions {
            exact_tests: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plan.packages, plan.baseline_packages);
    assert!(plan.tests.is_empty());
}

#[test]
fn resolves_external_and_inline_modules_in_library_binary_and_integration_targets() {
    let (directory, _) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", "mod outer;\n");
    write(root, "core/src/outer.rs", "mod inner { mod cases; }\n");
    write(
        root,
        "core/src/outer/inner/cases.rs",
        "#[test] fn changed() { assert_eq!(1, 1); }\n#[test] fn stable() {}\n",
    );
    write(
        root,
        "core/src/main.rs",
        "fn main() {}\n#[test] fn binary_case() { assert_eq!(1, 1); }\n",
    );
    write(root, "core/tests/suite/main.rs", "mod cases;\n");
    write(
        root,
        "core/tests/suite/cases.rs",
        "mod nested { #[test] fn integration_case() { assert_eq!(1, 1); } }\n",
    );
    let base = commit(root);
    for file in [
        "core/src/outer/inner/cases.rs",
        "core/src/main.rs",
        "core/tests/suite/cases.rs",
    ] {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        write(root, file, &source.replace("1, 1", "2, 2"));
    }
    let head = commit(root);
    let comparison = git::compare(root, &base, &head, None).unwrap();
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert!(plan.packages.is_empty(), "{:?}", plan.reasons);
    assert_eq!(
        plan.tests,
        [
            ("sample-core", "outer::inner::cases::changed"),
            ("sample-core::bin/sample-core", "binary_case"),
            ("sample-core::suite", "cases::nested::integration_case"),
        ]
        .map(|(binary, name)| TestCase {
            package: "sample-core".into(),
            binary: binary.into(),
            name: name.into(),
        })
    );
    let plan_path = root.join("target/source-plan.json");
    std::fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../run-tests.py");
    let output = std::process::Command::new("python3")
        .arg(script)
        .arg(plan_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["selected_count"], 3);
    assert_eq!(report["executed"], true);
    assert!(report["fallback"].is_null());
}

#[test]
fn selects_new_integration_targets_and_async_cases_without_a_graph() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/tests/new.rs", "#[test] fn new_case() {}\n");
    write(
        root,
        "core/src/lib.rs",
        &format!("{SOURCE}\n#[tokio::test] async fn async_case() {{}}\n"),
    );
    let head = commit(root);
    let plan = make_plan(
        &git::compare(root, &base, &head, None).unwrap(),
        &PlanOptions::default(),
    )
    .unwrap();
    assert!(plan.packages.is_empty());
    assert_eq!(plan.tests.len(), 2);
    assert_eq!(plan.tests[0].name, "async_case");
    assert_eq!(plan.tests[1].binary, "sample-core::new");
}

#[test]
fn helper_production_deletion_rename_and_unknown_attribute_changes_keep_the_baseline() {
    for source in [
        SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
        SOURCE.replace("fn changed()", "fn renamed()"),
        SOURCE.replace("#[test] fn changed() { assert_eq!(1, 1); }", ""),
        SOURCE.replace("#[test] fn changed()", "#[custom] #[test] fn changed()"),
        SOURCE.replace("#[test] fn changed()", "#[ignore] #[test] fn changed()"),
        SOURCE.replace("mod tests {", "mod tests { fn helper() {}"),
        SOURCE.replace("assert_eq!(1, 1)", "assert_eq!(2, 2)") + "\npub fn helper() {}\n",
    ] {
        let (directory, base) = fixture();
        let root = directory.path();
        write(root, "core/src/lib.rs", &source);
        let head = commit(root);
        let plan = make_plan(
            &git::compare(root, &base, &head, None).unwrap(),
            &PlanOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.packages, plan.baseline_packages, "{source}");
        assert!(plan.tests.is_empty());
    }
}

#[test]
fn test_functions_used_as_helpers_retain_callers_in_same_or_external_modules() {
    for external in [false, true] {
        let (directory, _) = fixture();
        let root = directory.path();
        let mut source = "#[test] fn changed() { assert_eq!(1, 1); }\n".to_owned();
        if external {
            source.push_str("mod callers;\n");
            write(
                root,
                "core/src/callers.rs",
                "#[test] fn caller() { super::changed(); }\n",
            );
        } else {
            source.push_str("#[test] fn caller() { changed(); }\n");
        }
        write(root, "core/src/lib.rs", &source);
        let base = commit(root);
        write(root, "core/src/lib.rs", &source.replace("1, 1", "2, 2"));
        let head = commit(root);
        let plan = make_plan(
            &git::compare(root, &base, &head, None).unwrap(),
            &PlanOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.packages, plan.baseline_packages);
        assert!(plan.tests.is_empty());
    }
}

#[test]
fn unsupported_module_layout_and_unparseable_sources_fall_back() {
    for source in [
        "#[path = \"cases.rs\"] mod cases;\n",
        "include!(\"cases.rs\");\n",
        "mod cases;\ninvalid syntax\n",
    ] {
        let (directory, _) = fixture();
        let root = directory.path();
        write(root, "core/src/lib.rs", source);
        write(
            root,
            "core/src/cases.rs",
            "#[test] fn changed() { assert_eq!(1, 1); }\n",
        );
        let base = commit(root);
        write(
            root,
            "core/src/cases.rs",
            "#[test] fn changed() { assert_eq!(2, 2); }\n",
        );
        let head = commit(root);
        let plan = make_plan(
            &git::compare(root, &base, &head, None).unwrap(),
            &PlanOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.packages, plan.baseline_packages);
        assert!(plan.tests.is_empty());
    }
}
