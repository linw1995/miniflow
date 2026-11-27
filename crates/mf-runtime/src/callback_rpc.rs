//! Invocation-scoped runner I/O and finite callbacks over one authenticated Unix connection.

mod channel;
#[cfg(feature = "callback-host")]
pub mod host;

use crate::{NodeValues, RunnerDiagnostic, StreamCancellation, WorkflowArguments, callback::*};
use channel::Channel;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, ensure};
use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const LAUNCH_ENV: &str = "MF_RUNNER_RPC_V1";
/// Large business records need not fit the much smaller callback/control envelope.
pub const MAX_OUTPUT_FRAME: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IoGrant {
    pub inputs: bool,
    pub output: bool,
    pub diagnostics: bool,
}

impl IoGrant {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    fn events(self) -> bool {
        self.output || self.diagnostics
    }
}

/// Only the launcher's immediate child consumes this grant. It contains no business values.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub parent: u32,
    pub socket: PathBuf,
    pub bindings: Vec<String>,
    #[serde(default, skip_serializing_if = "IoGrant::is_empty")]
    pub io: IoGrant,
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    GetInputs {},
    Output(NodeValues),
    Diagnostic(RunnerDiagnostic),
    Flush {},
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Event(Event),
    Call(Request),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Startup {
    inputs: WireValues,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Flushed {
    flushed: bool,
}

pub type Consumer<T> = Box<dyn FnMut(T, &StreamCancellation) -> Result<(), CallbackError> + Send>;

/// Independent capabilities for one launch. Consumers run in wire order and may exert backpressure.
/// A blocking consumer must observe cancellation, and must not wait for the child to exit.
#[derive(Default)]
pub struct HostSession {
    pub callbacks: HostCallbacks,
    pub inputs: Option<WorkflowArguments>,
    pub output: Option<Consumer<NodeValues>>,
    pub diagnostics: Option<Consumer<RunnerDiagnostic>>,
}

impl HostSession {
    fn grant(&self) -> IoGrant {
        IoGrant {
            inputs: self.inputs.is_some(),
            output: self.output.is_some(),
            diagnostics: self.diagnostics.is_some(),
        }
    }
}

/// Serve callbacks only, without taking over any runner I/O.
pub fn serve(
    socket: UnixStream,
    callbacks: &HostCallbacks,
    cancel: &StreamCancellation,
) -> Result<(), CallbackError> {
    serve_session(
        socket,
        HostSession {
            callbacks: callbacks.clone(),
            ..Default::default()
        },
        cancel,
    )
}

/// Serve an already authenticated peer. An event sink error prevents the final drain receipt.
pub fn serve_session(
    socket: UnixStream,
    session: HostSession,
    cancel: &StreamCancellation,
) -> Result<(), CallbackError> {
    dispatch(socket, session, cancel, &mut false)
}

fn dispatch(
    socket: UnixStream,
    mut session: HostSession,
    cancel: &StreamCancellation,
    completed: &mut bool,
) -> Result<(), CallbackError> {
    let mut channel = Channel::new(socket)?;
    let io = session.grant();
    let limit = if io.output {
        MAX_OUTPUT_FRAME
    } else {
        MAX_CALLBACK_FRAME
    };
    while let Some(bytes) = channel.read_frame(limit, cancel)? {
        let message: Incoming = crate::decode_unique_json(&bytes).context(CodecSnafu)?;
        ensure!(
            bytes.len() < MAX_CALLBACK_FRAME
                || matches!(message, Incoming::Event(Event::Output(_))),
            ProtocolSnafu {
                reason: "control message exceeds limit"
            }
        );
        ensure!(
            session.inputs.is_none() || matches!(message, Incoming::Event(Event::GetInputs {})),
            ProtocolSnafu {
                reason: "startup inputs must be requested first"
            }
        );
        match message {
            Incoming::Event(Event::GetInputs {}) => {
                let inputs = session.inputs.take().context(ProtocolSnafu {
                    reason: "startup inputs not granted or already consumed",
                })?;
                inputs.check_encoded_size()?;
                channel.send(
                    &Startup {
                        inputs: WireValues::encode(&inputs.0),
                    },
                    MAX_CALLBACK_FRAME,
                    false,
                    cancel,
                )?;
                *completed = !io.events();
            }
            Incoming::Event(Event::Output(values)) => {
                session.output.as_mut().context(ProtocolSnafu {
                    reason: "output not granted",
                })?(values, cancel)?;
            }
            Incoming::Event(Event::Diagnostic(diagnostic)) => {
                session.diagnostics.as_mut().context(ProtocolSnafu {
                    reason: "diagnostics not granted",
                })?(diagnostic, cancel)?;
            }
            Incoming::Event(Event::Flush {}) => {
                ensure!(
                    io.events(),
                    ProtocolSnafu {
                        reason: "unexpected drain request"
                    }
                );
                channel.send(
                    &Flushed { flushed: true },
                    MAX_CALLBACK_FRAME,
                    false,
                    cancel,
                )?;
                *completed = true;
                return Ok(());
            }
            Incoming::Call(Request { id, binding, args }) => {
                ensure!(
                    id == channel.next_id()?,
                    ProtocolSnafu {
                        reason: "out-of-order callback request"
                    }
                );
                let handler = session.callbacks.get(&binding).context(ProtocolSnafu {
                    reason: "unbound callback",
                })?;
                let reply = handler.call(&WorkflowArguments(args.decode()?), cancel)?;
                channel.send(
                    &Response {
                        id,
                        value: WireValues::encode(&reply.outputs),
                    },
                    MAX_CALLBACK_FRAME,
                    false,
                    cancel,
                )?;
                reply.delivered();
                channel.last_id = id;
            }
        }
    }
    ensure!(
        !io.events() && session.inputs.is_none(),
        ProtocolSnafu {
            reason: "runner closed before completing hosted I/O"
        }
    );
    Ok(())
}

#[derive(Clone)]
struct Link {
    channel: Arc<Mutex<Channel>>,
    execution: StreamCancellation,
}

impl Link {
    fn poison(&self) {
        self.execution.cancel(
            crate::StreamExecutionSnafu {
                message: "runner RPC previously failed",
            }
            .build(),
        );
    }

    fn exchange<T>(
        &self,
        op: impl FnOnce(&mut Channel) -> Result<T, CallbackError>,
    ) -> Result<T, CallbackError> {
        let mut channel = self.channel.lock().unwrap();
        if let Some(error) = self.execution.failure() {
            return Err(error.into());
        }
        op(&mut channel).inspect_err(|_| self.poison())
    }
}

pub struct Client {
    link: Link,
    bindings: Vec<String>,
    io: IoGrant,
}

impl Client {
    pub fn io(&self) -> IoGrant {
        self.io
    }

    pub fn callbacks(&self) -> HostCallbacks {
        self.bindings
            .iter()
            .map(|name| {
                let binding = name.clone();
                let link = self.link.clone();
                let backpressured = self.io.events();
                (
                    name.clone(),
                    HostCallback::new(move |args, cancel| {
                        let reply: CallbackReply = link.exchange(|channel| {
                            let id = channel.next_id()?;
                            channel.send(
                                &Request {
                                    id,
                                    binding: binding.clone(),
                                    args: WireValues::encode(&args.0),
                                },
                                MAX_CALLBACK_FRAME,
                                backpressured,
                                cancel,
                            )?;
                            let reply: Response = channel.reply(cancel)?;
                            ensure!(
                                reply.id == id,
                                ProtocolSnafu {
                                    reason: "unexpected callback result ID"
                                }
                            );
                            channel.last_id = id;
                            Ok(reply.value.decode()?.into())
                        })?;
                        let link = link.clone();
                        Ok(reply.on_delivery(move |valid| {
                            if !valid {
                                link.poison();
                            }
                        }))
                    }),
                )
            })
            .collect()
    }

    pub fn take_inputs(&mut self) -> Result<Option<WorkflowArguments>, CallbackError> {
        if !std::mem::take(&mut self.io.inputs) {
            return Ok(None);
        }
        self.link.exchange(|channel| {
            let cancel = &self.link.execution;
            channel.send(&Event::GetInputs {}, MAX_CALLBACK_FRAME, false, cancel)?;
            let reply: Startup = channel.reply(cancel)?;
            let inputs = WorkflowArguments(reply.inputs.decode()?);
            inputs.check_encoded_size()?;
            Ok(Some(inputs))
        })
    }

    pub fn output(
        &self,
        values: NodeValues,
        cancel: &StreamCancellation,
    ) -> Result<(), CallbackError> {
        ensure!(
            self.io.output,
            ProtocolSnafu {
                reason: "output not granted"
            }
        );
        self.link.exchange(|channel| {
            channel.send(&Event::Output(values), MAX_OUTPUT_FRAME, true, cancel)
        })
    }

    pub fn diagnostic(&self, value: RunnerDiagnostic) -> Result<(), CallbackError> {
        ensure!(
            self.io.diagnostics,
            ProtocolSnafu {
                reason: "diagnostics not granted"
            }
        );
        self.link.exchange(|channel| {
            channel.send(
                &Event::Diagnostic(value),
                MAX_CALLBACK_FRAME,
                true,
                &self.link.execution,
            )
        })
    }

    /// Only one round trip, after the last event. A successful exit cannot race an undrained or rejected record.
    pub fn finish(self) -> Result<(), CallbackError> {
        if self.io.events() {
            self.link.exchange(|channel| {
                let cancel = &self.link.execution;
                channel.send(&Event::Flush {}, MAX_CALLBACK_FRAME, true, cancel)?;
                let ack: Flushed = channel.reply(cancel)?;
                ensure!(
                    ack.flushed,
                    ProtocolSnafu {
                        reason: "host did not drain runner events"
                    }
                );
                Ok(())
            })?;
        }
        Ok(())
    }
}

pub fn connect(
    grant: Grant,
    sites: &CallbackSites,
    execution: &StreamCancellation,
) -> Result<Client, CallbackError> {
    let mut seen = std::collections::BTreeSet::new();
    ensure!(
        (!grant.bindings.is_empty() || !grant.io.is_empty())
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
    Ok(Client {
        link: Link {
            channel: Arc::new(Mutex::new(Channel::new(socket)?)),
            execution: execution.clone(),
        },
        bindings: grant.bindings,
        io: grant.io,
    })
}

/// Environment inheritance is not delegation to another executable.
pub fn from_env(
    sites: &CallbackSites,
    execution: &StreamCancellation,
) -> Result<Option<Client>, CallbackError> {
    use std::os::unix::ffi::OsStrExt;
    let Some(value) = std::env::var_os(LAUNCH_ENV) else {
        return Ok(None);
    };
    let grant: Grant = crate::decode_unique_json(value.as_bytes()).context(CodecSnafu)?;
    if grant.parent != unsafe { libc::getppid() } as u32 {
        return Ok(None);
    }
    connect(grant, sites, execution).map(Some)
}
