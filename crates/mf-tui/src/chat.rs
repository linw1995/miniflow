//! A single conversation execution lane over a supplied workflow executable.

use crate::{
    run::{LocalObservation, LocalOutcome, RunError, RunOptions},
    runner::{PreparedRunner, RunnerError, RunnerInvocation},
};
use mf_chat::contract::{
    ChatContract, CompletionState, ConversationExit, ReconciledCompletion, TurnExit,
};
use mf_chat::{
    ChatDatabase, Invocation, ModelInfo, ModelSelection, ModelSelectionCancellation, Session,
    SessionPage, UncommittedCompletion, display,
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod preview;
use preview::Capture;
pub use preview::{LivePreview, PreviewError};
mod transcript;
mod ui;
pub use ui::run_chat;
const QUEUE_CAPACITY: usize = 16;
const PAGE_SIZE: usize = 50;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ChatRunError {
    #[snafu(transparent)]
    Json { source: serde_json::Error },
    #[snafu(transparent)]
    Domain { source: mf_chat::ChatError },
    #[snafu(transparent)]
    Inputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(transparent)]
    Observation { source: RunError },
    #[snafu(transparent)]
    Runner { source: RunnerError },
    #[snafu(display("could not {operation}: {source}"))]
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[snafu(display("chat requires terminal stdin and stderr"))]
    TerminalRequired,
    #[snafu(display("turn cancelled before launch"))]
    Cancelled,
    #[snafu(display("chat command queue is full"))]
    QueueFull,
    #[snafu(display("chat controller is closed"))]
    Closed,
    #[snafu(display("chat controller worker panicked"))]
    WorkerPanic,
    #[snafu(display("finish, cancel, or recover the current turn first"))]
    Busy,
    #[snafu(display("there is no reply to recover"))]
    MissingReply,
    #[snafu(display("invalid chat options: {reason}"))]
    Options { reason: &'static str },
}

#[derive(Clone, Debug)]
pub struct ChatOptions {
    pub database_path: PathBuf,
    pub profile: String,
    pub run: RunOptions,
}

pub struct ChatLaunch {
    pub runner: PreparedRunner,
    pub contract: ChatContract,
    pub options: ChatOptions,
    pub parameters: mf_runtime::WorkflowArguments,
}
impl ChatLaunch {
    pub fn prepare(executable: &Path, options: ChatOptions) -> Result<Self, ChatRunError> {
        ensure!(
            !options.profile.trim().is_empty(),
            OptionsSnafu {
                reason: "requires a nonblank profile"
            }
        );
        let runner =
            PreparedRunner::load(executable, mf_chat::contract::MAX_RECOVERY_BYTES as u64)?;
        let contract = ChatContract::from_manifest(runner.manifest())?;
        let parameters = options
            .run
            .parsed_workflow_arguments(&contract.parameters)?;
        ModelSelection::prepare(
            &options.database_path,
            &options.profile,
            &contract,
            &parameters,
        )?;
        Ok(Self {
            runner,
            contract,
            options,
            parameters,
        })
    }
}

pub enum ChatCommand {
    ListSessions {
        after: Option<String>,
    },
    Load {
        session_id: String,
        before: Option<i64>,
    },
    ListModels,
    SelectModel {
        model: String,
    },
    Submit {
        session_id: Option<String>,
        prompt: String,
    },
    Retry {
        turn_id: String,
    },
    Save,
    Abandon,
}

pub enum ChatEvent {
    Sessions(SessionPage),
    Loaded {
        session: Session,
        page: display::Page,
        context_tokens: Option<u64>,
    },
    Models(Result<Vec<ModelInfo>, String>),
    ModelSelected {
        model: Option<String>,
        context_window: Option<u64>,
        automatic: bool,
        warning: Option<String>,
    },
    ContextWindow {
        model: String,
        context_window: Option<u64>,
    },
    Submitted {
        session: Session,
        invocation: Invocation,
        prompt: String,
    },
    Observing(LocalObservation),
    Preview {
        invocation: Invocation,
        preview: LivePreview,
    },
    Compacting {
        invocation: Invocation,
        active: bool,
    },
    Finished {
        invocation: Invocation,
        resolution: Result<ReconciledCompletion, mf_chat::ChatError>,
        status: Option<ExitStatus>,
        stderr: String,
        capture_error: Option<String>,
    },
    Error {
        admission: bool,
        source: ChatRunError,
    },
}

pub struct ChatController {
    _database: Arc<ChatDatabase>,
    commands: SyncSender<ChatCommand>,
    events: Receiver<ChatEvent>,
    deferred: Mutex<Option<ChatEvent>>,
    preview: Arc<Mutex<Capture>>,
    preview_revision: AtomicU64,
    compacting: Arc<Mutex<(Option<Invocation>, bool)>>,
    presented_compacting: AtomicBool,
    presented_invocation: Mutex<Option<Invocation>>,
    supports_model_selection: bool,
    initial_model: Option<String>,
    loading_default_model: bool,
    model_selection_cancel: ModelSelectionCancellation,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), ChatRunError>>>,
}
impl ChatController {
    pub fn open(launch: ChatLaunch) -> Result<Self, ChatRunError> {
        let database = Arc::new(ChatDatabase::open(&launch.options.database_path)?);
        let mut model_selection = ModelSelection::prepare(
            database.path(),
            &launch.options.profile,
            &launch.contract,
            &launch.parameters,
        )?;
        model_selection.commit_startup_preferences()?;
        let supports_model_selection = model_selection.supports_model_selection();
        let loading_default_model = model_selection.needs_default_model();
        let initial_model = if loading_default_model {
            None
        } else {
            model_selection.display_model()
        };
        let model_selection_cancel = model_selection.cancellation();
        let (commands, input) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (output, events) = mpsc::sync_channel(QUEUE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let preview = Arc::new(Mutex::new(Capture::default()));
        let compacting = Arc::new(Mutex::new((None, false)));
        let controller = Controller {
            launch: Arc::new(launch),
            model_selection,
            database: Arc::clone(&database),
            input,
            output,
            closed: Arc::clone(&closed),
            busy: Arc::clone(&busy),
            cancel: Arc::clone(&cancel),
            retained: None,
            preview: preview.clone(),
            compacting: compacting.clone(),
        };
        let worker = thread::Builder::new()
            .name("mf-chat-controller".into())
            .spawn(move || controller.run())
            .context(IoSnafu {
                operation: "start chat controller",
            })?;
        Ok(Self {
            _database: database,
            commands,
            events,
            deferred: Mutex::new(None),
            preview,
            preview_revision: AtomicU64::new(0),
            compacting,
            presented_compacting: AtomicBool::new(false),
            presented_invocation: Mutex::new(None),
            supports_model_selection,
            initial_model,
            loading_default_model,
            model_selection_cancel,
            closed,
            busy,
            cancel,
            worker: Some(worker),
        })
    }
    pub fn supports_model_selection(&self) -> bool {
        self.supports_model_selection
    }
    pub fn initial_model(&self) -> Option<&str> {
        self.initial_model.as_deref()
    }
    pub fn loading_default_model(&self) -> bool {
        self.loading_default_model
    }
    pub fn send(&self, command: ChatCommand) -> Result<(), ChatRunError> {
        ensure!(!self.closed.load(Ordering::Acquire), ClosedSnafu);
        let admission = matches!(
            command,
            ChatCommand::Submit { .. } | ChatCommand::Retry { .. }
        );
        if admission {
            ensure!(
                self.busy
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok(),
                BusySnafu
            );
            self.cancel.store(false, Ordering::Release);
        }
        let result = match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => QueueFullSnafu.fail(),
            Err(TrySendError::Disconnected(_)) => ClosedSnafu.fail(),
        };
        if result.is_err() && admission {
            self.busy.store(false, Ordering::Release);
        }
        result
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.model_selection_cancel.cancel_active();
    }
    pub fn try_event(&self) -> Option<ChatEvent> {
        let event = self
            .deferred
            .lock()
            .unwrap()
            .take()
            .or_else(|| self.events.try_recv().ok());
        if let Some(ChatEvent::Submitted { invocation, .. }) = &event {
            *self.presented_invocation.lock().unwrap() = Some(invocation.clone());
            self.presented_compacting.store(false, Ordering::Release);
        }
        if matches!(event, Some(ChatEvent::Finished { .. }))
            && let Some(preview) = self.updated_preview()
        {
            *self.deferred.lock().unwrap() = event;
            return Some(preview);
        }
        if let Some(ChatEvent::Finished { invocation, .. }) = &event {
            let mut preview = self.preview.lock().unwrap();
            if preview.invocation.as_ref() == Some(invocation) {
                // The view already received the last preview, including any unfinished reply.
                let revision = preview.revision;
                *preview = Capture::default();
                preview.frame.revision = revision;
            }
            *self.presented_invocation.lock().unwrap() = None;
            self.presented_compacting.store(false, Ordering::Release);
        }
        event
            .or_else(|| self.updated_preview())
            .or_else(|| self.updated_compacting())
    }

