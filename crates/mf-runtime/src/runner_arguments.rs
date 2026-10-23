use crate::{
    MAX_WORKFLOW_INPUT_BYTES, NodeTarget, NodeTargetKind, WorkflowArguments, WorkflowInputError,
    WorkflowInputSchema,
};
use clap::{Arg, ArgAction, ArgMatches, Command, builder::OsStringValueParser};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum RunnerArgumentError {
    #[snafu(transparent)]
    Cli { source: clap::Error },
    #[snafu(transparent)]
    Target { source: crate::NodeTargetError },
    #[snafu(display("{message}"))]
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

#[derive(Clone, Debug, Default)]
pub struct RunnerInvocation {
    pub verbose: u8,
    pub node: Option<String>,
    pub inputs: WorkflowArguments,
    pub context: WorkflowArguments,
}

impl RunnerInvocation {
    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
        workflow: &WorkflowInputSchema,
        targets: &[NodeTarget],
    ) -> Result<Self, RunnerArgumentError> {
        // Register the union before selection so --node can follow its input arguments.
        let schemas = std::iter::once(workflow).chain(
            targets
                .iter()
                .filter(|target| target.kind == NodeTargetKind::Task)
                .map(|target| &target.inputs),
        );
        let command = argument_command(schemas, true, Some(targets));
        let matches = parse_arguments(arguments, command)?;
        let node = matches.get_one::<String>("node").cloned();
        let target = node
            .as_deref()
            .map(|id| NodeTarget::find(targets, id))
            .transpose()?;
        let schema = target.map_or(workflow, |target| &target.inputs);
        if matches.get_flag("help") {
            let mut help = argument_command([schema], true, Some(targets));
            if let Some(target) = target {
                help = help.about(format!("Run workflow node {}", target.id));
            }
            display_help(help)?;
        }
        let context = matches.get_one::<OsString>("context");
        let context_file = matches.get_one::<OsString>("context-file");
        ensure!(
            target.is_some() || (context.is_none() && context_file.is_none()),
            InvalidSnafu {
                message: "context transport requires --node"
            }
        );
        let options = decode_options(&matches, schema, true)?;
        let inputs = options.inputs;
        let context = if let Some(json) = context {
            WorkflowArguments::from_json(
                json.to_str()
                    .context(InvalidSnafu {
                        message: "--context requires UTF-8 JSON",
                    })?
                    .as_bytes(),
            )?
        } else if let Some(path) = context_file {
            WorkflowArguments::from_json(&read_context_file(Path::new(path))?)?
        } else {
            WorkflowArguments::default()
        };
        if let Some(target) = target {
            target.validate_context(&context)?;
            let encoded =
                serde_json::to_vec(&serde_json::json!({"inputs": inputs, "context": context}))
                    .context(crate::WorkflowInputJsonSnafu)?;
            WorkflowArguments::check_size(encoded.len())?;
        }
        Ok(Self {
            verbose: options.verbose,
            node,
            inputs,
            context,
        })
    }
}

/// Builds workflow help from the embedded input schema without preparing providers.
pub fn runner_help_command(schema: &WorkflowInputSchema) -> Command {
    argument_command([schema], true, Some(&[]))
}

fn read_context_file(path: &Path) -> Result<Vec<u8>, RunnerArgumentError> {
    let read = || -> io::Result<Vec<u8>> {
        let mut options = File::options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("expected a regular context file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_WORKFLOW_INPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    read().context(ReadSnafu {
        path: path.to_owned(),
    })
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
        let command = argument_command([schema], allow_verbose, None);
        let matches = parse_arguments(arguments, command)?;
        if matches.get_flag("help") {
            display_help(argument_command([schema], allow_verbose, None))?;
        }
        decode_options(&matches, schema, allow_verbose)
    }
}

