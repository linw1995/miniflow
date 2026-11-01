use crate::{
    error::*,
    git::{command, verify_snapshot},
    graph,
    model::*,
};
use serde::Deserialize;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}
#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}
#[derive(Deserialize)]
struct Dependency {
    path: Option<PathBuf>,
}
#[derive(Deserialize)]
struct Target {
    test: bool,
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

fn workspace_packages(root: &Path) -> Result<BTreeMap<String, Package>, Error> {
    let metadata: Metadata = serde_json::from_str(&command(
        root,
        "cargo",
        &[
            "metadata",
            "--no-deps",
            "--locked",
            "--all-features",
            "--format-version",
            "1",
        ],
    )?)
    .context(JsonSnafu {
        action: "read Cargo workspace metadata",
    })?;
    let members: BTreeSet<_> = metadata.workspace_members.into_iter().collect();
    Ok(metadata
        .packages
        .into_iter()
        .filter(|p| members.contains(&p.id))
        .map(|p| (p.name.clone(), p))
        .collect())
}

fn package_roots(packages: &BTreeMap<String, Package>) -> BTreeMap<PathBuf, String> {
    packages
        .values()
        .map(|p| {
            (
                p.manifest_path.parent().unwrap().to_path_buf(),
                p.name.clone(),
            )
        })
        .collect()
}

fn owning_package<'a>(roots: &'a BTreeMap<PathBuf, String>, path: &Path) -> Option<&'a String> {
    roots
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, name)| name)
}