    fn updated_compacting(&self) -> Option<ChatEvent> {
        let expected = self.presented_invocation.lock().unwrap().clone();
        let (invocation, active) = {
            let state = self.compacting.lock().unwrap();
            (state.0.clone(), state.1)
        };
        if expected.is_none() || invocation != expected {
            return None;
        }
        if self.presented_compacting.swap(active, Ordering::AcqRel) == active {
            return None;
        }
        invocation.map(|invocation| ChatEvent::Compacting { invocation, active })
    }

    fn updated_preview(&self) -> Option<ChatEvent> {
        let expected = self.presented_invocation.lock().unwrap().clone();
        let preview = self.preview.try_lock().ok()?;
        if expected.is_none() || preview.invocation != expected {
            return None;
        }
        if self.preview_revision.load(Ordering::Acquire) == preview.revision {
            return None;
        }
        self.preview_revision
            .store(preview.revision, Ordering::Release);
        if preview.segments.is_empty() && preview.warning.is_none() {
            return None;
        }
        preview
            .invocation
            .as_ref()
            .map(|invocation| ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.frame.clone(),
            })
    }
    pub fn close(mut self) -> Result<(), ChatRunError> {
        self.join_worker()
    }
    fn join_worker(&mut self) -> Result<(), ChatRunError> {
        self.closed.store(true, Ordering::Release);
        self.model_selection_cancel.cancel();
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(result) => result?,
                Err(_) => return WorkerPanicSnafu.fail(),
            }
        }
        Ok(())
    }
}
impl Drop for ChatController {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

enum RetainedResponse {
    Ready(UncommittedCompletion),
    Exited(LocalOutcome),
}
struct Retained {
    invocation: Invocation,
    response: RetainedResponse,
}
struct Controller {
    launch: Arc<ChatLaunch>,
    model_selection: ModelSelection,
    database: Arc<ChatDatabase>,
    input: Receiver<ChatCommand>,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    retained: Option<Retained>,
    preview: Arc<Mutex<Capture>>,
    compacting: Arc<Mutex<(Option<Invocation>, bool)>>,
}

impl Controller {
    fn emit(&self, event: ChatEvent) {
        emit(&self.output, &self.closed, event);
    }

