//! Current-value notifications at stream execution boundaries.

use crate::{Inputs, StreamObservation};
use mf_telemetry::stream_data::{DataIdentity, Direction};
use std::{fmt, sync::Arc};

type DataCallback = dyn Fn(DataIdentity, Direction, &Inputs) + Send + Sync;

#[derive(Clone)]
pub struct StreamDataObserver {
    observation: StreamObservation,
    callback: Arc<DataCallback>,
}

impl fmt::Debug for StreamDataObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamDataObserver")
            .finish_non_exhaustive()
    }
}

impl StreamDataObserver {
    /// The callback must be nonblocking and must not unwind.
    pub fn new(
        observation: StreamObservation,
        callback: impl Fn(DataIdentity, Direction, &Inputs) + Send + Sync + 'static,
    ) -> Self {
        Self {
            observation,
            callback: Arc::new(callback),
        }
    }

    pub fn matches(&self, observation: &StreamObservation) -> bool {
        self.observation.is_same_run(observation)
    }

    pub fn record(&self, identity: DataIdentity, direction: Direction, ports: &Inputs) {
        (self.callback)(identity, direction, ports);
    }
}
