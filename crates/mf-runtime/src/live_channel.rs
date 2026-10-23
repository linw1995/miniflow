use crate::{LiveOutputEvent, LivePublisher, StreamCancellation, StreamError};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct LiveLimits {
    pub max_pending_records: usize,
    pub max_pending_bytes: usize,
    pub max_record_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for LiveLimits {
    fn default() -> Self {
        Self {
            max_pending_records: 64,
            max_pending_bytes: 4 * 1024 * 1024,
            max_record_bytes: 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveMetrics {
    pub pending_records: usize,
    pub pending_bytes: usize,
    pub total_bytes: usize,
}

#[derive(Debug, Snafu)]
pub enum LiveReceiveError {
    #[snafu(display("live output is not ready"))]
    Empty,
    #[snafu(display("live output is closed"))]
    Closed,
    #[snafu(transparent)]
    Failed { source: StreamError },
}

struct State {
    queue: VecDeque<(LiveOutputEvent, usize)>,
    bytes: usize,
    total: usize,
    receiver_alive: bool,
    closed: bool,
    failure: Option<StreamError>,
    cancellation: Option<StreamCancellation>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    limits: LiveLimits,
}

struct Sender(Arc<Shared>);

impl Drop for Sender {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        if state.failure.is_none() {
            state.failure = state
                .cancellation
                .as_ref()
                .and_then(StreamCancellation::failure);
        }
        state.closed = true;
        self.0.changed.notify_all();
    }
}

pub struct LiveReceiver(Arc<Shared>);

impl Drop for LiveReceiver {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.receiver_alive = false;
        if !state.queue.is_empty() {
            state.failure.get_or_insert_with(|| {
                crate::stream_instance::OutputSnafu {
                    message: "live output receiver disconnected",
                }
                .build()
            });
        }
        state.queue.clear();
        state.bytes = 0;
        self.0.changed.notify_all();
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("live output record exceeds {limit} bytes"))]
struct RecordLimit {
    limit: usize,
}

struct SizeWriter {
    size: usize,
    limit: usize,
}

impl io::Write for SizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.size {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                RecordLimitSnafu { limit: self.limit }.build(),
            ));
        }
        self.size += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encoded_size<T: serde::Serialize + ?Sized>(
    event: &T,
    limit: usize,
) -> Result<usize, StreamError> {
    let mut writer = SizeWriter { size: 0, limit };
    serde_json::to_writer(&mut writer, event).context(crate::stream_instance::OutputEncodeSnafu)?;
    Ok(writer.size)
}

fn resource(message: &str) -> StreamError {
    crate::stream_instance::ResourceSnafu { message }.build()
}

impl Sender {
    fn flush(&self, cancellation: &StreamCancellation) -> Result<(), StreamError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            if state.queue.is_empty() {
                return Ok(());
            }
            state = self
                .0
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
    fn publish(
        &self,
        event: LiveOutputEvent,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        let bytes = match encoded_size(&event, self.0.limits.max_record_bytes) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.0
                    .state
                    .lock()
                    .unwrap()
                    .failure
                    .get_or_insert(error.clone());
                self.0.changed.notify_all();
                return Err(error);
            }
        };
        let mut state = self.0.state.lock().unwrap();
        state
            .cancellation
            .get_or_insert_with(|| cancellation.without_deadline());
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if let Some(error) = cancellation.failure() {
                return Err(error);
            }
            if !state.receiver_alive {
                return crate::stream_instance::OutputSnafu {
                    message: "live output receiver disconnected",
                }
                .fail();
            }
            if bytes > self.0.limits.max_total_bytes - state.total {
                let error = resource("live output exceeds the cumulative byte limit");
                state.failure = Some(error.clone());
                self.0.changed.notify_all();
                return Err(error);
            }
            if state.queue.len() < self.0.limits.max_pending_records
                && bytes <= self.0.limits.max_pending_bytes - state.bytes
            {
                break;
            }
            state = self
                .0
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        state.bytes += bytes;
        state.total += bytes;
        state.queue.push_back((event, bytes));
        self.0.changed.notify_all();
        Ok(())
    }
}

impl LiveReceiver {
    fn pop(state: &mut State) -> Result<LiveOutputEvent, LiveReceiveError> {
        if let Some((event, bytes)) = state.queue.pop_front() {
            state.bytes -= bytes;
            return Ok(event);
        }
        if let Some(error) = &state.failure {
            return Err(error.clone().into());
        }
        if state.closed {
            return ClosedSnafu.fail();
        }
        if let Some(error) = state
            .cancellation
            .as_ref()
            .and_then(StreamCancellation::failure)
        {
            return Err(error.into());
        }
        EmptySnafu.fail()
    }

    pub fn try_recv(&self) -> Result<LiveOutputEvent, LiveReceiveError> {
        let result = Self::pop(&mut self.0.state.lock().unwrap());
        self.0.changed.notify_all();
        result
    }

    pub fn recv(&self) -> Result<Option<LiveOutputEvent>, StreamError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            match Self::pop(&mut state) {
                Ok(event) => {
                    self.0.changed.notify_all();
                    return Ok(Some(event));
                }
                Err(LiveReceiveError::Closed) => return Ok(None),
                Err(LiveReceiveError::Failed { source }) => return Err(source),
                Err(LiveReceiveError::Empty) => {
                    state = self
                        .0
                        .changed
                        .wait_timeout(state, Duration::from_millis(20))
                        .unwrap()
                        .0;
                }
            }
        }
    }

    pub fn metrics(&self) -> LiveMetrics {
        let state = self.0.state.lock().unwrap();
        LiveMetrics {
            pending_records: state.queue.len(),
            pending_bytes: state.bytes,
            total_bytes: state.total,
        }
    }
}

pub fn live_channel(limits: LiveLimits) -> Result<(LivePublisher, LiveReceiver), StreamError> {
    ensure!(
        limits.max_pending_records > 0
            && limits.max_record_bytes > 0
            && limits.max_record_bytes <= limits.max_pending_bytes
            && limits.max_pending_bytes <= limits.max_total_bytes,
        crate::stream_instance::ConfigurationSnafu {
            message: "live limits require positive record capacity and record <= pending <= total bytes"
        }
    );
    let shared = Arc::new(Shared {
        limits,
        state: Mutex::new(State {
            queue: VecDeque::new(),
            bytes: 0,
            total: 0,
            receiver_alive: true,
            closed: false,
            failure: None,
            cancellation: None,
        }),
        changed: Condvar::new(),
    });
    let sender = Arc::new(Sender(shared.clone()));
    let flushing = sender.clone();
    let publisher = LivePublisher::with_flush(
        move |event, cancellation| sender.publish(event, cancellation),
        move |cancellation| flushing.flush(cancellation),
    );
    Ok((publisher, LiveReceiver(shared)))
}
