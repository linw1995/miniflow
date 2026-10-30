use mfn_test_affected::{PlanOptions, git, make_plan};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

mod common;
use common::*;

fn compare(root: &Path, base: &str, head: &str) -> mfn_test_affected::Comparison {
    git::compare(root, base, head, None).unwrap()
}

/// The normalized execution signature a compatible graph must bind to;
/// mirrors scripts/run-cov.sh and plan-side validation.
fn graph_signature(toolchain: &str, profile: &str) -> String {
    let environment: serde_json::Map<_, _> = [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
    ]
    .into_iter()
    .filter_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| (name.to_owned(), json!(value)))
    })
    .collect();
    json!({"environment":environment,"features":"all","profile":profile,"target":std::env::var("CARGO_BUILD_TARGET").ok(),"toolchain":toolchain_identity(toolchain)})
        .to_string()
}

/// Mirrors the plan-side platform-independent toolchain identity
/// (`rustc -vV` release and commit hash; the host line is irrelevant).
fn toolchain_identity(verbose: &str) -> String {
    let release = verbose
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .unwrap();
    let commit = verbose
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .unwrap();
    format!("rustc {release} ({commit})")
}

/// (package, binary, test name, covered file -> hit lines) for one test.
type CoveredTestSpec<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, Vec<(u32, u32)>)>);

/// Writes a synthetic coverage graph; each covered test carries one lcov
/// export per (file, hit lines) entry.
fn write_graph(
    root: &Path,
    name: &str,
    commit: &str,
    signature: String,
    covered: Vec<CoveredTestSpec<'_>>,
) -> PathBuf {
    use mfn_test_affected::graph::{CoverageManifest, CoveredTest, GraphWriter};
    let tests = covered
        .into_iter()
        .enumerate()
        .map(|(index, (package, binary, test, files))| {
            let mut export = String::new();
            for (file, hits) in files {
                export.push_str(&format!("SF:{file}\n"));
                for (line, count) in hits {
                    export.push_str(&format!("DA:{line},{count}\n"));
                }
                export.push_str("end_of_record\n");
            }
            let lcov = root.join(format!("target/{name}-{index}.lcov"));
            fs::write(&lcov, export).unwrap();
            CoveredTest {
                package: package.into(),
                binary: binary.into(),
                name: test.into(),
                lcov,
            }
        })
        .collect();
    let path = root.join(format!("target/{name}.sqlite"));
    let mut writer = GraphWriter::create(&path).unwrap();
    writer
        .index(
            &CoverageManifest {
                commit_sha: commit.into(),
                run_id: "7".into(),
                attempt: 3,
                signature,
                tests,
            },
            root,
        )
        .unwrap();
    path
}

fn graph_source(path: PathBuf, commit: &str) -> mfn_test_affected::CoverageGraphInput {
    mfn_test_affected::CoverageGraphInput::Resolved(mfn_test_affected::GraphSource {
        path,
        commit: commit.into(),
        run_id: "7".into(),
        attempt: 3,
        profile: "ci".into(),
    })
}

fn graph_options(graph: mfn_test_affected::CoverageGraphInput) -> PlanOptions {
    PlanOptions {
        coverage_graph: Some(graph),
        ..Default::default()
    }
}