pub fn make_plan(comparison: &Comparison, options: &PlanOptions) -> Result<TestPlan, Error> {
    let started = Instant::now();
    verify_snapshot(&comparison.base_root, &comparison.base)?;
    verify_snapshot(&comparison.head_root, &comparison.head)?;
    let packages = workspace_packages(&comparison.head_root)?;
    ensure!(
        !packages.is_empty(),
        InvalidSnafu {
            message: "workspace contains no packages"
        }
    );
    let roots = package_roots(&packages);
    let dependencies: BTreeMap<String, BTreeSet<String>> = packages
        .values()
        .map(|p| {
            let deps = p
                .dependencies
                .iter()
                .filter_map(|d| d.path.as_ref().and_then(|path| roots.get(path)))
                .filter(|name| **name != p.name)
                .cloned()
                .collect();
            (p.name.clone(), deps)
        })
        .collect();
    let tree = command(&comparison.head_root, "git", &["rev-parse", "HEAD^{tree}"])?
        .trim()
        .into();
    let toolchain = command(&comparison.head_root, "rustc", &["-vV"])?;
    let mut graph = resolve_coverage_graph(comparison, options, &toolchain);
    let mut reasons = BTreeMap::<String, Vec<String>>::new();
    let mut direct = BTreeSet::new();
    let mut production = BTreeSet::new();
    let mut tests = BTreeSet::new();
    let mut fallback = Vec::new();
    let mut ignored_files = Vec::new();
    let mut base_roots = None;
    for change in &comparison.changes {
        let path = Path::new(&change.path);
        let absolute = comparison.head_root.join(path);
        let base_owner = if change.deleted {
            let base_roots = match base_roots.as_ref() {
                Some(base_roots) => base_roots,
                None => {
                    base_roots.insert(package_roots(&workspace_packages(&comparison.base_root)?))
                }
            };
            owning_package(base_roots, &comparison.base_root.join(path))
        } else {
            None
        };
        let owner = base_owner.or_else(|| owning_package(&roots, &absolute));
        if let Some(owner) = owner {
            if !packages.contains_key(owner) {
                // The base owner disappeared from the compared head, so the
                // full main/nightly runs provide the backstop.
                ignored_files.push(change.path.clone());
                continue;
            }
            let owner = owner.clone();
            direct.insert(owner.clone());
            reasons.entry(owner.clone()).or_default().push(format!(
                "{}: {}",
                if change.deleted {
                    "Deleted file"
                } else {
                    "Changed file"
                },
                change.path
            ));
            let source_tests = options.exact_tests.then(|| {
                crate::source_tests::changed(
                    comparison,
                    change,
                    &owner,
                    packages[&owner]
                        .targets
                        .iter()
                        .filter(|target| target.test)
                        .map(|target| {
                            (
                                target.name.as_str(),
                                target.kind.as_slice(),
                                target.src_path.as_path(),
                            )
                        }),
                )
            });
            match &source_tests {
                Some(Ok(Some(changes))) if changes.exclusive => {
                    reasons.entry(owner.clone()).or_default().push(format!(
                        "Test-only source change: mapped {} to {} exact tests without coverage history",
                        change.path,
                        changes.cases.len()
                    ));
                    tests.extend(changes.cases.iter().cloned());
                }
                Some(Err(())) => {
                    // Changed test identities are unproven. Retain package
                    // scope so the new or modified tests still execute.
                    production.insert(owner.clone());
                    reasons.entry(owner.clone()).or_default().push(format!(
                        "Test source change: retained package scope for {} because exact test identities are unproven",
                        change.path
                    ));
                }
                _ if options.exact_tests
                    && change.path.ends_with(".rs")
                    && graph.open.is_some() =>
                {
                    if let Some(Ok(Some(changes))) = &source_tests {
                        reasons.entry(owner.clone()).or_default().push(format!(
                            "Test source change: mapped {} to {} exact tests alongside coverage narrowing",
                            change.path,
                            changes.cases.len()
                        ));
                        tests.extend(changes.cases.iter().cloned());
                    }
                    let open = graph.open.as_mut().unwrap();
                    // The graph narrows one file at a time; unresolved files
                    // keep the existing package scope with a recorded reason.
                    match graph_select(open, change) {
                        Ok(selected) => {
                            reasons.entry(owner.clone()).or_default().push(format!(
                                "Coverage graph {}: mapped {} to {} exact tests",
                                open.record.commit_sha,
                                change.path,
                                selected.len()
                            ));
                            for test in &selected {
                                if direct.insert(test.package.clone()) {
                                    reasons
                                        .entry(test.package.clone())
                                        .or_default()
                                        .push(format!(
                                            "Coverage graph {}: covers changed lines in {}",
                                            open.record.commit_sha, change.path
                                        ));
                                }
                            }
                            tests.extend(selected);
                        }
                        Err(reason) => {
                            production.insert(owner.clone());
                            reasons.entry(owner.clone()).or_default().push(format!(
                                "Coverage graph {}: retained package scope for {}: {}",
                                open.record.commit_sha, change.path, reason
                            ));
                            if let Some(plan) = graph.plan.as_mut() {
                                plan.widened.push(GraphWidening {
                                    file: change.path.clone(),
                                    package: owner.clone(),
                                    reason,
                                });
                            }
                        }
                    }
                }
                _ => {
                    production.insert(owner.clone());
                }
            }
        } else if deferred_input(&change.path) {
            ignored_files.push(change.path.clone());
        } else {
            let consumers: Vec<_> = shared_consumers(&change.path)
                .iter()
                .filter(|name| packages.contains_key(**name))
                .collect();
            if consumers.is_empty() {
                fallback.push(format!(
                    "Shared or unclassified input changed: {}",
                    change.path
                ));
            }
            for owner in consumers {
                direct.insert((*owner).to_string());
                production.insert((*owner).to_string());
                reasons
                    .entry((*owner).to_string())
                    .or_default()
                    .push(format!("Consumes shared input: {}", change.path));
            }
        }
    }
    let baseline = closure(&direct, &dependencies);
    let (baseline, selected) = if fallback.is_empty() {
        (baseline, closure(&production, &dependencies))
    } else {
        tests.clear();
        let all: BTreeSet<_> = packages.keys().cloned().collect();
        (all.clone(), all)
    };
    tests.retain(|test| !selected.contains(&test.package));
    for package in &selected {
        reasons
            .entry(package.clone())
            .or_default()
            .push(if direct.contains(package) {
                "Retained production or unproved scope".into()
            } else {
                "Depends on a changed production package or is covered by fallback".into()
            });
    }
    let mut plan = TestPlan {
        comparison_mode: comparison.comparison_mode,
        repository: comparison.repository.clone(),
        base: comparison.base.clone(),
        head: comparison.head.clone(),
        snapshot: comparison.head_root.clone(),
        tree,
        configuration: options.configuration.clone(),
        toolchain,
        build_environment: build_environment()?,
        baseline_packages: baseline.into_iter().collect(),
        packages: selected.into_iter().collect(),
        tests: tests.into_iter().collect(),
        reasons,
        fallback_reasons: fallback,
        ignored_files,
        baseline_count: None,
        selected_count: None,
        planning_ms: 0,
        coverage_graph: graph.plan,
    };
    if let Some(path) = &options.inventory {
        apply_inventory(&mut plan, path, &packages)?;
    }
    plan.planning_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(plan)
}

