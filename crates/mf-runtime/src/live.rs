use crate::{
    DefinitionId, EmissionError, EmissionSink, NodeResult, StreamCancellation, StreamError,
    StreamInvocation, StreamOutputContract, ValueRef,
};
use std::sync::Arc;

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

#[derive(Clone)]
pub struct LivePublisher(Arc<Publish>);

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
        Self(Arc::new(publish))
    }

    pub fn publish(
        &self,
        event: LiveOutputEvent,
        cancellation: &StreamCancellation,
    ) -> Result<(), StreamError> {
        if let Some(error) = cancellation.failure() {
            return Err(error);
        }
        (self.0)(event, cancellation)
    }
}

pub(super) struct ExportSink<'a> {
    pub publisher: LivePublisher,
    pub exports: &'a [StreamOutputContract],
    pub invocation: StreamInvocation,
    pub cancellation: StreamCancellation,
}

impl ExportSink<'_> {
    fn publish(
        &self,
        export: &StreamOutputContract,
        event: LiveOutputKind,
    ) -> Result<(), StreamError> {
        self.publisher.publish(
            LiveOutputEvent {
                export: export.name.clone(),
                source: export.path.clone(),
                invocation: self.invocation.clone(),
                event,
            },
            &self.cancellation,
        )
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
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        for export in self.exports {
            if let Some(value) = result.outputs.get(&export.port) {
                self.publish(
                    export,
                    LiveOutputKind::Value {
                        value: value.clone(),
                    },
                )?;
            }
        }
        Ok(())
    }

    fn fail(&mut self, error: EmissionError) {
        use snafu::IntoError;
        self.cancellation.cancel(
            crate::stream_instance::ProducerSnafu {
                definition_id: self.invocation.node.as_str(),
            }
            .into_error(error.into()),
        );
    }
}
