//! A private callback listener for exactly one supervised child process.

use mf_runtime::{
    StreamCancellation,
    callback::{CallbackError, CodecSnafu, HostCallbacks, TransportSnafu},
    callback_rpc::{self, Grant, LAUNCH_ENV},
};
use snafu::ResultExt;
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
    callbacks: HostCallbacks,
}

impl Listener {
    pub fn prepare(
        command: &mut Command,
        callbacks: HostCallbacks,
    ) -> Result<Option<Self>, CallbackError> {
        command.env_remove(LAUNCH_ENV);
        if callbacks.is_empty() {
            return Ok(None);
        }
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
            bindings: callbacks.keys().cloned().collect(),
        };
        command.env(
            LAUNCH_ENV,
            serde_json::to_string(&grant).context(CodecSnafu)?,
        );
        Ok(Some(Self {
            directory,
            socket,
            callbacks,
        }))
    }

    pub fn start(
        self,
        pid: u32,
        parent: &StreamCancellation,
        failed: impl FnOnce(CallbackError) + Send + 'static,
    ) -> Result<Server, CallbackError> {
        let cancellation = parent.child();
        let cancel = cancellation.clone();
        let worker = thread::Builder::new()
            .name("mf-callback-rpc".into())
            .spawn(move || {
                if let Err(error) = self.serve(pid, &cancel)
                    && cancel.failure().is_none()
                {
                    failed(error);
                }
            })
            .context(TransportSnafu)?;
        Ok(Server {
            cancellation,
            worker: Some(worker),
        })
    }

    fn serve(self, pid: u32, cancel: &StreamCancellation) -> Result<(), CallbackError> {
        while cancel.failure().is_none() {
            match self.socket.accept() {
                Ok((stream, _)) => {
                    if peer_pid(&stream).context(TransportSnafu)? != pid {
                        continue;
                    }
                    // The grant cannot be reconnected to, even by descendants of the runner.
                    drop(self.socket);
                    drop(self.directory);
                    return callback_rpc::serve(stream, &self.callbacks, cancel);
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
    worker: Option<JoinHandle<()>>,
}

impl Server {
    pub fn cancel(&self) {
        self.cancellation.cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "callback listener stopped",
            }
            .build(),
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