#[test]
fn combines_changed_packages_with_consumers_and_keeps_caller_checkout() {
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
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.baseline_packages, ["sample-app", "sample-core"]);

    assert_eq!(plan.packages, ["sample-app"]);
    assert_eq!(plan.tests.len(), 1);
    assert_eq!(plan.tests[0].name, "tests::changed");
    assert!(plan.selected_count.is_none());
    let input = mfn_test_affected::PlanInput {
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
fn retains_behavior_changes_shared_inputs_and_unknown_input_fallback() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    write(root, ".config/nextest.toml", "[profile.default]\n");
    let next = commit(root);
    let comparison = compare(root, &base, &next);
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert_eq!(plan.ignored_files, [".config/nextest.toml"]);
    assert!(plan.fallback_reasons.is_empty());
    write(root, "scripts/unknown.sh", "exit 0\n");
    let next = commit(root);
    let comparison = compare(root, &base, &next);
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
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
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert!(plan.packages.is_empty() && plan.tests.is_empty());
    write(&comparison.head_root, "core/src/lib.rs", "changed\n");
    assert!(make_plan(&comparison, &PlanOptions::default()).is_err());
    assert!(git::compare(root, "missing", &head, None).is_err());
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
    let toolchain = git::command(root, "rustc", &["-vV"]).unwrap();
    let graph = write_graph(
        root,
        "script",
        &base,
        graph_signature(&toolchain, "ci"),
        vec![(
            "sample-core",
            "sample-core",
            "tests::changed",
            vec![("core/src/lib.rs", vec![(2, 1)])],
        )],
    );
    let options = {
        let base = base.clone();
        move || PlanOptions {
            coverage_graph: Some(graph_source(graph.clone(), &base)),
            ..Default::default()
        }
    };
    let mut plan = make_plan(&comparison, &options()).unwrap();
    assert_eq!(plan.tests.len(), 1);
    assert_eq!(plan.tests[0].name, "tests::changed");
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
    // Plans saved by older planners lack the coverage graph outcome and may
    // still carry the removed analysis fields; the consumer tolerates both.
    let legacy = legacy.as_object_mut().unwrap();
    legacy.remove("coverage_graph");
    legacy.insert("analysis_status".into(), json!("analyzed"));
    legacy.insert("analysis_ms".into(), json!(1));
    legacy.insert("observed_relationships".into(), json!(0));
    fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
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
        &PlanOptions {
            inventory: Some(inventory.clone()),
            ..options()
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
            &PlanOptions {
                inventory: Some(inventory.clone()),
                ..options()
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
    let plan = make_plan(&comparison, &options()).unwrap();
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
fn rejects_partial_inventories() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    let inventory = root.join("target/partial.json");
    fs::write(
        &inventory,
        json!({"head":head,"configuration":plan.configuration,"toolchain":plan.toolchain,
        "build_environment":plan.build_environment,"inventory":{"rust-suites":{}}})
        .to_string(),
    )
    .unwrap();
    let options = PlanOptions {
        inventory: Some(inventory),
        ..Default::default()
    };
    assert!(make_plan(&comparison, &options).is_err());
}

#[test]
fn rejects_workspaces_without_packages() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = []\nresolver = \"3\"\n",
    );
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let error = make_plan(&comparison, &PlanOptions::default()).unwrap_err();
    assert!(error.to_string().contains("workspace contains no packages"));
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
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    assert!(git::compare(root, &base, &head, Some(Path::new("relative"))).is_err());
    let manifest = fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
    write(
        root,
        "core/Cargo.toml",
        &(manifest + "\n[features]\nselected = []\n"),
    );
    let next = commit(root);
    let comparison = compare(root, &head, &next);
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.fallback_reasons.is_empty());
}

#[test]
fn deleted_sources_follow_base_package_ownership_after_a_move() {
    let (directory, _) = fixture();
    let root = directory.path();
    write(
        root,
        "legacy/Cargo.toml",
        "[package]\nname = \"sample-legacy\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "legacy/src/lib.rs", "pub fn legacy() -> u32 { 1 }\n");
    let manifest =
        "[workspace]\nmembers = [\"core\", \"app\", \"other\", \"legacy\"]\nresolver = \"3\"\n";
    write(root, "Cargo.toml", manifest);
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let base = commit(root);
    git::command(root, "git", &["mv", "legacy", "moved"]).unwrap();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"core\", \"app\", \"other\", \"moved\"]\nresolver = \"3\"\n",
    );
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    assert!(
        comparison
            .changes
            .iter()
            .any(|change| { change.path == "legacy/src/lib.rs" && change.deleted })
    );
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.packages, ["sample-legacy"]);
    assert!(plan.fallback_reasons.is_empty());
}

