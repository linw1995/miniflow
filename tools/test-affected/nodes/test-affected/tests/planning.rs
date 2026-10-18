use mfn_lsp::{AnalysisOutcome, Gap, GapKind, Profile, ServerConfig, Status};
use mfn_test_affected::{PlanOptions, git, make_plan};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

mod common;
use common::*;

fn outcome(comparison: &mfn_test_affected::Comparison) -> AnalysisOutcome {
    AnalysisOutcome {
        request: comparison.request.clone(),
        server: ServerConfig {
            command: "unused".into(),
            args: vec![],
            profile: Profile::Standard,
            extensions: vec![],
            deadline_ms: 1000,
            max_requests: 10,
            max_bytes: 8192,
        },
        status: Status::Analyzed,
        evidence: vec![],
        gaps: vec![],
        elapsed_ms: 1,
    }
}
fn compare(root: &Path, base: &str, head: &str) -> mfn_test_affected::Comparison {
    git::compare(root, base, head, None, json!({})).unwrap()
}

#[test]
fn combines_isolated_tests_with_production_consumers_and_keeps_caller_checkout() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    write(
        root,
        "app/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.baseline_packages, ["sample-app", "sample-core"]);

    assert_eq!(plan.packages, ["sample-app"]);
    assert_eq!(plan.tests.len(), 1);
    assert_eq!(plan.tests[0].name, "tests::changed");
    assert_eq!(plan.tests[0].binary, "sample-core");
    assert!(plan.selected_count.is_none());
    let input = mfn_lsp::AnalyzeOutput {
        outcome: serde_json::to_value(outcome(&comparison)).unwrap().into(),
        context: mf_runtime::ValueRef::from(json!({"comparison":comparison,"options":{}}))
            .into_secret(),
    };
    let result = mf_runtime::TypedTaskNode::execute(
        &mfn_test_affected::PlanNode,
        input,
        &mut mf_runtime::ExecutionContext::default(),
    )
    .unwrap();
    assert!(result.outputs.plan.is_secret());

    assert_eq!(git::revision(root, "HEAD").unwrap(), head);
    assert_eq!(
        git::command(root, "git", &["branch", "--show-current"])
            .unwrap()
            .trim(),
        "main"
    );
    assert!(comparison.head_root.join("core/src/lib.rs").exists());
    assert!(!comparison.head_root.join("target/debug").exists());
}

#[test]
fn retains_behavior_changes_shared_inputs_and_analyzer_failures() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let mut analysis = outcome(&comparison);
    let plan = make_plan(&comparison, &analysis, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    analysis.gaps.push(Gap {
        snapshot: comparison.request.snapshots[1].id.clone(),
        operation: "initialize".into(),
        kind: GapKind::AnalyzerFailure,
        message: "unavailable".into(),
    });
    analysis.status = Status::Incomplete;
    let fallback = make_plan(&comparison, &analysis, &PlanOptions::default()).unwrap();
    assert_eq!(fallback.packages.len(), 3);
    assert!(
        fallback
            .fallback_reasons
            .iter()
            .any(|reason| reason.contains("unavailable"))
    );
    write(root, ".config/nextest.toml", "[profile.default]\n");
    let next = commit(root);
    let comparison = compare(root, &base, &next);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert_eq!(plan.ignored_files, [".config/nextest.toml"]);
    assert!(plan.fallback_reasons.is_empty());
    write(root, "scripts/unknown.sh", "exit 0\n");
    let next = commit(root);
    let comparison = compare(root, &base, &next);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages.len(), 3);
    assert!(plan.fallback_reasons[0].contains("scripts/unknown.sh"));
}

