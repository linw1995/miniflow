use crate::{
    DefinitionId, EmissionSink, NodeResult, StreamCancellation, StreamError, StreamInvocation,
    StreamOutputContract, ValueRef,
};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveOutputEvent {
    pub export: String,
    pub source: Vec<DefinitionId>,
    pub invocation: StreamInvocation,
    pub event: LiveOutputKind,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveOutputKind {
    Open,
    Value { value: ValueRef },
    Close { completed: bool },
}

type Publish =
    dyn Fn(LiveOutputEvent, &StreamCancellation) -> Result<(), StreamError> + Send + Sync;
type Flush = dyn Fn(&StreamCancellation) -> Result<(), StreamError> + Send + Sync;

struct Callbacks {
    publish: Box<Publish>,
    flush: Box<Flush>,
}

#[derive(Clone)]
pub struct LivePublisher {
    callbacks: Arc<Callbacks>,
    exports: Option<Arc<BTreeSet<String>>>,
}

impl std::fmt::Debug for LivePublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivePublisher").finish_non_exhaustive()
    }
}

impl LivePublisher {
    pub fn new(
        publish: impl Fn(LiveOutputEvent, &StreamCancellation) -> Result<(), StreamError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_flush(publish, |_| Ok(()))
    }

    pub fn with_flush(
        publish: impl Fn(LiveOutputEvent, &StreamCancellation) -> Result<(), StreamError>
        + Send
        + Sync
        + 'static,
        flush: impl Fn(&StreamCancellation) -> Result<(), StreamError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            callbacks: Arc::new(Callbacks {
                publish: Box::new(publish),
                flush: Box::new(flush),
            }),
            exports: None,
        }
    }

    pub fn select(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.exports = Some(Arc::new(names.into_iter().collect()));
        self
    }

    pub fn is_subscribed(&self, name: &str) -> bool {
        self.exports
            .as_ref()
            .is_none_or(|names| names.contains(name))
    }

    pub fn publish(
        &self,
        event: LiveOutputEvent,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        if !self.is_subscribed(&event.export) {
            return Ok(());
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.callbacks.publish)(event, cancellation)
        }))
        .unwrap_or_else(|payload| Err(crate::stream_instance::panic_error(payload)))
    }

    pub fn flush(&self, cancellation: &StreamCancellation) -> Result<(), StreamError> {
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.callbacks.flush)(cancellation)
        }))
        .unwrap_or_else(|payload| Err(crate::stream_instance::panic_error(payload)))
    }
}

pub(super) fn with_exports<T>(
    exports: &[StreamOutputContract],
    context: &mut crate::ExecutionContext,
    sink: &mut dyn EmissionSink,
    invoke: impl FnOnce(
        &mut crate::ExecutionContext,
        &mut dyn EmissionSink,
    ) -> Result<T, crate::NodeExecutionError>,
) -> Result<T, crate::NodeExecutionError> {
    let Some(publisher) = context
        .live_publisher()
        .filter(|_| !exports.is_empty())
        .cloned()
    else {
        return invoke(context, sink);
    };
    let selected = exports
        .iter()
        .filter(|export| publisher.is_subscribed(&export.name))
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return invoke(context, sink);
    }
    let mut exported = ExportSink {
        publisher,
        exports: &selected,
        invocation: StreamInvocation::new(context)?,
        cancellation: context.cancellation(),
        deadline: None,
    };
    exported.open()?;
    let result = invoke(
        context,
        &mut FanoutSink {
            first: sink,
            second: &mut exported,
        },
    );
    let closed = exported.close(result.is_ok());
    match result {
        Ok(result) => {
            closed?;
            Ok(result)
        }
        Err(error) => Err(error),
    }
}

pub(super) struct ExportSink<'a> {
    pub publisher: LivePublisher,
    pub exports: &'a [StreamOutputContract],
    pub invocation: StreamInvocation,
    pub cancellation: StreamCancellation,
    pub deadline: Option<std::time::Instant>,
}

impl ExportSink<'_> {
    fn publish(
        &self,
        export: &StreamOutputContract,
        event: LiveOutputKind,
    ) -> Result<(), StreamError> {
        let cancellation = self.deadline.map_or_else(
            || self.cancellation.clone(),
            |deadline| self.cancellation.with_deadline(deadline),
        );
        self.publish_with_cancellation(export, event, &cancellation)
    }

    fn publish_with_cancellation(
        &self,
        export: &StreamOutputContract,
        event: LiveOutputKind,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        self.publisher
            .publish(
                LiveOutputEvent {
                    export: export.name.clone(),
                    source: export.path.clone(),
                    invocation: self.invocation.clone(),
                    event,
                },
                cancellation,
            )
            .inspect_err(|error| self.cancellation.cancel(error.clone()))
    }

    pub fn open(&self) -> Result<(), StreamError> {
        for export in self.exports {
            self.publish(export, LiveOutputKind::Open)?;
        }
        Ok(())
    }

    pub fn close(&self, completed: bool) -> Result<(), StreamError> {
        for export in self.exports {
            self.publish(export, LiveOutputKind::Close { completed })?;
        }
        Ok(())
    }
}

impl EmissionSink for ExportSink<'_> {
    fn set_deadline(&mut self, deadline: std::time::Instant) {
        self.deadline = Some(
            self.deadline
                .map_or(deadline, |current| current.min(deadline)),
        );
    }
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.send_with_cancellation(result, &self.cancellation.clone())
    }

    fn send_with_cancellation(
        &mut self,
        result: NodeResult,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        for export in self.exports {
            if let Some(value) = result.outputs.get(&export.port) {
                self.publish_with_cancellation(
                    export,
                    LiveOutputKind::Value {
                        value: value.clone(),
                    },
                    cancellation,
                )?;
            }
        }
        Ok(())
    }
}

pub(super) struct FanoutSink<'a> {
    pub first: &'a mut dyn EmissionSink,
    pub second: &'a mut dyn EmissionSink,
}

impl EmissionSink for FanoutSink<'_> {
    fn set_deadline(&mut self, deadline: std::time::Instant) {
        self.first.set_deadline(deadline);
        self.second.set_deadline(deadline);
    }
    fn is_connected(&self) -> bool {
        self.first.is_connected() || self.second.is_connected()
    }

    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        self.first.send(result.clone())?;
        self.second.send(result)?;
        Ok(())
    }

    fn send_with_cancellation(
        &mut self,
        result: NodeResult,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        self.first
            .send_with_cancellation(result.clone(), cancellation)?;
        self.second.send_with_cancellation(result, cancellation)
    }
}
