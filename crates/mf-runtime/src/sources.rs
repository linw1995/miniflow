use crate::{
    InputResource, NodePluginFailedSnafu, StreamCapacitySnafu, StreamClosedSnafu,
    StreamCompilationSnafu, StreamError, StreamInputSnafu, StreamPreparationSnafu,
    StreamResourceSnafu, ValueRef, ValueType,
};
use snafu::ResultExt;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Wake, Waker},
};

#[derive(Clone, Debug, Default)]
pub struct StreamCancellation(Arc<Mutex<CancellationState>>);

#[derive(Debug, Default)]
struct CancellationState {
    failure: Option<StreamError>,
    wakers: Vec<Waker>,
}

impl StreamCancellation {
    pub fn failure(&self) -> Option<StreamError> {
        self.0.lock().unwrap().failure.clone()
    }

    pub fn cancel(&self, failure: StreamError) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            if state.failure.is_some() {
                return;
            }
            state.failure = Some(failure);
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    pub fn register(&self, waker: Waker) {
        let mut state = self.0.lock().unwrap();
        if state.failure.is_some() {
            drop(state);
            waker.wake();
        } else {
            state.wakers.push(waker);
        }
    }

    pub fn register_failure(&self, waker: Waker) {
        let mut state = self.0.lock().unwrap();
        if state.failure.is_some() {
            drop(state);
            waker.wake();
        } else {
            // Publish the original failure before waking its dependent source readers.
            state.wakers.insert(0, waker);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelMetrics {
    pub accepted: u64,
    pub published: u64,
    pub pending: usize,
    pub closed: bool,
    pub waiting_senders: usize,
}

#[derive(Debug, Default)]
struct ChannelState {
    queue: VecDeque<ValueRef>,
    closed: bool,
    senders: usize,
    accepted: u64,
    published: u64,
    waiting_senders: usize,
    failure: Option<StreamError>,
    cancellation: Weak<Mutex<CancellationState>>,
}

#[derive(Debug)]
struct Channel {
    state: Mutex<ChannelState>,
    changed: Condvar,
    value_type: ValueType,
    capacity: usize,
}

#[derive(Debug)]
pub struct ChannelSender(Arc<Channel>);

impl Clone for ChannelSender {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap().senders += 1;
        Self(Arc::clone(&self.0))
    }
}

impl Drop for ChannelSender {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.senders -= 1;
        if state.senders == 0 {
            state.closed = true;
            self.0.changed.notify_all();
        }
    }
}

impl ChannelSender {
    pub fn send(&self, value: impl Into<ValueRef>) -> Result<(), StreamError> {
        self.admit(value.into(), true)
    }

    pub fn try_send(&self, value: &ValueRef) -> Result<(), StreamError> {
        self.admit(value.clone(), false)
    }

    fn admit(&self, value: ValueRef, wait: bool) -> Result<(), StreamError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if state.closed {
                return Err(self.reject(state, StreamClosedSnafu.build()));
            }
            if let Err(error) = self
                .0
                .value_type
                .validate_shared(&value)
                .context(StreamInputSnafu)
            {
                return Err(self.reject(state, error));
            }
            if state.queue.len() < self.0.capacity {
                break;
            }
            if !wait {
                return StreamCapacitySnafu.fail();
            }
            state.waiting_senders += 1;
            state = self.0.changed.wait(state).unwrap();
            state.waiting_senders -= 1;
        }
        let Some(next) = state.accepted.checked_add(1) else {
            let error = StreamResourceSnafu {
                message: "channel admission counter exhausted".to_owned(),
            }
            .build();
            return Err(self.reject(state, error));
        };
        state.accepted = next;
        state.queue.push_back(value);
        self.0.changed.notify_all();
        Ok(())
    }

    fn reject(
        &self,
        mut state: std::sync::MutexGuard<'_, ChannelState>,
        error: StreamError,
    ) -> StreamError {
        if let Some(cancellation) = state.cancellation.upgrade() {
            // The instance must see the cause before the source can return a wrapped error.
            drop(state);
            StreamCancellation(cancellation).cancel(error.clone());
            self.0
                .state
                .lock()
                .unwrap()
                .failure
                .get_or_insert_with(|| error.clone());
        } else {
            state.failure.get_or_insert_with(|| error.clone());
        }
        self.0.changed.notify_all();
        error
    }

    pub fn close(&self) {
        self.0.state.lock().unwrap().closed = true;
        self.0.changed.notify_all();
    }

    pub fn failure(&self) -> Option<StreamError> {
        self.0.state.lock().unwrap().failure.clone()
    }

    pub fn metrics(&self) -> ChannelMetrics {
        let state = self.0.state.lock().unwrap();
        ChannelMetrics {
            accepted: state.accepted,
            published: state.published,
            pending: state.queue.len(),
            closed: state.closed,
            waiting_senders: state.waiting_senders,
        }
    }
}

struct ChannelWake {
    channel: Weak<Channel>,
    cancellation: Weak<Mutex<CancellationState>>,
}

impl Wake for ChannelWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let Some(cancellation) = self.cancellation.upgrade() else {
            return;
        };
        let failure = cancellation.lock().unwrap().failure.clone();
        if let Some(channel) = self.channel.upgrade() {
            // Readers check this state while holding the same mutex before waiting.
            let mut state = channel.state.lock().unwrap();
            if state.failure.is_none() {
                state.failure = failure;
            }
            channel.changed.notify_all();
        }
    }
}

#[derive(Debug, Default)]
pub struct ExecutionResources {
    stdin: Option<Arc<Mutex<crate::StreamInput>>>,
    channels: BTreeMap<String, Arc<Channel>>,
}

