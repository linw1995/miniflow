use crate::CompiledWorkflow;
use snafu::{ResultExt, Snafu};
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
    if let Err(error) = generate() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let workflow = mf_compiler::CompiledWorkflow::from_json(include_str!("workflow-plan.json"))?;
    let source = workflow.generate_execution_plans(&registry)?;
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    std::fs::write(output.join("flow-plans.rs"), source)?;
    Ok(())
}
"#;

const MAIN: &str = r#"mod workflow;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => { eprintln!("{error}"); std::process::ExitCode::FAILURE }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = mf_runtime::RunnerCommand::parse(std::env::args_os().skip(1))?;
    match command {
        mf_runtime::RunnerCommand::Describe => {
            use std::io::Write;
            let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
            let description = mf_compiler::describe_compiled(&plan)?;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&description.to_json()?)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
            return Ok(());
        }
        mf_runtime::RunnerCommand::DescribeInterface => {
            let mut stdio = mf_runtime::StreamStdio::claim()?;
            let registry = mf_runtime::NodeRegistry::from_inventory()?;
            let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
            let interface = mf_compiler::describe_interface(&plan, &registry)?;
            stdio.write_json(&interface)?;
            return Ok(());
        }
        mf_runtime::RunnerCommand::Validate => {
            let registry = mf_runtime::NodeRegistry::from_inventory()?;
            #[cfg(feature = "streaming")]
            { workflow::prepare_stream(&registry)?; }
            #[cfg(not(feature = "streaming"))]
            { workflow::prepare_workflow(&registry, None)?; }
            return Ok(());
        }
        mf_runtime::RunnerCommand::Execute(_) => {}
    }
    let mf_runtime::RunnerCommand::Execute(arguments) = command else { unreachable!() };
    #[cfg(feature = "streaming")]
    { execute_stream(arguments) }
    #[cfg(not(feature = "streaming"))]
    {
        let outputs = execute(arguments)?;
        println!("{}", serde_json::to_string(&outputs)?);
        Ok(())
    }
}

#[cfg(feature = "streaming")]
fn execute_stream(arguments: mf_runtime::WorkflowArguments) -> Result<(), Box<dyn std::error::Error>> {
    if capture_requested() { return Err("snapshot capture is unsupported for streaming instances".into()); }
    let mut stdio = mf_runtime::StreamStdio::claim()?;
    #[cfg(feature = "telemetry")]
    let providers = match mf_telemetry::otlp::TelemetryProviders::from_env() {
        Ok(providers) => providers,
        Err(error) => { eprintln!("telemetry export unavailable: {error}"); None }
    };
    #[cfg(feature = "telemetry")]
    let observation = providers.as_ref().and_then(|providers| {
        let setup = (|| -> Result<_, Box<dyn std::error::Error>> {
            let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
            Ok(plan.start_stream_observation(&providers.observer(), run_id())?)
        })();
        match setup {
            Ok(observation) => Some(observation),
            Err(error) => { eprintln!("telemetry observation unavailable: {error}"); None }
        }
    });
    #[cfg(not(feature = "telemetry"))]
    let observation: Option<mf_runtime::StreamObservation> = None;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let _context = observation.as_ref().map(mf_runtime::StreamObservation::enter);
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let prepared = workflow::prepare_stream(&registry).inspect_err(|error| {
            if let Some(observation) = &observation { observation.preparation_failed(error.to_string()); }
        })?;
        prepared.plan().input_schema().validate(&arguments)?;
        let needs_stdin = prepared.plan().input_schema().stdin_owner(&arguments)?.is_some();
        let stdin = if needs_stdin { stdio.take_input() } else { None };
        let instance = mf_runtime::FlowRuntime::default().start_stream(prepared, mf_runtime::StreamOptions { observation: observation.clone(), arguments, stdin, ..Default::default() })?;
        stdio.run(instance)?;
        Ok(())
    })();
    if let (Err(error), Some(observation)) = (&result, &observation) { observation.preparation_failed(error.to_string()); }
    #[cfg(feature = "telemetry")]
    if let Some(providers) = providers {
        for diagnostic in providers.shutdown() { eprintln!("telemetry export incomplete: {diagnostic}"); }
    }
    result
}

#[cfg(feature = "telemetry")]
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

#[cfg(all(not(feature = "telemetry"), not(feature = "streaming")))]
fn execute(arguments: mf_runtime::WorkflowArguments) -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    if capture_requested() {
        eprintln!("data history unavailable: recompile the runner without --no-telemetry");
    }
    execute_workflow(None, None, arguments)
}

