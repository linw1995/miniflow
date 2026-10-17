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
    let legacy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../scripts");
    let check = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import json, pathlib, sys
sys.dont_write_bytecode = True
sys.path.insert(0, sys.argv[1])
import affected_tests as a
root = pathlib.Path(sys.argv[2])
packages = a.workspace_packages(root)
direct, _, fallback = a.classify(root, json.loads(sys.argv[3]), packages)
lines = ['digraph rust_analyzer_crate_graph {']
for index, package in enumerate(packages.values()):
    name = package['targets'][0]['name'].replace('-', '_')
    lines.append(f'_n{index}[label="{name}"][shape="box"];')
lines.append('}')
graph, _ = a.package_graph('\n'.join(lines), packages)
print(json.dumps(sorted(packages if fallback else a.select(direct, graph))))
"#,
        )
        .arg(legacy)
        .arg(&comparison.head_root)
        .arg(
            serde_json::to_string(
                &comparison
                    .changes
                    .iter()
                    .map(|c| &c.path)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let baseline: Vec<String> = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(plan.baseline_packages, baseline);

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
    assert_eq!(
        make_plan(&comparison, &outcome(&comparison), &PlanOptions::default())
            .unwrap()
            .packages
            .len(),
        3
    );
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
            inventory: Some(inventory),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(counted.baseline_count, Some(4));
    assert_eq!(counted.selected_count, Some(1));
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
