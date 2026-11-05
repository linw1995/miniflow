use crate::{
    BuildGuard, BuildInputs, RunnerOptions, SupportPackages, atomic_copy,
    validate_runtime_identity, write_dependency_project_with_options,
};
use snafu::{IntoError, ResultExt, Snafu};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Snafu)]
#[snafu(display("{stage} failed; build project at {project:?}: {source}"))]
pub struct PipelineError {
    pub stage: &'static str,
    pub project: PathBuf,
    pub source: Box<dyn Error + Send + Sync>,
}

fn at<T, E: Error + Send + Sync + 'static>(
    stage: &'static str,
    project: &Path,
    result: Result<T, E>,
) -> Result<T, PipelineError> {
    result
        .map_err(Box::<dyn Error + Send + Sync>::from)
        .context(PipelineSnafu {
            stage,
            project: project.to_owned(),
        })
}
fn failure(stage: &'static str, project: &Path, message: impl Into<String>) -> PipelineError {
    PipelineSnafu {
        stage,
        project: project.to_owned(),
    }
    .into_error(Box::new(io::Error::other(message.into())) as Box<dyn Error + Send + Sync>)
}

pub struct CompileRequest<'a> {
    pub definition: &'a Path,
    pub output: &'a Path,
    pub locked: bool,
    pub build_dir: Option<&'a Path>,
    pub support: &'a SupportPackages,
}

pub fn compile_project(request: &CompileRequest<'_>) -> Result<PathBuf, PipelineError> {
    compile_project_with_options(request, &RunnerOptions::default())
}

pub fn compile_project_with_options(
    request: &CompileRequest<'_>,
    options: &RunnerOptions,
) -> Result<PathBuf, PipelineError> {
    let inputs = at(
        "input resolution",
        request.definition,
        BuildInputs::new(request.definition),
    )?;
    let output = at(
        "output validation",
        request.definition,
        inputs.check_output(request.output),
    )?;
    let discovery = discover_provider_flows(request, options, &inputs, &output)?;
    let registry = at(
        "host registry",
        &inputs.definition,
        mf_runtime::NodeRegistry::from_inventory(),
    )?;
    // Provider probes stop at a stable dependency closure. Do not return their
    // last partial result: resolve the complete authoring bundle exactly once.
    let mut resolved = at(
        "Flow source resolution",
        &inputs.definition,
        discovery.bundle.resolve(&registry),
    )?;
    resolved.sources.insert(inputs.definition.clone());
    check_source_destinations(&inputs, &output, &resolved.sources)?;
    let mut protected: Vec<&Path> = resolved.sources.iter().map(PathBuf::as_path).collect();
    protected.extend([inputs.lock.as_path(), output.as_path()]);
    let plan = at(
        "structural validation",
        &inputs.definition,
        crate::compiler::plan_resolved_definition(&resolved.definition, &resolved.contracts),
    )?;
    let guard_path = inputs.lock.with_extension("lock.guard");
    let _guard = at(
        "dependency lock",
        &inputs.definition,
        BuildGuard::acquire(&guard_path),
    )?;
    let directory = at(
        "build directory",
        &inputs.definition,
        crate::BuildDirectory::open_protected(&inputs.definition, request.build_dir, &protected),
    )?;
    let project = directory.path.clone();
    eprintln!(
        "{} build directory {project:?}",
        if directory.reused {
            "reused"
        } else {
            "created"
        }
    );
    at(
        "project generation",
        &project,
        write_dependency_project_with_options(&project, &plan, request.support, options),
    )?;
    let metadata = resolve_project(&project, &inputs.lock, request.locked)?;
    at(
        "runtime compatibility",
        &project,
        validate_runtime_identity(&metadata),
    )?;
    let executable = build_runner(&project)?;
    if !request.locked {
        let lock = at("lock read", &project, fs::read(project.join("Cargo.lock")))?;
        at(
            "lock persistence",
            &project,
            crate::state::write_if_changed(&inputs.lock, &lock),
        )?;
    }
    let stage = if request.locked {
        "executable installation"
    } else {
        "executable installation (dependency lock was updated)"
    };
    at(stage, &project, atomic_copy(&executable, &output))?;
    Ok(project)
}

/// Protect known source paths before reporting pending providers or touching locks.
/// Only discovery's authorized source facts participate; configuration JSON is opaque.
fn check_source_destinations(
    inputs: &BuildInputs,
    output: &Path,
    sources: &BTreeSet<PathBuf>,
) -> Result<(), PipelineError> {
    if sources.contains(output) || sources.contains(&inputs.lock) {
        return Err(failure(
            "output validation",
            &inputs.definition,
            "output or lock would replace a referenced Flow source",
        ));
    }
    let guard_path = inputs.lock.with_extension("lock.guard");
    if output == guard_path || sources.contains(&guard_path) {
        return Err(failure(
            "output validation",
            &inputs.definition,
            "output or a referenced Flow source would replace the dependency build guard",
        ));
    }
    Ok(())
}