/// An opened, validated coverage graph ready for changed-range queries.
struct OpenGraph {
    reader: graph::GraphReader,
    record: graph::Record,
    /// Files that changed between the graph commit and the comparison base;
    /// empty when the graph commit is the base itself.
    lagging: BTreeSet<String>,
}

struct GraphResolution {
    open: Option<OpenGraph>,
    plan: Option<GraphPlan>,
}

/// Validates the supplied coverage graph descriptor against the graph itself
/// and the current execution. Every failure degrades to a recorded reason;
/// planning never fails because of the graph.
fn resolve_coverage_graph(
    comparison: &Comparison,
    options: &PlanOptions,
    toolchain: &str,
) -> GraphResolution {
    let Some(input) = &options.coverage_graph else {
        return GraphResolution {
            open: None,
            plan: None,
        };
    };
    // Acquisition-side unavailability is preserved as-is, whatever the plan
    // precision.
    let source = match input {
        CoverageGraphInput::Unavailable { reason } => {
            return GraphResolution {
                open: None,
                plan: Some(GraphPlan {
                    reason: Some(reason.clone()),
                    ..Default::default()
                }),
            };
        }
        CoverageGraphInput::Resolved(source) => source,
    };
    if !options.exact_tests {
        return GraphResolution {
            open: None,
            plan: Some(GraphPlan {
                reason: Some("exact test selection is disabled for this plan".into()),
                ..Default::default()
            }),
        };
    }
    match open_coverage_graph(comparison, options, source, toolchain) {
        Ok(open) => GraphResolution {
            plan: Some(GraphPlan {
                commit: Some(open.record.commit_sha.clone()),
                run_id: Some(open.record.run_id.clone()),
                attempt: u32::try_from(open.record.attempt).ok(),
                schema_version: Some(open.record.schema_version),
                ..Default::default()
            }),
            open: Some(open),
        },
        Err(reason) => GraphResolution {
            open: None,
            plan: Some(GraphPlan {
                reason: Some(reason),
                ..Default::default()
            }),
        },
    }
}

fn open_coverage_graph(
    comparison: &Comparison,
    options: &PlanOptions,
    source: &GraphSource,
    toolchain: &str,
) -> Result<OpenGraph, String> {
    let mut reader = graph::GraphReader::open(&source.path)
        .map_err(|error| format!("coverage graph is unreadable: {error}"))?;
    let record = reader
        .record(&source.commit)
        .map_err(|error| format!("coverage graph record is unreadable: {error}"))?
        .ok_or_else(|| {
            format!(
                "coverage graph has no record for the resolved commit {}",
                source.commit
            )
        })?;
    if record.schema_version != graph::SCHEMA_VERSION {
        return Err(format!(
            "coverage graph schema version {} is not supported version {}",
            record.schema_version,
            graph::SCHEMA_VERSION
        ));
    }
    if record.run_id != source.run_id || record.attempt != i64::from(source.attempt) {
        return Err("coverage graph identity does not match the run it was acquired from".into());
    }
    let signature: Value = serde_json::from_str(&record.signature)
        .map_err(|_| "coverage graph execution signature is malformed".to_string())?;
    let expected = execution_signature(&options.configuration, &source.profile, toolchain)
        .map_err(|error| format!("cannot bind the current execution signature: {error}"))?;
    if signature != expected {
        return Err("coverage graph execution signature differs from the current execution".into());
    }
    let lagging = if source.commit == comparison.base {
        BTreeSet::new()
    } else {
        command(
            &comparison.repository,
            "git",
            &[
                "diff",
                "--no-renames",
                "--name-only",
                "-z",
                &source.commit,
                &comparison.base,
                "--",
            ],
        )
        .map_err(|error| {
            format!("cannot compare the coverage graph commit to the comparison base: {error}")
        })?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
    };
    Ok(OpenGraph {
        reader,
        record,
        lagging,
    })
}

