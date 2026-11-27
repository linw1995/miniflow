//! A private session listener for exactly one supervised runner process.

use super::{Grant, HostSession, LAUNCH_ENV, dispatch};
use crate::{
    StreamCancellation,
    callback::{CallbackError, CallbackSites, CodecSnafu, ProtocolSnafu, TransportSnafu},
};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    fs::Permissions,
    io,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::Command,
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Listener {
    directory: tempfile::TempDir,
    socket: UnixListener,
    session: HostSession,
}

impl Listener {
    /// Prepare only the requested transports. An empty session clears inherited authority and opens no socket.
    pub fn prepare_session(
        command: &mut Command,
        sites: &CallbackSites,
        session: HostSession,
    ) -> Result<Option<Self>, CallbackError> {
        command.env_remove(LAUNCH_ENV);
        let io = session.grant();
        if session.callbacks.is_empty() && io.is_empty() {
            return Ok(None);
        }
        session.validate_inputs()?;
        ensure!(
            session
                .callbacks
                .keys()
                .all(|name| sites.contains_key(name)),
            ProtocolSnafu {
                reason: "callback not declared by the workflow"
            }
        );
        // sockaddr_un is short even when the workspace or TMPDIR is deeply nested.
        let directory = tempfile::Builder::new()
            .prefix("mf-rpc-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .context(TransportSnafu)?;
        let path = directory.path().join("rpc");
        let socket = UnixListener::bind(&path).context(TransportSnafu)?;
        socket.set_nonblocking(true).context(TransportSnafu)?;
        let grant = Grant {
            socket: path,
            parent: std::process::id(),
            bindings: session.callbacks.keys().cloned().collect(),
            io,
        };
        command.env(
            LAUNCH_ENV,
            serde_json::to_string(&grant).context(CodecSnafu)?,
        );
        Ok(Some(Self {
            directory,
            socket,
            session,
        }))
    }

    /// Bind the prepared listener to the PID returned by this command's spawn.
    pub fn start(
        self,
        pid: u32,
        parent: &StreamCancellation,
        failed: impl FnOnce(CallbackError) + Send + 'static,
    ) -> Result<Server, CallbackError> {
        let cancellation = parent.child();
        let cancel = cancellation.clone();
        let mut completed = self.session.grant().is_empty();
        let worker = thread::Builder::new()
            .name("mf-runner-rpc".into())
            .spawn(move || {
                if let Err(error) = self.serve(pid, &cancel, &mut completed)
                    && cancel.failure().is_none()
                {
                    failed(error);
                    return false;
                }
                completed
            })
            .context(TransportSnafu)?;
        Ok(Server {
            cancellation,
            worker: Some(worker),
        })
    }

    fn serve(
        self,
        pid: u32,
        cancel: &StreamCancellation,
        completed: &mut bool,
    ) -> Result<(), CallbackError> {
        while cancel.failure().is_none() {
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if peer_pid(&stream).context(TransportSnafu)? != pid {
                        continue;
                    }
                    // The grant cannot be reconnected to, even by descendants of the runner.
                    drop(self.socket);
                    drop(self.directory);
                    return dispatch(stream, self.session, cancel, completed);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e).context(TransportSnafu),
            }
        }
        Ok(())
    }
}

fn peer_pid(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    let pid = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::LocalPeerPid)?;
    #[cfg(target_os = "linux")]
    let pid =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)?.pid();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return Ok(pid as u32);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "callback peer identity requires Linux or macOS",
    ))
}

pub struct Server {
    cancellation: StreamCancellation,
    worker: Option<JoinHandle<bool>>,
}

impl Server {
    /// After reaping the child, reject even a zero exit if its granted event session never drained.
    /// Transport and handler errors are also reported to the supervisor's failure callback.
    pub fn finish(mut self) -> Result<(), CallbackError> {
        self.cancel();
        let completed = self
            .worker
            .take()
            .expect("RPC server owns its worker")
            .join()
            .ok()
            .context(ProtocolSnafu {
                reason: "runner RPC worker panicked",
            })?;
        ensure!(
            completed,
            ProtocolSnafu {
                reason: "runner exited without completing hosted I/O"
            }
        );
        Ok(())
    }

    pub fn cancel(&self) {
        self.cancellation.cancel(
            crate::StreamExecutionSnafu {
                message: "callback listener stopped",
            }
            .build(),
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.cancel();
            let _ = worker.join();
        }
    }
}
