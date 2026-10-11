use crate::{
    MAX_WORKFLOW_INPUT_BYTES, ValueType, WorkflowArguments, WorkflowInputError, WorkflowInputSchema,
};
use snafu::{ResultExt, Snafu};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum RunnerArgumentError {
    #[snafu(display(
        "{message}; usage: workflow [-v|-vv|-vvv|--verbose] [--NODE.PORT VALUE ...] [--inputs JSON|--inputs-file PATH]"
    ))]
    Invalid { message: String },
    #[snafu(display("could not read workflow arguments from {path:?}: {source}"))]
    Read { path: PathBuf, source: io::Error },
    #[snafu(display("workflow argument `--{name}`: {source}"))]
    JsonValue {
        name: String,
        source: serde_json::Error,
    },
    #[snafu(transparent)]
    Arguments { source: WorkflowInputError },
}

#[derive(Debug)]
pub struct RunnerArguments {
    /// Local diagnostic detail, capped at three.
    pub verbose: u8,
    pub inputs: WorkflowArguments,
}

impl RunnerArguments {
    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
        schema: &WorkflowInputSchema,
    ) -> Result<Self, RunnerArgumentError> {
        WorkflowArguments::parse_options(arguments, schema, true)
    }
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

    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
        schema: &WorkflowInputSchema,
    ) -> Result<Self, RunnerArgumentError> {
        Ok(Self::parse_options(arguments, schema, false)?.inputs)
    }

    fn parse_options(
        arguments: impl IntoIterator<Item = OsString>,
        schema: &WorkflowInputSchema,
        allow_verbose: bool,
    ) -> Result<RunnerArguments, RunnerArgumentError> {
        let invalid = |message: &str| {
            InvalidSnafu {
                message: message.to_owned(),
            }
            .build()
        };
        let mut arguments = arguments.into_iter();
        let mut verbose = 0_u8;
        let mut inputs = None;
        let mut values = Self::default();
        let mut size = 0;
        while let Some(argument) = arguments.next() {
            size += argument.len();
            Self::check_size(size)?;
            match argument.to_str() {
                Some(flag)
                    if allow_verbose
                        && (flag == "--verbose"
                            || (flag.starts_with("-v")
                                && flag[1..].bytes().all(|byte| byte == b'v'))) =>
                {
                    let count = if flag == "--verbose" {
                        1
                    } else {
                        flag.len() - 1
                    };
                    verbose = (usize::from(verbose) + count).min(3) as u8;
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
                Some(flag) if flag.starts_with("--") => {
                    let (name, inline) = flag[2..]
                        .split_once('=')
                        .map_or((&flag[2..], None), |(name, value)| (name, Some(value)));
                    let input = schema.parameter(name).ok_or_else(|| {
                        InvalidSnafu {
                            message: format!("unknown workflow argument `--{name}`"),
                        }
                        .build()
                    })?;
                    let value = match inline {
                        Some(value) => OsString::from(value),
                        None => arguments
                            .next()
                            .filter(|value| !value.to_string_lossy().starts_with("--"))
                            .ok_or_else(|| invalid("missing workflow argument value"))?,
                    };
                    if inline.is_none() {
                        size += value.len();
                    }
                    Self::check_size(size)?;
                    let value = value
                        .to_str()
                        .ok_or_else(|| invalid("workflow input values must be UTF-8"))?;
                    let value = match &input.value_type {
                        ValueType::String => serde_json::Value::String(value.to_owned()).into(),
                        ValueType::Nullable(inner)
                            if **inner == ValueType::String && value != "null" =>
                        {
                            serde_json::Value::String(value.to_owned()).into()
                        }
                        _ => crate::workflow_inputs::from_unique_json::<serde_json::Value>(
                            value.as_bytes(),
                        )
                        .context(JsonValueSnafu { name })?
                        .into(),
                    };
                    if values.0.insert(name.into(), value).is_some() {
                        return Err(invalid("workflow input can be supplied only once"));
                    }
                }
                _ => return Err(invalid("unknown workflow argument")),
            }
        }
        if inputs.is_some() && !values.0.is_empty() {
            return Err(invalid(
                "named workflow inputs cannot be combined with --inputs or --inputs-file",
            ));
        }
        let values = match inputs {
            None => values,
            Some((true, path)) => WorkflowArguments::from_file(Path::new(&path))?,
            Some((false, json)) => WorkflowArguments::from_json(
                json.to_str()
                    .ok_or_else(|| invalid("--inputs requires UTF-8 JSON"))?
                    .as_bytes(),
            )?,
        };
        schema.validate(&values)?;
        let encoded = serde_json::to_vec(&values).context(crate::WorkflowInputJsonSnafu)?;
        Self::check_size(encoded.len())?;
        Ok(RunnerArguments {
            verbose,
            inputs: values,
        })
    }
}
