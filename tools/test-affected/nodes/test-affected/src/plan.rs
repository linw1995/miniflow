use crate::{
    classify::isolated_changes,
    error::*,
    git::{command, verify_snapshot},
    model::*,
};
use mfn_lsp::{AnalysisOutcome, GapKind};
use serde::Deserialize;
use serde_json::Value;
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
    let mut reasons = BTreeMap::<String, Vec<String>>::new();
    let mut direct = BTreeSet::new();
    let mut production = BTreeSet::new();
    let mut tests = BTreeSet::new();
    let mut fallback = Vec::new();
    for change in &comparison.changes {
        let path = Path::new(&change.path);
        if matches!(
            path.file_name().and_then(|p| p.to_str()),
            Some("Cargo.toml" | "Cargo.lock")
        ) {
            fallback.push(format!("Dependency configuration changed: {}", change.path));
            continue;
        }
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
            if let Some(selected) = isolated_tests(comparison, change, &packages[owner], root) {
                tests.extend(selected);
            } else {
                production.insert(owner.clone());
            }
        } else if change.path.starts_with("docs/")
            || change.path.starts_with("openspec/")
            || matches!(
                change.path.as_str(),
                "README.md" | "CONTRIBUTING.md" | "AGENTS.md"
            )
        {
            continue;
        } else {
            fallback.push(format!(
                "Shared or unclassified input changed: {}",
                change.path
            ));
        }
    }
    let baseline = closure(&direct, &dependencies);
    if outcome
        .gaps
        .iter()
        .any(|g| matches!(g.kind, GapKind::AnalyzerFailure | GapKind::Limit))
    {
        fallback.push("Semantic analyzer failed; retaining the full workspace".into());
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
    let tree = command(&comparison.head_root, "git", &["rev-parse", "HEAD^{tree}"])?
        .trim()
        .into();
    let toolchain = command(&comparison.head_root, "rustc", &["-vV"])?;
    let mut plan = TestPlan {
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
    };
    if let Some(path) = &options.inventory {
        apply_inventory(&mut plan, path, &packages)?;
    }
    plan.planning_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(plan)
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
            parsed.items.iter().find(|item| matches!(item, syn::Item::Mod(m) if m.ident == component && m.content.is_none() && !m.attrs.iter().any(|a| a.path().is_ident("path"))))?;
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