#[test]
fn resolves_merge_base_and_rejects_changed_snapshots_and_analysis_identities() {
    let (directory, base) = fixture();
    let root = directory.path();
    git::command(root, "git", &["checkout", "-qb", "feature"]).unwrap();
    write(root, "docs/feature.md", "feature\n");
    let head = commit(root);
    git::command(root, "git", &["checkout", "-q", "main"]).unwrap();
    write(root, "docs/main.md", "main\n");
    let advanced = commit(root);
    let comparison = compare(root, &advanced, &head);
    assert_eq!(comparison.base, base);
    assert!(git::verify_snapshot(&comparison.head_root, &base).is_err());
    assert_eq!(
        comparison
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        ["docs/feature.md"]
    );
    let analysis = outcome(&comparison);
    let plan = make_plan(&comparison, &analysis, &PlanOptions::default()).unwrap();
    assert!(plan.packages.is_empty() && plan.tests.is_empty());
    let mut wrong = analysis.clone();
    wrong.request.snapshots[1].id = "wrong".into();
    assert!(make_plan(&comparison, &wrong, &PlanOptions::default()).is_err());
    let mut wrong = analysis.clone();
    wrong.evidence.push(mfn_lsp::SnapshotEvidence {
        snapshot: "another-snapshot".into(),
        server_info: json!(null),
        position_encoding: "utf-16".into(),
        progress_notifications: 0,
        documents: vec![],
    });
    assert!(make_plan(&comparison, &wrong, &PlanOptions::default()).is_err());
    write(&comparison.head_root, "core/src/lib.rs", "changed\n");
    assert!(make_plan(&comparison, &analysis, &PlanOptions::default()).is_err());
    assert!(git::compare(root, "missing", &head, None, json!({})).is_err());
    let nested = root.join("target/not-a-worktree");
    fs::create_dir_all(&nested).unwrap();
    assert!(git::verify_snapshot(&nested, &git::revision(root, "HEAD").unwrap()).is_err());
}

