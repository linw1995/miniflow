use crate::{LiveOutputEvent, LivePublisher, StreamCancellation, StreamError};
use snafu::{Snafu, ensure};
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct LiveLimits {
    pub max_pending_records: usize,
}

impl Default for LiveLimits {
    fn default() -> Self {
        Self {
            max_pending_records: 64,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveMetrics {
    pub pending_records: usize,
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
    queue: VecDeque<LiveOutputEvent>,
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
        self.0.changed.notify_all();
    }
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
            if state.queue.len() < self.0.limits.max_pending_records {
                break;
            }
            state = self
                .0
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        state.queue.push_back(event);
        self.0.changed.notify_all();
        Ok(())
    }
}

impl LiveReceiver {
    fn pop(state: &mut State) -> Result<LiveOutputEvent, LiveReceiveError> {
        if let Some(event) = state.queue.pop_front() {
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
        }
    }
}

pub fn live_channel(limits: LiveLimits) -> Result<(LivePublisher, LiveReceiver), StreamError> {
    ensure!(
        limits.max_pending_records > 0,
        crate::stream_instance::ConfigurationSnafu {
            message: "live limits require positive record capacity"
        }
    );
    let shared = Arc::new(Shared {
        limits,
        state: Mutex::new(State {
            queue: VecDeque::new(),
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