/// Maps one changed file to the distinct tests covering its base-side line
/// ranges. Unresolved files widen to package scope with the returned reason;
/// query failures widen the same way, never failing the plan.
fn graph_select(graph: &mut OpenGraph, change: &Change) -> Result<Vec<TestCase>, String> {
    if graph.lagging.contains(&change.path) {
        return Err(
            "the file changed between the coverage graph commit and the comparison base".into(),
        );
    }
    let known = graph
        .reader
        .has_file(&graph.record.commit_sha, &change.path)
        .map_err(|error| format!("coverage graph file query failed: {error}"))?;
    if !known {
        return Err("the file is absent from the coverage graph".into());
    }
    let selected = graph
        .reader
        .tests_covering(&graph.record.commit_sha, &change.path, &change.before_lines)
        .map_err(|error| format!("coverage graph range query failed: {error}"))?;
    if selected.is_empty() {
        return Err("no test covers the changed ranges".into());
    }
    Ok(selected)
}

/// The normalized execution signature coverage graphs are bound to; aligned
/// with the coverage identity recorded by `scripts/run-cov.sh`.
fn execution_signature(
    configuration: &BuildConfiguration,
    profile: &str,
    toolchain: &str,
) -> Result<Value, String> {
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
            .map(|value| (name.to_owned(), Value::from(value)))
    })
    .collect();
    Ok(json!({
        "environment": environment,
        "features": if configuration.all_features {
            Value::from("all")
        } else {
            json!(configuration.features)
        },
        "profile": profile,
        "target": configuration.target,
        "toolchain": toolchain_identity(toolchain)?,
    }))
}

/// The platform-independent toolchain identity bound into graph signatures.
/// Line coverage attributes source lines to tests regardless of the host
/// architecture, so only the release and commit hash participate; platform
/// differences stay sound through the consumers' inventory widen-on-mismatch
/// (over-selection is harmless, missed selection is impossible). The full
/// `rustc -vV` text is retained elsewhere for exact environment reproduction.
fn toolchain_identity(verbose: &str) -> Result<String, String> {
    let field = |name: &str| {
        verbose
            .lines()
            .find_map(|line| line.strip_prefix(name).map(str::trim))
            .filter(|value| !value.is_empty())
    };
    match (field("release:"), field("commit-hash:")) {
        (Some(release), Some(commit_hash)) => Ok(format!("rustc {release} ({commit_hash})")),
        _ => Err("rustc -vV output lacks a release or commit-hash identity".into()),
    }
}

fn deferred_input(path: &str) -> bool {
    // Full main/nightly runs cover repository-wide configuration changes.
    path.starts_with("docs/")
        || path.starts_with("openspec/")
        || path.starts_with(".github/")
        || matches!(
            path,
            "Cargo.toml"
                | "Cargo.lock"
                | "flake.nix"
                | "flake.lock"
                | "rust-toolchain"
                | "rust-toolchain.toml"
                | ".cargo/config"
                | ".cargo/config.toml"
                | ".config/nextest.toml"
                | ".gitignore"
                | ".gitattributes"
                | ".editorconfig"
                | ".pre-commit-config.yaml"
                | ".markdownlint.yaml"
                | "rustfmt.toml"
                | "codecov.yml"
                | "LICENSE"
                | "about.toml"
                | "about.hbs"
                | "README.md"
                | "CONTRIBUTING.md"
                | "AGENTS.md"
                | "CHANGELOG.md"
                | "tools/test-affected/README.md"
                | "tools/test-affected/test_runner.py"
                | "scripts/pr_validation.py"
                | "scripts/test_pr_validation.py"
                | "scripts/ci_monitor.py"
                | "scripts/test_ci_monitor.py"
                | "scripts/nightly_tests.py"
                | "scripts/test_nightly_tests.py"
                | "scripts/check_semantic_title.py"
                | "scripts/run-cov.sh"
                | "scripts/test_coverage.py"
                | "scripts/coverage_selection.py"
                | "scripts/test_coverage_selection.py"
                | "scripts/prune-test-cache.py"
                | "scripts/test_prune_test_cache.py"
                | "scripts/generate-third-party-notices.sh"
                | "scripts/check-third-party-notices.sh"
                | "scripts/test_tool_boundaries.py"
                | "scripts/test_affected_tests.py"
                | "scripts/test_cargo_cache.py"
                | "scripts/test_nextest_build_cache.py"
        )
}

