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
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

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
    #[snafu(display("{source}; invocation cleanup failed: {cleanup}"))]
    Cleanup {
        source: Box<ChatRunError>,
        cleanup: Box<mf_chat::ChatError>,
    },
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
        let controller = Controller {
            launch,
            database: Arc::clone(&database),
            input,
            output,
            closed: Arc::clone(&closed),
            busy: Arc::clone(&busy),
            cancel: Arc::clone(&cancel),
            retained: None,
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
        self.events.try_recv().ok()
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
    launch: ChatLaunch,
    database: Arc<ChatDatabase>,
    input: Receiver<ChatCommand>,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    retained: Option<Retained>,
}
impl Controller {
    fn emit(&self, mut event: ChatEvent) {
        while !self.closed.load(Ordering::Acquire) {
            match self.output.try_send(event) {
                Ok(()) => return,
                Err(TrySendError::Full(pending)) => event = pending,
                Err(TrySendError::Disconnected(_)) => return,
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
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
        let result = self.execute_turn(&session, &invocation, &prompt, retry_of);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(source) => {
                let outcome = match &source {
                    ChatRunError::Cancelled => TurnExit::Cancelled,
                    ChatRunError::Observation {
                        source: RunError::Supervise { .. },
                    } => TurnExit::Interrupted,
                    _ => TurnExit::Failed,
                };
                return match self.database.finalize_invocation(
                    invocation,
                    outcome,
                    &source.to_string(),
                ) {
                    Ok(_) => Err(source),
                    Err(cleanup) => Err(CleanupSnafu {
                        cleanup: Box::new(cleanup),
                    }
                    .into_error(Box::new(source))),
                };
            }
        };
        let mut resolution = reconcile_turn(&self.database, &invocation, &outcome);
        if let Ok(resolution) = &mut resolution
            && let CompletionState::Uncommitted(pending) = &resolution.state
        {
            match self.database.complete_turn(
                pending.completion.invocation.clone(),
                pending.completion.result.clone(),
            ) {
                Ok(completion) => resolution.state = CompletionState::Committed(completion),
                Err(source) => resolution.diagnostics.push(source),
            }
        }
        let status = outcome.status;
        let stderr = outcome.stderr.clone();
        let capture_error = outcome.capture_error.clone();
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
                response: RetainedResponse::Exited(outcome),
            }),
            _ => None,
        };
        self.busy.store(self.retained.is_some(), Ordering::Release);
        self.emit(ChatEvent::Finished {
            invocation,
            resolution,
            status: Some(status),
            stderr,
            capture_error,
        });
        Ok(())
    }
    fn execute_turn(
        &self,
        session: &Session,
        invocation: &Invocation,
        prompt: &str,
        retry_of: Option<String>,
    ) -> Result<LocalOutcome, ChatRunError> {
        let started = self.database.begin_turn_with_invocation(
            invocation.clone(),
            prompt,
            mf_chat::ModelIdentity {
                profile: session.profile.clone(),
                model: None,
            },
            retry_of,
            mf_chat::DEFAULT_HISTORY_BYTES,
        )?;
        self.emit(ChatEvent::Submitted {
            session: session.clone(),
            invocation: invocation.clone(),
            prompt: prompt.into(),
        });
        let arguments = self
            .launch
            .contract
            .arguments(started.request, &self.launch.parameters)?;
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
        let mut run = LocalRun::start(
            command,
            description,
            mf_telemetry::identity::RunId::new(),
            mf_chat::contract::MAX_RECOVERY_BYTES as u64,
        )?;
        self.emit(ChatEvent::Observing(run.observation()));
        loop {
            if self.cancel.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
                run.cancel();
            }
            if let Some(outcome) = run.poll()? {
                return Ok(outcome);
            }
            thread::sleep(Duration::from_millis(10));
        }
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

fn reconcile_turn(
    database: &ChatDatabase,
    invocation: &Invocation,
    outcome: &LocalOutcome,
) -> Result<ReconciledCompletion, mf_chat::ChatError> {
    let reason = if outcome.interrupted {
        TurnExit::Cancelled
    } else if outcome.status.code().is_none() {
        TurnExit::Interrupted
    } else {
        TurnExit::Failed
    };
    database.reconcile_completion(ConversationExit {
        invocation,
        stdout: outcome
            .capture_error
            .is_none()
            .then_some(outcome.stdout.as_slice()),
        outcome: reason,
        summary: if outcome.stderr.is_empty() {
            "runner exited without completion"
        } else {
            &outcome.stderr
        },
    })
}

#[cfg(test)]
mod tests;
