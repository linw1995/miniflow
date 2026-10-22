use crate::{
    classify::{isolated_changes, plain_attributes},
    error::*,
    git::{command, verify_snapshot},
    graph,
    model::*,
};
use mfn_lsp::{AnalysisOutcome, GapKind};
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
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
    test: bool,
}

pub fn make_plan(
    comparison: &Comparison,
    outcome: &AnalysisOutcome,
    options: &PlanOptions,
) -> Result<TestPlan, Error> {
    let started = Instant::now();
    ensure!(
        outcome.request == comparison.request,
        InvalidSnafu {
            message: "analysis request differs from the Git comparison"
        }
    );
    ensure!(
        outcome
            .evidence
            .iter()
            .map(|s| &s.snapshot)
            .chain(outcome.gaps.iter().map(|g| &g.snapshot))
            .all(|id| comparison.request.snapshots.iter().any(|s| &s.id == id)),
        InvalidSnafu {
            message: "analysis evidence belongs to an unrequested snapshot"
        }
    );
    verify_snapshot(&comparison.base_root, &comparison.base)?;
    verify_snapshot(&comparison.head_root, &comparison.head)?;
    validate_configuration(outcome, &options.configuration)?;
    let metadata: Metadata = serde_json::from_str(&command(
        &comparison.head_root,
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
    let packages: BTreeMap<_, _> = metadata
        .packages
        .into_iter()
        .filter(|p| members.contains(&p.id))
        .map(|p| (p.name.clone(), p))
        .collect();
    ensure!(
        !packages.is_empty(),
        InvalidSnafu {
            message: "workspace contains no packages"
        }
    );
    let roots: BTreeMap<_, _> = packages
        .values()
        .map(|p| {
            (
                p.manifest_path.parent().unwrap().to_path_buf(),
                p.name.clone(),
            )
        })
        .collect();
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
    let graph = resolve_coverage_graph(options, &toolchain);
    let mut reasons = BTreeMap::<String, Vec<String>>::new();
    let mut direct = BTreeSet::new();
    let mut production = BTreeSet::new();
    let mut tests = BTreeSet::new();
    let mut fallback = Vec::new();
    let mut ignored_files = Vec::new();
    for change in &comparison.changes {
        let path = Path::new(&change.path);
        let absolute = comparison.head_root.join(path);
        let owner = roots
            .iter()
            .filter(|(root, _)| absolute.starts_with(root))
            .max_by_key(|(root, _)| root.components().count());
        if let Some((root, owner)) = owner {
            direct.insert(owner.clone());
            reasons
                .entry(owner.clone())
                .or_default()
                .push(format!("Changed file: {}", change.path));
            if options.exact_tests
                && let Some(selected) = isolated_tests(comparison, change, &packages[owner], root)
            {
                tests.extend(selected);
            } else {
                production.insert(owner.clone());
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
    for gap in outcome
        .gaps
        .iter()
        .filter(|g| matches!(g.kind, GapKind::AnalyzerFailure | GapKind::Limit))
    {
        fallback.push(format!(
            "Semantic analyzer {} failed for {}: {}; retaining the full workspace",
            gap.operation, gap.snapshot, gap.message
        ));
    }
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
    for test in &tests {
        reasons
            .entry(test.package.clone())
            .or_default()
            .push(format!(
                "Verified isolated test body: {}::{}",
                test.binary, test.name
            ));
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
        analysis_status: outcome.status.clone(),
        baseline_count: None,
        selected_count: None,
        observed_relationships: outcome
            .evidence
            .iter()
            .flat_map(|s| &s.documents)
            .flat_map(|d| &d.symbols)
            .map(|s| s.references.len() + s.incoming_calls.len())
            .sum(),
        analysis_ms: outcome.elapsed_ms,
        planning_ms: 0,
        coverage_graph: graph,
    };
    if let Some(path) = &options.inventory {
        apply_inventory(&mut plan, path, &packages)?;
    }
    plan.planning_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(plan)
}

/// Validates the supplied coverage graph descriptor against the graph itself
/// and the current execution. Every failure degrades to a recorded reason;
/// planning never fails because of the graph.
fn resolve_coverage_graph(options: &PlanOptions, toolchain: &str) -> Option<GraphPlan> {
    let input = options.coverage_graph.as_ref()?;
    let source = match input {
        CoverageGraphInput::Unavailable { reason } => {
            return Some(GraphPlan {
                reason: Some(reason.clone()),
                ..Default::default()
            });
        }
        CoverageGraphInput::Resolved(source) => source,
    };
    match validate_coverage_graph(options, source, toolchain) {
        Ok(record) => Some(GraphPlan {
            commit: Some(record.commit_sha),
            run_id: Some(record.run_id),
            attempt: u32::try_from(record.attempt).ok(),
            schema_version: Some(record.schema_version),
            ..Default::default()
        }),
        Err(reason) => Some(GraphPlan {
            reason: Some(reason),
            ..Default::default()
        }),
    }
}

fn validate_coverage_graph(
    options: &PlanOptions,
    source: &GraphSource,
    toolchain: &str,
) -> Result<graph::Record, String> {
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
    if signature != execution_signature(&options.configuration, &source.profile, toolchain) {
        return Err("coverage graph execution signature differs from the current execution".into());
    }
    Ok(record)
}

/// The normalized execution signature coverage graphs are bound to; aligned
/// with the coverage identity recorded by `scripts/run-cov.sh`.
fn execution_signature(
    configuration: &BuildConfiguration,
    profile: &str,
    toolchain: &str,
) -> Value {
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
    json!({
        "environment": environment,
        "features": if configuration.all_features {
            Value::from("all")
        } else {
            json!(configuration.features)
        },
        "profile": profile,
        "target": configuration.target,
        "toolchain": toolchain,
    })
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

fn validate_configuration(
    outcome: &AnalysisOutcome,
    config: &BuildConfiguration,
) -> Result<(), Error> {
    ensure!(
        !(config.all_features && (!config.features.is_empty() || config.no_default_features)),
        InvalidSnafu {
            message: "all_features cannot be combined with explicit/no-default features"
        }
    );
    if outcome.server.profile == mfn_lsp::Profile::RustAnalyzer
        && outcome.status != mfn_lsp::Status::NotRequested
    {
        let cargo = &outcome.request.initialization_options["cargo"];
        ensure!(
            cargo["target"].as_str() == config.target.as_deref(),
            InvalidSnafu {
                message: "LSP and test target configurations differ"
            }
        );
        if config.all_features {
            ensure!(
                cargo["features"] == "all",
                InvalidSnafu {
                    message: "LSP must analyze all selected features"
                }
            );
        } else {
            ensure!(
                cargo["features"].is_null() || cargo["features"].is_array(),
                InvalidSnafu {
                    message: "LSP and test feature configurations differ"
                }
            );
            let features: Vec<String> = cargo["features"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            ensure!(
                features == config.features
                    && cargo["noDefaultFeatures"].as_bool().unwrap_or(false)
                        == config.no_default_features,
                InvalidSnafu {
                    message: "LSP and test feature configurations differ"
                }
            );
        }
    }
    Ok(())
}

fn isolated_tests(
    comparison: &Comparison,
    change: &Change,
    package: &Package,
    root: &Path,
) -> Option<Vec<TestCase>> {
    if !change.path.ends_with(".rs") {
        return None;
    }
    let old = fs::read_to_string(comparison.base_root.join(&change.path)).ok()?;
    let new = fs::read_to_string(comparison.head_root.join(&change.path)).ok()?;
    let absolute = comparison.head_root.join(&change.path);
    let integration = package
        .targets
        .iter()
        .find(|t| t.kind.iter().any(|k| k == "test") && t.src_path == absolute);
    let changed = isolated_changes(&old, &new, integration.is_some())?;
    let (binary, prefix) = if let Some(target) = integration {
        (format!("{}::{}", package.name, target.name), String::new())
    } else {
        // Only conventional library module paths are narrowed; unusual #[path] modules retain baseline.
        let source_root = root.join("src");
        let relative = absolute.strip_prefix(&source_root).ok()?;
        package.targets.iter().find(|t| {
            t.kind.iter().any(|k| k == "lib") && t.src_path == source_root.join("lib.rs")
        })?;
        let mut components: Vec<_> = relative
            .components()
            .map(|c| c.as_os_str().to_str().map(str::to_owned))
            .collect::<Option<_>>()?;
        let filename = components.pop()?;
        let stem = filename.strip_suffix(".rs")?;
        if !matches!(stem, "lib" | "mod") {
            components.push(stem.into());
        }
        let mut module_file = source_root.join("lib.rs");
        let mut module_directory = source_root.clone();
        for component in &components {
            let parsed = syn::parse_file(&fs::read_to_string(&module_file).ok()?).ok()?;
            parsed.items.iter().find(|item| matches!(item, syn::Item::Mod(m) if m.ident == component && m.content.is_none() && plain_attributes(&m.attrs)))?;
            let flat = module_directory.join(format!("{component}.rs"));
            let nested = module_directory.join(component).join("mod.rs");
            module_file = if flat.is_file() && !nested.exists() {
                flat
            } else if nested.is_file() && !flat.exists() {
                nested
            } else {
                return None;
            };
            module_directory = module_directory.join(component);
        }
        if module_file != absolute {
            return None;
        }
        let prefix = if components.is_empty() {
            String::new()
        } else {
            format!("{}::", components.join("::"))
        };
        (package.name.clone(), prefix)
    };
    Some(
        changed
            .into_iter()
            .map(|name| TestCase {
                package: package.name.clone(),
                binary: binary.clone(),
                name: format!("{prefix}{name}"),
            })
            .collect(),
    )
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
