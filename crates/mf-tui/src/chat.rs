//! A single conversation execution lane over a supplied workflow executable.

use crate::{
    manifest::{ManifestReadError, read_manifest},
    run::{LocalObservation, LocalOutcome, LocalRun, RunError, RunOptions},
};
use mf_chat::contract::{
    ChatContract, CompletionState, ConversationExit, ReconciledCompletion, TurnExit,
};
use mf_chat::{ChatDatabase, Invocation, MessagePage, Session, SessionPage, UncommittedCompletion};
use mf_runtime::WorkflowManifest;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod preview;
pub use preview::LivePreview;
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
    Manifest { source: ManifestReadError },
    #[snafu(transparent)]
    Inputs {
        source: mf_runtime::WorkflowInputError,
    },
    #[snafu(transparent)]
    Observation { source: RunError },
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
    pub executable: PathBuf,
    pub manifest: WorkflowManifest,
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
        let manifest = read_manifest(executable)?;
        let contract = ChatContract::from_manifest(&manifest)?;
        let supplied = options.run.workflow_arguments(&contract.parameters)?;
        let parameters =
            mf_runtime::resolve_workflow_inputs(&contract.parameters, &supplied, |name| {
                std::env::var_os(name)
            })?;
        Ok(Self {
            executable: executable.to_owned(),
            manifest,
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
        page: MessagePage,
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
    preview: Arc<Mutex<LivePreview>>,
    preview_revision: AtomicU64,
    presented_invocation: Mutex<Option<Invocation>>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), ChatRunError>>>,
}
impl ChatController {
    pub fn open(launch: ChatLaunch) -> Result<Self, ChatRunError> {
        let database = Arc::new(ChatDatabase::open(&launch.options.database_path)?);
        let (commands, input) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (output, events) = mpsc::sync_channel(QUEUE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let preview = Arc::new(Mutex::new(LivePreview::default()));
        let controller = Controller {
            launch: Arc::new(launch),
            database: Arc::clone(&database),
            input,
            output,
            closed: Arc::clone(&closed),
            busy: Arc::clone(&busy),
            cancel: Arc::clone(&cancel),
            retained: None,
            preview: preview.clone(),
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
            presented_invocation: Mutex::new(None),
            closed,
            busy,
            cancel,
            worker: Some(worker),
        })
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
        }
        if matches!(event, Some(ChatEvent::Finished { .. }))
            && let Some(preview) = self.updated_preview()
        {
            *self.deferred.lock().unwrap() = event;
            return Some(preview);
        }
        if matches!(event, Some(ChatEvent::Finished { .. })) {
            *self.presented_invocation.lock().unwrap() = None;
        }
        event.or_else(|| self.updated_preview())
    }

    fn updated_preview(&self) -> Option<ChatEvent> {
        let expected = self.presented_invocation.lock().unwrap().clone();
        let preview = self.preview.lock().unwrap();
        if expected.is_none() || preview.invocation != expected {
            return None;
        }
        if self.preview_revision.load(Ordering::Acquire) == preview.revision {
            return None;
        }
        self.preview_revision
            .store(preview.revision, Ordering::Release);
        if preview.segments.is_empty() && !preview.omitted {
            return None;
        }
        preview
            .invocation
            .as_ref()
            .map(|invocation| ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.clone(),
            })
    }
    pub fn close(mut self) -> Result<(), ChatRunError> {
        self.join_worker()
    }
    fn join_worker(&mut self) -> Result<(), ChatRunError> {
        self.closed.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| WorkerPanicSnafu.build())??;
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
    database: Arc<ChatDatabase>,
    input: Receiver<ChatCommand>,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    retained: Option<Retained>,
    preview: Arc<Mutex<LivePreview>>,
}
impl Controller {
    fn emit(&self, event: ChatEvent) {
        emit(&self.output, &self.closed, event);
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
                let mut page = self.database.messages(&session_id, PAGE_SIZE, after)?;
                page.messages.retain(|record| record.message.sequence < end);
                self.emit(ChatEvent::Loaded { session, page });
            }
            ChatCommand::Submit { session_id, prompt } => {
                ensure!(
                    !prompt.trim().is_empty(),
                    OptionsSnafu {
                        reason: "prompt must not be empty"
                    }
                );
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
        ensure!(self.retained.is_none(), BusySnafu);
        let invocation = Invocation::new(&session.id);
        if self.launch.contract.answer_stream.is_some() {
            self.preview.lock().unwrap().begin(invocation.clone());
        }
        let captured = Arc::new(Mutex::new(None));
        let cancellation = mf_runtime::StreamCancellation::default();
        let backend = ExternalBackend {
            cancellation: cancellation.clone(),
            launch: Arc::clone(&self.launch),
            session: session.clone(),
            output: self.output.clone(),
            closed: Arc::clone(&self.closed),
            cancel: Arc::clone(&self.cancel),
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
            max_history_bytes: mf_chat::DEFAULT_HISTORY_BYTES,
        };
        let preview_consumer = self.launch.contract.answer_stream.as_ref().map(|_| {
            let outer_cancellation = cancellation.clone();
            let preview = self.preview.clone();
            let closed = self.closed.clone();
            let cancel = self.cancel.clone();
            let invocation = invocation.clone();
            mf_chat::PreviewConsumer::new(move |event, cancellation| {
                if let Some(error) = cancellation.failure() {
                    return Err(error.into());
                }
                if cancel.load(Ordering::Acquire) || closed.load(Ordering::Acquire) {
                    outer_cancellation.cancel_with_message("chat request cancelled");
                    return Err(outer_cancellation
                        .failure()
                        .expect("cancellation was recorded")
                        .into());
                }
                // Consume and compact independently of terminal refresh or inspection.
                preview.lock().unwrap().apply(&invocation, event)
            })
        });
        let resolution = mf_chat::workflow::run_conversation_with_preview(
            &self.database,
            request,
            Arc::new(backend),
            cancellation,
            true,
            None,
            preview_consumer,
        );
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
    session: Session,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    captured: Arc<Mutex<Option<LocalOutcome>>>,
}

impl mf_chat::ConversationBackend for ExternalBackend {
    fn invoke(
        &self,
        invocation: &Invocation,
        request: mf_chat::ConversationInputs,
        context: &mut mf_runtime::ExecutionContext,
        preview: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), mf_chat::ChatError>>,
        _: Option<&mf_telemetry::observation::Observer>,
    ) -> Result<mf_chat::ConversationResult, mf_chat::ChatError> {
        emit(
            &self.output,
            &self.closed,
            ChatEvent::Submitted {
                session: self.session.clone(),
                invocation: invocation.clone(),
                prompt: request.prompt.clone(),
            },
        );
        let execution = self.execute(request, context, preview);
        let failure = match execution.as_ref().err() {
            Some(ChatRunError::Cancelled) => TurnExit::Cancelled,
            Some(ChatRunError::Observation {
                source: RunError::Supervise { .. },
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
        } else if let Some(message) = &outcome.capture_error {
            mf_chat::CaptureSnafu { message }.fail().boxed()
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
        mut preview: Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), mf_chat::ChatError>>,
    ) -> Result<(LocalOutcome, Option<mf_chat::ChatError>), ChatRunError> {
        let arguments = self
            .launch
            .contract
            .arguments(request, &self.launch.parameters)?;
        let mut file = tempfile::NamedTempFile::new().context(IoSnafu {
            operation: "create private workflow arguments",
        })?;
        serde_json::to_writer(&mut file, &arguments)?;
        let size = file
            .as_file()
            .metadata()
            .context(IoSnafu {
                operation: "inspect private workflow arguments",
            })?
            .len();
        ensure!(
            size <= mf_runtime::MAX_WORKFLOW_INPUT_BYTES as u64,
            mf_runtime::WorkflowInputTooLargeSnafu {
                limit: mf_runtime::MAX_WORKFLOW_INPUT_BYTES
            }
        );
        ensure!(
            !self.cancel.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire),
            CancelledSnafu
        );
        let mut command = Command::new(&self.launch.executable);
        command.arg("--inputs-file").arg(file.path());
        for _ in 0..self.launch.options.run.verbose.min(3) {
            command.arg("--verbose");
        }
        let description = self
            .launch
            .manifest
            .description()
            .map_err(mf_chat::ChatError::from)?;
        let run_id = mf_telemetry::identity::RunId::new();
        let mut decoder = mf_chat::contract::ConversationOutputDecoder::new(&self.launch.contract);
        let mut run = if self.launch.contract.streaming {
            LocalRun::start_stream(
                command,
                description,
                run_id,
                crate::run::StreamCaptureOptions {
                    cancellation: context.cancellation(),
                },
            )?
        } else {
            LocalRun::start(
                command,
                description,
                run_id,
                mf_chat::contract::MAX_RECOVERY_BYTES as u64,
            )?
        };
        emit(
            &self.output,
            &self.closed,
            ChatEvent::Observing(run.observation()),
        );
        let mut protocol_failure = None;
        loop {
            if self.cancel.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
                self.cancellation
                    .cancel_with_message("chat request cancelled");
                run.cancel();
            }
            if context.cancellation().failure().is_some() {
                run.cancel();
            }
            if protocol_failure.is_none() {
                if let Err(error) = Self::consume_records(&run, &mut decoder, &mut preview) {
                    protocol_failure = Some(error);
                    run.cancel();
                }
            } else {
                while run.try_record().is_ok() {}
            }
            if let Some(mut outcome) = run.poll()? {
                if protocol_failure.is_none()
                    && let Err(error) = Self::consume_records(&run, &mut decoder, &mut preview)
                {
                    protocol_failure = Some(error);
                }
                if self.launch.contract.streaming {
                    if let Some(result) = decoder.result() {
                        outcome.stdout = serde_json::to_vec(result)?;
                    }
                    if protocol_failure.is_none()
                        && outcome.capture_error.is_none()
                        && let Err(error) = decoder.finish()
                    {
                        protocol_failure = Some(error);
                    }
                }
                return Ok((outcome, protocol_failure));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn consume_records(
        run: &LocalRun,
        decoder: &mut mf_chat::contract::ConversationOutputDecoder,
        preview: &mut Option<&mut dyn FnMut(mf_chat::TextDelta) -> Result<(), mf_chat::ChatError>>,
    ) -> Result<(), mf_chat::ChatError> {
        while let Ok(record) = run.try_record() {
            if let Some(delta) = decoder.accept(record)?
                && let Some(consume) = preview.as_mut()
            {
                consume(delta)?;
            }
        }
        Ok(())
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
