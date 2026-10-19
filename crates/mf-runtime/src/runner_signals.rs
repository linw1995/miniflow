//! Signal-to-cancellation bridge installed by standalone runners, not embedded runtimes.

use crate::{StreamCancellation, StreamError};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::{Handle, Signals},
};
use snafu::{ResultExt, Snafu};
use std::{
    io,
    thread::{self, JoinHandle},
};

#[derive(Debug, Snafu)]
pub enum RunnerSignalError {
    #[snafu(display("could not register runner signals: {source}"))]
    Register { source: io::Error },
    #[snafu(display("could not start runner signal listener: {source}"))]
    Spawn { source: io::Error },
    #[snafu(display("runner signal listener is already bound"))]
    AlreadyBound,
}

pub struct RunnerSignals {
    signals: Option<Signals>,
    handle: Handle,
    worker: Option<JoinHandle<()>>,
}
impl RunnerSignals {
    // Register before starting workers so an early interrupt remains pending until bind.
    pub fn new() -> Result<Self, RunnerSignalError> {
        let signals = Signals::new([SIGINT, SIGTERM]).context(RegisterSnafu)?;
        Ok(Self {
            handle: signals.handle(),
            signals: Some(signals),
            worker: None,
        })
    }
    pub fn bind(&mut self, cancellation: StreamCancellation) -> Result<(), RunnerSignalError> {
        let mut signals = self
            .signals
            .take()
            .ok_or_else(|| AlreadyBoundSnafu.build())?;
        self.worker = Some(
            thread::Builder::new()
                .name("workflow-signals".into())
                .spawn(move || {
                    for signal in signals.forever() {
                        cancellation.cancel(StreamError::Execution {
                            message: format!("workflow interrupted by signal {signal}"),
                        });
                    }
                })
                .context(SpawnSnafu)?,
        );
        Ok(())
    }
}
impl Drop for RunnerSignals {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn interrupt_received_before_binding_is_not_lost() {
        const CHILD: &str = "MF_RUNNER_SIGNAL_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let mut signals = RunnerSignals::new().unwrap();
            assert_eq!(unsafe { libc::raise(SIGINT) }, 0);
            let cancellation = StreamCancellation::default();
            signals.bind(cancellation.clone()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while cancellation.failure().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(cancellation.failure().is_some());
            return;
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runner_signals::tests::interrupt_received_before_binding_is_not_lost",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
