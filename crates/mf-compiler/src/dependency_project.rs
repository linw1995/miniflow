use crate::CompiledWorkflow;
use snafu::{OptionExt, ResultExt, Snafu};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub enum SupportPackages {
    Registry,
    Local { crates_dir: PathBuf },
}

#[derive(Clone, Copy, Debug)]
pub struct RunnerOptions {
    pub telemetry: bool,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self { telemetry: true }
    }
}

#[derive(Debug, Snafu)]
pub enum DependencyProjectError {
    #[snafu(display(
        "generated project directory {path:?} must not be a symbolic link; use a fresh build directory"
    ))]
    LinkedDirectory { path: PathBuf },
    #[snafu(display("could not synchronize generated file: {source}"))]
    State { source: crate::StateError },
    #[snafu(display("invalid dependency {alias}: {message}"))]
    Dependency { alias: String, message: String },
    #[snafu(display("could not generate runner code: {source}"))]
    Plan { source: crate::PlanError },
    #[snafu(display("could not write generated project at {path:?}: {source}"))]
    Write { path: PathBuf, source: io::Error },
}

const BUILD: &str = r#"
fn main() {
    println!("cargo:rerun-if-changed=workflow-plan.json");
    println!("cargo:rustc-check-cfg=cfg(mf_streaming)");
    if let Err(error) = generate() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let workflow = mf_compiler::CompiledWorkflow::from_json(include_str!("workflow-plan.json"))?;
    let artifacts = workflow.generate_runner_execution_plans(&registry)?;
    let manifest = mf_runtime::WorkflowManifest::from_bytes(&artifacts.manifest_bytes)?;
    let workflow = mf_compiler::plan_definition(&manifest.definition)?;
    if workflow.definition.execution.is_some() {
        println!("cargo:rustc-cfg=mf_streaming");
    }
    let source = workflow.generate_runner_artifacts()?;
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(output.join("workflow.rs"), source.rust_source)?;
    std::fs::write(output.join("flow-plans.rs"), artifacts.rust_source)?;
    std::fs::write(output.join("workflow-manifest.bin"), artifacts.manifest_bytes)?;
    std::fs::write(output.join("typed-fast-paths.json"), artifacts.typed_plan_json)?;
    std::fs::write(output.join("node-targets.json"), artifacts.node_targets_json)?;
    Ok(())
}
"#;

const MAIN: &str = r#"mod workflow;

#[cfg_attr(target_os = "linux", unsafe(link_section = ".mf_manifest"))]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__mf_manifest"))]
static WORKFLOW_MANIFEST: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/workflow-manifest.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/workflow-manifest.bin"));

fn workflow_manifest() -> Result<mf_runtime::WorkflowManifest, mf_runtime::WorkflowManifestError> {
    mf_runtime::WorkflowManifest::from_bytes(&WORKFLOW_MANIFEST)
}

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    let mut signals = match mf_runtime::RunnerSignals::new() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = run(#[cfg(unix)] &mut signals);
    let code = match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if let Some(mf_runtime::RunnerArgumentError::Cli { source }) = error.downcast_ref() {
                let _ = source.print();
                std::process::ExitCode::from(source.exit_code() as u8)
            } else {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        }
    };
    #[cfg(unix)]
    let code = signals.exit_code().unwrap_or(code);
    code
}