#[test]
fn removed_package_sources_defer_without_forcing_workspace_fallback() {
    let (directory, _) = fixture();
    let root = directory.path();
    write(
        root,
        "legacy/Cargo.toml",
        "[package]\nname = \"sample-legacy\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "legacy/src/lib.rs", "pub fn legacy() -> u32 { 1 }\n");
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"core\", \"app\", \"other\", \"legacy\"]\nresolver = \"3\"\n",
    );
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let base = commit(root);
    fs::remove_dir_all(root.join("legacy")).unwrap();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"core\", \"app\", \"other\"]\nresolver = \"3\"\n",
    );
    git::command(root, "cargo", &["generate-lockfile", "--offline"]).unwrap();
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    assert!(
        comparison
            .changes
            .iter()
            .any(|change| { change.path == "legacy/src/lib.rs" && change.deleted })
    );
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert!(plan.packages.is_empty());
    assert!(plan.tests.is_empty());
    assert!(plan.fallback_reasons.is_empty());
    for path in [
        "Cargo.lock",
        "Cargo.toml",
        "legacy/Cargo.toml",
        "legacy/src/lib.rs",
    ] {
        assert!(
            plan.ignored_files.iter().any(|ignored| ignored == path),
            "{path}"
        );
    }
}

#[test]
fn records_coverage_graph_provenance_and_degradation_reasons() {
    use mfn_test_affected::{CoverageGraphInput, GraphSource};
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let toolchain = git::command(root, "rustc", &["-vV"]).unwrap();
    // Line 2 is executable but outside the changed range, so the graph stays
    // compatible without narrowing the selection.
    let inert = |name: &str, commit: &str, signature: String| {
        write_graph(
            root,
            name,
            commit,
            signature,
            vec![(
                "sample-core",
                "sample-core",
                "tests::stable",
                vec![("core/src/lib.rs", vec![(2, 1)])],
            )],
        )
    };
    let source = |path: PathBuf| {
        CoverageGraphInput::Resolved(GraphSource {
            path,
            commit: base.clone(),
            run_id: "7".into(),
            attempt: 3,
            profile: "ci".into(),
        })
    };
    let options = |graph| PlanOptions {
        coverage_graph: Some(graph),
        ..Default::default()
    };

    // A compatible graph records its provenance; uncovered changed ranges
    // widen to package scope with a recorded reason.
    let plan = make_plan(
        &comparison,
        &options(source(inert(
            "compatible",
            &base,
            graph_signature(&toolchain, "ci"),
        ))),
    )
    .unwrap();
    let graph = plan.coverage_graph.unwrap();
    assert_eq!(graph.commit.as_deref(), Some(base.as_str()));
    assert_eq!(graph.run_id.as_deref(), Some("7"));
    assert_eq!(graph.attempt, Some(3));
    assert_eq!(graph.schema_version, Some(1));
    assert!(graph.reason.is_none());
    assert_eq!(graph.widened.len(), 1);
    assert_eq!(graph.widened[0].file, "core/src/lib.rs");
    assert_eq!(graph.widened[0].package, "sample-core");
    assert!(graph.widened[0].reason.contains("no test covers"));
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);

    // Every incompatibility degrades with a recorded reason, never an error.
    for (graph, fragment) in [
        (source(root.join("target/absent.sqlite")), "unreadable"),
        (
            // The descriptor names a commit the graph never indexed.
            source(inert("foreign", &head, graph_signature(&toolchain, "ci"))),
            "no record for the resolved commit",
        ),
        (
            CoverageGraphInput::Resolved(GraphSource {
                run_id: "8".into(),
                ..match source(inert("bound", &base, graph_signature(&toolchain, "ci"))) {
                    CoverageGraphInput::Resolved(source) => source,
                    _ => unreachable!(),
                }
            }),
            "does not match the run",
        ),
        (
            source(inert(
                "other-profile",
                &base,
                graph_signature(&toolchain, "other"),
            )),
            "execution signature differs",
        ),
    ] {
        let plan = make_plan(&comparison, &options(graph)).unwrap();
        let graph = plan.coverage_graph.unwrap();
        assert!(graph.commit.is_none(), "{fragment}");
        assert!(
            graph.reason.as_deref().unwrap().contains(fragment),
            "{fragment}: {graph:?}"
        );
        assert_eq!(plan.packages, ["sample-app", "sample-core"], "{fragment}");
    }

    // The acquisition side can report unavailability directly.
    let plan = make_plan(
        &comparison,
        &options(CoverageGraphInput::Unavailable {
            reason: "no eligible ancestor".into(),
        }),
    )
    .unwrap();
    assert_eq!(
        plan.coverage_graph.unwrap().reason.as_deref(),
        Some("no eligible ancestor")
    );

    // A graph produced on another host with the same toolchain release and
    // commit is compatible: the host line never participates.
    let other_host = toolchain
        .lines()
        .map(|line| {
            if line.starts_with("host: ") {
                "host: x86_64-unknown-linux-gnu".into()
            } else {
                line.into()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
        + "\n";
    let plan = make_plan(
        &comparison,
        &options(source(inert(
            "other-host",
            &base,
            graph_signature(&other_host, "ci"),
        ))),
    )
    .unwrap();
    let graph = plan.coverage_graph.unwrap();
    assert_eq!(graph.commit.as_deref(), Some(base.as_str()));
    assert!(graph.reason.is_none(), "{graph:?}");

    // A different toolchain release or commit still rejects the graph.
    for (marker, replacement) in [
        ("release: ", "release: 0.0.0"),
        ("commit-hash: ", "commit-hash: 0"),
    ] {
        let mismatched = toolchain
            .lines()
            .map(|line| {
                if line.starts_with(marker) {
                    replacement.into()
                } else {
                    line.into()
                }
            })
            .collect::<Vec<String>>()
            .join("\n")
            + "\n";
        let plan = make_plan(
            &comparison,
            &options(source(inert(
                &format!("mismatch-{marker}"),
                &base,
                graph_signature(&mismatched, "ci"),
            ))),
        )
        .unwrap();
        let graph = plan.coverage_graph.unwrap();
        assert!(graph.commit.is_none(), "{marker}");
        assert!(
            graph
                .reason
                .as_deref()
                .unwrap()
                .contains("execution signature differs"),
            "{marker}: {graph:?}"
        );
    }
    // Without a descriptor the plan carries no graph outcome at all.
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert!(plan.coverage_graph.is_none());
}

#[test]
fn narrows_package_scope_through_coverage_graph_with_conservative_widening() {
    let (directory, base) = fixture();
    let root = directory.path();
    let toolchain = git::command(root, "rustc", &["-vV"]).unwrap();
    let signature = graph_signature(&toolchain, "ci");

    // A fully covered production change narrows to exact selectors, including
    // covering tests owned by another package.
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let graph = write_graph(
        root,
        "covering",
        &base,
        signature.clone(),
        vec![
            (
                "sample-core",
                "sample-core",
                "tests::stable",
                vec![("core/src/lib.rs", vec![(1, 1)])],
            ),
            (
                "sample-app",
                "sample-app::integration",
                "hits_core",
                vec![("core/src/lib.rs", vec![(1, 2)])],
            ),
        ],
    );
    let plan = make_plan(&comparison, &graph_options(graph_source(graph, &base))).unwrap();
    assert!(plan.packages.is_empty());
    assert_eq!(
        plan.tests
            .iter()
            .map(|test| (
                test.package.as_str(),
                test.binary.as_str(),
                test.name.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("sample-app", "sample-app::integration", "hits_core"),
            ("sample-core", "sample-core", "tests::stable"),
        ]
    );
    // Both the owning and the covering packages stay in the recorded baseline.
    assert_eq!(plan.baseline_packages, ["sample-app", "sample-core"]);
    let provenance = plan.coverage_graph.unwrap();
    assert_eq!(provenance.commit.as_deref(), Some(base.as_str()));
    assert!(provenance.widened.is_empty());
    assert!(
        plan.reasons["sample-core"]
            .iter()
            .any(|reason| reason.contains("mapped core/src/lib.rs to 2 exact tests"))
    );
    assert!(
        plan.reasons["sample-app"]
            .iter()
            .any(|reason| reason.contains("covers changed lines in core/src/lib.rs"))
    );

    // A file changed between the graph commit and the comparison base widens
    // to package scope: the graph predates it.
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 3 }"),
    );
    let newer = commit(root);
    let comparison = compare(root, &head, &newer);
    let graph = write_graph(
        root,
        "lagging",
        &base,
        signature.clone(),
        vec![(
            "sample-core",
            "sample-core",
            "tests::stable",
            vec![("core/src/lib.rs", vec![(1, 1)])],
        )],
    );
    let plan = make_plan(&comparison, &graph_options(graph_source(graph, &base))).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    let provenance = plan.coverage_graph.unwrap();
    assert_eq!(provenance.widened.len(), 1);
    assert!(
        provenance.widened[0]
            .reason
            .contains("changed between the coverage graph commit and the comparison base")
    );

    // A file absent from the graph widens to package scope.
    let graph = write_graph(
        root,
        "absent-file",
        &head,
        signature.clone(),
        vec![(
            "sample-core",
            "sample-core",
            "tests::stable",
            vec![("core/src/other.rs", vec![(1, 1)])],
        )],
    );
    let plan = make_plan(&comparison, &graph_options(graph_source(graph, &head))).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    let provenance = plan.coverage_graph.unwrap();
    assert_eq!(provenance.widened.len(), 1);
    assert!(
        provenance.widened[0]
            .reason
            .contains("absent from the coverage graph")
    );

    // Mixed files in one package: the resolved file contributes nothing once
    // its sibling widens the package and its reverse dependencies.
    write(
        root,
        "core/src/extra.rs",
        "pub fn extra() -> u32 {\n    1\n}\n",
    );
    let with_extra = commit(root);
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 4 }"),
    );
    write(
        root,
        "core/src/extra.rs",
        "pub fn extra() -> u32 {\n    2\n}\n",
    );
    let mixed = commit(root);
    let comparison = compare(root, &with_extra, &mixed);
    let graph = write_graph(
        root,
        "mixed",
        &with_extra,
        signature.clone(),
        vec![(
            "sample-core",
            "sample-core",
            "tests::stable",
            vec![
                ("core/src/lib.rs", vec![(1, 1)]),
                // extra.rs is known but its changed line 2 is not covered.
                ("core/src/extra.rs", vec![(1, 1)]),
            ],
        )],
    );
    let plan = make_plan(
        &comparison,
        &graph_options(graph_source(graph, &with_extra)),
    )
    .unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    let provenance = plan.coverage_graph.unwrap();
    assert_eq!(provenance.widened.len(), 1);
    assert_eq!(provenance.widened[0].file, "core/src/extra.rs");
    assert!(provenance.widened[0].reason.contains("no test covers"));

    // Test-only source changes refine independently of coverage history,
    // even when a compatible graph contains no corresponding file.
    write(root, "app/src/lib.rs", &SOURCE.replace("1, 1", "9, 9"));
    let test_body = commit(root);
    let comparison = compare(root, &mixed, &test_body);
    let graph = write_graph(root, "test-body", &mixed, signature.clone(), vec![]);
    let plan = make_plan(&comparison, &graph_options(graph_source(graph, &mixed))).unwrap();
    assert_eq!(plan.tests.len(), 1);
    assert_eq!(plan.tests[0].name, "tests::changed");
    assert!(plan.packages.is_empty());
    let provenance = plan.coverage_graph.unwrap();
    assert!(provenance.widened.is_empty());

    // Unknown inputs keep the full-workspace fallback and clear graph
    // selectors; the graph provenance remains recorded.
    let mut comparison = compare(root, &head, &newer);
    comparison.changes.push(mfn_test_affected::Change {
        path: "scripts/unknown.sh".into(),
        deleted: false,
        before_lines: vec![],
    });
    let graph = write_graph(
        root,
        "fallback",
        &head,
        signature.clone(),
        vec![(
            "sample-core",
            "sample-core",
            "tests::stable",
            vec![("core/src/lib.rs", vec![(1, 1)])],
        )],
    );
    let plan = make_plan(
        &comparison,
        &graph_options(graph_source(graph.clone(), &head)),
    )
    .unwrap();
    assert_eq!(plan.packages.len(), 3);
    assert!(plan.tests.is_empty());
    assert!(plan.coverage_graph.unwrap().commit.is_some());
    assert!(!plan.fallback_reasons.is_empty());

    // Empty selections stay empty with the graph provenance recorded.
    let empty_graph = write_graph(root, "empty", &newer, signature.clone(), vec![]);
    let comparison = compare(root, &newer, &newer);
    let plan = make_plan(
        &comparison,
        &graph_options(graph_source(empty_graph, &newer)),
    )
    .unwrap();
    assert!(plan.packages.is_empty() && plan.tests.is_empty());
    assert!(plan.coverage_graph.unwrap().commit.is_some());

    // Package precision records why the descriptor was not consumed.
    let comparison = compare(root, &head, &newer);
    let mut options = graph_options(graph_source(graph, &head));
    options.exact_tests = false;
    let plan = make_plan(&comparison, &options).unwrap();
    assert_eq!(
        plan.coverage_graph.unwrap().reason.as_deref(),
        Some("exact test selection is disabled for this plan")
    );
}