fn argument_command<'a>(
    schemas: impl IntoIterator<Item = &'a WorkflowInputSchema>,
    allow_verbose: bool,
    targets: Option<&[NodeTarget]>,
) -> Command {
    let mut command = Command::new("workflow")
        .no_binary_name(true)
        .about("Run a compiled workflow")
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .help("Print help for the workflow or selected node")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("inputs")
                .long("inputs")
                .value_name("JSON")
                .help("Workflow inputs as JSON (up to 1 MiB)")
                .value_parser(OsStringValueParser::new())
                .allow_hyphen_values(true)
                .conflicts_with("inputs-file"),
        )
        .arg(
            Arg::new("inputs-file")
                .long("inputs-file")
                .value_name("PATH")
                .help("Read workflow inputs from a JSON file (up to 1 MiB)")
                .value_parser(OsStringValueParser::new())
                .allow_hyphen_values(true),
        );
    if allow_verbose {
        command = command.arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Increase diagnostic detail: -v summaries, -vv metadata, -vvv full records")
                .action(ArgAction::Count),
        );
    }
    if let Some(targets) = targets {
        command = command
            .arg(
                Arg::new("node")
                    .long("node")
                    .value_name("ID")
                    .help("Execute a single node instead of the workflow")
                    .value_parser(clap::builder::NonEmptyStringValueParser::new()),
            )
            .arg(
                Arg::new("context")
                    .long("context")
                    .value_name("JSON")
                    .help("Context values as JSON for --node (up to 1 MiB)")
                    .value_parser(OsStringValueParser::new())
                    .conflicts_with("context-file"),
            )
            .arg(
                Arg::new("context-file")
                    .long("context-file")
                    .value_name("PATH")
                    .help("Read --node context from a regular JSON file (up to 1 MiB)")
                    .value_parser(OsStringValueParser::new()),
            );
        let tasks: Vec<_> = targets
            .iter()
            .filter(|target| target.kind == NodeTargetKind::Task)
            .map(|target| target.id.as_str())
            .collect();
        if !tasks.is_empty() {
            command = command.after_help(format!(
                "Task targets:\n  {}\n\nUse --node ID --help to inspect a node's inputs.",
                tasks.join("\n  ")
            ));
        }
    }
    let mut parameters = BTreeMap::new();
    for schema in schemas {
        for (node, ports) in &schema.inputs {
            for (port, input) in ports {
                parameters
                    .entry(crate::output_id(node, port))
                    .or_insert(input);
            }
        }
    }
    for (name, input) in parameters {
        command = command.arg(
            Arg::new(name.clone())
                .long(name)
                .value_name("VALUE")
                .help(format!(
                    "{} ({}){}{}{}",
                    input.value_type,
                    if input.required {
                        "required"
                    } else {
                        "optional"
                    },
                    input
                        .binding
                        .env
                        .as_ref()
                        .map(|name| format!("; env: {name}"))
                        .unwrap_or_default(),
                    if input.binding.default.is_some() {
                        "; default configured"
                    } else {
                        ""
                    },
                    if input.binding.secret { "; secret" } else { "" },
                ))
                .value_parser(OsStringValueParser::new())
                .conflicts_with_all(["inputs", "inputs-file"]),
        );
    }
    command
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    command: Command,
) -> Result<ArgMatches, RunnerArgumentError> {
    let value_flags: BTreeSet<_> = command
        .get_arguments()
        .filter(|argument| argument.get_action().takes_values())
        .filter_map(Arg::get_long)
        .collect();
    let mut arguments = arguments.into_iter().peekable();
    let mut normalized = Vec::new();
    let mut size = 0;
    while let Some(mut argument) = arguments.next() {
        let flag = argument
            .to_str()
            .and_then(|flag| flag.strip_prefix("--"))
            .map(|flag| {
                flag.split_once('=')
                    .map_or((flag, false), |(name, _)| (name, true))
            });
        let control = flag.is_some_and(|(name, _)| {
            value_flags.contains(name) && matches!(name, "node" | "context" | "context-file")
        });
        let transport = flag.is_some_and(|(name, _)| matches!(name, "inputs" | "inputs-file"));
        if !control {
            // Transport payloads have their own byte limits, separate from named CLI inputs.
            size += match flag {
                Some((name, _)) if transport => name.len() + 2,
                _ => argument.len(),
            };
            WorkflowArguments::check_size(size)?;
        }
        let value = if flag.is_some_and(|(name, inline)| !inline && value_flags.contains(name)) {
            arguments.next_if(|value| transport || !value.to_string_lossy().starts_with("--"))
        } else {
            None
        };
        if let Some(value) = value {
            if !control && !transport {
                size += value.len();
                WorkflowArguments::check_size(size)?;
            }
            // Short-option-looking values were literal data before clap; long options remain flags.
            if !transport && value.to_string_lossy().starts_with('-') {
                argument.push("=");
                argument.push(value);
                normalized.push(argument);
            } else {
                normalized.extend([argument, value]);
            }
        } else {
            normalized.push(argument);
        }
    }
    Ok(command.try_get_matches_from(normalized)?)
}

fn display_help(command: Command) -> Result<(), RunnerArgumentError> {
    let command = command.mut_arg("help", |argument| argument.action(ArgAction::Help));
    command.try_get_matches_from(["--help"])?;
    Ok(())
}

fn decode_options(
    matches: &ArgMatches,
    schema: &WorkflowInputSchema,
    allow_verbose: bool,
) -> Result<RunnerArguments, RunnerArgumentError> {
    let mut values = WorkflowArguments::default();
    for name in matches.ids().filter(|name| {
        !matches!(
            name.as_str(),
            "help" | "verbose" | "node" | "context" | "context-file" | "inputs" | "inputs-file"
        )
    }) {
        let name = name.as_str();
        let input = schema.parameter(name).with_context(|| InvalidSnafu {
            message: format!("unknown workflow argument `--{name}`"),
        })?;
        let value = matches.get_one::<OsString>(name).unwrap();
        let value = value.to_str().context(InvalidSnafu {
            message: "workflow input values must be UTF-8",
        })?;
        let value = input.parse_text(value).context(JsonValueSnafu { name })?;
        values.0.insert(name.into(), value);
    }
    let mut values = if let Some(path) = matches.get_one::<OsString>("inputs-file") {
        WorkflowArguments::from_file(Path::new(path))?
    } else if let Some(json) = matches.get_one::<OsString>("inputs") {
        WorkflowArguments::from_json(
            json.to_str()
                .context(InvalidSnafu {
                    message: "--inputs requires UTF-8 JSON",
                })?
                .as_bytes(),
        )?
    } else {
        values
    };
    schema.mark_secrets(&mut values);
    schema.validate(&values)?;
    let encoded = serde_json::to_vec(&values).context(crate::WorkflowInputJsonSnafu)?;
    WorkflowArguments::check_size(encoded.len())?;
    Ok(RunnerArguments {
        verbose: if allow_verbose {
            matches.get_count("verbose").min(3)
        } else {
            0
        },
        inputs: values,
    })
}
