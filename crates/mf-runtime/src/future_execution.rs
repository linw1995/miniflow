use crate::{StreamCancellation, StreamError};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    future::{Future, pending, poll_fn},
    pin::pin,
    task::Poll,
    time::Instant,
};

#[derive(Debug, Snafu)]
pub enum FutureExecutionError {
    #[snafu(display("future execution cannot nest inside an active executor: {source}"))]
    NestedExecutor {
        source: futures::executor::EnterError,
    },
    #[snafu(display("synchronous future execution requires a blocking thread outside Tokio"))]
    AsyncContext,
    #[snafu(display("future execution deadline exceeded"))]
    DeadlineExceeded,
    #[snafu(display("future execution cancelled: {source}"))]
    Cancelled { source: StreamError },
    #[cfg(feature = "tokio")]
    #[snafu(display("could not initialize the asynchronous backend: {source}"))]
    BackendInitialization {
        source: std::sync::Arc<std::io::Error>,
    },
}

impl From<FutureExecutionError> for crate::NodeExecutionError {
    fn from(source: FutureExecutionError) -> Self {
        Box::<dyn std::error::Error + Send + Sync>::from(source).into()
    }
}

#[derive(Debug, Default)]
pub struct FutureExecutor {
    #[cfg(feature = "tokio")]
    backend: std::sync::OnceLock<Result<tokio::runtime::Runtime, std::sync::Arc<std::io::Error>>>,
}

impl FutureExecutor {
    pub fn execute<F: Future>(
        &self,
        future: F,
        cancellation: &StreamCancellation,
        deadline: Option<Instant>,
        _workers: usize,
    ) -> Result<F::Output, FutureExecutionError> {
        check_limits(cancellation, deadline)?;
        // Probe entry before block_on installs its own guard; retain the typed failure.
        drop(futures::executor::enter().context(NestedExecutorSnafu)?);
        #[cfg(feature = "tokio")]
        ensure!(
            tokio::runtime::Handle::try_current().is_err(),
            AsyncContextSnafu
        );

        let operation = async {
            let mut future = pin!(future);
            let mut cancelled = pin!(cancellation.cancelled());
            let mut elapsed = pin!(async {
                if let Some(deadline) = deadline {
                    futures_timer::Delay::new(deadline.saturating_duration_since(Instant::now()))
                        .await;
                } else {
                    pending::<()>().await;
                }
            });
            poll_fn(|context| {
                if let Poll::Ready(source) = cancelled.as_mut().poll(context) {
                    return Poll::Ready(Err(source).context(CancelledSnafu));
                }
                if elapsed.as_mut().poll(context).is_ready() {
                    return Poll::Ready(DeadlineExceededSnafu.fail());
                }
                if let Err(error) = check_limits(cancellation, deadline) {
                    return Poll::Ready(Err(error));
                }
                match future.as_mut().poll(context) {
                    Poll::Ready(value) => {
                        Poll::Ready(check_limits(cancellation, deadline).map(|()| value))
                    }
                    Poll::Pending => Poll::Pending,
                }
            })
            .await
        };

        #[cfg(feature = "tokio")]
        {
            let backend = self
                .backend
                .get_or_init(|| {
                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(_workers)
                        .thread_name("workflow-async")
                        .enable_all()
                        .build()
                        .map_err(std::sync::Arc::new)
                })
                .as_ref()
                .map_err(std::sync::Arc::clone)
                .context(BackendInitializationSnafu)?;
            backend.block_on(operation)
        }
        #[cfg(not(feature = "tokio"))]
        futures::executor::block_on(operation)
    }
}

pub fn check_limits(
    cancellation: &StreamCancellation,
    deadline: Option<Instant>,
) -> Result<(), FutureExecutionError> {
    if let Some(source) = cancellation.failure() {
        return Err(source).context(CancelledSnafu);
    }
    ensure!(
        deadline.is_none_or(|deadline| Instant::now() < deadline),
        DeadlineExceededSnafu
    );
    Ok(())
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;
    use std::{error::Error, sync::Arc};

    #[test]
    fn backend_initialization_failure_retains_its_source_without_fallback() {
        let executor = FutureExecutor::default();
        assert!(executor.backend.get().is_none());
        executor
            .backend
            .set(Err(Arc::new(std::io::Error::other("allocation failure"))))
            .unwrap();
        let error = executor
            .execute(
                async { panic!("must not poll") },
                &StreamCancellation::default(),
                None,
                1,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            FutureExecutionError::BackendInitialization { .. }
        ));
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<Arc<std::io::Error>>()
            .unwrap();
        assert_eq!(source.kind(), std::io::ErrorKind::Other);
    }
}