#[cfg(all(feature = "telemetry", not(feature = "streaming")))]
fn execute(arguments: mf_runtime::WorkflowArguments) -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let providers = match mf_telemetry::otlp::TelemetryProviders::from_env() {
        Ok(providers) => providers,
        Err(error) => { eprintln!("telemetry export unavailable: {error}"); None }
    };
    let result = (|| {
        let Some(providers) = providers.as_ref() else {
            if capture_requested() { eprintln!("data history unavailable: configure an OTLP logs endpoint"); }
            return execute_workflow(None, None, arguments);
        };
        let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
        let run_id = run_id();
        let observation = match plan.start_observation(&providers.observer(), run_id) {
            Ok(observation) => Some(observation),
            Err(error) => { eprintln!("telemetry observation unavailable: {error}"); None }
        };
        let snapshots = if capture_requested() {
            match snapshot_recorder(&plan, run_id) {
                Ok(recorder) => Some(recorder),
                Err(error) => { eprintln!("data history unavailable: {error}"); None }
            }
        } else { None };
        execute_workflow(observation, snapshots, arguments)
    })();
    if let Some(providers) = providers {
        for diagnostic in providers.shutdown() { eprintln!("telemetry export incomplete: {diagnostic}"); }
    }
    result
}

#[cfg(all(feature = "telemetry", not(feature = "streaming")))]
fn snapshot_recorder(plan: &mf_compiler::CompiledWorkflow, run_id: mf_telemetry::identity::RunId) -> Result<mf_runtime::SnapshotRecorder, Box<dyn std::error::Error>> {
    let description = mf_compiler::describe_compiled(plan)?;
    let mut exporter = mf_telemetry::otlp::SnapshotExporter::from_env(description.workflow_id, run_id)?;
    Ok(mf_runtime::SnapshotRecorder::with_sink(move |record| {
        exporter.emit(serde_json::to_value(record)?)?;
        if matches!(record, mf_runtime::SnapshotRecord::End) { exporter.finish()?; }
        Ok(())
    })?)
}

#[cfg(not(feature = "streaming"))]
fn execute_workflow(observation: Option<mf_runtime::RunObservation>, snapshots: Option<mf_runtime::SnapshotRecorder>, arguments: mf_runtime::WorkflowArguments) -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let result = mf_runtime::ExecutionContext::run(observation, |state| -> Result<_, Box<dyn std::error::Error>> {
        state.set_workflow_arguments(arguments);
        #[cfg(unix)]
        {
            let plan = mf_compiler::CompiledWorkflow::from_json(include_str!("../workflow-plan.json"))?;
            if plan.definition.version.supports_startup_inputs() {
                state.set_stdin(mf_runtime::TextInput::claim()?);
            }
        }
        if let Some(snapshots) = &snapshots { state.set_snapshot_recorder(snapshots.clone()); }
        let registry = mf_runtime::NodeRegistry::from_inventory()?;
        let flow = workflow::prepare_workflow(&registry, state.observation_mut())?;
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
        .ok_or_else(|| DependencyProjectError::Dependency {
            alias: "path".into(),
            message: "path is not UTF-8".into(),
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
    let artifacts = plan.generate_artifacts().context(PlanSnafu)?;
    let definition = &plan.definition;
    let mut features = Vec::new();
    if options.telemetry {
        features.push("\"telemetry\"");
    }
    if definition.execution.is_some() {
        features.push("\"streaming\"");
    }
    let default_features = features.join(", ");
    let mut manifest = format!(
        "[package]\nname = \"mf-generated-workflow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[features]\ndefault = [{default_features}]\ntelemetry = [\"mf-telemetry/otlp\"]\nstreaming = []\n\n[dependencies]\nserde_json = \"1.0.151\"\nsnafu = \"0.9.2\"\n"
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
            Err(source) => return Err(DependencyProjectError::Write { path, source }),
        }
    }
    fs::create_dir_all(project.join("src")).context(WriteSnafu {
        path: project.to_owned(),
    })?;
    for (name, value) in [
        ("Cargo.toml", manifest.as_str()),
        ("src/main.rs", main.as_str()),
        ("build.rs", build.as_str()),
        ("src/workflow.rs", artifacts.rust_source.as_str()),
        ("workflow-plan.json", artifacts.plan_json.as_str()),
    ] {
        crate::state::write_if_changed(&project.join(name), value.as_bytes())
            .context(StateSnafu)?;
    }
    Ok(())
}
