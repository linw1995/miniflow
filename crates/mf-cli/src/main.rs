use clap::{Args, CommandFactory, Parser, Subcommand};
use mf_compiler::{
    CompileRequest, PipelineError, RunnerOptions, SupportPackages, compile_project_with_options,
};
use snafu::{ResultExt, Snafu};
#[cfg(feature = "development-support")]
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "mf", version, about)]
struct Cli {
    /// Increase execution detail: -v summaries, -vv metadata, -vvv full records.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    fn verbosity(&self) -> u8 {
        // Global Count arguments overwrite parent counts when repeated in a subcommand.
        let local = match &self.command {
            Command::Compile(options) => options.verbose,
            Command::Run { verbose, .. } => *verbose,
        };
        self.verbose.saturating_add(local).min(3)
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compile a workflow definition into a native executable.
    Compile(CompileOptions),
    /// Run a workflow executable.
    Run {
        /// Increase execution detail: -v summaries, -vv metadata, -vvv full records.
        #[arg(short, long, action = clap::ArgAction::Count)]
        verbose: u8,
        /// Path to the workflow executable.
        executable: PathBuf,
        /// Observe workflow execution in the terminal UI.
        #[arg(long)]
        tui: bool,
        /// Keep an interactive conversation using this executable.
        #[arg(long, requires_all = ["tui", "chat_db"])]
        chat: bool,
        /// SQLite database owned by this chat application.
        #[arg(long, value_name = "PATH", requires = "chat")]
        chat_db: Option<PathBuf>,
        /// Session profile label; model configuration stays in the executable.
        #[arg(long, value_name = "NAME", requires = "chat")]
        chat_profile: Option<String>,
        /// Workflow startup arguments keyed by qualified node.port names.
        #[arg(long, value_name = "JSON", conflicts_with = "inputs_file")]
        inputs: Option<String>,
        /// Read workflow startup arguments from a JSON file (up to 1 MiB).
        #[arg(long, value_name = "PATH")]
        inputs_file: Option<PathBuf>,
        /// Workflow parameters, such as --read.path /data/events.txt.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, conflicts_with_all = ["inputs", "inputs_file"])]
        arguments: Vec<std::ffi::OsString>,
    },
}

#[derive(Debug, Snafu)]
enum CliError {
    #[snafu(display("could not run workflow {path:?}: {source}"))]
    Execute { path: PathBuf, source: io::Error },
    #[cfg(unix)]
    #[snafu(display("could not inspect workflow {path:?}: {source}"))]
    Inspect {
        path: PathBuf,
        source: mf_tui::manifest::ManifestReadError,
    },
    #[snafu(display("could not write command help: {source}"))]
    HelpOutput { source: io::Error },
    #[snafu(transparent)]
    Build { source: PipelineError },
    #[cfg(unix)]
    #[snafu(transparent)]
    Run { source: mf_tui::run::RunError },
    #[cfg(unix)]
    #[snafu(transparent)]
    Chat { source: mf_tui::chat::ChatRunError },
    #[cfg(not(unix))]
    #[snafu(display("TUI execution is supported on Linux and macOS"))]
    UnsupportedTui,
    #[cfg(not(unix))]
    #[snafu(display("workflow manifest inspection is supported on Linux and macOS"))]
    UnsupportedInspection,
}

struct RunHelp {
    executable: Option<PathBuf>,
}

impl RunHelp {
    fn parse(arguments: &[OsString]) -> Option<Self> {
        let command = Cli::command().mut_subcommand("run", |command| {
            command
                .disable_help_flag(true)
                .mut_args(|argument| {
                    argument
                        .required(false)
                        .requires(clap::builder::Resettable::Reset)
                        .conflicts_with(clap::builder::Resettable::Reset)
                })
                .arg(
                    clap::Arg::new("help")
                        .short('h')
                        .long("help")
                        .action(clap::ArgAction::SetTrue),
                )
        });
        // A failed probe goes through the unchanged parser for its normal diagnostics.
        let matches = command.try_get_matches_from(arguments).ok()?;
        let matches = matches.subcommand_matches("run")?;
        if !matches.get_flag("help") {
            return None;
        }
        Some(Self {
            executable: matches.get_one::<PathBuf>("executable").cloned(),
        })
    }

    fn print(self) -> Result<u8, CliError> {
        let mut command = Cli::command();
        command.build();
        let cli_help = command
            .find_subcommand_mut("run")
            .expect("run subcommand")
            .render_help();
        let Some(executable) = self.executable else {
            writeln!(io::stdout().lock(), "{cli_help}").context(HelpOutputSnafu)?;
            return Ok(0);
        };
        #[cfg(unix)]
        {
            let manifest = mf_tui::manifest::read_manifest(&executable)
                .context(InspectSnafu { path: executable })?;
            let runner_help = mf_runtime::runner_help_command(&manifest.input_schema).render_help();
            writeln!(
                io::stdout().lock(),
                "CLI help:\n{cli_help}\n\nRunner help:\n{runner_help}"
            )
            .context(HelpOutputSnafu)?;
            Ok(0)
        }
        #[cfg(not(unix))]
        {
            let _ = executable;
            UnsupportedInspectionSnafu.fail()
        }
    }
}

