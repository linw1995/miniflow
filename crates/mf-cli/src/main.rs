use clap::{Args, Parser, Subcommand};
use mf_compiler::{
    CompileRequest, PipelineError, RunnerOptions, SupportPackages, compile_project_with_options,
};
use snafu::Snafu;
#[cfg(feature = "development-support")]
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "mf", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compile a workflow definition into a native executable.
    Compile(CompileOptions),
    /// Run a workflow executable with the terminal UI.
    Run {
        /// Path to the workflow executable.
        executable: PathBuf,
        /// Observe workflow execution in the terminal UI.
        #[arg(long, required = true)]
        tui: bool,
        /// Workflow startup arguments keyed by initial node and input port.
        #[arg(long, value_name = "JSON", conflicts_with = "inputs_file")]
        inputs: Option<String>,
        /// Read workflow startup arguments from a JSON file (up to 1 MiB).
        #[arg(long, value_name = "PATH")]
        inputs_file: Option<PathBuf>,
    },
}

#[derive(Debug, Snafu)]
enum CliError {
    #[snafu(transparent)]
    Build { source: PipelineError },
    #[cfg(unix)]
    #[snafu(transparent)]
    Run { source: mf_tui::run::RunError },
    #[cfg(not(unix))]
    #[snafu(display("TUI execution is supported on Linux and macOS"))]
    UnsupportedTui,
}

#[derive(Debug, Args)]
struct CompileOptions {
    /// Path to the workflow definition.
    definition: PathBuf,
    /// Destination for the compiled executable.
    #[arg(long, value_name = "PATH")]
    output: PathBuf,
    /// Require the existing workflow dependency lock.
    #[arg(long)]
    locked: bool,
    /// Disable telemetry in the generated runner.
    #[arg(long = "no-telemetry", action = clap::ArgAction::SetFalse)]
    telemetry: bool,
    /// Directory for reusable generated sources and build artifacts.
    #[arg(long, value_name = "PATH")]
    build_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<u8, CliError> {
    match cli.command {
        Command::Compile(options) => {
            compile(options)?;
            Ok(0)
        }
        Command::Run {
            executable,
            inputs,
            inputs_file,
            ..
        } => {
            #[cfg(unix)]
            {
                Ok(mf_tui::run::run_executable_with_options(
                    &executable,
                    &mf_tui::run::RunOptions {
                        inputs,
                        inputs_file,
                    },
                )?)
            }
            #[cfg(not(unix))]
            {
                let _ = (executable, inputs, inputs_file);
                UnsupportedTuiSnafu.fail()
            }
        }
    }
}

fn compile(options: CompileOptions) -> Result<(), CliError> {
    let support = support_packages();
    compile_project_with_options(
        &CompileRequest {
            definition: &options.definition,
            output: &options.output,
            locked: options.locked,
            build_dir: options.build_dir.as_deref(),
            support: &support,
        },
        &RunnerOptions {
            telemetry: options.telemetry,
        },
    )?;
    Ok(())
}

fn support_packages() -> SupportPackages {
    #[cfg(feature = "development-support")]
    {
        let crates_dir = env::var_os("MF_DEV_SUPPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
        SupportPackages::Local { crates_dir }
    }
    #[cfg(not(feature = "development-support"))]
    SupportPackages::Registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compile_defaults() {
        let cli = Cli::try_parse_from(["mf", "compile", "flow.json", "--output", "flow"]).unwrap();
        let Command::Compile(options) = cli.command else {
            panic!("expected compile command");
        };
        assert_eq!(options.definition, PathBuf::from("flow.json"));
        assert_eq!(options.output, PathBuf::from("flow"));
        assert!(!options.locked);
        assert!(options.telemetry);
        assert_eq!(options.build_dir, None);
    }

    #[test]
    fn parses_compile_options_in_any_order() {
        for args in [
            [
                "mf",
                "compile",
                "flow.json",
                "--locked",
                "--no-telemetry",
                "--output",
                "flow",
                "--build-dir",
                "build",
            ],
            [
                "mf",
                "compile",
                "--output",
                "flow",
                "--build-dir",
                "build",
                "--no-telemetry",
                "--locked",
                "flow.json",
            ],
        ] {
            let Command::Compile(options) = Cli::try_parse_from(args).unwrap().command else {
                panic!("expected compile command");
            };
            assert_eq!(options.definition, PathBuf::from("flow.json"));
            assert_eq!(options.output, PathBuf::from("flow"));
            assert!(options.locked);
            assert!(!options.telemetry);
            assert_eq!(options.build_dir, Some(PathBuf::from("build")));
        }
    }

    #[test]
    fn rejects_repeated_compile_options() {
        for repeated in [
            &["--locked", "--locked"][..],
            &["--no-telemetry", "--no-telemetry"],
            &["--output", "another"],
            &["--build-dir", "build", "--build-dir", "another"],
        ] {
            let args = ["mf", "compile", "flow.json", "--output", "flow"]
                .into_iter()
                .chain(repeated.iter().copied());
            let error = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn parses_run_options_in_either_order() {
        for args in [
            ["mf", "run", "./flow", "--tui"],
            ["mf", "run", "--tui", "./flow"],
        ] {
            let Command::Run {
                executable, tui, ..
            } = Cli::try_parse_from(args).unwrap().command
            else {
                panic!("expected run command");
            };
            assert_eq!(executable, PathBuf::from("./flow"));
            assert!(tui);
        }
    }

    #[test]
    fn run_options_parse_startup_arguments_and_reject_conflicts() {
        let cli = Cli::try_parse_from(["mf", "run", "./flow", "--tui", "--inputs", "{}"]).unwrap();
        let Command::Run {
            inputs,
            inputs_file,
            ..
        } = cli.command
        else {
            panic!("expected run command");
        };
        assert_eq!(inputs.as_deref(), Some("{}"));
        assert!(inputs_file.is_none());
        for arguments in [
            vec!["--inputs", "{}", "--inputs-file", "args.json"],
            vec!["--inputs", "{}", "--inputs", "{}"],
        ] {
            assert!(
                Cli::try_parse_from(
                    ["mf", "run", "./flow", "--tui"]
                        .into_iter()
                        .chain(arguments)
                )
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = OsString::from_vec(b"flow-\xff".to_vec());
        let cli = Cli::try_parse_from([
            OsString::from("mf"),
            "compile".into(),
            path.clone(),
            "--output".into(),
            path.clone(),
            "--build-dir".into(),
            path.clone(),
        ])
        .unwrap();
        let Command::Compile(options) = cli.command else {
            panic!("expected compile command");
        };
        assert_eq!(options.definition, PathBuf::from(&path));
        assert_eq!(options.output, PathBuf::from(&path));
        assert_eq!(options.build_dir, Some(PathBuf::from(&path)));

        let cli = Cli::try_parse_from([
            OsString::from("mf"),
            "run".into(),
            path.clone(),
            "--tui".into(),
        ])
        .unwrap();
        let Command::Run { executable, .. } = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(executable, PathBuf::from(path));
    }
}
