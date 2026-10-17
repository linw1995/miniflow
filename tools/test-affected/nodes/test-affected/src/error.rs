use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("{action}: {source}"))]
    Io {
        action: String,
        source: std::io::Error,
    },
    #[snafu(display("{action}: {source}"))]
    Json {
        action: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display("command output is not UTF-8: {source}"))]
    Encoding { source: std::string::FromUtf8Error },
    #[snafu(display("{program} exited with {status}: {stderr}"))]
    Command {
        program: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[snafu(display("invalid test analysis: {message}"))]
    Invalid { message: String },
}

impl From<Error> for mf_runtime::NodeExecutionError {
    fn from(source: Error) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}