#[test]
fn graph_derived_selectors_execute_through_the_plan_consumer() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(
        root,
        "core/src/lib.rs",
        &SOURCE.replace("-> u32 { 1 }", "-> u32 { 2 }"),
    );
    let head = commit(root);
    let comparison = compare(root, &base, &head);
    let toolchain = git::command(root, "rustc", &["-vV"]).unwrap();
    let graph = write_graph(
        root,
        "execute",
        &base,
        graph_signature(&toolchain, "ci"),
        vec![(
            "sample-core",
            "sample-core",
            "tests::stable",
            vec![("core/src/lib.rs", vec![(1, 1)])],
        )],
    );
    let plan = make_plan(&comparison, &graph_options(graph_source(graph, &base))).unwrap();
    assert_eq!(plan.tests.len(), 1);
    assert_eq!(plan.tests[0].name, "tests::stable");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../run-tests.py");
    let path = root.join("target/plan.json");
    let run = |plan: &mfn_test_affected::TestPlan, args: &[&str]| {
        fs::write(&path, serde_json::to_vec(plan).unwrap()).unwrap();
        Command::new("python3")
            .arg(&script)
            .arg(&path)
            .env("CARGO_TARGET_DIR", root.join("target/shared-build"))
            .args(args)
            .output()
            .unwrap()
    };
    // The graph-derived selector executes exactly the covering test.
    let output = run(&plan, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"selected_count\": 1"), "{stdout}");
    assert!(stdout.contains("\"executed\": true"), "{stdout}");
    // A stale graph identity widens to the recorded baseline with a diagnostic.
    let mut stale = plan.clone();
    stale.tests[0].name = "no_longer_listed".into();
    let output = run(&stale, &["--list-only"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("exact identities are absent from the runnable head inventory"),
        "{stdout}"
    );
    assert!(stdout.contains("\"selected_count\": 4"), "{stdout}");
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
    let plan_input = || mfn_test_affected::PlanInput {
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
    context.cancellation().cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancel fixture",
        }
        .build(),
    );
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
            deleted: path == "removed/src/lib.rs",
            before_lines: vec![],
        }];
        let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
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
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
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
        ComparisonMode::Snapshot,
    )
    .unwrap();
    assert_eq!(comparison.base, first_merge.trim());
    assert_eq!(comparison.changes.len(), 1);
    assert_eq!(comparison.changes[0].path, "app/src/lib.rs");
    let plan = make_plan(&comparison, &PlanOptions::default()).unwrap();
    assert_eq!(plan.comparison_mode, ComparisonMode::Snapshot);
    assert_eq!(plan.packages, ["sample-app"]);
    let cumulative = compare(root, first_merge.trim(), second_merge.trim());
    assert_ne!(cumulative.base, comparison.base);
    assert_eq!(cumulative.changes.len(), 2);
    let broader = make_plan(&cumulative, &PlanOptions::default()).unwrap();
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
        json!({"mode":"commit","head":first,"options":{"exact_tests":false}}),
    )
    .unwrap();
    let first_comparison = git::compare_request(root, &first_request, None).unwrap();
    assert_eq!(first_comparison.comparison_mode, ComparisonMode::Commit);
    assert!(
        git::command(&first_comparison.base_root, "git", &["ls-tree", "HEAD"])
            .unwrap()
            .is_empty()
    );
    let first_plan = make_plan(&first_comparison, &first_request.options).unwrap();
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
    let request: ComparisonRequest =
        serde_json::from_value(json!({"mode":"worktree","options":{"exact_tests":false}})).unwrap();
    let comparison = git::compare_request(root, &request, None).unwrap();
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
    let repeated = git::compare_request(root, &request, None).unwrap();
    assert_eq!(repeated.head, comparison.head);
    let strict: ComparisonRequest = serde_json::from_value(
        json!({"mode":"snapshot","base":base,"head":base,"require_checkout":true}),
    )
    .unwrap();
    assert!(git::compare_request(root, &strict, None).is_err());
    let shallow = tempfile::tempdir().unwrap();
    let source = url::Url::from_directory_path(root).unwrap();
    git::command(
        shallow.path(),
        "git",
        &["clone", "--quiet", "--depth=1", source.as_str(), "."],
    )
    .unwrap();
    let commit_request: ComparisonRequest =
        serde_json::from_value(json!({"mode":"commit"})).unwrap();
    assert!(git::compare_request(shallow.path(), &commit_request, None).is_err());
}