#[test]
fn script_verifies_selection_runs_cases_and_preserves_empty_stale_and_failed_plans() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let mut plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    let passing_plan = plan.clone();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../run-tests.py");
    let path = root.join("target/plan.json");
    let run = |plan: &mfn_test_affected::TestPlan, args: &[&str]| {
        fs::write(&path, serde_json::to_vec(plan).unwrap()).unwrap();
        Command::new("python3")
            .arg(&script)
            .arg(&path)
            // Revisit older snapshots under the same inherited build directory.
            .env("CARGO_TARGET_DIR", root.join("target/shared-build"))
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&plan, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"selected_count\": 1"));
    let mut legacy = serde_json::to_value(&plan).unwrap();
    legacy.as_object_mut().unwrap().remove("ignored_files");
    legacy.as_object_mut().unwrap().remove("comparison_mode");
    assert!(
        serde_json::from_value::<mfn_test_affected::TestPlan>(legacy.clone())
            .unwrap()
            .ignored_files
            .is_empty()
    );
    fs::write(&path, legacy.to_string()).unwrap();
    let output = Command::new("python3")
        .arg(&script)
        .arg(&path)
        .arg("--list-only")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"selected_count\": 1"));
    let inventory = root.join("target/inventory.json");
    let output = run(
        &plan,
        &[
            "--list-only",
            "--inventory-out",
            inventory.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let counted = make_plan(
        &comparison,
        &outcome(&comparison),
        &PlanOptions {
            inventory: Some(inventory.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(counted.baseline_count, Some(4));
    assert_eq!(counted.selected_count, Some(1));
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&inventory).unwrap()).unwrap();
    for case in ["identity", "shape", "missing_case"] {
        let mut supplied = original.clone();
        match case {
            "identity" => supplied["head"] = json!("another-head"),
            "shape" => supplied["inventory"] = json!({}),
            _ => {
                supplied["inventory"]["rust-suites"]["sample-core"]["testcases"]
                    .as_object_mut()
                    .unwrap()
                    .remove("tests::changed");
            }
        }
        fs::write(&inventory, supplied.to_string()).unwrap();
        let result = make_plan(
            &comparison,
            &outcome(&comparison),
            &PlanOptions {
                inventory: Some(inventory.clone()),
                ..Default::default()
            },
        );
        if case == "missing_case" {
            let fallback = result.unwrap();
            assert_eq!(fallback.packages, fallback.baseline_packages);
            assert!(fallback.tests.is_empty());
            assert_eq!(fallback.selected_count, fallback.baseline_count);
        } else {
            assert!(result.is_err(), "{case}");
        }
    }
    plan.tests[0].name = "missing_generated_case".into();
    let output = run(&plan, &["--list-only"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("exact identities are absent"));
    plan.tests.clear();
    assert!(run(&plan, &[]).status.success());
    plan.head = base;
    assert!(!run(&plan, &[]).status.success());
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("assert_eq!(1, 1)", "assert_eq!(1, 2)"),
    );
    let failing = commit(root);
    let comparison = compare(root, &head, &failing);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    let output = run(&plan, &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("FAIL"));
    let output = run(&passing_plan, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_feature_configuration_drift_and_partial_inventories() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let comparison = git::compare(
        root,
        &base,
        &head,
        None,
        json!({"cargo":{"features":"all"}}),
    )
    .unwrap();
    let mut analysis = outcome(&comparison);
    analysis.server.profile = Profile::RustAnalyzer;
    let mut options = PlanOptions::default();
    options.configuration.all_features = false;
    assert!(make_plan(&comparison, &analysis, &options).is_err());
    options.configuration.all_features = true;
    let plan = make_plan(&comparison, &analysis, &options).unwrap();
    let inventory = root.join("target/partial.json");
    fs::write(
        &inventory,
        json!({"head":head,"configuration":plan.configuration,"toolchain":plan.toolchain,
        "build_environment":plan.build_environment,"inventory":{"rust-suites":{}}})
        .to_string(),
    )
    .unwrap();
    options.inventory = Some(inventory);
    assert!(make_plan(&comparison, &analysis, &options).is_err());
}

#[test]
fn maps_conventional_modules_and_integration_targets_but_retains_ambiguous_paths() {
    let (directory, _) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", "mod flat; mod nested;\n");
    write(root, "core/src/flat.rs", SOURCE);
    write(root, "core/src/nested/mod.rs", SOURCE);
    write(
        root,
        "core/tests/external.rs",
        "#[test] fn external() { assert_eq!(1, 1); }\n",
    );
    let base = commit(root);
    for path in [
        "core/src/flat.rs",
        "core/src/nested/mod.rs",
        "core/tests/external.rs",
    ] {
        let text = fs::read_to_string(root.join(path)).unwrap();
        write(root, path, &text.replace("1, 1", "2, 2"));
    }
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert!(plan.packages.is_empty());
    assert_eq!(
        plan.tests
            .iter()
            .map(|t| (t.binary.as_str(), t.name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("sample-core", "flat::tests::changed"),
            ("sample-core", "nested::tests::changed"),
            ("sample-core::external", "external"),
        ]
    );

    // Two candidate module files cannot establish one executable identity.
    write(root, "core/src/flat/mod.rs", SOURCE);
    let ambiguous_base = commit(root);
    write(root, "core/src/flat.rs", &SOURCE.replace("1, 1", "3, 3"));
    let head = commit(root);
    let comparison = compare(root, &ambiguous_base, &head);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
}

#[test]
fn feature_profiles_must_match_the_execution_configuration() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let mut comparison = compare(root, &base, &head);
    let mut options = PlanOptions::default();
    options.configuration.all_features = false;
    options.configuration.features = vec!["selected".into()];
    options.configuration.no_default_features = true;
    for (cargo, valid) in [
        (
            json!({"features":["selected"],"noDefaultFeatures":true}),
            true,
        ),
        (
            json!({"features":["other"],"noDefaultFeatures":true}),
            false,
        ),
        (
            json!({"features":["selected"],"noDefaultFeatures":false}),
            false,
        ),
        (
            json!({"features":["selected"],"noDefaultFeatures":true,"target":"other-target"}),
            false,
        ),
    ] {
        comparison.request.initialization_options = json!({"cargo":cargo});
        let mut analysis = outcome(&comparison);
        analysis.server.profile = Profile::RustAnalyzer;
        assert_eq!(make_plan(&comparison, &analysis, &options).is_ok(), valid);
    }
    options.configuration.all_features = true;
    assert!(make_plan(&comparison, &outcome(&comparison), &options).is_err());
    options.configuration.features.clear();
    options.configuration.no_default_features = false;
    let mut analysis = outcome(&comparison);
    analysis.server.profile = Profile::RustAnalyzer;
    analysis.request.initialization_options = json!({"cargo":{"features":[]}});
    comparison.request = analysis.request.clone();
    assert!(make_plan(&comparison, &analysis, &options).is_err());
}

#[test]
fn additions_deletions_and_manifest_changes_preserve_conservative_scope() {
    let (directory, _) = fixture();
    let root = directory.path();
    write(root, "core/src/old.rs", "pub fn old() {}\n");
    let base = commit(root);
    git::command(root, "git", &["mv", "core/src/old.rs", "core/src/new.rs"]).unwrap();
    write(root, "core/data.txt", "fixture\n");
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    assert_eq!(comparison.changes.len(), 3);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    assert!(git::compare(root, &base, &head, Some(Path::new("relative")), json!({})).is_err());
    let manifest = fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
    write(
        root,
        "core/Cargo.toml",
        &(manifest + "\n[features]\nselected = []\n"),
    );
    let next = commit(root);
    let comparison = compare(root, &head, &next);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.fallback_reasons.is_empty());
}

#[test]
fn task_boundaries_preserve_error_sources_and_reject_cancelled_work() {
    use mf_runtime::{ExecutionContext, TypedTaskNode};
    use std::error::Error as _;
    let node = mfn_test_affected::GitNode {
        config: Default::default(),
    };
    let input = || mfn_test_affected::GitInput {
        repository: "/missing-test-affected-repository".into(),
        base: Some("HEAD".into()),
        request: None,
        head: None,
        artifacts: None,
        options: None,
    };
    let mut context = ExecutionContext::default();
    let error = node.execute(input(), &mut context).err().unwrap();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<mfn_test_affected::Error>()
            .is_some()
    );
    let plan_input = || mfn_lsp::AnalyzeOutput {
        outcome: json!(null).into(),
        context: json!({}).into(),
    };
    let error = mfn_test_affected::PlanNode
        .execute(plan_input(), &mut context)
        .err()
        .unwrap();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<mfn_test_affected::Error>()
            .is_some()
    );
    context
        .cancellation()
        .cancel(mf_runtime::StreamError::Execution {
            message: "cancel fixture".into(),
        });
    assert!(node.execute(input(), &mut context).is_err());
    assert!(
        mfn_test_affected::PlanNode
            .execute(plan_input(), &mut context)
            .is_err()
    );
}

