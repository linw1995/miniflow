use mf_compiler::{
    BinaryBuildError, DefinitionParseError, NodeRegistryError, PlanError, WorkflowCompileError,
    WorkflowDefinition, build_executable, compile_definition,
};
use snafu::{ResultExt, Snafu};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "Usage: mf compile <definition> --output <path> [--locked]";

#[derive(Debug, Snafu)]
enum CliError {
    #[snafu(display("{message}\n{USAGE}"))]
    Usage { message: String },
    #[snafu(display("could not read definition {path:?}: {source}"))]
    ReadDefinition { path: PathBuf, source: io::Error },
    #[snafu(display("could not resolve definition path {path:?}: {source}"))]
    ResolveDefinition { path: PathBuf, source: io::Error },
    #[snafu(display("could not resolve output path {path:?}: {source}"))]
    ResolveOutput { path: PathBuf, source: io::Error },
    #[snafu(display("output path {path:?} would overwrite the source definition"))]
    OutputOverwritesDefinition { path: PathBuf },
    #[snafu(display("could not parse definition: {source}"))]
    ParseDefinition { source: DefinitionParseError },
    #[snafu(display("could not load plugin registry: {source}"))]
    Registry { source: NodeRegistryError },
    #[snafu(display("could not compile workflow: {source}"))]
    Compile { source: WorkflowCompileError },
    #[snafu(display("could not generate workflow source: {source}"))]
    Generate { source: PlanError },
    #[snafu(display("could not read current directory: {source}"))]
    CurrentDirectory { source: io::Error },
    #[snafu(display(
        "could not find a source workspace for {definition:?}; run from a miniflow source checkout"
    ))]
    WorkspaceNotFound { definition: PathBuf },
    #[snafu(display("could not build workflow executable: {source}"))]
    Build { source: BinaryBuildError },
}

struct CompileOptions {
    definition: PathBuf,
    output: PathBuf,
    locked: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), CliError> {
    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        return UsageSnafu {
            message: "missing command",
        }
        .fail();
    };
    if command == OsStr::new("--help") || command == OsStr::new("-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if command != OsStr::new("compile") {
        return UsageSnafu {
            message: "unknown command",
        }
        .fail();
    }
    let options = parse_compile_args(args)?;
    compile(options)
}

fn parse_compile_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<CompileOptions, CliError> {
    let Some(definition) = args.next() else {
        return UsageSnafu {
            message: "missing definition path",
        }
        .fail();
    };
    let mut output = None;
    let mut locked = false;
    while let Some(option) = args.next() {
        if option == OsStr::new("--locked") && !locked {
            locked = true;
        } else if option == OsStr::new("--output") && output.is_none() {
            output = Some(args.next().ok_or_else(|| CliError::Usage {
                message: "missing output path".into(),
            })?);
        } else {
            return UsageSnafu {
                message: "unexpected or repeated argument",
            }
            .fail();
        }
    }
    let output = output.ok_or_else(|| CliError::Usage {
        message: "missing --output option".into(),
    })?;
    Ok(CompileOptions {
        definition: definition.into(),
        output: output.into(),
        locked,
    })
}

fn compile(options: CompileOptions) -> Result<(), CliError> {
    if options.locked {
        return UsageSnafu {
            message: "--locked requires the project dependency build pipeline",
        }
        .fail();
    }
    let source = fs::read_to_string(&options.definition).context(ReadDefinitionSnafu {
        path: options.definition.clone(),
    })?;
    let definition = WorkflowDefinition::from_json(&source).context(ParseDefinitionSnafu)?;
    let registry = mf_bundle::registry().context(RegistrySnafu)?;
    let compiled = compile_definition(&definition, &registry).context(CompileSnafu)?;
    let artifacts = compiled.generate_artifacts().context(GenerateSnafu)?;

    let definition_path =
        fs::canonicalize(&options.definition).context(ResolveDefinitionSnafu {
            path: options.definition.clone(),
        })?;
    match fs::canonicalize(&options.output) {
        Ok(path) if path == definition_path => {
            return OutputOverwritesDefinitionSnafu {
                path: options.output.clone(),
            }
            .fail();
        }
        Ok(_) => {}
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(CliError::ResolveOutput {
                path: options.output.clone(),
                source,
            });
        }
    }
    let cwd = env::current_dir().context(CurrentDirectorySnafu)?;
    let workspace = definition_path
        .parent()
        .and_then(find_workspace)
        .or_else(|| find_workspace(&cwd))
        .ok_or_else(|| CliError::WorkspaceNotFound {
            definition: definition_path,
        })?;
    let crates_dir = workspace.join("crates");
    build_executable(
        &artifacts,
        &options.output,
        &crates_dir.join("mf-runtime"),
        &crates_dir.join("mf-bundle"),
    )
    .context(BuildSnafu)
}

fn find_workspace(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|candidate| {
        if candidate.join("crates/mf-runtime/Cargo.toml").is_file()
            && candidate.join("crates/mf-bundle/Cargo.toml").is_file()
        {
            Some(candidate.to_path_buf())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_locked_in_either_option_order() {
        for args in [
            ["flow.json", "--locked", "--output", "flow"],
            ["flow.json", "--output", "flow", "--locked"],
        ] {
            let options = parse_compile_args(args.into_iter().map(OsString::from)).unwrap();
            assert!(options.locked);
            assert_eq!(options.output, PathBuf::from("flow"));
        }
        assert!(
            parse_compile_args(
                ["f", "--locked", "--locked"]
                    .into_iter()
                    .map(OsString::from)
            )
            .is_err()
        );
    }
}
