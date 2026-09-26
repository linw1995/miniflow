use mf_compiler::{CompileRequest, PipelineError, SupportPackages, compile_project};
use snafu::{ResultExt, Snafu};
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "Usage: mf compile <definition> --output <path> [--locked]";

#[derive(Debug, Snafu)]
enum CliError {
    #[snafu(display("{message}\n{USAGE}"))]
    Usage { message: String },
    #[snafu(display("{source}"))]
    Build { source: PipelineError },
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
    let support = support_packages();
    compile_project(&CompileRequest {
        definition: &options.definition,
        output: &options.output,
        locked: options.locked,
        support: &support,
    })
    .context(BuildSnafu)?;
    Ok(())
}

fn support_packages() -> SupportPackages {
    #[cfg(feature = "development-support")]
    if let Some(path) = env::var_os("MF_DEV_SUPPORT_ROOT") {
        return SupportPackages::Local {
            crates_dir: path.into(),
        };
    }
    SupportPackages::Registry
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