#[test]
fn shared_input_policy_scopes_known_consumers() {
    let (directory, _) = fixture();
    let root = directory.path();
    for package in ["core", "app", "other"] {
        let path = format!("{package}/Cargo.toml");
        let manifest = fs::read_to_string(root.join(&path))
            .unwrap()
            .replace("sample-core", "mf-compiler")
            .replace("sample-app", "mf-cli")
            .replace("sample-other", "mfn-test-affected");
        write(root, &path, &manifest);
    }
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let base = commit(root);
    write(root, "docs/policy.md", "policy\n");
    let head = commit(root);
    let mut comparison = compare(root, &base, &head);
    // Exercise classification against real package metadata without invalidating snapshots.
    for (path, expected) in [
        ("Cargo.toml", vec![]),
        ("Cargo.lock", vec![]),
        (".config/nextest.toml", vec![]),
        (".github/workflows/CI.yaml", vec![]),
        ("docs/guide.md", vec![]),
        ("openspec/specs/example/spec.md", vec![]),
        ("scripts/run-cov.sh", vec![]),
        ("scripts/pr_validation.py", vec![]),
        ("tools/test-affected/README.md", vec![]),
        ("core/Cargo.toml", vec!["mf-cli", "mf-compiler"]),
        (
            "core/tests/fixture/Cargo.lock",
            vec!["mf-cli", "mf-compiler"],
        ),
        ("core/README.md", vec!["mf-cli", "mf-compiler"]),
        ("examples/example.json", vec!["mf-cli", "mf-compiler"]),
        (
            "scripts/nextest-build-cache.py",
            vec!["mf-cli", "mf-compiler"],
        ),
        ("scripts/nextest-cargo.sh", vec!["mf-cli", "mf-compiler"]),
        ("scripts/cargo_cache.py", vec!["mf-cli", "mf-compiler"]),
        ("scripts/check-release-support.sh", vec!["mf-cli"]),
        ("scripts/affected_tests.py", vec!["mfn-test-affected"]),
        (
            "tools/test-affected/run-tests.py",
            vec!["mfn-test-affected"],
        ),
        (
            "tools/test-affected/flows/plan.json",
            vec!["mfn-test-affected"],
        ),
        (
            "scripts/unknown.py",
            vec!["mf-cli", "mf-compiler", "mfn-test-affected"],
        ),
        (
            "removed/src/lib.rs",
            vec!["mf-cli", "mf-compiler", "mfn-test-affected"],
        ),
    ] {
        comparison.changes = vec![mfn_test_affected::Change {
            path: path.into(),
            before: vec![],
            after: vec![],
        }];
        let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
        assert_eq!(plan.packages, expected, "{path}");
        assert_eq!(
            plan.ignored_files,
            if expected.is_empty() {
                vec![path]
            } else {
                vec![]
            }
        );
    }
    let (directory, base) = fixture();
    write(directory.path(), "examples/unknown.json", "{}\n");
    let head = commit(directory.path());
    let comparison = compare(directory.path(), &base, &head);
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages.len(), 3);
    assert!(!plan.fallback_reasons.is_empty());
}