#[test]
fn normalized_requests_retain_provenance_and_package_only_scope() {
    use mf_runtime::{ExecutionContext, TypedTaskNode};
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let request = mf_runtime::ValueRef::from(json!({"mode":"snapshot","base":base,"head":head,"require_checkout":true,"options":{"exact_tests":false},"validation":{"run":42},"execution":{"enabled":true,"opaque":{"key":42}}})).into_secret();
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
    let output = mfn_test_affected::PlanNode
        .execute(
            mfn_test_affected::PlanInput {
                context: analysis_input.context,
            },
            &mut ExecutionContext::default(),
        )
        .unwrap()
        .outputs;
    assert!(
        output.plan.is_secret() && output.validation.is_secret() && output.execution.is_secret()
    );
    assert_eq!(
        output.execution,
        json!({"enabled":true,"opaque":{"key":42}})
    );
    assert_eq!(output.validation, json!({"run":42}));
    let plan: mfn_test_affected::TestPlan =
        serde_json::from_value(serde_json::to_value(output.plan).unwrap()).unwrap();
    assert_eq!(plan.packages, ["sample-app", "sample-core"]);
    assert!(plan.tests.is_empty());
    let plan_file = root.join("target/package-plan.json");
    fs::write(
        &plan_file,
        json!({"plan":plan,"validation":{"run":42},"execution":{"executed":false}}).to_string(),
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