fn run(#[cfg(unix)] signals: &mut mf_runtime::RunnerSignals) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = workflow_manifest()?;
    let targets: Vec<mf_runtime::NodeTarget> = serde_json::from_str(
        include_str!(concat!(env!("OUT_DIR"), "/node-targets.json")),
    )?;
    let invocation = mf_runtime::RunnerInvocation::parse(
        std::env::args_os().skip(1), &manifest.input_schema, &targets,
    )?;
    if let Some(id) = invocation.node.as_deref() {
        let target = mf_runtime::NodeTarget::find(&targets, id)?;
        return execute_selected(target, invocation.inputs, invocation.context, invocation.verbose, #[cfg(unix)] signals);
    }
    let arguments = invocation.inputs;
    #[cfg(mf_streaming)]
    { execute_stream(arguments, invocation.verbose, #[cfg(unix)] signals) }
    #[cfg(not(mf_streaming))]
    {
        let outputs = execute(arguments, invocation.verbose, #[cfg(unix)] signals)?;
        println!("{}", serde_json::to_string(&outputs)?);
        Ok(())
    }
}

fn execute_selected(
    target: &mf_runtime::NodeTarget,
    inputs: mf_runtime::WorkflowArguments,
    context: mf_runtime::WorkflowArguments,
    verbose: u8,
    #[cfg(unix)] signals: &mut mf_runtime::RunnerSignals,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdio = mf_runtime::StreamStdio::claim()?;
    #[cfg(feature = "telemetry")]
    let providers = match mf_telemetry::otlp::TelemetryProviders::from_env() {
        Ok(providers) => providers,
        Err(error) => { eprintln!("telemetry export unavailable: {error}"); None }
    };
    #[cfg(feature = "telemetry")]
    let observer = providers.as_ref().map(|providers| providers.observer_with_verbosity(verbose));
    #[cfg(not(feature = "telemetry"))]
    let observer = None;
    let observer = observer.or_else(|| (verbose > 0).then(|| mf_telemetry::observation::Observer::stderr(verbose)));
    let run_id = run_id();
    let observation = observer.and_then(|observer| {
        match target.start_observation(&observer, run_id) {
            Ok(observation) => Some(observation),
            Err(error) => { eprintln!("telemetry observation unavailable: {error}"); None }
        }
    });
    #[cfg(feature = "telemetry")]
    let snapshots = if capture_requested() {
        match snapshot_recorder_for_id(target.description.workflow_id.clone(), run_id) {
            Ok(recorder) => Some(recorder),
            Err(error) => { eprintln!("data history unavailable: {error}"); None }
        }
    } else { None };
    #[cfg(not(feature = "telemetry"))]
    let snapshots: Option<SnapshotExport> = {
        if capture_requested() { eprintln!("data history unavailable: recompile the runner without --no-telemetry"); }
        None
    };
    let result = mf_runtime::ExecutionContext::run(observation, |state| -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(unix)]
        signals.bind(state.cancellation())?;
        if target.inputs.stdin_owner(&inputs)?.is_some() {
            if let Some(input) = stdio.take_input() { state.set_stdin(input); }
        }
        if let Some(snapshots) = &snapshots { state.set_snapshot_recorder(snapshots.recorder()); }
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        #[cfg(mf_streaming)]
        let outputs = {
            let prepared = workflow::prepare_stream(&registry).inspect_err(|error| {
                if let Some(observation) = state.observation_mut() { observation.preparation_failed_unattributed(error.to_string()); }
            })?;
            workflow_manifest()?.validate_prepared_schema(prepared.plan().input_schema())?;
            workflow::verify_stream_metadata(&prepared)?;
            let task = target.bind_stream(&prepared)?;
            mf_runtime::FlowRuntime::default().execute_target_in_context(&task, state, inputs, context)?
        };
        #[cfg(not(mf_streaming))]
        let outputs = {
            let flow = workflow::prepare_workflow(&registry, state.observation_mut())?;
            workflow_manifest()?.validate_prepared_schema(flow.input_schema())?;
            let task = target.bind_flow(&flow)?;
            mf_runtime::FlowRuntime::default().execute_target_in_context(&task, state, inputs, context)?
        };
        stdio.write_json(&outputs).inspect_err(|error| {
            if let Some(observation) = state.observation_mut() { observation.output_selection_failed(error.to_string()); }
        })?;
        Ok(())
    });
    if let Some(snapshots) = snapshots {
        snapshots.finish();
        if let Some(error) = snapshots.diagnostic() { eprintln!("data history incomplete: {error}"); }
    }
    #[cfg(feature = "telemetry")]
    if let Some(providers) = providers {
        for diagnostic in providers.shutdown() { eprintln!("telemetry export incomplete: {diagnostic}"); }
    }
    result
}

#[cfg(mf_streaming)]
fn execute_stream(arguments: mf_runtime::WorkflowArguments, verbose: u8, #[cfg(unix)] signals: &mut mf_runtime::RunnerSignals) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdio = mf_runtime::StreamStdio::claim()?;
    #[cfg(feature = "telemetry")]
    let providers = match mf_telemetry::otlp::TelemetryProviders::from_env() {
        Ok(providers) => providers,
        Err(error) => { eprintln!("telemetry export unavailable: {error}"); None }
    };
    #[cfg(feature = "telemetry")]
    let observer = providers.as_ref().map(|providers| providers.observer_with_verbosity(verbose));
    #[cfg(not(feature = "telemetry"))]
    let observer = None;
    let observer = observer.or_else(|| (verbose > 0).then(|| mf_telemetry::observation::Observer::stderr(verbose)));
    let run_id = run_id();
    let observation = observer.and_then(|observer| {
        let setup = (|| -> Result<_, Box<dyn std::error::Error>> {
            Ok(observer.start_stream(workflow_manifest()?.description()?, run_id)?)
        })();
        match setup {
            Ok(observation) => Some(observation),
            Err(error) => { eprintln!("telemetry observation unavailable: {error}"); None }
        }
    });
    #[cfg(feature = "telemetry")]
    let snapshots = if capture_requested() {
        let setup = (|| -> Result<_, Box<dyn std::error::Error>> {
            snapshot_recorder_for_id(workflow_manifest()?.workflow_id()?, run_id)
        })();
        match setup {
            Ok(recording) => Some(recording),
            Err(error) => { eprintln!("data capture unavailable: {error}"); None }
        }
    } else { None };
    #[cfg(not(feature = "telemetry"))]
    let snapshots: Option<SnapshotExport> = {
        if capture_requested() { eprintln!("data capture unavailable: telemetry-enabled build required"); }
        None
    };
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let _context = observation.as_ref().map(mf_runtime::StreamObservation::enter);
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let prepared = workflow::prepare_stream(&registry).inspect_err(|error| {
            if let Some(observation) = &observation { observation.preparation_failed(error.to_string()); }
        })?;
        workflow_manifest()?.validate_prepared_schema(prepared.plan().input_schema())?;
        workflow::verify_stream_metadata(&prepared)?;
        prepared.plan().input_schema().validate(&arguments)?;
        let needs_stdin = prepared.plan().input_schema().stdin_owner(&arguments)?.is_some();
        let stdin = if needs_stdin { stdio.take_input() } else { None };
        let instance = mf_runtime::FlowRuntime::default().start_stream(prepared, mf_runtime::StreamOptions { observation: observation.clone(), snapshots: snapshots.as_ref().map(SnapshotExport::recorder), arguments, stdin, ..Default::default() })?;
        #[cfg(unix)]
        signals.bind(instance.cancellation())?;
        stdio.run(instance)?;
        Ok(())
    })();
    if let (Err(error), Some(observation)) = (&result, &observation) { observation.preparation_failed(error.to_string()); }
    if let Some(snapshots) = snapshots {
        snapshots.finish();
        if let Some(error) = snapshots.diagnostic() { eprintln!("data history incomplete: {error}"); }
    }
    #[cfg(feature = "telemetry")]
    if let Some(providers) = providers {
        for diagnostic in providers.shutdown() { eprintln!("telemetry export incomplete: {diagnostic}"); }
    }
    result
}