#[derive(Debug, Args)]
struct CompileOptions {
    /// Increase diagnostic verbosity.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
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
    match invoke(std::env::args_os().collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn invoke(arguments: Vec<OsString>) -> Result<u8, CliError> {
    if let Some(help) = RunHelp::parse(&arguments) {
        return help.print();
    }
    let cli = Cli::parse_from(arguments);
    if let Command::Run { arguments, .. } = &cli.command
        && arguments.first().is_some_and(|argument| {
            !argument
                .to_str()
                .is_some_and(|argument| argument.starts_with("--"))
        })
    {
        Cli::command()
            .error(
                clap::error::ErrorKind::UnknownArgument,
                "workflow parameters must start with --node.port",
            )
            .exit();
    }
    run(cli)
}

fn run(cli: Cli) -> Result<u8, CliError> {
    let verbose = cli.verbosity();
    match cli.command {
        Command::Compile(options) => {
            compile(options, verbose > 0)?;
            Ok(0)
        }
        Command::Run {
            executable,
            tui,
            chat,
            chat_db,
            chat_profile,
            inputs,
            inputs_file,
            arguments,
            ..
        } => {
            if !tui {
                let mut command = std::process::Command::new(&executable);
                for _ in 0..verbose {
                    command.arg("--verbose");
                }
                if let Some(inputs) = inputs {
                    command.arg("--inputs").arg(inputs);
                }
                if let Some(path) = inputs_file {
                    command.arg("--inputs-file").arg(path);
                }
                let status = command
                    .args(arguments)
                    .status()
                    .context(ExecuteSnafu { path: executable })?;
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    return Ok(status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
                        as u8);
                }
                #[cfg(not(unix))]
                return Ok(status.code().unwrap_or(1) as u8);
            }
            #[cfg(unix)]
            {
                let options = mf_tui::run::RunOptions {
                    verbose,
                    inputs,
                    inputs_file,
                    arguments,
                };
                if chat {
                    Ok(mf_tui::chat::run_chat(
                        &executable,
                        mf_tui::chat::ChatOptions {
                            database_path: chat_db.expect("clap requires chat database"),
                            profile: chat_profile.unwrap_or_else(|| "default".into()),
                            run: options,
                        },
                    )?)
                } else {
                    Ok(mf_tui::run::run_executable_with_options(
                        &executable,
                        &options,
                    )?)
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (
                    executable,
                    inputs,
                    inputs_file,
                    arguments,
                    chat,
                    chat_db,
                    chat_profile,
                );
                UnsupportedTuiSnafu.fail()
            }
        }
    }
}

fn compile(options: CompileOptions, verbose: bool) -> Result<(), CliError> {
    let support = support_packages();
    let started = std::time::Instant::now();
    if verbose {
        eprintln!(
            "[verbose] compiling {:?} to {:?} (locked={}, telemetry={}, support={support:?})",
            options.definition, options.output, options.locked, options.telemetry
        );
    }
    let project = compile_project_with_options(
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
    if verbose {
        eprintln!(
            "[verbose] installed {:?} in {:.3}s; build project at {project:?}",
            options.output,
            started.elapsed().as_secs_f64()
        );
    }
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
    fn parses_verbose_for_compile_and_direct_run() {
        for args in [
            vec!["mf", "-v", "compile", "flow.json", "--output", "flow"],
            vec!["mf", "run", "./flow", "-v", "--read.path", "data.txt"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cli.verbosity(), 1);
            if let Command::Run { tui, .. } = cli.command {
                assert!(!tui);
            }
        }
    }

    #[test]
    fn verbosity_clusters_preserve_option_like_workflow_values() {
        for (flags, expected) in [
            (vec!["-vv"], 2),
            (vec!["-v", "--verbose"], 2),
            (vec!["-vvv"], 3),
            (vec!["-vvvv"], 4),
        ] {
            let cli = Cli::try_parse_from(
                ["mf", "run", "flow"]
                    .into_iter()
                    .chain(flags)
                    .chain(["--read.path", "-vvv"]),
            )
            .unwrap();
            assert_eq!(cli.verbosity(), expected.min(3));
            let Command::Run { arguments, .. } = cli.command else {
                panic!("expected run command");
            };
            assert_eq!(
                arguments,
                ["--read.path", "-vvv"].map(std::ffi::OsString::from)
            );
        }
    }

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
    fn passes_named_workflow_parameters_after_tui_options() {
        for separator in [false, true] {
            let mut flags = vec!["mf", "run", "./flow", "--tui"];
            if separator {
                flags.push("--");
            }
            flags.extend(["--read.path", "data.txt", "--repeat.count=-2"]);
            let Command::Run { arguments, .. } = Cli::try_parse_from(flags).unwrap().command else {
                panic!("expected run command");
            };
            assert_eq!(
                arguments,
                ["--read.path", "data.txt", "--repeat.count=-2"].map(std::ffi::OsString::from)
            );
        }
        assert!(
            Cli::try_parse_from([
                "mf",
                "run",
                "./flow",
                "--tui",
                "--inputs",
                "{}",
                "--read.path",
                "data.txt"
            ])
            .is_err()
        );
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

#[cfg(test)]
mod chat_tests {
    use super::*;
    #[test]
    fn chat_options_require_terminal_mode_database_and_explicit_chat() {
        for args in [
            vec!["mf", "run", "flow", "--chat"],
            vec!["mf", "run", "flow", "--chat", "--chat-db", "chat.sqlite"],
            vec!["mf", "run", "flow", "--tui", "--chat-db", "chat.sqlite"],
            vec!["mf", "run", "flow", "--chat-profile", "default"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        let cli = Cli::try_parse_from([
            "mf",
            "run",
            "flow",
            "--tui",
            "--chat",
            "--chat-db",
            "chat.sqlite",
            "--chat-profile",
            "work",
            "--task.path",
            "input",
        ])
        .unwrap();
        let Command::Run {
            chat,
            chat_db,
            chat_profile,
            arguments,
            ..
        } = cli.command
        else {
            panic!("expected run")
        };
        assert!(chat);
        assert_eq!(chat_db, Some(PathBuf::from("chat.sqlite")));
        assert_eq!(chat_profile.as_deref(), Some("work"));
        assert_eq!(arguments.len(), 2);
    }
}