#[test]
fn snapshot_mode_preserves_incremental_base_and_plan_provenance() {
    use mfn_test_affected::{ComparisonMode, GitConfig};
    let (directory, base) = fixture();
    let root = directory.path();
    git::command(root, "git", &["checkout", "-qb", "feature"]).unwrap();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    commit(root);
    git::command(root, "git", &["checkout", "-qb", "target", &base]).unwrap();
    write(
        root,
        "other/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 3 }"),
    );
    let target = commit(root);
    git::command(
        root,
        "git",
        &["merge", "--no-ff", "feature", "-m", "first merge"],
    )
    .unwrap();
    let first_merge = git::revision(root, "HEAD").unwrap();
    git::command(root, "git", &["checkout", "-q", "feature"]).unwrap();
    write(
        root,
        "app/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    commit(root);
    git::command(root, "git", &["checkout", "-q", "--detach", &target]).unwrap();
    git::command(
        root,
        "git",
        &["merge", "--no-ff", "feature", "-m", "second merge"],
    )
    .unwrap();
    let second_merge = git::revision(root, "HEAD").unwrap();
    let comparison = git::compare_with_mode(
        root,
        first_merge.trim(),
        second_merge.trim(),
        None,
        json!({}),
        ComparisonMode::Snapshot,
    )
    .unwrap();
    assert_eq!(comparison.base, first_merge.trim());
    assert_eq!(comparison.changes.len(), 1);
    assert_eq!(comparison.changes[0].path, "app/src/lib.rs");
    let plan = make_plan(&comparison, &outcome(&comparison), &PlanOptions::default()).unwrap();
    assert_eq!(plan.comparison_mode, ComparisonMode::Snapshot);
    assert_eq!(plan.packages, ["sample-app"]);
    let cumulative = compare(root, first_merge.trim(), second_merge.trim());
    assert_ne!(cumulative.base, comparison.base);
    assert_eq!(cumulative.changes.len(), 2);
    let broader = make_plan(&cumulative, &outcome(&cumulative), &PlanOptions::default()).unwrap();
    assert_eq!(broader.baseline_packages.len(), 2);
    assert!(serde_json::from_value::<GitConfig>(json!({"comparison_mode":"unknown"})).is_err());
    let config: GitConfig = serde_json::from_value(json!({"comparison_mode":"snapshot"})).unwrap();
    assert_eq!(config.comparison_mode, ComparisonMode::Snapshot);
    let result = mf_runtime::TypedTaskNode::execute(
        &mfn_test_affected::GitNode { config },
        mfn_test_affected::GitInput {
            repository: root.to_str().unwrap().into(),
            base: Some(first_merge.trim().into()),
            request: None,
            head: Some(second_merge.trim().into()),
            artifacts: None,
            options: None,
        },
        &mut mf_runtime::ExecutionContext::default(),
    )
    .unwrap();
    let context = serde_json::to_value(result.outputs.context).unwrap();
    assert_eq!(context["comparison"]["base"], first_merge.trim());
}