    fn discover_default_model(&mut self) {
        let update = self.model_selection.discover_default_model();
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.emit(ChatEvent::ModelSelected {
            model: update.model,
            context_window: update.context_window,
            automatic: true,
            warning: update.warning,
        });
    }
}

fn emit(output: &SyncSender<ChatEvent>, closed: &AtomicBool, mut event: ChatEvent) {
    while !closed.load(Ordering::Acquire) {
        match output.try_send(event) {
            Ok(()) => return,
            Err(TrySendError::Full(pending)) => event = pending,
            Err(TrySendError::Disconnected(_)) => return,
        }
        thread::sleep(Duration::from_millis(10));
    }
}

impl Controller {
    fn run(mut self) -> Result<(), ChatRunError> {
        if self.model_selection.needs_default_model() && !self.closed.load(Ordering::Acquire) {
            self.discover_default_model();
        } else if !self.closed.load(Ordering::Acquire)
            && let Some(model) = self.model_selection.display_model()
            && let Some(window) = self.model_selection.probe_context_window(&model)
            && !self.closed.load(Ordering::Acquire)
        {
            self.emit(ChatEvent::ContextWindow {
                model,
                context_window: Some(window),
            });
        }
        while !self.closed.load(Ordering::Acquire) {
            match self.input.recv_timeout(Duration::from_millis(50)) {
                Ok(command) => {
                    if self.closed.load(Ordering::Acquire) {
                        break;
                    }
                    let admission = matches!(
                        command,
                        ChatCommand::Submit { .. } | ChatCommand::Retry { .. }
                    );
                    if let Err(source) = self.command(command) {
                        if self.closed.load(Ordering::Acquire) {
                            if matches!(source, ChatRunError::Cancelled) {
                                break;
                            }
                            return Err(source);
                        }
                        if admission {
                            self.busy.store(self.retained.is_some(), Ordering::Release);
                        }
                        self.emit(ChatEvent::Error { admission, source });
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if self.retained.is_some() {
            self.recover(true)?;
        }
        Ok(())
    }
    fn command(&mut self, command: ChatCommand) -> Result<(), ChatRunError> {
        match command {
            ChatCommand::ListSessions { after } => {
                self.emit(ChatEvent::Sessions(
                    self.database.list_sessions(false, PAGE_SIZE, after)?,
                ));
            }
            ChatCommand::Load { session_id, before } => {
                let session = self.database.session(&session_id)?;
                let end = before.unwrap_or(session.next_sequence);
                let after = (end > PAGE_SIZE as i64).then_some(end - PAGE_SIZE as i64 - 1);
                let mut page = self.database.display_page(&session_id, PAGE_SIZE, after)?;
                page.messages.retain(|record| record.sequence < end);
                let context_tokens = self.database.context_tokens(&session.id)?;
                self.emit(ChatEvent::Loaded {
                    session,
                    page,
                    context_tokens,
                });
            }
            ChatCommand::ListModels => {
                let result = self
                    .model_selection
                    .list_models()
                    .map_err(|error| error.to_string());
                if !self.closed.load(Ordering::Acquire) {
                    self.emit(ChatEvent::Models(result));
                }
            }
            ChatCommand::SelectModel { model } => {
                ensure!(self.retained.is_none(), BusySnafu);
                let needs_probe = self.model_selection.cached_context_window(&model).is_none();
                let update = self.model_selection.select_model(&model)?;
                self.emit(ChatEvent::ModelSelected {
                    model: update.model,
                    context_window: update.context_window,
                    automatic: false,
                    warning: update.warning,
                });
                if needs_probe
                    && let Some(window) = self.model_selection.probe_context_window(&model)
                    && !self.closed.load(Ordering::Acquire)
                {
                    self.emit(ChatEvent::ContextWindow {
                        model,
                        context_window: Some(window),
                    });
                }
            }
            ChatCommand::Submit { session_id, prompt } => {
                ensure!(
                    !prompt.trim().is_empty(),
                    OptionsSnafu {
                        reason: "prompt must not be empty"
                    }
                );
                ensure!(self.retained.is_none(), BusySnafu);
                let session = if let Some(id) = session_id {
                    self.database.session(&id)?
                } else {
                    let title: String = prompt
                        .trim()
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(48)
                        .collect();
                    self.database.create_provisional_session(
                        &self.launch.options.profile,
                        None,
                        title,
                    )?
                };
                self.submit(session, prompt, None)?;
            }
            ChatCommand::Retry { turn_id } => {
                ensure!(self.retained.is_none(), BusySnafu);
                let (turn, prompt) = self.database.retry_prompt(&turn_id)?;
                self.submit(
                    self.database.session(&turn.session_id)?,
                    prompt,
                    Some(turn_id),
                )?;
            }
            ChatCommand::Save => self.recover(false)?,
            ChatCommand::Abandon => self.recover(true)?,
        }
        Ok(())
    }
    fn submit(
        &mut self,
        session: Session,
        prompt: String,
        retry_of: Option<String>,
    ) -> Result<(), ChatRunError> {
        let parameters = mf_runtime::resolve_workflow_inputs(
            &self.launch.contract.parameters,
            self.model_selection.effective_parameters(),
            |name| std::env::var_os(name),
        )?;
        let invocation = Invocation::new(&session.id);
        if self.launch.contract.has_previews() {
            self.preview
                .lock()
                .unwrap()
                .begin(&self.database, invocation.clone());
        }
        let captured = Arc::new(Mutex::new(None));
        let cancellation = mf_runtime::StreamCancellation::default();
        let backend = ExternalBackend {
            cancellation: cancellation.clone(),
            launch: Arc::clone(&self.launch),
            parameters,
            session: session.clone(),
            output: self.output.clone(),
            closed: Arc::clone(&self.closed),
            cancel: Arc::clone(&self.cancel),
            preview: Arc::clone(&self.preview),
            captured: Arc::clone(&captured),
        };
        let request = mf_chat::nodes::BeginTurnInputs {
            database_path: self
                .database
                .path()
                .to_str()
                .context(OptionsSnafu {
                    reason: "database path must be UTF-8",
                })?
                .into(),
            session_id: session.id.clone(),
            turn_id: invocation.turn_id.clone(),
            call_id: invocation.call_id.clone(),
            prompt,
            model: mf_chat::ModelIdentity {
                profile: session.profile,
                model: None,
            },
            retry_of,
            result_alternatives: Some(self.launch.contract.alternatives),
        };
        let preview_consumer = self.launch.contract.has_previews().then(|| {
            let capture = self.preview.clone();
            let seal = self.preview.clone();
            let outer = cancellation.clone();
            let closed = self.closed.clone();
            let cancel = self.cancel.clone();
            mf_chat::PreviewConsumer::new(move |event, cancellation| {
                if cancel.load(Ordering::Acquire) || closed.load(Ordering::Acquire) {
                    outer.cancel(
                        mf_runtime::StreamExecutionSnafu {
                            message: "chat request cancelled",
                        }
                        .build(),
                    );
                }
                if let Some(error) = outer.failure().or_else(|| cancellation.failure()) {
                    return Err(error.into());
                }
                capture.lock().unwrap().deliver(event)
            })
            .with_draft_seal(move || seal.lock().unwrap().seal())
        });
        *self.compacting.lock().unwrap() = (Some(invocation.clone()), false);
        let host_calls = Mutex::new(mf_chat::CallEventCollector::default());
        let compacting = Arc::clone(&self.compacting);
        let compact_invocation = invocation.clone();
        let _compaction_events = mf_telemetry::events::collect(move |event| {
            let mut calls = host_calls.lock().unwrap();
            if calls.observe(event).is_ok() {
                let active = calls.has_active_purpose("compaction");
                let mut state = compacting.lock().unwrap();
                if state.0.as_ref() == Some(&compact_invocation) {
                    state.1 = active;
                }
            }
        });
        let resolution = mf_chat::workflow::run_conversation_with_context_windows(
            &self.database,
            request,
            Arc::new(backend),
            cancellation,
            true,
            None,
            preview_consumer,
            self.model_selection.context_windows(),
        );
        // A captured result may need save-only recovery without visiting the finish node.
        if matches!(
            &resolution,
            Err(_)
                | Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(_),
                    ..
                })
        ) {
            self.preview.lock().unwrap().seal();
        }
        *self.compacting.lock().unwrap() = (None, false);
        let outcome = captured
            .lock()
            .expect("capture state was not poisoned")
            .take();
        if outcome.is_none() {
            let mut resolution = resolution?;
            ensure!(
                !self.cancel.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire),
                CancelledSnafu
            );
            return Err(resolution.diagnostics.remove(0).into());
        }
        let status = outcome.as_ref().map(|outcome| outcome.status);
        let stderr = outcome
            .as_ref()
            .map(|outcome| outcome.stderr.clone())
            .unwrap_or_default();
        let capture_error = outcome
            .as_ref()
            .and_then(|outcome| outcome.capture_error.clone());
        self.retained = match &resolution {
            Ok(ReconciledCompletion {
                state: CompletionState::Uncommitted(pending),
                ..
            }) => Some(Retained {
                invocation: invocation.clone(),
                response: RetainedResponse::Ready(pending.clone()),
            }),
            Err(_) => Some(Retained {
                invocation: invocation.clone(),
                response: RetainedResponse::Exited(
                    outcome.expect("failed reconciliation retains the exit"),
                ),
            }),
            _ => None,
        };
        self.busy.store(self.retained.is_some(), Ordering::Release);
        self.emit(ChatEvent::Finished {
            invocation,
            resolution,
            status,
            stderr,
            capture_error,
        });
        Ok(())
    }
    fn recover(&mut self, abandon: bool) -> Result<(), ChatRunError> {
        let retained = self.retained.as_ref().context(MissingReplySnafu)?;
        let invocation = retained.invocation.clone();
        let mut resolution = match &retained.response {
            RetainedResponse::Ready(pending) => ReconciledCompletion {
                state: CompletionState::Uncommitted(pending.clone()),
                diagnostics: vec![],
            },
            RetainedResponse::Exited(outcome) => {
                reconcile_turn(&self.database, &invocation, outcome)?
            }
        };
        if let CompletionState::Uncommitted(pending) = resolution.state {
            self.retained.as_mut().unwrap().response = RetainedResponse::Ready(pending.clone());
            resolution.state = if abandon {
                self.database.abandon_recovered(&pending)?;
                self.database
                    .reconcile_completion(ConversationExit {
                        invocation: &invocation,
                        stdout: None,
                        outcome: TurnExit::Cancelled,
                        summary: "reply discarded",
                    })?
                    .state
            } else {
                CompletionState::Committed(self.database.save_recovered(pending)?)
            };
        }
        self.retained = None;
        self.busy.store(false, Ordering::Release);
        self.emit(ChatEvent::Finished {
            invocation,
            resolution: Ok(resolution),
            status: None,
            stderr: String::new(),
            capture_error: None,
        });
        Ok(())
    }
}

struct ExternalBackend {
    cancellation: mf_runtime::StreamCancellation,
    launch: Arc<ChatLaunch>,
    parameters: mf_runtime::WorkflowArguments,
    session: Session,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    preview: Arc<Mutex<Capture>>,
    captured: Arc<Mutex<Option<LocalOutcome>>>,
}

impl mf_chat::ConversationBackend for ExternalBackend {
    fn registered_workflow(
        &self,
    ) -> Option<(
        &mf_runtime::WorkflowManifest,
        &mf_runtime::WorkflowArguments,
    )> {
        Some((self.launch.runner.manifest(), &self.parameters))
    }