/// Discover the complete provider closure before touching the user-selected build
/// directory. Every pass reads the original authoring file: a frozen partial pass
/// must not hide fields whose contracts become available after linking a provider.
fn discover_provider_flows(
    request: &CompileRequest<'_>,
    options: &RunnerOptions,
    inputs: &BuildInputs,
    output: &Path,
) -> Result<crate::FlowDiscovery, PipelineError> {
    // CLI process inventory is not the selected provider bundle (tests and hosts
    // may link extra crates or different feature sets). Only the probe grants
    // authored configuration fields Flow authority for this compilation.
    let mut initial = at(
        "Flow source discovery",
        &inputs.definition,
        crate::flow_sources::discover_unlinked_workflow(&inputs.definition),
    )?;
    check_source_destinations(inputs, output, &initial.sources)?;
    let mut builtin_catalog = BTreeMap::new();
    // A static public builtin can itself supply the first provider dependencies.
    // Only explicitly requested names are snapshotted, never host node contracts.
    for _ in 0..64 {
        if !initial.definition.dependencies.is_empty() || initial.pending.is_empty() {
            break;
        }
        let changed = at(
            "public builtin catalog",
            &inputs.definition,
            crate::flow_sources::snapshot_pending_builtins(
                &initial.pending,
                &BTreeSet::new(),
                &mut builtin_catalog,
            ),
        )?;
        if !changed {
            break;
        }
        initial = at(
            "Flow source discovery",
            &inputs.definition,
            crate::flow_sources::discover_unlinked_with_catalog(
                &inputs.definition,
                &builtin_catalog,
            ),
        )?;
        check_source_destinations(inputs, output, &initial.sources)?;
    }
    // No provider probe is needed when discovery already has no pending metadata.
    // This never infers executable roles from arbitrary configuration JSON.
    if initial.pending.is_empty()
        && (builtin_catalog.is_empty() || initial.definition.dependencies.is_empty())
    {
        return Ok(initial);
    }
    if initial.definition.dependencies.is_empty() {
        at(
            "Flow source discovery",
            &inputs.definition,
            initial.check_pending_nodes(),
        )?;
        return Err(failure(
            "Flow source discovery",
            &inputs.definition,
            format!(
                "no selected provider supplies the required contracts: {:?}",
                initial.pending
            ),
        ));
    }
    if request.locked {
        // Temporary probes may prune the generated root record as Cargo discovers
        // the provider closure, but they must never relax a missing-lock request
        // or open/write the user's managed build directory.
        at(
            "dependency resolution",
            &inputs.definition,
            fs::metadata(&inputs.lock),
        )?;
    }
    let mut probe_directory = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        probe_directory.permissions(fs::Permissions::from_mode(0o700));
    }
    let mut probe = at(
        "Flow metadata probe directory",
        &inputs.definition,
        if request.locked {
            probe_directory.prefix("mf-flow-resolution-").tempdir()
        } else {
            // Preserve a private, standalone diagnostic project on failure without
            // opening the managed cache before provider discovery is complete.
            probe_directory
                .prefix(".mf-build-flow-resolution-")
                .tempdir_in(
                    inputs
                        .definition
                        .parent()
                        .expect("canonical workflow has a parent"),
                )
        },
    )?;
    probe.disable_cleanup(!request.locked);
    let project = probe.path();
    let mut selected = initial.definition;
    let mut sources = initial.sources;
    at(
        "public builtin catalog",
        &inputs.definition,
        crate::flow_sources::snapshot_pending_builtins(
            &initial.pending,
            &selected
                .dependencies
                .values()
                .map(|dependency| dependency.package.clone())
                .collect(),
            &mut builtin_catalog,
        ),
    )?;
    for _ in 0..64 {
        at(
            "Flow metadata probe generation",
            project,
            crate::dependency_project::write_resolution_project(
                project,
                &selected,
                request.support,
                options,
            ),
        )?;
        let selected_packages: BTreeSet<_> = selected
            .dependencies
            .values()
            .map(|dependency| dependency.package.clone())
            .collect();
        let catalog_path = project.join("builtin-catalog.json");
        let catalog_bytes = at(
            "public builtin catalog serialization",
            project,
            serde_json::to_vec(&(&builtin_catalog, &selected_packages)),
        )?;
        at(
            "public builtin catalog snapshot",
            project,
            fs::write(&catalog_path, catalog_bytes),
        )?;
        let metadata = resolve_discovery_project(project, &inputs.lock, request.locked)?;
        at(
            "runtime compatibility",
            project,
            validate_runtime_identity(&metadata),
        )?;
        let executable = build_runner(project)?;
        let result_path = project.join("flow-discovery.json");
        let status = at(
            "Flow metadata probe",
            project,
            Command::new(executable)
                .arg(&inputs.definition)
                .arg(&result_path)
                .arg(&catalog_path)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status(),
        )?;
        if !status.success() {
            return Err(failure(
                "Flow metadata probe",
                project,
                format!("provider-linked Flow resolution exited with {status}"),
            ));
        }
        let result = at(
            "Flow metadata probe output",
            project,
            fs::read(&result_path),
        )?;
        let mut discovery: crate::flow_sources::FlowDiscovery = at(
            "Flow metadata probe output",
            project,
            serde_json::from_slice(&result),
        )?;
        sources.extend(std::mem::take(&mut discovery.sources));
        check_source_destinations(inputs, output, &sources)?;
        let catalog_changed = at(
            "public builtin catalog",
            &inputs.definition,
            crate::flow_sources::snapshot_pending_builtins(
                &discovery.pending,
                &discovery
                    .definition
                    .dependencies
                    .values()
                    .map(|dependency| dependency.package.clone())
                    .collect(),
                &mut builtin_catalog,
            ),
        )?;
        if !catalog_changed && selected.dependencies == discovery.definition.dependencies {
            if !discovery.pending.is_empty() {
                at(
                    "Flow source discovery",
                    &inputs.definition,
                    discovery.check_pending_nodes(),
                )?;
                return Err(failure(
                    "Flow source discovery",
                    &inputs.definition,
                    format!(
                        "selected providers do not supply the required Flow contracts or builtins: {:?}",
                        discovery.pending
                    ),
                ));
            }
            discovery.sources = sources;
            // Only failed unlocked probes are retained. Locked probes always keep
            // their temporary-only cleanup policy and never touch the managed cache.
            probe.disable_cleanup(false);
            return Ok(discovery);
        }
        selected = discovery.definition;
    }
    Err(failure(
        "Flow source discovery",
        &inputs.definition,
        "provider dependency discovery did not converge after 64 passes",
    ))
}

