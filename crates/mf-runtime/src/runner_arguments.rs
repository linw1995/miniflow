use crate::{MAX_WORKFLOW_INPUT_BYTES, WorkflowArguments, WorkflowInputError};
use snafu::{ResultExt, Snafu};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum RunnerCommand {
    Describe,
    DescribeInterface,
    Validate,
    Execute(WorkflowArguments),
}

#[derive(Debug, Snafu)]
pub enum RunnerArgumentError {
    #[snafu(display(
        "{message}; usage: workflow [--validate|--describe|--describe-interface|--inputs JSON|--inputs-file PATH]"
    ))]
    Invalid { message: String },
    #[snafu(display("could not read workflow arguments from {path:?}: {source}"))]
    Read { path: PathBuf, source: io::Error },
    #[snafu(transparent)]
    Arguments { source: WorkflowInputError },
}

impl WorkflowArguments {
    pub fn from_file(path: &Path) -> Result<Self, RunnerArgumentError> {
        let read = || -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            File::open(path)?
                .take(MAX_WORKFLOW_INPUT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        let bytes = read().context(ReadSnafu {
            path: path.to_owned(),
        })?;
        Ok(Self::from_json(&bytes)?)
    }
}

impl RunnerCommand {
    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, RunnerArgumentError> {
        let invalid = |message: &str| {
            InvalidSnafu {
                message: message.to_owned(),
            }
            .build()
        };
        let mut arguments = arguments.into_iter();
        let mut mode = None;
        let mut inputs = None;
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--describe" | "--describe-interface" | "--validate") => {
                    if mode.is_some() {
                        return Err(invalid("select one inspection or validation mode"));
                    }
                    mode = Some(match argument.to_str().unwrap() {
                        "--describe" => Self::Describe,
                        "--describe-interface" => Self::DescribeInterface,
                        _ => Self::Validate,
                    });
                }
                Some("--inputs" | "--inputs-file") => {
                    if inputs.is_some() {
                        return Err(invalid(
                            "--inputs and --inputs-file can be supplied only once and are mutually exclusive",
                        ));
                    }
                    let value = arguments
                        .next()
                        .ok_or_else(|| invalid("missing workflow argument value"))?;
                    inputs = Some((argument == "--inputs-file", value));
                }
                _ => return Err(invalid("unknown workflow argument")),
            }
        }
        if let Some(mode) = mode {
            if inputs.is_some() {
                return Err(invalid(
                    "inspection and validation modes do not accept execution arguments",
                ));
            }
            return Ok(mode);
        }
        let values = match inputs {
            None => WorkflowArguments::default(),
            Some((true, path)) => WorkflowArguments::from_file(Path::new(&path))?,
            Some((false, json)) => WorkflowArguments::from_json(
                json.to_str()
                    .ok_or_else(|| invalid("--inputs requires UTF-8 JSON"))?
                    .as_bytes(),
            )?,
        };
        Ok(Self::Execute(values))
    }
}
