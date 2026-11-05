use crate::{
    EmissionSink, ExecutionContext, FlowOutputs, MessageId, NodeResult, StreamCancellation,
    StreamError, StreamInstance, WorkflowRunError,
};
use snafu::ResultExt;
use std::{
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Wake, Waker},
    thread::JoinHandle,
};

/// One selected Flow emission. Terminal results are returned only by `join`.
#[derive(Clone, Debug)]
pub struct FlowEmission {
    pub result: NodeResult,
    /// Event selections retain their source frame, including startup domain zero.
    /// Finite TaskStream emissions have no message frame identifier.
    pub message: Option<MessageId>,
}

/// A running passed Flow, independent of its receiver's driving policy.
///
/// `recv` drains incremental emissions; `join` drains any remaining emissions and
/// returns authoritative terminal outputs for finite Task/TaskStream bodies, or
/// `None` for Event bodies. Dropping an unfinished invocation cancels and joins it.
/// The invocation owns a child cancellation token: parent cancellation propagates
/// down, but ordinary body failure, explicit child cancellation, and dropping this
/// handle do not cancel the parent or sibling invocations. Receivers decide whether
/// a failure should abort their execution. Cancellation is cooperative: custom
/// synchronous bodies must check their context or publish through the supplied sink.
///
/// Inputs are fixed by the startup context; there is no externally writable input
/// channel or `close_input` operation. Finite upstream completion naturally delivers
/// `NodeEvent::UpstreamClosed` to Event operators, allowing graceful flushing.
/// Cancellation and dropping an unfinished invocation abort instead of flushing.
pub struct FlowInvocation {
    backend: InvocationBackend,
}

enum InvocationBackend {
    Finite {
        shared: Arc<Shared>,
        worker: Option<JoinHandle<Result<FlowOutputs, WorkflowRunError>>>,
    },
    Event(Option<StreamInstance>),
}

#[derive(Default)]
struct State {
    output: Option<NodeResult>,
    done: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    cancellation: StreamCancellation,
    workers: Option<crate::RuntimeWorkerHandle>,
}

impl Shared {
    fn wait_for_progress<'a>(
        &'a self,
        state: std::sync::MutexGuard<'a, State>,
    ) -> std::sync::MutexGuard<'a, State> {
        if let Some(workers) = &self.workers
            && workers.is_current_worker()
        {
            drop(state);
            let helped = workers.help_one();
            let state = self.state.lock().unwrap();
            if helped {
                return state;
            }
            self.changed
                .wait_timeout(state, std::time::Duration::from_millis(1))
                .unwrap()
                .0
        } else {
            self.changed.wait(state).unwrap()
        }
    }

    fn wait_until_done(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.done {
            state = self.wait_for_progress(state);
        }
    }
}

struct CancellationWake(Weak<Shared>);
impl Wake for CancellationWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(shared) = self.0.upgrade() {
            let _guard = shared.state.lock().unwrap();
            shared.changed.notify_all();
        }
    }
}

struct InvocationSink(Arc<Shared>);
impl EmissionSink for InvocationSink {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        let mut state = self.0.state.lock().unwrap();
        loop {
            if let Some(error) = self.0.cancellation.failure() {
                return Err(error);
            }
            if state.output.is_none() {
                state.output = Some(result);
                self.0.changed.notify_all();
                return Ok(());
            }
            state = self.0.changed.wait(state).unwrap();
        }
    }
}

/// Keeps the typed stream/runtime source in the standard workflow error chain.
pub fn workflow_result<T>(result: Result<T, StreamError>) -> Result<T, WorkflowRunError> {
    result
        .boxed()
        .context(crate::NodePluginFailedSnafu)
        .context(crate::NodeExecutionSnafu {
            definition_id: "<flow>",
        })
}

