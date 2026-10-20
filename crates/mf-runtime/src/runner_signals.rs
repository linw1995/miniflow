//! Process-wide interrupt handling explicitly installed by standalone runners.

use crate::{StreamCancellation, StreamError};
use signal_hook::{
    SigId,
    consts::{SIGINT, SIGTERM},
    iterator::{Handle, Signals},
    low_level,
};
use snafu::{ResultExt, Snafu};
use std::{
    io,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
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

/// Keep this guard alive through execution, cleanup, and exit-status selection.
///
/// The first interrupt requests cancellation; a second interrupt exits immediately.
/// Installing handlers changes process-wide signal behavior and is inappropriate
/// for implicit use by embedded runtimes.
pub struct RunnerSignals {
    received: Arc<AtomicI32>,
    registrations: Vec<SigId>,
    signals: Option<Signals>,
    handle: Option<Handle>,
    worker: Option<JoinHandle<()>>,
}

impl RunnerSignals {
    pub fn new() -> Result<Self, RunnerSignalError> {
        let mut guard = Self {
            received: Arc::new(AtomicI32::new(0)),
            registrations: Vec::new(),
            signals: None,
            handle: None,
            worker: None,
        };
        for signal in [SIGINT, SIGTERM] {
            let received = guard.received.clone();
            // Only lock-free atomics and async-signal-safe exit run in the handler.
            // Count here so repeated interrupts work even before binding or while
            // the listener is blocked waking a cancellation consumer.
            let registration = unsafe {
                low_level::register(signal, move || {
                    if received
                        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                    {
                        low_level::exit(128 + signal);
                    }
                })
            }
            .context(RegisterSnafu)?;
            guard.registrations.push(registration);
        }
        let signals = Signals::new([SIGINT, SIGTERM]).context(RegisterSnafu)?;
        guard.handle = Some(signals.handle());
        guard.signals = Some(signals);
        Ok(guard)
    }

    pub fn bind(&mut self, cancellation: StreamCancellation) -> Result<(), RunnerSignalError> {
        let mut signals = self
            .signals
            .take()
            .ok_or_else(|| AlreadyBoundSnafu.build())?;
        // Publish an early interrupt before returning, without waiting for the listener.
        let signal = self.received.load(Ordering::SeqCst);
        if signal != 0 {
            cancel(&cancellation, signal);
        }
        self.worker = Some(
            thread::Builder::new()
                .name("workflow-signals".into())
                .spawn(move || {
                    for signal in signals.forever() {
                        cancel(&cancellation, signal);
                    }
                })
                .context(SpawnSnafu)?,
        );
        Ok(())
    }

    pub fn exit_code(&self) -> Option<ExitCode> {
        let signal = self.received.load(Ordering::SeqCst);
        (signal != 0).then(|| ExitCode::from((128 + signal) as u8))
    }
}

fn cancel(cancellation: &StreamCancellation, signal: i32) {
    cancellation.cancel(StreamError::Execution {
        message: format!("workflow interrupted by signal {signal}"),
    });
}

impl Drop for RunnerSignals {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.close();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for registration in self.registrations.drain(..) {
            low_level::unregister(registration);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn interrupts_preserve_pending_cancellation_and_force_repeated_shutdown() {
        const CHILD: &str = "MF_RUNNER_SIGNALS_TEST";
        if let Ok(mode) = std::env::var(CHILD) {
            let mut signals = RunnerSignals::new().unwrap();
            let cancellation = StreamCancellation::default();
            let signal = if mode.ends_with("term") {
                SIGTERM
            } else {
                SIGINT
            };
            if mode.starts_with("pending") {
                low_level::raise(signal).unwrap();
                signals.bind(cancellation.clone()).unwrap();
                assert!(cancellation.failure().is_some());
            } else if mode.starts_with("repeat") {
                // No listener is running: escalation must happen in the handler.
                low_level::raise(SIGINT).unwrap();
                low_level::raise(signal).unwrap();
                panic!("second interrupt did not terminate the process");
            } else {
                signals.bind(cancellation.clone()).unwrap();
                low_level::raise(signal).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while cancellation.failure().is_none() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                assert!(cancellation.failure().is_some());
            }
            assert_eq!(
                signals.exit_code(),
                Some(ExitCode::from((128 + signal) as u8))
            );
            return;
        }
        for mode in [
            "pending-int",
            "pending-term",
            "bound-int",
            "bound-term",
            "repeat-int",
            "repeat-term",
        ] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runner_signals::tests::interrupts_preserve_pending_cancellation_and_force_repeated_shutdown",
                ])
                .env(CHILD, mode)
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("signal test timed out: {mode}");
                }
                thread::sleep(Duration::from_millis(5));
            };
            let expected = match mode {
                "repeat-int" => 130,
                "repeat-term" => 143,
                _ => 0,
            };
            assert_eq!(status.code(), Some(expected), "{mode}: {status}");
        }
    }
}