fn run_id() -> mf_telemetry::identity::RunId {
    match std::env::var("MF_RUN_ID") {
        Ok(value) => match mf_telemetry::identity::RunId::try_from(value) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("invalid MF_RUN_ID: {error}; using a new run ID");
                mf_telemetry::identity::RunId::new()
            }
        },
        Err(std::env::VarError::NotPresent) => mf_telemetry::identity::RunId::new(),
        Err(error) => {
            eprintln!("invalid MF_RUN_ID: {error}; using a new run ID");
            mf_telemetry::identity::RunId::new()
        }
    }
}

fn capture_requested() -> bool {
    std::env::var(mf_telemetry::SNAPSHOT_CAPTURE_ENV).is_ok_and(|value| value == "1")
}

#[cfg(not(mf_streaming))]
fn execute(arguments: mf_runtime::WorkflowArguments, verbose: u8, #[cfg(unix)] signals: &mut mf_runtime::RunnerSignals) -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    #[cfg(feature = "telemetry")]
    let providers = match mf_telemetry::otlp::TelemetryProviders::from_env() {
        Ok(providers) => providers,
        Err(error) => { eprintln!("telemetry export unavailable: {error}"); None }
    };
    #[cfg(feature = "telemetry")]
    let observer = providers.as_ref().map(|providers| providers.observer_with_verbosity(verbose));
    #[cfg(not(feature = "telemetry"))]
    let observer = None;
    let observer = observer.or_else(|| (verbose > 0).then(|| mf_telemetry::observation::Observer::stderr(verbose)));
    let result = (|| -> Result<_, Box<dyn std::error::Error>> {
        let Some(observer) = observer else {
            if capture_requested() {
                #[cfg(feature = "telemetry")]
                eprintln!("data history unavailable: configure an OTLP logs endpoint");
                #[cfg(not(feature = "telemetry"))]
                eprintln!("data history unavailable: recompile the runner without --no-telemetry");
            }
            return execute_workflow(None, None, arguments, #[cfg(unix)] signals);
        };
        let description = workflow_manifest()?.description()?;
        #[cfg(feature = "telemetry")]
        let workflow_id = description.workflow_id.clone();
        let run_id = run_id();
        let started = if description.version.supports_loops() {
            observer.start_with_description(description, run_id)
        } else {
            let nodes = description.nodes.into_iter().map(|node| mf_runtime::NodeIdentity {
                id: node.id, kind: node.kind, path: Vec::new(),
            }).collect();
            observer.start(description.workflow_id, run_id, nodes)
        };
        let observation = match started {
            Ok(observation) => Some(observation),
            Err(error) => { eprintln!("telemetry observation unavailable: {error}"); None }
        };
        #[cfg(feature = "telemetry")]
        let snapshots = if capture_requested() {
            if providers.is_some() {
                match snapshot_recorder_for_id(workflow_id, run_id) {
                    Ok(recorder) => Some(recorder),
                    Err(error) => { eprintln!("data history unavailable: {error}"); None }
                }
            } else {
                eprintln!("data history unavailable: configure an OTLP logs endpoint");
                None
            }
        } else { None };
        #[cfg(not(feature = "telemetry"))]
        let snapshots = {
            if capture_requested() { eprintln!("data history unavailable: recompile the runner without --no-telemetry"); }
            None
        };
        execute_workflow(observation, snapshots, arguments, #[cfg(unix)] signals)
    })();
    #[cfg(feature = "telemetry")]
    if let Some(providers) = providers {
        for diagnostic in providers.shutdown() { eprintln!("telemetry export incomplete: {diagnostic}"); }
    }
    result
}

struct SnapshotExport(mf_telemetry::capture::SnapshotPublisher);

impl SnapshotExport {
    fn recorder(&self) -> mf_runtime::SnapshotRecorder {
        let publisher = self.0.clone();
        mf_runtime::SnapshotRecorder::new(move |event| publisher.record(event))
    }
    fn finish(&self) { self.0.finish(); }
    fn diagnostic(&self) -> Option<std::sync::Arc<mf_telemetry::capture::SnapshotPublishError>> {
        self.0.diagnostic()
    }
}

#[cfg(feature = "telemetry")]
fn snapshot_recorder_for_id(workflow_id: mf_telemetry::identity::WorkflowId, run_id: mf_telemetry::identity::RunId) -> Result<SnapshotExport, Box<dyn std::error::Error>> {
    let mut exporter = mf_telemetry::otlp::SnapshotExporter::from_env(workflow_id, run_id)?;
    Ok(SnapshotExport(mf_telemetry::capture::SnapshotPublisher::new(move |record| {
        let complete = record["record"] == "end";
        exporter.emit(record)?;
        if complete { exporter.finish()?; }
        Ok(())
    })?))
}

#[cfg(not(mf_streaming))]
fn execute_workflow(observation: Option<mf_runtime::RunObservation>, snapshots: Option<SnapshotExport>, arguments: mf_runtime::WorkflowArguments, #[cfg(unix)] signals: &mut mf_runtime::RunnerSignals) -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let result = mf_runtime::ExecutionContext::run(observation, |state| -> Result<_, Box<dyn std::error::Error>> {
        #[cfg(unix)]
        signals.bind(state.cancellation())?;
        state.set_workflow_arguments(arguments);
        #[cfg(unix)]
        {
            let manifest = workflow_manifest()?;
            let definition = &manifest.definition;
            if definition.version.supports_startup_inputs() {
                state.set_stdin(mf_runtime::TextInput::claim()?);
            }
        }
        if let Some(snapshots) = &snapshots { state.set_snapshot_recorder(snapshots.recorder()); }
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let flow = workflow::prepare_workflow(&registry, state.observation_mut())?;
        workflow_manifest()?.validate_prepared_schema(flow.input_schema())?;
        Ok(workflow::run_workflow_in_context(&flow, state)?)
    });
    if let Some(snapshots) = snapshots {
        snapshots.finish();
        if let Some(error) = snapshots.diagnostic() { eprintln!("data history incomplete: {error}"); }
    }
    result
}
"#;

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize to JSON")
}