fn shared_consumers(path: &str) -> &'static [&'static str] {
    if path.starts_with("examples/") {
        return &["mf-cli", "mf-compiler"];
    }
    if path.starts_with("tools/test-affected/flows/") {
        return &["mfn-test-affected"];
    }
    match path {
        "scripts/nextest-build-cache.py" => &["mf-cli", "mf-compiler"],
        "scripts/nextest-cargo.sh" => &["mf-cli", "mf-compiler"],
        "scripts/cargo_cache.py" => &["mf-cli", "mf-compiler"],
        "scripts/check-release-support.sh" => &["mf-cli"],
        "scripts/affected_tests.py" => &["mfn-test-affected"],
        "tools/test-affected/run-tests.py" => &["mfn-test-affected"],
        _ => &[],
    }
}

fn closure(
    direct: &BTreeSet<String>,
    dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut selected = direct.clone();
    let mut queue: VecDeque<_> = direct.iter().cloned().collect();
    while let Some(dependency) = queue.pop_front() {
        for (package, deps) in dependencies {
            if deps.contains(&dependency) && selected.insert(package.clone()) {
                queue.push_back(package.clone());
            }
        }
    }
    selected
}

fn apply_inventory(
    plan: &mut TestPlan,
    path: &Path,
    packages: &BTreeMap<String, Package>,
) -> Result<(), Error> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).context(IoSnafu {
        action: "read test inventory",
    })?)
    .context(JsonSnafu {
        action: "decode test inventory",
    })?;
    ensure!(
        value["head"] == plan.head
            && value["configuration"]
                == serde_json::to_value(&plan.configuration).context(JsonSnafu {
                    action: "encode build configuration"
                })?
            && value["toolchain"] == plan.toolchain
            && value["build_environment"]
                == serde_json::to_value(&plan.build_environment).context(JsonSnafu {
                    action: "encode build environment"
                })?,
        InvalidSnafu {
            message: "inventory source/configuration identity mismatch"
        }
    );
    let Some(suites) = value
        .pointer("/inventory/rust-suites")
        .and_then(Value::as_object)
    else {
        return InvalidSnafu {
            message: "inventory has no test suites",
        }
        .fail();
    };
    let mut cases = BTreeSet::new();
    for (id, suite) in suites {
        if let (Some(package), Some(tests)) = (
            suite["package-name"].as_str(),
            suite["testcases"].as_object(),
        ) {
            for (name, test) in tests {
                if test.pointer("/filter-match/status").and_then(Value::as_str) == Some("matches") {
                    cases.insert(TestCase {
                        package: package.into(),
                        binary: id.clone(),
                        name: name.clone(),
                    });
                }
            }
        }
    }
    let available: BTreeSet<_> = suites
        .values()
        .filter_map(|s| s["package-name"].as_str())
        .collect();
    ensure!(
        plan.baseline_packages
            .iter()
            .all(|p| available.contains(p.as_str()) || !packages[p].targets.iter().any(|t| t.test)),
        InvalidSnafu {
            message: "inventory does not cover the complete baseline"
        }
    );
    plan.baseline_count = Some(
        cases
            .iter()
            .filter(|t| plan.baseline_packages.contains(&t.package))
            .count(),
    );
    if plan.tests.iter().any(|t| !cases.contains(t)) {
        plan.packages = plan.baseline_packages.clone();
        plan.tests.clear();
        plan.fallback_reasons
            .push("Exact test identity is not in the supplied runnable inventory".into());
    }
    plan.selected_count = Some(
        cases
            .iter()
            .filter(|t| plan.packages.contains(&t.package) || plan.tests.contains(t))
            .count(),
    );
    Ok(())
}

pub fn build_environment() -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
        "CARGO_INCREMENTAL",
    ] {
        if let Some(value) = std::env::var_os(name) {
            let Ok(value) = value.into_string() else {
                return InvalidSnafu {
                    message: format!("non-UTF-8 build environment: {name}"),
                }
                .fail();
            };
            values.insert(name.into(), value);
        }
    }
    Ok(values)
}