    fn builtin_title_parameters(&self) -> Option<mf_runtime::WorkflowArguments> {
        self.launch
            .contract
            .builtin_title_parameters(&self.parameters)
    }

    fn builtin_compaction_parameters(&self) -> Option<mf_runtime::WorkflowArguments> {
        self.launch
            .contract
            .builtin_compaction_parameters(&self.parameters)
    }

    fn invoke(
        &self,
        invocation: &Invocation,
        request: mf_chat::ConversationInputs,
        context: &mut mf_runtime::ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), mf_chat::ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<mf_chat::ConversationResult, mf_chat::ChatError> {
        emit(
            &self.output,
            &self.closed,
            ChatEvent::Submitted {
                session: self.session.clone(),
                invocation: invocation.clone(),
                prompt: request.prompt()?.to_owned(),
            },
        );
        let execution = self.execute(request, context, preview);
        let failure = match execution.as_ref().err() {
            Some(ChatRunError::Runner {
                source: RunnerError::Cancelled { .. },
            }) => TurnExit::Cancelled,
            Some(ChatRunError::Runner {
                source:
                    RunnerError::Run {
                        source: RunError::Supervise { .. },
                    },
            }) => TurnExit::Interrupted,
            _ => TurnExit::Failed,
        };
        let (outcome, protocol_failure) = execution.boxed().context(mf_chat::BackendSnafu {
            outcome: failure,
            reply: None,
        })?;
        let reason = if protocol_failure.is_some()
            && !self.cancel.load(Ordering::Acquire)
            && !self.closed.load(Ordering::Acquire)
        {
            TurnExit::Failed
        } else {
            exit_reason(&outcome)
        };
        let result = if let Some(failure) = protocol_failure {
            Err(Box::new(failure) as Box<dyn std::error::Error + Send + Sync>)
        } else if let Some(failure) = &outcome.capture_failure {
            Err(Box::new(failure.clone()) as Box<dyn std::error::Error + Send + Sync>)
        } else if !outcome.status.success() {
            mf_chat::CaptureSnafu {
                message: "workflow process exited unsuccessfully",
            }
            .fail()
            .boxed()
        } else {
            mf_chat::ConversationResult::from_json(&outcome.stdout).boxed()
        };
        let result = result.map(|mut response| {
            outcome.call_events.apply(&mut response.usages);
            response
        });
        let reply = if result.is_err() {
            mf_chat::ConversationResult::from_json(&outcome.stdout)
                .ok()
                .map(|mut response| {
                    outcome.call_events.apply(&mut response.usages);
                    Box::new(response)
                })
        } else {
            None
        };
        *self
            .captured
            .lock()
            .expect("capture state was not poisoned") = Some(outcome);
        result.context(mf_chat::BackendSnafu {
            outcome: reason,
            reply,
        })
    }
}

impl ExternalBackend {
    fn execute(
        &self,
        request: mf_chat::ConversationInputs,
        context: &mut mf_runtime::ExecutionContext,
        mut preview: Option<
            &mut dyn FnMut(mf_chat::PreviewDelta) -> Result<(), mf_chat::ChatError>,
        >,
    ) -> Result<(LocalOutcome, Option<mf_chat::ChatError>), ChatRunError> {
        let arguments = self.launch.contract.arguments(request, &self.parameters)?;
        let mut decoder = mf_chat::contract::ConversationOutputDecoder::new(&self.launch.contract);
        let completion = RunnerInvocation {
            runner: &self.launch.runner,
            arguments: &arguments,
            verbose: self.launch.options.run.verbose,
            snapshots: true,
        }
        .run_batches(
            context.cancellation(),
            |observation| {
                emit(
                    &self.output,
                    &self.closed,
                    ChatEvent::Observing(observation),
                )
            },
            |records| {
                // Stage only validated output; the view publishes a handle only on its
                // corresponding application Flow receipt, which may arrive after settlement.
                let mut batch = Vec::new();
                let mut forward =
                    |batch: &mut Vec<mf_chat::PreviewDelta>| -> Result<(), mf_chat::ChatError> {
                        if batch.is_empty() {
                            return Ok(());
                        }
                        if let Some(consume) = preview.as_mut() {
                            self.preview.lock().unwrap().stage(batch);
                            for delta in batch.drain(..) {
                                consume(delta)?;
                            }
                        } else {
                            batch.clear();
                        }
                        Ok(())
                    };
                for record in records {
                    match decoder.accept(record) {
                        Ok(Some(delta)) => batch.push(delta),
                        Ok(None) => continue,
                        Err(error) => {
                            forward(&mut batch)?;
                            return Err(error);
                        }
                    }
                    if batch.len() == 32 {
                        forward(&mut batch)?;
                    }
                }
                forward(&mut batch)
            },
            || {
                let cancelled =
                    self.cancel.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire);
                if cancelled {
                    self.cancellation.cancel(
                        mf_runtime::StreamExecutionSnafu {
                            message: "chat request cancelled",
                        }
                        .build(),
                    );
                }
                cancelled
            },
        )?;
        let mut outcome = completion.outcome;
        let mut protocol_failure = completion.protocol_failure;
        if self.launch.contract.streaming {
            outcome.stdout = decoder
                .result()
                .map(serde_json::to_vec)
                .transpose()?
                .unwrap_or_default();
            if protocol_failure.is_none()
                && outcome.capture_error.is_none()
                && let Err(error) = decoder.finish()
            {
                protocol_failure = Some(error);
            }
        }
        Ok((outcome, protocol_failure))
    }
}

fn exit_reason(outcome: &LocalOutcome) -> TurnExit {
    if outcome.interrupted {
        TurnExit::Cancelled
    } else if outcome.status.code().is_none() {
        TurnExit::Interrupted
    } else {
        TurnExit::Failed
    }
}

fn reconcile_turn(
    database: &ChatDatabase,
    invocation: &Invocation,
    outcome: &LocalOutcome,
) -> Result<ReconciledCompletion, mf_chat::ChatError> {
    let reason = exit_reason(outcome);
    let stdout = outcome
        .capture_error
        .is_none()
        .then_some(outcome.stdout.as_slice())
        .or_else(|| {
            mf_chat::ConversationResult::from_json(&outcome.stdout)
                .is_ok()
                .then_some(outcome.stdout.as_slice())
        });
    let result = stdout.map(|bytes| {
        mf_chat::ConversationResult::from_json(bytes).map(|mut response| {
            outcome.call_events.apply(&mut response.usages);
            response
        })
    });
    database.reconcile_result(
        invocation,
        result,
        reason,
        if outcome.stderr.is_empty() {
            "runner exited without completion"
        } else {
            &outcome.stderr
        },
    )
}

#[cfg(test)]
mod tests;