fn build_runner(project: &Path) -> Result<PathBuf, PipelineError> {
    let mut child = at(
        "Cargo build",
        project,
        cargo_command(project)
            .args([
                "build",
                "--release",
                "--locked",
                "--message-format=json-render-diagnostics",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn(),
    )?;
    let mut executable = None;
    let stream = child.stdout.take().unwrap();
    let mut stream_error = None;
    for line in io::BufReader::new(stream).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                stream_error = Some(error);
                break;
            }
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if value["reason"] == "compiler-artifact"
                && value["target"]["name"] == "mf-generated-workflow"
            {
                executable = value["executable"].as_str().map(PathBuf::from);
                if value["fresh"] == true {
                    eprintln!("reused compiled runner");
                }
            }
            if let Some(rendered) = value["message"]["rendered"].as_str() {
                eprint!("{rendered}");
            }
        } else {
            eprintln!("{line}");
        }
    }
    let status = at("Cargo build", project, child.wait())?;
    if let Some(error) = stream_error {
        return at("Cargo diagnostics", project, Err(error));
    }
    if !status.success() {
        return Err(failure(
            "Cargo build",
            project,
            format!("Cargo build failed with {status}"),
        ));
    }
    executable.filter(|path| path.is_file()).ok_or_else(|| {
        failure(
            "Cargo build",
            project,
            "Cargo did not produce a runner executable",
        )
    })
}

pub fn cargo_command(project: &Path) -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command
        .current_dir(project)
        .env("CARGO_TARGET_DIR", project.join("target"));
    command
}

pub fn resolve_project(
    project: &Path,
    flow_lock: &Path,
    locked: bool,
) -> Result<serde_json::Value, PipelineError> {
    resolve_project_metadata(project, flow_lock, locked, false)
}

/// Discovery manifests intentionally change between provider-linked passes.
/// Seed Cargo from the authoritative lock without passing --locked to temporary
/// probe metadata. Locked discovery stays offline and proves the resulting graph
/// is an authoritative-lock subset before building any probe Rust code. The final
/// runner still uses `resolve_project` with the real request; neither metadata
/// path writes the authoritative file.
fn resolve_discovery_project(
    project: &Path,
    flow_lock: &Path,
    locked: bool,
) -> Result<serde_json::Value, PipelineError> {
    let metadata = resolve_project_metadata(project, flow_lock, false, locked)?;
    if locked {
        validate_discovery_lock(project, flow_lock)?;
    }
    Ok(metadata)
}

fn resolve_project_metadata(
    project: &Path,
    flow_lock: &Path,
    locked: bool,
    offline: bool,
) -> Result<serde_json::Value, PipelineError> {
    // Lock synchronization copies into the generated project only. The final
    // runner retains --locked, including validation of its complete manifest.
    let working = project.join("Cargo.lock");
    match fs::read(flow_lock) {
        Ok(contents) => at(
            "lock synchronization",
            project,
            crate::state::write_if_changed(&working, &contents),
        )?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if locked {
                return Err(failure(
                    "dependency resolution",
                    project,
                    format!("--locked requires an existing dependency lock at {flow_lock:?}"),
                ));
            }
            if let Err(error) = fs::remove_file(&working)
                && error.kind() != io::ErrorKind::NotFound
            {
                return at("lock synchronization", project, Err(error));
            }
        }
        Err(error) => return at("lock synchronization", project, Err(error)),
    }
    let mut command = cargo_command(project);
    command.args(["metadata", "--format-version", "1"]);
    if locked {
        command.arg("--locked");
    }
    if offline {
        command.arg("--offline");
    }
    let output = at("dependency resolution", project, command.output())?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(failure(
            "dependency resolution",
            project,
            format!("Cargo metadata exited with {}", output.status),
        ));
    }
    at(
        "dependency resolution",
        project,
        serde_json::from_slice(&output.stdout),
    )
}

