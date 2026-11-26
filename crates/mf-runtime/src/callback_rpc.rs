//! Invocation-scoped callback RPC over a local Unix socket. Direction determines the message schema.

use crate::{StreamCancellation, WorkflowArguments, callback::*};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const LAUNCH_ENV: &str = "MF_RUNNER_RPC_V1";

/// Only the launcher's immediate child consumes this grant. It contains no business inputs.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub parent: u32,
    pub socket: PathBuf,
    pub bindings: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: u64,
    binding: String,
    args: WireValues,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    id: u64,
    value: WireValues,
}

const IO_TICK: Duration = Duration::from_millis(50);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);

struct Channel {
    socket: BufReader<UnixStream>,
    last: u64,
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn check(
    cancellation: &StreamCancellation,
    deadline: Option<Instant>,
) -> Result<(), CallbackError> {
    if let Some(error) = cancellation.failure() {
        return Err(error.into());
    }
    ensure!(
        deadline.is_none_or(|end| Instant::now() < end),
        ProtocolSnafu {
            reason: "RPC transfer timed out"
        }
    );
    Ok(())
}

impl Channel {
    fn new(socket: UnixStream) -> Result<Self, CallbackError> {
        // Accepted sockets may inherit the listener's nonblocking flag on BSD.
        socket.set_nonblocking(false).context(TransportSnafu)?;
        socket
            .set_read_timeout(Some(IO_TICK))
            .context(TransportSnafu)?;
        socket
            .set_write_timeout(Some(IO_TICK))
            .context(TransportSnafu)?;
        Ok(Self {
            socket: BufReader::new(socket),
            last: 0,
        })
    }

    fn next_id(&self) -> Result<u64, CallbackError> {
        self.last.checked_add(1).context(ProtocolSnafu {
            reason: "callback ID exhausted",
        })
    }

    fn receive<T: DeserializeOwned>(
        &mut self,
        cancel: &StreamCancellation,
    ) -> Result<Option<T>, CallbackError> {
        let mut bytes = Vec::new();
        let mut deadline = None;
        loop {
            check(cancel, deadline)?;
            let input = match self.socket.fill_buf() {
                Ok(input) => input,
                Err(e) if transient(&e) => continue,
                Err(e) => return Err(e).context(TransportSnafu),
            };
            if input.is_empty() {
                ensure!(
                    bytes.is_empty(),
                    ProtocolSnafu {
                        reason: "incomplete RPC message"
                    }
                );
                return Ok(None);
            }
            let newline = input.iter().position(|b| *b == b'\n');
            let len = newline.unwrap_or(input.len());
            ensure!(
                bytes.len() + len < MAX_CALLBACK_FRAME,
                ProtocolSnafu {
                    reason: "RPC message exceeds limit"
                }
            );
            bytes.extend_from_slice(&input[..len]);
            self.socket.consume(len + usize::from(newline.is_some()));
            if newline.is_some() {
                return crate::decode_unique_json(&bytes)
                    .context(CodecSnafu)
                    .map(Some);
            }
            deadline.get_or_insert_with(|| Instant::now() + TRANSFER_TIMEOUT);
        }
    }

    fn send(
        &mut self,
        value: &impl Serialize,
        cancel: &StreamCancellation,
    ) -> Result<(), CallbackError> {
        let bytes = encode_control(value)?;
        let deadline = Some(Instant::now() + TRANSFER_TIMEOUT);
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            check(cancel, deadline)?;
            match self.socket.get_mut().write(rest) {
                Ok(0) => {
                    return Err(io::Error::from(io::ErrorKind::WriteZero)).context(TransportSnafu);
                }
                Ok(n) => rest = &rest[n..],
                Err(e) if transient(&e) => continue,
                Err(e) => return Err(e).context(TransportSnafu),
            }
        }
        Ok(())
    }

    fn call(
        &mut self,
        binding: &str,
        args: &WorkflowArguments,
        cancel: &StreamCancellation,
    ) -> Result<CallbackReply, CallbackError> {
        let id = self.next_id()?;
        self.send(
            &Request {
                id,
                binding: binding.into(),
                args: WireValues::encode(&args.0),
            },
            cancel,
        )?;
        let reply: Response = self.receive(cancel)?.context(ProtocolSnafu {
            reason: "host closed without a result",
        })?;
        ensure!(
            reply.id == id,
            ProtocolSnafu {
                reason: "unexpected callback result ID"
            }
        );
        self.last = id;
        Ok(reply.value.decode()?.into())
    }
}

/// Serve an already authenticated peer. The connection is consumed and never resumed after an error.
pub fn serve(
    socket: UnixStream,
    callbacks: &HostCallbacks,
    cancel: &StreamCancellation,
) -> Result<(), CallbackError> {
    let mut channel = Channel::new(socket)?;
    while let Some(Request { id, binding, args }) = channel.receive(cancel)? {
        ensure!(
            id == channel.next_id()?,
            ProtocolSnafu {
                reason: "out-of-order callback request"
            }
        );
        let handler = callbacks.get(&binding).context(ProtocolSnafu {
            reason: "unbound callback",
        })?;
        let reply = handler.call(&WorkflowArguments(args.decode()?), cancel)?;
        channel.send(
            &Response {
                id,
                value: WireValues::encode(&reply.outputs),
            },
            cancel,
        )?;
        reply.delivered();
        channel.last = id;
    }
    Ok(())
}

pub fn connect(
    grant: Grant,
    sites: &CallbackSites,
    failed: &Arc<AtomicBool>,
) -> Result<HostCallbacks, CallbackError> {
    use std::collections::BTreeSet;
    let mut seen = BTreeSet::new();
    ensure!(
        !grant.bindings.is_empty()
            && grant
                .bindings
                .iter()
                .all(|name| sites.contains_key(name) && seen.insert(name)),
        ProtocolSnafu {
            reason: "empty, unknown or duplicate callback binding"
        }
    );
    ensure!(
        grant.socket.is_absolute(),
        ProtocolSnafu {
            reason: "RPC socket path must be absolute"
        }
    );
    let socket = UnixStream::connect(&grant.socket).context(TransportSnafu)?;
    let channel = Arc::new(Mutex::new(Channel::new(socket)?));
    Ok(grant
        .bindings
        .into_iter()
        .map(|name| {
            let channel = channel.clone();
            let failed = failed.clone();
            (
                name.clone(),
                HostCallback::new(move |args, cancel| {
                    let mut channel = channel.lock().unwrap();
                    ensure!(
                        !failed.load(Ordering::Acquire),
                        ProtocolSnafu {
                            reason: "callback RPC previously failed"
                        }
                    );
                    let reply = channel
                        .call(&name, args, cancel)
                        .inspect_err(|_| failed.store(true, Ordering::Release))?;
                    let failed = failed.clone();
                    Ok(reply.on_delivery(move |valid| {
                        if !valid {
                            failed.store(true, Ordering::Release);
                        }
                    }))
                }),
            )
        })
        .collect())
}

pub(super) fn from_env(
    sites: &CallbackSites,
    failed: &Arc<AtomicBool>,
) -> Result<HostCallbacks, CallbackError> {
    use std::os::unix::ffi::OsStrExt;
    let Some(value) = std::env::var_os(LAUNCH_ENV) else {
        return Ok(Default::default());
    };
    let grant: Grant = crate::decode_unique_json(value.as_bytes()).context(CodecSnafu)?;
    // A plugin may start another standalone executable. Environment inheritance is not delegation.
    if grant.parent != unsafe { libc::getppid() } as u32 {
        return Ok(Default::default());
    }
    connect(grant, sites, failed)
}