#[test]
fn worktree_and_root_commit_requests_preserve_source_and_index() {
    use mfn_test_affected::{ComparisonMode, ComparisonRequest};
    let (directory, first) = fixture();
    let root = directory.path();
    let first_request: ComparisonRequest = serde_json::from_value(
        json!({"mode":"commit","head":first,"analyze":false,"options":{"exact_tests":false}}),
    )
    .unwrap();
    let first_comparison = git::compare_request(root, &first_request, None, json!({})).unwrap();
    assert_eq!(first_comparison.comparison_mode, ComparisonMode::Commit);
    assert!(
        git::command(&first_comparison.base_root, "git", &["ls-tree", "HEAD"])
            .unwrap()
            .is_empty()
    );
    assert!(first_comparison.request.snapshots.is_empty());
    let first_plan = make_plan(
        &first_comparison,
        &outcome(&first_comparison),
        &first_request.options,
    )
    .unwrap();
    assert_eq!(first_plan.packages.len(), 3);

    write(
        root,
        ".gitignore",
        "/target/\n/ignored.txt\n/private-staged.txt\n",
    );
    let base = commit(root);
    write(root, "core/src/lib.rs", &SOURCE.replace("{ 1 }", "{ 2 }"));
    git::command(root, "git", &["add", "core/src/lib.rs"]).unwrap();
    write(root, "app/src/lib.rs", &SOURCE.replace("{ 1 }", "{ 3 }"));
    write(root, "other/src/new.rs", "pub fn added() {}\n");
    write(root, "private-staged.txt", "forced into index\n");
    git::command(root, "git", &["add", "-f", "private-staged.txt"]).unwrap();
    write(root, "ignored.txt", "not part of the snapshot\n");
    let index = fs::read(root.join(".git/index")).unwrap();
    let request: ComparisonRequest = serde_json::from_value(
        json!({"mode":"worktree","analyze":false,"options":{"exact_tests":false}}),
    )
    .unwrap();
    let comparison = git::compare_request(root, &request, None, json!({})).unwrap();
    assert_eq!(comparison.comparison_mode, ComparisonMode::Worktree);
    assert_eq!(comparison.base, base);
    assert_ne!(comparison.head, base);
    assert_eq!(git::revision(root, "HEAD").unwrap(), base);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read_to_string(comparison.head_root.join("private-staged.txt")).unwrap(),
        "forced into index\n"
    );
    assert!(comparison.head_root.join("other/src/new.rs").is_file());
    assert!(!comparison.head_root.join("ignored.txt").exists());
    assert_eq!(
        comparison
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        [
            "app/src/lib.rs",
            "core/src/lib.rs",
            "other/src/new.rs",
            "private-staged.txt"
        ]
    );
    assert!(
        git::command(&comparison.head_root, "git", &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    let repeated = git::compare_request(root, &request, None, json!({})).unwrap();
    assert_eq!(repeated.head, comparison.head);
    let strict: ComparisonRequest = serde_json::from_value(
        json!({"mode":"snapshot","base":base,"head":base,"require_checkout":true}),
    )
    .unwrap();
    assert!(git::compare_request(root, &strict, None, json!({})).is_err());
    let shallow = tempfile::tempdir().unwrap();
    let source = url::Url::from_directory_path(root).unwrap();
    git::command(
        shallow.path(),
        "git",
        &["clone", "--quiet", "--depth=1", source.as_str(), "."],
    )
    .unwrap();
    let commit_request: ComparisonRequest =
        serde_json::from_value(json!({"mode":"commit","analyze":false})).unwrap();
    assert!(git::compare_request(shallow.path(), &commit_request, None, json!({})).is_err());
}

#[test]
fn normalized_requests_retain_provenance_and_package_only_scope() {
    use mf_runtime::{ExecutionContext, TypedStreamNode, TypedTaskNode};
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let request = mf_runtime::ValueRef::from(json!({"mode":"snapshot","base":base,"head":head,"analyze":false,"require_checkout":true,"options":{"exact_tests":false},"validation":{"run":42}})).into_secret();
    let analysis_input = mfn_test_affected::GitNode {
        config: Default::default(),
    }
    .execute(
        mfn_test_affected::GitInput {
            repository: root.to_str().unwrap().into(),
            base: None,
            head: None,
            request: Some(request),
            artifacts: None,
            options: None,
        },
        &mut ExecutionContext::default(),
    )
    .unwrap()
    .outputs;
    assert!(analysis_input.context.is_secret());
    let mut analyzer = mfn_lsp::AnalyzeNode::new(ServerConfig {
        command: "/missing-server-must-not-run".into(),
        ..outcome(&compare(root, &base, &head)).server
    })
    .unwrap();
    let mut final_output = None;
    analyzer
        .execute(
            analysis_input,
            &mut ExecutionContext::default(),
            &mut |result| {
                final_output = Some(
                    mfn_test_affected::PlanNode
                        .execute(result.outputs, &mut ExecutionContext::default())?
                        .outputs,
                );
                Ok(())
            },
        )
        .unwrap();
    let output = final_output.unwrap();
    assert!(output.plan.is_secret() && output.validation.is_secret());
    assert_eq!(output.validation, json!({"run":42}));
    let plan: mfn_test_affected::TestPlan =
        serde_json::from_value(serde_json::to_value(output.plan).unwrap()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    assert_eq!(plan.analysis_status, Status::NotRequested);
    let plan_file = root.join("target/package-plan.json");
    fs::write(
        &plan_file,
        json!({"plan":plan,"validation":{"run":42}}).to_string(),
    )
    .unwrap();
    let runner = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../run-tests.py");
    let loaded = Command::new("python3")
        .args([
            "-c",
            "import runpy,sys; m=runpy.run_path(sys.argv[1]); m['load_plan'](sys.argv[2])",
        ])
        .arg(runner)
        .arg(plan_file)
        .output()
        .unwrap();
    assert!(
        loaded.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded.stderr)
    );
}