fn path_string(path: &Path) -> Result<String, DependencyProjectError> {
    let path = fs::canonicalize(path).context(WriteSnafu {
        path: path.to_owned(),
    })?;
    path.to_str()
        .map(str::to_owned)
        .with_context(|| DependencySnafu {
            alias: "path",
            message: "path is not UTF-8",
        })
}

pub fn write_dependency_project(
    project: &Path,
    plan: &CompiledWorkflow,
    support: &SupportPackages,
) -> Result<(), DependencyProjectError> {
    write_dependency_project_with_options(project, plan, support, &RunnerOptions::default())
}

pub fn write_dependency_project_with_options(
    project: &Path,
    plan: &CompiledWorkflow,
    support: &SupportPackages,
    options: &RunnerOptions,
) -> Result<(), DependencyProjectError> {
    let plan_json = plan.to_json().context(PlanSnafu)?;
    let definition = &plan.definition;
    let mut features = Vec::new();
    if options.telemetry {
        features.push("\"telemetry\"");
    }
    let default_features = features.join(", ");
    let mut manifest = format!(
        "[package]\nname = \"mf-generated-workflow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[features]\ndefault = [{default_features}]\ntelemetry = [\"mf-telemetry/otlp\"]\n\n[dependencies]\nserde_json = \"1.0.151\"\nsnafu = \"0.9.2\"\n"
    );
    let mut build_dependencies = String::from("\n[build-dependencies]\n");
    for package in ["mf-runtime", "mf-compiler", "mf-telemetry"] {
        let source = match support {
            SupportPackages::Registry => format!(
                "version = {}",
                quoted(&format!("={}", env!("CARGO_PKG_VERSION")))
            ),
            SupportPackages::Local { crates_dir } => format!(
                "path = {}",
                quoted(&path_string(&crates_dir.join(package))?)
            ),
        };
        manifest.push_str(&format!(
            "{package} = {{ {source}, default-features = false }}\n"
        ));
        if package == "mf-compiler" {
            build_dependencies.push_str(&format!(
                "{package} = {{ {source}, default-features = false, features = [\"codegen\"] }}\n"
            ));
        } else if package == "mf-runtime" {
            build_dependencies.push_str(&format!(
                "{package} = {{ {source}, default-features = false }}\n"
            ));
        }
    }
    let mut main = String::new();
    let mut build = String::new();
    for (index, (alias, dependency)) in definition.dependencies.iter().enumerate() {
        dependency
            .validate()
            .map_err(|message| DependencyProjectError::Dependency {
                alias: alias.clone(),
                message,
            })?;
        let name = format!("node_{index}");
        main.push_str(&format!("extern crate {name} as _;\n"));
        build.push_str(&format!("extern crate {name} as _;\n"));
        let mut fields = vec![format!("package = {}", quoted(&dependency.package))];
        if let Some(version) = &dependency.version {
            fields.push(format!("version = {}", quoted(version)));
        }
        if let Some(git) = &dependency.git {
            fields.push(format!("git = {}", quoted(git)));
        }
        if let Some(rev) = &dependency.rev {
            fields.push(format!("rev = {}", quoted(rev)));
        }
        if let Some(path) = &dependency.path {
            fields.push(format!("path = {}", quoted(&path_string(path)?)));
        }
        fields.push(format!(
            "default-features = {}",
            dependency.default_features
        ));
        let features: Vec<_> = dependency.features.iter().map(|f| quoted(f)).collect();
        fields.push(format!("features = [{}]", features.join(", ")));
        manifest.push_str(&format!("{name} = {{ {} }}\n", fields.join(", ")));
        build_dependencies.push_str(&format!("{name} = {{ {} }}\n", fields.join(", ")));
    }
    let dependencies_offset = manifest
        .find("\n[dependencies]\n")
        .expect("generated dependency section");
    manifest.insert_str(dependencies_offset, &build_dependencies);
    build.push_str(BUILD);
    main.push_str(MAIN);
    for name in ["src", "target"] {
        let path = project.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return LinkedDirectorySnafu { path }.fail();
            }
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(source).context(WriteSnafu { path }),
        }
    }
    fs::create_dir_all(project.join("src")).context(WriteSnafu {
        path: project.to_owned(),
    })?;
    for (name, value) in [
        ("Cargo.toml", manifest.as_str()),
        ("src/main.rs", main.as_str()),
        ("build.rs", build.as_str()),
        (
            "src/workflow.rs",
            "include!(concat!(env!(\"OUT_DIR\"), \"/workflow.rs\"));\n",
        ),
        ("workflow-plan.json", plan_json.as_str()),
    ] {
        crate::state::write_if_changed(&project.join(name), value.as_bytes())
            .context(StateSnafu)?;
    }
    Ok(())
}