impl FlowInvocation {
    pub fn start_body(
        body: Arc<crate::subgraph::SubgraphBody>,
        mut context: ExecutionContext,
    ) -> Result<Self, WorkflowRunError> {
        context.set_cancellation(context.cancellation().child());
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            changed: Condvar::new(),
            cancellation: context.cancellation(),
            workers: context.worker_handle(),
        });
        shared
            .cancellation
            .register(Waker::from(Arc::new(CancellationWake(Arc::downgrade(
                &shared,
            )))));
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("flow-invocation".into())
            .spawn(move || {
                let _workers = context.worker_scope();
                let observation = context.observation().cloned();
                let _span = observation.as_ref().map(crate::RunObservation::enter);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    workflow_result(context.cancellation().failure().map_or(Ok(()), Err))?;
                    let outputs = body(&mut context, &mut InvocationSink(worker_shared.clone()))?;
                    workflow_result(context.cancellation().failure().map_or(Ok(()), Err))?;
                    Ok(outputs)
                }))
                .unwrap_or_else(|panic| {
                    workflow_result(Err(crate::stream_instance::panic_error(panic)))
                });
                let mut state = worker_shared.state.lock().unwrap();
                state.done = true;
                worker_shared.changed.notify_all();
                result
            })
            .context(crate::stream_instance::ThreadSpawnSnafu {
                thread: "flow-invocation",
            });
        Ok(Self {
            backend: InvocationBackend::Finite {
                shared,
                worker: Some(workflow_result(worker)?),
            },
        })
    }

    pub fn from_stream(instance: StreamInstance) -> Self {
        Self {
            backend: InvocationBackend::Event(Some(instance)),
        }
    }

    pub fn cancellation(&self) -> StreamCancellation {
        match &self.backend {
            InvocationBackend::Finite { shared, .. } => shared.cancellation.clone(),
            InvocationBackend::Event(instance) => instance.as_ref().unwrap().cancellation(),
        }
    }

    pub fn recv(&self) -> Result<Option<FlowEmission>, WorkflowRunError> {
        match &self.backend {
            InvocationBackend::Finite { shared, .. } => {
                let mut state = shared.state.lock().unwrap();
                loop {
                    workflow_result(shared.cancellation.failure().map_or(Ok(()), Err))?;
                    if let Some(result) = state.output.take() {
                        shared.changed.notify_all();
                        return Ok(Some(FlowEmission {
                            result,
                            message: None,
                        }));
                    }
                    if state.done {
                        return Ok(None);
                    }
                    state = shared.wait_for_progress(state);
                }
            }
            InvocationBackend::Event(instance) => {
                workflow_result(instance.as_ref().unwrap().recv()).map(|output| {
                    output.map(|output| FlowEmission {
                        result: output.outputs.into(),
                        message: Some(output.message),
                    })
                })
            }
        }
    }

    /// Discards unconsumed emissions before waiting, so bounded producers can finish.
    /// Errors from finite bodies are authoritative here, even after `recv` returns `None`.
    pub fn join(mut self) -> Result<Option<FlowOutputs>, WorkflowRunError> {
        // A cancellation error must not skip joining and cleanup.
        let _ = self.drain();
        match &mut self.backend {
            InvocationBackend::Finite { shared, worker } => {
                shared.wait_until_done();
                worker
                    .take()
                    .unwrap()
                    .join()
                    .unwrap_or_else(|panic| {
                        workflow_result(Err(crate::stream_instance::panic_error(panic)))
                    })
                    .map(Some)
            }
            InvocationBackend::Event(instance) => {
                workflow_result(instance.take().unwrap().join()).map(|_| None)
            }
        }
    }

    fn drain(&self) -> Result<(), WorkflowRunError> {
        while self.recv()?.is_some() {}
        Ok(())
    }
}

impl Drop for FlowInvocation {
    fn drop(&mut self) {
        if let InvocationBackend::Finite { shared, worker } = &mut self.backend
            && let Some(worker) = worker.take()
        {
            if !shared.state.lock().unwrap().done {
                shared.cancellation.cancel(
                    crate::StreamExecutionSnafu {
                        message: "Flow invocation dropped before completion",
                    }
                    .build(),
                );
            }
            shared.wait_until_done();
            let _ = worker.join();
        }
        // StreamInstance implements its own cancellation and join on drop.
    }
}