#[derive(serde::Deserialize)]
struct DiscoveryLock {
    package: Vec<LockedPackage>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
struct LockedPackageId {
    name: String,
    version: String,
    source: Option<String>,
}

#[derive(serde::Deserialize)]
struct LockedPackage {
    #[serde(flatten)]
    id: LockedPackageId,
    checksum: Option<String>,
    replace: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

fn read_discovery_lock(project: &Path, path: &Path) -> Result<DiscoveryLock, PipelineError> {
    let source = at(
        "discovery lock validation",
        project,
        fs::read_to_string(path),
    )?;
    at(
        "discovery lock validation",
        project,
        toml::from_str(&source),
    )
}

fn locked_dependencies(
    project: &Path,
    package: &LockedPackage,
    lock: &DiscoveryLock,
) -> Result<std::collections::BTreeSet<LockedPackageId>, PipelineError> {
    package
        .dependencies
        .iter()
        .map(|dependency| {
            // Cargo omits unambiguous versions and sources from dependency strings.
            // Expand both locks to package identities before comparing their edges.
            let mut words = dependency.splitn(3, ' ');
            let name = words.next().unwrap_or_default();
            let version = words.next();
            let source = words
                .next()
                .map(|source| source.trim_start_matches('(').trim_end_matches(')'));
            let candidates: Vec<_> = lock
                .package
                .iter()
                .filter(|candidate| {
                    candidate.id.name == name
                        && version.is_none_or(|version| candidate.id.version == version)
                        && source
                            .is_none_or(|source| candidate.id.source.as_deref() == Some(source))
                })
                .collect();
            if candidates.len() != 1 {
                return Err(failure(
                    "discovery lock validation",
                    project,
                    format!(
                        "could not identify locked dependency `{dependency}` of `{}`",
                        package.id.name
                    ),
                ));
            }
            Ok(candidates[0].id.clone())
        })
        .collect()
}

/// A temporary discovery manifest selects only part of the final provider graph.
/// Before running any Rust build code, prove that its packages and activated edges
/// are a subset of the authoritative lock. The generated root record alone may
/// change; checksums and registry/git source identities must remain identical.
fn validate_discovery_lock(project: &Path, flow_lock: &Path) -> Result<(), PipelineError> {
    let authoritative = read_discovery_lock(project, flow_lock)?;
    let probe = read_discovery_lock(project, &project.join("Cargo.lock"))?;
    for package in &probe.package {
        if package.id.name == "mf-generated-workflow"
            && package.id.version == "0.1.0"
            && package.id.source.is_none()
        {
            continue;
        }
        let expected = authoritative
            .package
            .iter()
            .find(|expected| expected.id == package.id);
        let Some(expected) = expected else {
            return Err(failure(
                "discovery lock validation",
                project,
                format!(
                    "--locked discovery selected an unlocked package: {:?}",
                    package.id
                ),
            ));
        };
        if expected.replace != package.replace {
            return Err(failure(
                "discovery lock validation",
                project,
                format!(
                    "--locked discovery changed replacement for `{}`",
                    package.id.name
                ),
            ));
        }
        if expected.checksum != package.checksum {
            return Err(failure(
                "discovery lock validation",
                project,
                format!(
                    "--locked discovery changed checksum for `{}`",
                    package.id.name
                ),
            ));
        }
        let expected_dependencies = locked_dependencies(project, expected, &authoritative)?;
        let selected_dependencies = locked_dependencies(project, package, &probe)?;
        if !selected_dependencies.is_subset(&expected_dependencies) {
            return Err(failure(
                "discovery lock validation",
                project,
                format!(
                    "--locked discovery added dependencies to `{}`",
                    package.id.name
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkflowDefinition;
    use serde_json::json;

    fn host_plain(
        _: serde_json::Value,
    ) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
        panic!("host inventory must not construct the external provider")
    }

    fn host_prepared(
        _: &str,
        _: serde_json::Value,
        _: std::collections::BTreeMap<String, mf_runtime::PreparedSubgraph>,
    ) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
        panic!("host inventory must not prepare the external provider")
    }

    inventory::submit! {
        mf_runtime::NodeRegistration {
            kind: "fixture.first",
            factory: mf_runtime::NodeFactory::Subgraph {
                parameter_types: || BTreeMap::from([
                    ("host_extra".into(), mf_runtime::ConfigParameterType::Flow),
                ]), plain: host_plain, prepared: host_prepared,
            },
        }
    }

    #[test]
    fn locked_probe_graph_allows_subsets_but_rejects_identity_checksum_and_edge_changes() {
        let root = tempfile::tempdir().unwrap();
        let authoritative = root.path().join("authoritative.lock");
        let full = r#"
version = 4
[[package]]
name = "mf-generated-workflow"
version = "0.1.0"
dependencies = ["parent", "leaf"]
[[package]]
name = "parent"
version = "1.0.0"
dependencies = ["leaf"]
[[package]]
name = "leaf"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "original"
"#;
        fs::write(&authoritative, full).unwrap();
        let working = root.path().join("Cargo.lock");
        let subset = full
            .replace(
                "dependencies = [\"parent\", \"leaf\"]",
                "dependencies = [\"parent\"]",
            )
            .replace("dependencies = [\"leaf\"]", "dependencies = []");
        fs::write(&working, &subset).unwrap();
        validate_discovery_lock(root.path(), &authoritative).unwrap();
        for changed in [
            subset.replace("checksum = \"original\"", "checksum = \"changed\""),
            subset.replace(
                "name = \"leaf\"\nversion = \"1.0.0\"",
                "name = \"leaf\"\nversion = \"1.0.1\"",
            ),
            subset.replace(
                "registry+https://github.com/rust-lang/crates.io-index",
                "git+https://example.invalid/repo#revision",
            ),
        ] {
            fs::write(&working, changed).unwrap();
            assert!(validate_discovery_lock(root.path(), &authoritative).is_err());
        }
        fs::write(&authoritative, subset).unwrap();
        fs::write(&working, full).unwrap();
        let error = validate_discovery_lock(root.path(), &authoritative).unwrap_err();
        assert!(error.to_string().contains("added dependencies"));
    }

    fn discover_fixture(
        root: &Path,
        authored: serde_json::Value,
        locked: bool,
    ) -> Result<crate::FlowDiscovery, PipelineError> {
        let source = root.join("flow.json");
        fs::write(&source, authored.to_string()).unwrap();
        let inputs = BuildInputs::new(&source).unwrap();
        let support = SupportPackages::Local {
            crates_dir: Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .to_owned(),
        };
        let output = root.join("runner");
        let build = root.join("managed-build");
        let result = discover_provider_flows(
            &CompileRequest {
                definition: &source,
                output: &output,
                locked,
                build_dir: Some(&build),
                support: &support,
            },
            &RunnerOptions { telemetry: false },
            &inputs,
            &output,
        );
        assert!(!output.exists());
        assert!(!build.exists());
        result
    }

    fn empty_public_builtin() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        WorkflowDefinition::from_json(r#"{"version":"2026-10-25","dependencies":{},"nodes":[]}"#)
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("compiler-empty-catalog", "empty", empty_public_builtin)
    }

    #[test]
    fn public_builtin_without_selected_receiver_does_not_complete_discovery() {
        let root = tempfile::tempdir().unwrap();
        let error = discover_fixture(
            root.path(),
            json!({
                "version":"2026-10-25", "dependencies":{},
                "flows":{"empty":{"builtin":"compiler-empty-catalog.empty"}},
                "nodes":[{"id":"call", "kind":"workflow.call", "config":{"flow":"empty"}}]
            }),
            false,
        )
        .err()
        .unwrap();
        assert_eq!(error.stage, "Flow source discovery");
        assert!(
            matches!(error.source.downcast_ref::<crate::WorkflowCompileError>(),
            Some(crate::WorkflowCompileError::UnknownNodeKind { definition_id, kind })
                if definition_id.as_str() == "call" && kind == mf_runtime::CALL_KIND)
        );
        let inputs = BuildInputs::new(&root.path().join("flow.json")).unwrap();
        let discovery = crate::flow_sources::discover_unlinked_with_catalog(
            &inputs.definition,
            &BTreeMap::from([(
                "compiler-empty-catalog.empty".into(),
                empty_public_builtin().unwrap(),
            )]),
        )
        .unwrap();
        assert_eq!(
            discovery.pending,
            BTreeSet::from([format!("node:{}", mf_runtime::CALL_KIND)])
        );
        let resolved = discovery;
        assert!(resolved.definition.dependencies.is_empty());
        assert_eq!(
            resolved.definition.nodes[0].config["body"],
            json!({
                "version":"2026-10-25", "dependencies":{}, "nodes":[], "edges":[], "outputs":[]
            })
        );
        assert_eq!(resolved.sources, BTreeSet::from([inputs.definition]));
        assert!(!inputs.lock.exists());
    }

    #[test]
    fn missing_provider_selection_and_missing_locked_file_fail_before_any_rust_build() {
        let root = tempfile::tempdir().unwrap();
        let error = discover_fixture(
            root.path(),
            json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[{"id":"unknown", "kind":"unselected.provider"}]
            }),
            false,
        )
        .err()
        .unwrap();
        assert_eq!(error.stage, "Flow source discovery");
        assert!(matches!(
            error.source.downcast_ref::<crate::WorkflowCompileError>(),
            Some(crate::WorkflowCompileError::UnknownNodeKind { definition_id, kind })
                if definition_id.as_str() == "unknown" && kind == "unselected.provider"
        ));
        for expected in ["unknown kind", "unknown", "unselected.provider"] {
            assert!(error.to_string().contains(expected), "{error}");
        }
        write_provider(root.path(), "resolution-locked", "fixture.locked", "");
        let error = discover_fixture(root.path(), json!({
            "version":"2026-10-28", "dependencies":{
                "provider":{"package":"resolution-locked", "path":"resolution-locked", "default-features":false}
            },
            "nodes":[{"id":"owner", "kind":"fixture.locked", "config":{}}]
        }), true).err().unwrap();
        assert_eq!(error.stage, "dependency resolution");
        assert_eq!(
            error.source.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        let inputs = BuildInputs::new(&root.path().join("flow.json")).unwrap();
        assert!(!inputs.lock.exists());
    }

    #[test]
    fn discovery_lock_errors_preserve_read_parse_and_ambiguous_edge_failures() {
        let root = tempfile::tempdir().unwrap();
        let authoritative = root.path().join("authoritative.lock");
        let error = read_discovery_lock(root.path(), &authoritative)
            .err()
            .unwrap();
        assert_eq!(error.stage, "discovery lock validation");
        assert_eq!(
            error.source.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        fs::write(&authoritative, "[[package]\nname = broken").unwrap();
        let error = read_discovery_lock(root.path(), &authoritative)
            .err()
            .unwrap();
        assert_eq!(error.stage, "discovery lock validation");
        assert!(error.source.downcast_ref::<toml::de::Error>().is_some());
        let ambiguous = r#"
version = 4
[[package]]
name = "parent"
version = "1.0.0"
dependencies = ["leaf"]
[[package]]
name = "leaf"
version = "1.0.0"
source = "registry+https://example.invalid/index"
[[package]]
name = "leaf"
version = "2.0.0"
source = "registry+https://example.invalid/index"
"#;
        fs::write(&authoritative, ambiguous).unwrap();
        fs::write(root.path().join("Cargo.lock"), ambiguous).unwrap();
        let error = validate_discovery_lock(root.path(), &authoritative).unwrap_err();
        assert_eq!(error.stage, "discovery lock validation");
        let source = error.source.downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.kind(), io::ErrorKind::Other);
        assert!(
            source
                .to_string()
                .contains("could not identify locked dependency `leaf` of `parent`")
        );
        let explicit = ambiguous.replace(
            "dependencies = [\"leaf\"]",
            "dependencies = [\"leaf 1.0.0 (registry+https://example.invalid/index)\"]",
        );
        fs::write(&authoritative, &explicit).unwrap();
        fs::write(root.path().join("Cargo.lock"), explicit).unwrap();
        validate_discovery_lock(root.path(), &authoritative).unwrap();
    }

    #[test]
    fn discovery_lock_rejects_changed_replacement_identity() {
        let root = tempfile::tempdir().unwrap();
        let authoritative = root.path().join("authoritative.lock");
        let original = r#"
version = 4
[[package]]
name = "parent"
version = "1.0.0"
[[package]]
name = "parent-fork"
version = "1.0.0"
"#;
        fs::write(&authoritative, original).unwrap();
        fs::write(
            root.path().join("Cargo.lock"),
            original.replace(
                "name = \"parent\"\nversion = \"1.0.0\"",
                "name = \"parent\"\nversion = \"1.0.0\"\nreplace = \"parent-fork 1.0.0\"",
            ),
        )
        .unwrap();
        let error = validate_discovery_lock(root.path(), &authoritative).unwrap_err();
        assert_eq!(error.stage, "discovery lock validation");
        let source = error.source.downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.kind(), io::ErrorKind::Other);
        assert!(
            source
                .to_string()
                .contains("changed replacement for `parent`")
        );
    }

    #[test]
    fn provider_probe_configuration_failure_preserves_managed_paths() {
        let root = tempfile::tempdir().unwrap();
        write_provider(
            root.path(),
            "resolution-invalid-config",
            "fixture.invalid-config",
            "",
        );
        let error = discover_fixture(root.path(), json!({
            "version":"2026-10-28", "dependencies":{
                "provider":{"package":"resolution-invalid-config", "path":"resolution-invalid-config", "default-features":false}
            },
            "nodes":[{"id":"invalid", "kind":"fixture.invalid-config", "config":{"body":{"path":42}}}]
        }), false).err().unwrap();
        assert_eq!(error.stage, "Flow metadata probe");
        let source = error.source.downcast_ref::<io::Error>().unwrap();
        assert_eq!(source.kind(), io::ErrorKind::Other);
        assert!(
            source
                .to_string()
                .contains("provider-linked Flow resolution exited")
        );
        let inputs = BuildInputs::new(&root.path().join("flow.json")).unwrap();
        assert!(!inputs.lock.exists());
    }

    fn write_provider(root: &Path, package: &str, kind: &str, extra: &str) {
        let directory = root.join(package);
        fs::create_dir_all(directory.join("src")).unwrap();
        let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("mf-runtime");
        fs::write(directory.join("Cargo.toml"), format!(
            "[package]\nname = {package:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[features]\nexpanded = []\n\n[dependencies]\nmf-runtime = {{ path = {:?} }}\nserde_json = \"1.0.151\"\ninventory = \"0.3.24\"\n", runtime.to_str().unwrap()
        )).unwrap();
        fs::write(directory.join("src/lib.rs"), format!(r#"
use mf_runtime::{{NodeBuildError, NodeFactory, NodeRegistration, PreparedNode, PreparedSubgraph}};
use serde_json::Value;
use std::collections::BTreeMap;
fn plain(_: Value) -> Result<PreparedNode, NodeBuildError> {{
    panic!("discovery must never invoke a node factory")
}}
fn prepared(_: &str, _: Value, _: BTreeMap<String, PreparedSubgraph>) -> Result<PreparedNode, NodeBuildError> {{
    panic!("discovery must never prepare a node")
}}
inventory::submit! {{
    NodeRegistration {{ kind: {kind:?}, factory: NodeFactory::Subgraph {{
        parameter_types: || BTreeMap::from([
            ("body".into(), mf_runtime::ConfigParameterType::Flow),
            ("host_extra".into(), mf_runtime::ConfigParameterType::Value),
        ]), plain, prepared,
    }} }}
}}
{extra}
"#)).unwrap();
    }

    fn public_catalog_definition() -> Result<WorkflowDefinition, crate::DefinitionParseError> {
        WorkflowDefinition::from_json(
            &json!({
                "version":"2026-10-28", "dependencies":{},
                "nodes":[{"id":"catalog-node", "kind":"fixture.first", "config":{
                    "body":{"definition":{"version":"2026-10-28", "dependencies":{}, "nodes":[]}},
                    "host_extra":{"path":"must-never-use-host-contract.json"}
                }}]
            })
            .to_string(),
        )
    }

    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("compiler-public-catalog", "example", public_catalog_definition)
    }
    inventory::submit! {
        mf_runtime::BuiltinFlowRegistration::new("resolution-first", "removed", public_catalog_definition)
    }

    #[test]
    fn public_builtin_catalog_preserves_selected_contracts_and_namespace_authority() {
        let root = tempfile::tempdir().unwrap();
        write_provider(
            root.path(),
            "resolution-first",
            "fixture.first",
            r#"
inventory::submit! {
    NodeRegistration { kind: "workflow.call", factory: NodeFactory::Subgraph {
        parameter_types: || BTreeMap::from([
            ("body".into(), mf_runtime::ConfigParameterType::Flow),
            ("host_extra".into(), mf_runtime::ConfigParameterType::Value),
        ]), plain, prepared,
    } }
}
"#,
        );
        let source = root.path().join("flow.json");
        for version in ["2026-10-25", "2026-10-28"] {
            let authored = json!({
                "version":version, "dependencies":{
                    "first":{"package":"resolution-first", "path":"resolution-first", "default-features":false}
                },
                "flows":{"catalog":{"builtin":"compiler-public-catalog.example"}},
                "nodes":[{"id":"call", "kind":"workflow.call", "config":{"flow":"catalog"}}]
            });
            let resolved = discover_fixture(root.path(), authored, false).unwrap();
            let inputs = BuildInputs::new(&source).unwrap();
            assert_eq!(resolved.definition.dependencies.len(), 1);
            assert_eq!(
                resolved
                    .definition
                    .dependencies
                    .values()
                    .next()
                    .unwrap()
                    .package,
                "resolution-first"
            );
            assert_eq!(
                resolved.sources,
                BTreeSet::from([inputs.definition.clone()])
            );
            assert_eq!(
                resolved.contracts["fixture.first"]["host_extra"],
                mf_runtime::ConfigParameterType::Value
            );
            let config = &resolved.definition.nodes[0].config["body"]["nodes"][0]["config"];
            assert!(config["body"].get("nodes").is_some());
            assert_eq!(
                config["host_extra"],
                json!({"path":"must-never-use-host-contract.json"})
            );
            crate::compiler::plan_resolved_definition(&resolved.definition, &resolved.contracts)
                .unwrap();
            assert!(!inputs.lock.exists());
        }
        let mut authored: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&source).unwrap()).unwrap();
        authored["flows"]["catalog"]["builtin"] = json!("resolution-first.removed");
        let error = discover_fixture(root.path(), authored, false)
            .err()
            .unwrap();
        let inputs = BuildInputs::new(&source).unwrap();
        assert!(
            error
                .to_string()
                .contains("builtin:resolution-first.removed"),
            "{error}"
        );
        assert!(!inputs.lock.exists());
    }

    #[test]
    fn provider_discovery_restarts_for_new_contracts_features_and_builtins() {
        let root = tempfile::tempdir().unwrap();
        write_provider(
            root.path(),
            "resolution-first",
            "fixture.first",
            r#"
#[cfg(feature = "expanded")]
inventory::submit! {
    NodeRegistration { kind: "fixture.feature", factory: NodeFactory::Plain(plain) }
}
"#,
        );
        write_provider(
            root.path(),
            "resolution-second",
            "fixture.second",
            r###"
fn builtin() -> Result<mf_runtime::WorkflowDefinition, mf_runtime::DefinitionParseError> {
    mf_runtime::WorkflowDefinition::from_json(r##"{
        "version":"2026-10-28", "dependencies":{},
        "nodes":[{"id":"feature", "kind":"fixture.feature"}]
    }"##)
}
mf_runtime::register_builtin_flow!("feature", builtin);
// Named alias discovery is intrinsic, but executable child traversal still
// requires the selected provider's explicit receiving contract.
inventory::submit! {
    NodeRegistration { kind: "workflow.call", factory: NodeFactory::Subgraph {
        parameter_types: || BTreeMap::from([
            ("body".into(), mf_runtime::ConfigParameterType::Flow),
            ("host_extra".into(), mf_runtime::ConfigParameterType::Value),
        ]), plain, prepared,
    } }
}
"###,
        );
        fs::create_dir(root.path().join("child")).unwrap();
        let authored = json!({
            "version":"2026-10-28", "dependencies":{
                "first":{"package":"resolution-first", "path":"resolution-first", "default-features":false}
            },
            "nodes":[{"id":"first", "kind":"fixture.first", "config":{"body":{"path":"child/flow.json"}}}]
        });
        let child = json!({
            "version":"2026-10-28", "dependencies":{
                "first":{"package":"resolution-first", "path":"../resolution-first", "features":["expanded"], "default-features":false},
                "second":{"package":"resolution-second", "path":"../resolution-second", "default-features":false}
            },
            "nodes":[{"id":"second", "kind":"fixture.second", "config":{"body":{"path":"grandchild.json"}}}]
        });
        let grandchild = json!({
            "version":"2026-10-28", "dependencies":{},
            "flows":{"feature":{"builtin":"resolution-second.feature"}},
            "nodes":[{"id":"builtin", "kind":"workflow.call", "config":{"flow":"feature"}}]
        });
        let source = root.path().join("flow.json");
        fs::write(root.path().join("child/flow.json"), child.to_string()).unwrap();
        fs::write(
            root.path().join("child/grandchild.json"),
            grandchild.to_string(),
        )
        .unwrap();
        let support = SupportPackages::Local {
            crates_dir: Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .to_owned(),
        };
        let resolved = discover_fixture(root.path(), authored.clone(), false).unwrap();
        let inputs = BuildInputs::new(&source).unwrap();
        assert!(!inputs.lock.exists());
        assert_eq!(resolved.sources.len(), 3);
        let first = resolved
            .definition
            .dependencies
            .values()
            .find(|dependency| dependency.package == "resolution-first")
            .unwrap();
        assert_eq!(first.features, ["expanded"]);
        assert_eq!(resolved.definition.dependencies.len(), 2);
        // Production uses this source-free planning entrypoint. Host inventory
        // deliberately disagrees with the selected provider's `body` contract.
        let selected_plan =
            crate::compiler::plan_resolved_definition(&resolved.definition, &resolved.contracts)
                .unwrap();
        assert_eq!(selected_plan.loop_orders.len(), 3);
        let host_plan = crate::plan_definition(&resolved.definition).unwrap();
        assert!(host_plan.loop_orders.is_empty());
        assert_eq!(host_plan.definition, resolved.definition);
        let child = &resolved.definition.nodes[0].config["body"];
        let grandchild = &child["nodes"][0]["config"]["body"];
        assert_eq!(
            grandchild["nodes"][0]["config"]["body"]["nodes"][0]["kind"],
            "fixture.feature"
        );

        // The authoritative lock describes the final provider closure, whereas
        // the first replay probe deliberately links only the authored root subset.
        let final_project = tempfile::tempdir().unwrap();
        crate::dependency_project::write_resolution_project(
            final_project.path(),
            &resolved.definition,
            &support,
            &RunnerOptions { telemetry: false },
        )
        .unwrap();
        resolve_project(final_project.path(), &inputs.lock, false).unwrap();
        let authoritative_lock = fs::read(final_project.path().join("Cargo.lock")).unwrap();
        fs::write(&inputs.lock, &authoritative_lock).unwrap();
        let replay = discover_fixture(root.path(), authored.clone(), true).unwrap();
        assert_eq!(replay.definition, resolved.definition);
        assert_eq!(replay.sources, resolved.sources);
        assert_eq!(fs::read(&inputs.lock).unwrap(), authoritative_lock);

        // Activating a new dependency through an already-locked local provider
        // must fail before Cargo executes any newly selected Rust build code.
        write_provider(root.path(), "resolution-unlocked", "fixture.unlocked", "");
        let sentinel = root.path().join("unlocked-build-script-executed");
        fs::write(root.path().join("resolution-unlocked/build.rs"), format!(
            "fn main() {{ std::fs::write({:?}, b\"executed\").unwrap(); panic!(\"unlocked build code ran\"); }}",
            sentinel.to_str().unwrap(),
        )).unwrap();
        let first_manifest = root.path().join("resolution-first/Cargo.toml");
        let manifest = fs::read_to_string(&first_manifest)
            .unwrap()
            .replace("expanded = []", "expanded = [\"dep:unlocked\"]");
        fs::write(&first_manifest, format!("{manifest}\n[dependencies.unlocked]\npackage = \"resolution-unlocked\"\npath = \"../resolution-unlocked\"\noptional = true\n")).unwrap();
        let error = discover_fixture(root.path(), authored, true).err().unwrap();
        assert_eq!(error.stage, "discovery lock validation", "{error}");
        assert!(error.to_string().contains("--locked discovery"), "{error}");
        assert!(
            !sentinel.exists(),
            "an unlocked provider build script executed"
        );
        assert_eq!(fs::read(&inputs.lock).unwrap(), authoritative_lock);
    }
}
