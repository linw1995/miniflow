//! Turn-scoped input for the Agent's next complete reply boundary.

use crate::{
    ChatError, ChatMessage, Invocation, error::*, schema::steering_deliveries as deliveries,
    storage, turn,
};
use diesel::prelude::*;
use mfn_llm::{
    CallOutcome,
    steering::{Boundary, Endpoint, MAX_PENDING_BYTES, MAX_PENDING_MESSAGES},
};
use snafu::{ResultExt, ensure};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use uuid::Uuid;

#[cfg(test)]
mod tests;

/// Clones share only this invocation's FIFO. Closing admission never submits a new turn.
#[derive(Clone)]
pub struct SteeringInbox {
    invocation: Invocation,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    pending: Vec<String>,
}

impl SteeringInbox {
    pub fn new(invocation: Invocation) -> Self {
        Self {
            invocation,
            state: Arc::default(),
        }
    }

    pub fn invocation(&self) -> &Invocation {
        &self.invocation
    }

    pub fn submit(&self, text: String) -> Result<(), ChatError> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, SteeringClosedSnafu);
        ensure!(
            !text.trim().is_empty(),
            InvalidSnafu {
                field: "steering",
                reason: "must contain nonblank text"
            }
        );
        ensure!(
            state.pending.len() < MAX_PENDING_MESSAGES
                && text
                    .len()
                    .saturating_add(state.pending.iter().map(String::len).sum())
                    <= MAX_PENDING_BYTES,
            InvalidSnafu {
                field: "steering inbox",
                reason: "pending input exceeds its message or byte limit"
            }
        );
        state.pending.push(text);
        Ok(())
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
    }

    /// Retrieve undelivered input for editing, never for automatic replay.
    pub fn take_unsent(&self) -> Vec<String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            std::mem::take(&mut state.pending)
        } else {
            Vec::new()
        }
    }

    fn take_at(&self, outcome: CallOutcome) -> Result<Vec<String>, ChatError> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, SteeringClosedSnafu);
        match outcome {
            CallOutcome::ToolCalls => {}
            CallOutcome::Completed if !state.pending.is_empty() => {}
            _ => {
                state.closed = true;
                return Ok(Vec::new());
            }
        }
        Ok(std::mem::take(&mut state.pending))
    }

    fn deliver(
        &self,
        db: &Path,
        invocation: &Invocation,
        boundary: Boundary,
        mut stream: impl Write,
    ) -> Result<(), ChatError> {
        let mut batch = self.take_at(boundary.outcome)?;
        // Do not hold the admission lock across SQLite or transport I/O.
        let result = (|| {
            if !batch.is_empty() {
                record(db, invocation, boundary.position, &batch)?;
            }
            let frame = serde_json::to_vec(&batch).context(JsonSnafu)?;
            stream.write_all(&frame).context(SteeringIoSnafu)
        })();
        if result.is_err() {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            batch.append(&mut state.pending);
            state.pending = batch;
        }
        result
    }
}

fn record(
    db: &Path,
    invocation: &Invocation,
    position: i64,
    batch: &[String],
) -> Result<(), ChatError> {
    ensure!(
        position > 0,
        InvalidSnafu {
            field: "steering position",
            reason: "must follow a completed reply"
        }
    );
    let mut db = storage::connect(db)?;
    db.immediate_transaction(|db| {
        let call = turn::load_call(db, &invocation.call_id)?;
        ensure!(
            call.state == "running"
                && call.purpose == "conversation"
                && call.session_id == invocation.session_id
                && call.turn_id.as_deref() == Some(&invocation.turn_id),
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        for (offset, text) in batch.iter().enumerate() {
            let position = position.checked_add(offset as i64).ok_or_else(|| {
                ArithmeticSnafu {
                    operation: "position steering messages",
                }
                .build()
            })?;
            diesel::insert_into(deliveries::table)
                .values((
                    deliveries::turn_id.eq(&invocation.turn_id),
                    deliveries::position.eq(position),
                    deliveries::content.eq(text),
                ))
                .execute(db)
                .context(DatabaseSnafu {
                    operation: "record steering delivery",
                })?;
        }
        Ok(())
    })
}

pub fn validate(
    db: &mut SqliteConnection,
    turn_id: &str,
    display: &[ChatMessage],
) -> Result<(), ChatError> {
    let admitted = deliveries::table
        .filter(deliveries::turn_id.eq(turn_id))
        .order(deliveries::position)
        .select((deliveries::position, deliveries::content))
        .load::<(i64, String)>(db)
        .context(DatabaseSnafu {
            operation: "validate steering deliveries",
        })?;
    let users = display
        .iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            ChatMessage::User { content, .. } => Some((index as i64, content)),
            _ => None,
        });
    ensure!(
        users.eq(admitted.iter().map(|(index, content)| (*index, content))),
        InvalidSnafu {
            field: "display steering",
            reason: "must exactly match host-delivered user input at its reply boundaries"
        }
    );
    Ok(())
}

/// Only this pull capability crosses the executable boundary. The runner never receives the
/// database path, application identity, or a channel for submitting user input.
struct Listener {
    address: SocketAddr,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), ChatError>>>,
    inbox: SteeringInbox,
    endpoint: Endpoint,
}

impl Listener {
    fn start(db: &Path, invocation: &Invocation, inbox: &SteeringInbox) -> Result<Self, ChatError> {
        ensure!(
            inbox.invocation() == invocation,
            ConflictSnafu {
                id: &invocation.call_id
            }
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context(SteeringIoSnafu)?;
        let address = listener.local_addr().context(SteeringIoSnafu)?;
        let token = Uuid::new_v4();
        let endpoint = Endpoint {
            port: address.port().into(),
            token: token.to_string(),
        };
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let queue = inbox.clone();
        let db = db.to_owned();
        let invocation = invocation.clone();
        let worker = thread::Builder::new()
            .name("mf-steering".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let mut stream = stream.context(SteeringIoSnafu)?;
                    // Reject other local clients before decoding anything or touching the inbox.
                    let timeout = Some(Duration::from_secs(1));
                    let mut auth = [0; 16];
                    if stream.set_read_timeout(timeout).is_err()
                        || stream.set_write_timeout(timeout).is_err()
                        || stream.read_exact(&mut auth).is_err()
                        || auth != *token.as_bytes()
                    {
                        continue;
                    }
                    let mut frame = Vec::new();
                    if (&stream).take(257).read_to_end(&mut frame).is_err() || frame.len() > 256 {
                        continue;
                    }
                    let Ok(boundary) = serde_json::from_slice(&frame) else {
                        continue;
                    };
                    queue.deliver(&db, &invocation, boundary, stream)?;
                }
                Ok(())
            })
            .context(WorkerSpawnSnafu)?;
        Ok(Self {
            address,
            stopped,
            worker: Some(worker),
            inbox: inbox.clone(),
            endpoint,
        })
    }
    fn finish(&mut self) -> Result<(), ChatError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        self.inbox.close();
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        match worker.join() {
            Ok(result) => result,
            Err(_) => WorkerPanicSnafu {
                id: "steering channel",
            }
            .fail(),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub fn with_endpoint<T>(
    db: &Path,
    invocation: &Invocation,
    inbox: Option<&SteeringInbox>,
    run: impl FnOnce(Option<Endpoint>) -> Result<T, ChatError>,
) -> Result<T, ChatError> {
    let Some(inbox) = inbox else { return run(None) };
    let mut listener = Listener::start(db, invocation, inbox)?;
    let result = run(Some(listener.endpoint.clone()));
    listener.finish()?;
    result
}