impl ExecutionResources {
    pub fn extend(&mut self, mut other: Self) -> Result<(), StreamError> {
        if (self.stdin.is_some() && other.stdin.is_some())
            || other
                .channels
                .keys()
                .any(|key| self.channels.contains_key(key))
        {
            return StreamPreparationSnafu {
                message: "input resource was supplied more than once".to_owned(),
            }
            .fail();
        }
        if let Some(stdin) = other.stdin.take() {
            self.stdin = Some(stdin);
        }
        self.channels.extend(std::mem::take(&mut other.channels));
        Ok(())
    }

    pub fn with_stdin(mut self, input: crate::StreamInput) -> Self {
        self.stdin = Some(Arc::new(Mutex::new(input)));
        self
    }

    pub fn channel(
        &mut self,
        node: &str,
        value_type: ValueType,
        capacity: usize,
    ) -> Result<ChannelSender, StreamError> {
        if capacity == 0 {
            return StreamPreparationSnafu {
                message: "channel capacity must be positive".to_owned(),
            }
            .fail();
        }
        value_type
            .check_depth()
            .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> { Box::new(source) })
            .context(StreamCompilationSnafu)?;
        let channel = self.channels.entry(node.into()).or_insert_with(|| {
            Arc::new(Channel {
                state: Mutex::new(ChannelState::default()),
                changed: Condvar::new(),
                value_type: value_type.clone(),
                capacity,
            })
        });
        if channel.value_type != value_type || channel.capacity != capacity {
            return StreamPreparationSnafu {
                message: format!("channel `{node}` already has different type or capacity"),
            }
            .fail();
        }
        channel.state.lock().unwrap().senders += 1;
        Ok(ChannelSender(Arc::clone(channel)))
    }

    pub fn available(&self, node: &str, resource: InputResource) -> bool {
        match resource {
            InputResource::Stdin => self.stdin.is_some(),
            InputResource::Channel => self.channels.contains_key(node),
        }
    }

    pub fn bind_cancellation(&self, cancellation: &StreamCancellation) {
        for channel in self.channels.values() {
            let failure = {
                let mut state = channel.state.lock().unwrap();
                state.cancellation = Arc::downgrade(&cancellation.0);
                state.failure.clone()
            };
            if let Some(failure) = failure {
                cancellation.cancel(failure);
            }
            cancellation.register(Waker::from(Arc::new(ChannelWake {
                channel: Arc::downgrade(channel),
                cancellation: Arc::downgrade(&cancellation.0),
            })));
        }
    }

    pub fn stdin_next(
        &self,
        value_type: &ValueType,
        cancellation: &StreamCancellation,
    ) -> Result<Option<ValueRef>, StreamError> {
        let input = self.stdin.as_ref().ok_or_else(|| {
            StreamPreparationSnafu {
                message: "stdin resource is unavailable".to_owned(),
            }
            .build()
        })?;
        input.lock().unwrap().next_value(value_type, cancellation)
    }

    pub fn channel_next(&self, node: &str) -> Result<Option<ValueRef>, StreamError> {
        let channel = self.channels.get(node).ok_or_else(|| {
            StreamPreparationSnafu {
                message: format!("channel resource `{node}` is unavailable"),
            }
            .build()
        })?;
        let mut state = channel.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure {
                return Err(error.clone());
            }
            if let Some(value) = state.queue.pop_front() {
                channel.changed.notify_all();
                return Ok(Some(value));
            }
            if state.closed {
                return Ok(None);
            }
            state = channel.changed.wait(state).unwrap();
        }
    }

    pub fn channel_published(&self, node: &str) -> Result<(), StreamError> {
        let channel = self.channels.get(node).ok_or_else(|| {
            StreamPreparationSnafu {
                message: format!("channel resource `{node}` is unavailable"),
            }
            .build()
        })?;
        let mut state = channel.state.lock().unwrap();
        state.published = state.published.checked_add(1).ok_or_else(|| {
            StreamResourceSnafu {
                message: "channel publication counter exhausted".to_owned(),
            }
            .build()
        })?;
        channel.changed.notify_all();
        Ok(())
    }
}

impl Drop for ExecutionResources {
    fn drop(&mut self) {
        for channel in self.channels.values() {
            let mut state = channel.state.lock().unwrap();
            state.closed = true;
            state.queue.clear();
            channel.changed.notify_all();
        }
    }
}

pub fn channel_source(value_type: ValueType) -> crate::PreparedNode {
    struct Source;
    impl crate::StreamNode for Source {
        fn execute(
            &mut self,
            _: crate::Inputs,
            context: &mut crate::ExecutionContext,
            emitter: &mut crate::Emitter<'_>,
        ) -> Result<(), crate::NodeExecutionError> {
            while let Some(value) = context
                .channel_next()
                .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> { Box::new(source) })
                .context(NodePluginFailedSnafu)?
            {
                emitter.send(crate::Outputs::from([("item".into(), value)]).into())?;
                context
                    .channel_published()
                    .map_err(|source| -> Box<dyn std::error::Error + Send + Sync> {
                        Box::new(source)
                    })
                    .context(NodePluginFailedSnafu)?;
            }
            Ok(())
        }
    }
    crate::PreparedNode::stream(
        Source,
        crate::NodeMetadata {
            ports: crate::NodePorts {
                inputs: Vec::new(),
                outputs: vec![crate::PortSpec::new("item", value_type, true)],
            },
            resources: vec![InputResource::Channel],
            ..Default::default()
        },
    )
}
