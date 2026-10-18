use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("invalid API URL: {source}"))]
    Url { source: url::ParseError },
    #[snafu(transparent)]
    Future {
        source: mf_runtime::FutureExecutionError,
    },
    #[snafu(display("external operation cancelled: {source}"))]
    Cancelled { source: mf_runtime::StreamError },
    #[snafu(display("HTTP read failed: {source}"))]
    Http { source: reqwest::Error },
    #[snafu(display("artifact archive is invalid: {source}"))]
    Archive { source: zip::result::ZipError },
    #[snafu(display("external response status: {status}"))]
    Response { status: u16 },
    #[snafu(display("evidence limit: {message}"))]
    Limit { message: &'static str },
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

impl From<Error> for mf_runtime::NodeBuildError {
    fn from(source: Error) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

impl From<Error> for mf_runtime::NodeExecutionError {
    fn from(source: Error) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}
