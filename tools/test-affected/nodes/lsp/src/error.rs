use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("invalid analysis request: {message}"))]
    Invalid { message: String },
    #[snafu(display("{action}: {source}"))]
    Io {
        action: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("invalid LSP JSON: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("LSP protocol: {message}"))]
    Protocol { message: String },
    #[snafu(display("analysis limit: {message}"))]
    Limit { message: &'static str },
    #[snafu(display("analysis cancelled: {source}"))]
    Cancelled { source: mf_runtime::StreamError },
    #[snafu(display("cannot reap language server: {source}"))]
    Cleanup { source: std::io::Error },
}

impl Error {
    pub fn fatal(&self) -> bool {
        matches!(
            self,
            Self::Invalid { .. } | Self::Cancelled { .. } | Self::Cleanup { .. }
        )
    }
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
