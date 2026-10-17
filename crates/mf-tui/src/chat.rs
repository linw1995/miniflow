//! Interactive chat orchestration over one supplied workflow executable.

use crate::{
    manifest::{ManifestReadError, read_manifest},
    run::{LocalObservation, LocalOutcome, LocalRun, RunError, RunOptions},
};
use mf_chat::executable::{
    ChatContract, CompletionState, ConversationExit, ReconciledCompletion, TurnExit,
};
use mf_chat::{
    ChatDatabase, Invocation, MessagePage, Session, SessionPage, UncommittedCompletion, UsageQuery,
    UsageStatistics,
};
use mf_runtime::WorkflowManifest;
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

mod ui;
pub use ui::run_chat;

pub const MAX_ACTIVE_CHAT_RUNS: usize = 4;
pub const MAX_RETAINED_CHAT_RUNS: usize = 64;
const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 128;

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
    #[snafu(display("chat command queue is full"))]
    QueueFull,
    #[snafu(display("chat controller is closed"))]
    Closed,
    #[snafu(display("chat shutdown failed: {errors:?}"))]
    Shutdown { errors: Vec<ChatRunError> },
    #[snafu(display("chat controller worker panicked"))]
    WorkerPanic,
    #[snafu(display("{source}; invocation cleanup failed: {cleanup}"))]
    Cleanup {
        source: Box<ChatRunError>,
        cleanup: Box<mf_chat::ChatError>,
    },
    #[snafu(display("chat session {session_id:?} already has an unresolved turn"))]
    SessionBusy { session_id: String },
    #[snafu(display("chat reached its {limit}-invocation capacity"))]
    Capacity { limit: usize },
    #[snafu(display("chat call {call_id:?} has no retained reply"))]
    MissingReply { call_id: String },
    #[snafu(display("invalid chat options: {reason}"))]
    Options { reason: &'static str },
}

#[derive(Clone, Debug)]
pub struct ChatOptions {
    pub database_path: PathBuf,
    pub profile: String,
    pub max_active: usize,
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
            options.max_active > 0
                && options.max_active <= MAX_ACTIVE_CHAT_RUNS
                && !options.profile.trim().is_empty(),
            OptionsSnafu {
                reason: "requires a nonblank profile and one to four active invocations"
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
        archived: bool,
        after: Option<String>,
    },
    Load {
        session_id: String,
        after: Option<i64>,
    },
    Create {
        system: Option<String>,
        title: Option<String>,
    },
    Rename {
        session_id: String,
        title: String,
    },
    Archive {
        session_id: String,
        archived: bool,
    },
    Submit {
        session_id: String,
        prompt: String,
    },
    Retry {
        session_id: String,
        turn_id: String,
    },
    Save {
        call_id: String,
    },
    Abandon {
        call_id: String,
    },
    Usage {
        session_id: Option<String>,
    },
}

pub enum ChatEvent {
    Sessions {
        archived: bool,
        page: SessionPage,
    },
    Loaded {
        session: Session,
        page: MessagePage,
        after: Option<i64>,
    },
    Created(Session),
    Changed(Session),
    Submitted {
        prompt: String,
        invocation: Invocation,
        cancel: Arc<AtomicBool>,
    },
    Observing {
        invocation: Invocation,
        observation: LocalObservation,
    },
    Finished {
        invocation: Invocation,
        resolution: ReconciledCompletion,
        status: Option<ExitStatus>,
        stderr: String,
        capture_error: Option<String>,
    },
    Usage {
        session_id: Option<String>,
        statistics: UsageStatistics,
    },
    RecoveryBlocked {
        invocation: Invocation,
        status: ExitStatus,
        stderr: String,
        capture_error: Option<String>,
        source: mf_chat::ChatError,
    },
    Failed {
        invocation: Invocation,
        source: ChatRunError,
    },
    Error {
        admission: bool,
        session_id: Option<String>,
        source: ChatRunError,
    },
}

pub struct ChatController {
    _database: Arc<ChatDatabase>,
    commands: SyncSender<ChatCommand>,
    events: Receiver<ChatEvent>,
    closed: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<ChatRunError>>>,
}
impl ChatController {
    pub fn open(launch: ChatLaunch) -> Result<Self, ChatRunError> {
        let database = Arc::new(ChatDatabase::open(&launch.options.database_path)?);
        let worker_database = Arc::clone(&database);
        let (commands, input) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (output, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let shared_closed = Arc::clone(&closed);
        let worker = thread::Builder::new()
            .name("mf-chat-controller".into())
            .spawn(move || {
                Controller::new(
                    Arc::new(launch),
                    worker_database,
                    input,
                    output,
                    shared_closed,
                )
                .run()
            })
            .context(IoSnafu {
                operation: "start chat controller",
            })?;
        Ok(Self {
            _database: database,
            commands,
            events,
            closed,
            worker: Some(worker),
        })
    }
    pub fn send(&self, command: ChatCommand) -> Result<(), ChatRunError> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => QueueFullSnafu.fail(),
            Err(TrySendError::Disconnected(_)) => ClosedSnafu.fail(),
        }
    }
    pub fn try_event(&self) -> Option<ChatEvent> {
        self.events.try_recv().ok()
    }
    pub fn close(mut self) -> Result<(), ChatRunError> {
        self.join_workers()
    }
    fn join_workers(&mut self) -> Result<(), ChatRunError> {
        self.closed.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let errors = worker.join().map_err(|_| WorkerPanicSnafu.build())?;
            ensure!(errors.is_empty(), ShutdownSnafu { errors });
        }
        Ok(())
    }
}
impl Drop for ChatController {
    fn drop(&mut self) {
        let _ = self.join_workers();
    }
}

struct Active {
    invocation: Invocation,
    cancel: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}
enum RetainedState {
    Ready(UncommittedCompletion),
    Exited(LocalOutcome),
}
struct Retained {
    invocation: Invocation,
    state: RetainedState,
}
enum WorkerEvent {
    Observing {
        invocation: Invocation,
        observation: LocalObservation,
    },
    Finished {
        invocation: Invocation,
        result: Box<Result<WorkerCompletion, ChatRunError>>,
    },
}
struct WorkerCompletion {
    resolution: Result<ReconciledCompletion, mf_chat::ChatError>,
    outcome: LocalOutcome,
}
struct Controller {
    launch: Arc<ChatLaunch>,
    database: Arc<ChatDatabase>,
    input: Receiver<ChatCommand>,
    output: SyncSender<ChatEvent>,
    closed: Arc<AtomicBool>,
    workers_in: Receiver<WorkerEvent>,
    workers_out: SyncSender<WorkerEvent>,
    active: BTreeMap<String, Active>,
    retained: BTreeMap<String, Retained>,
    outbox: VecDeque<ChatEvent>,
}
impl Controller {
    fn new(
        launch: Arc<ChatLaunch>,
        database: Arc<ChatDatabase>,
        input: Receiver<ChatCommand>,
        output: SyncSender<ChatEvent>,
        closed: Arc<AtomicBool>,
    ) -> Self {
        let (workers_out, workers_in) = mpsc::sync_channel(MAX_ACTIVE_CHAT_RUNS * 2);
        Self {
            launch,
            database,
            input,
            output,
            closed,
            workers_in,
            workers_out,
            active: BTreeMap::new(),
            retained: BTreeMap::new(),
            outbox: VecDeque::new(),
        }
    }
    fn run(mut self) -> Vec<ChatRunError> {
        let initial = self.database.list_sessions(false, 50, None);
        self.emit_result(
            None,
            false,
            initial
                .map(|page| ChatEvent::Sessions {
                    archived: false,
                    page,
                })
                .map_err(ChatRunError::from),
        );
        loop {
            while let Some(event) = self.outbox.pop_front() {
                match self.output.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(event)) => {
                        self.outbox.push_front(event);
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        self.closed.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            if self.closed.load(Ordering::Acquire) {
                break;
            }
            if self.outbox.len() < EVENT_CAPACITY - MAX_ACTIVE_CHAT_RUNS - 2 {
                while let Ok(event) = self.workers_in.try_recv() {
                    self.worker_event(event);
                }
                match self.input.try_recv() {
                    Err(TryRecvError::Disconnected) => break,
                    Ok(command) => self.command(command),
                    Err(TryRecvError::Empty) => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.shutdown()
    }
    fn emit_result(
        &mut self,
        session_id: Option<String>,
        admission: bool,
        result: Result<ChatEvent, ChatRunError>,
    ) {
        self.outbox.push_back(match result {
            Ok(event) => event,
            Err(source) => ChatEvent::Error {
                admission,
                session_id,
                source,
            },
        });
    }
    fn command(&mut self, command: ChatCommand) {
        let abandon = matches!(&command, ChatCommand::Abandon { .. });
        let admission = matches!(
            &command,
            ChatCommand::Submit { .. } | ChatCommand::Retry { .. }
        );
        let mut session_id = None;
        let result = (|| -> Result<ChatEvent, ChatRunError> {
            match command {
                ChatCommand::ListSessions { archived, after } => Ok(ChatEvent::Sessions {
                    archived,
                    page: self.database.list_sessions(archived, 50, after)?,
                }),
                ChatCommand::Load {
                    session_id: id,
                    after,
                } => {
                    session_id = Some(id.clone());
                    Ok(ChatEvent::Loaded {
                        session: self.database.session(&id)?,
                        page: self.database.messages(&id, 50, after)?,
                        after,
                    })
                }
                ChatCommand::Create { system, title } => Ok(ChatEvent::Created(
                    self.database
                        .create_session(&self.launch.options.profile, system, title)?,
                )),
                ChatCommand::Rename {
                    session_id: id,
                    title,
                } => {
                    session_id = Some(id.clone());
                    self.database.rename_session(&id, &title)?;
                    Ok(ChatEvent::Changed(self.database.session(&id)?))
                }
                ChatCommand::Archive {
                    session_id: id,
                    archived,
                } => {
                    session_id = Some(id.clone());
                    if archived {
                        self.database.archive_session(&id)?;
                    } else {
                        self.database.restore_session(&id)?;
                    }
                    Ok(ChatEvent::Changed(self.database.session(&id)?))
                }
                ChatCommand::Submit {
                    session_id: id,
                    prompt,
                } => {
                    session_id = Some(id.clone());
                    self.submit(&id, &prompt, None)
                }
                ChatCommand::Retry {
                    session_id: id,
                    turn_id,
                } => {
                    session_id = Some(id.clone());
                    let (turn, prompt) = self.database.retry_prompt(&turn_id)?;
                    ensure!(
                        turn.session_id == id,
                        OptionsSnafu {
                            reason: "retry must belong to the selected session"
                        }
                    );
                    self.submit(&turn.session_id, &prompt, Some(turn_id))
                }
                ChatCommand::Save { call_id } | ChatCommand::Abandon { call_id } => {
                    let invocation = self
                        .retained
                        .get(&call_id)
                        .context(MissingReplySnafu { call_id: &call_id })?
                        .invocation
                        .clone();
                    session_id = Some(invocation.session_id.clone());
                    let mut resolution = self.resolve_retained(&call_id)?;
                    if let CompletionState::Uncommitted(pending) = resolution.state {
                        resolution.state = if abandon {
                            let state = self.database.abandon_recovered(&pending)?;
                            match state {
                                mf_chat::executable::InvocationState::Committed(completion) => {
                                    CompletionState::Committed(completion)
                                }
                                mf_chat::executable::InvocationState::Finished(turn) => {
                                    CompletionState::Finished(turn)
                                }
                                mf_chat::executable::InvocationState::NotBegun => {
                                    CompletionState::NotBegun
                                }
                                mf_chat::executable::InvocationState::Running => {
                                    return OptionsSnafu {
                                        reason: "abandonment left the turn running",
                                    }
                                    .fail();
                                }
                            }
                        } else {
                            CompletionState::Committed(self.database.save_recovered(pending)?)
                        };
                    }
                    self.retained.remove(&call_id);
                    Ok(ChatEvent::Finished {
                        invocation,
                        resolution,
                        status: None,
                        stderr: String::new(),
                        capture_error: None,
                    })
                }
                ChatCommand::Usage { session_id: id } => {
                    session_id = id.clone();
                    Ok(ChatEvent::Usage {
                        session_id: id.clone(),
                        statistics: self.database.usage(UsageQuery {
                            group_by: Some(mf_chat::UsageGrouping::Purpose),
                            session_id: id,
                            ..Default::default()
                        })?,
                    })
                }
            }
        })();
        self.emit_result(session_id, admission, result);
    }
    fn submit(
        &mut self,
        session_id: &str,
        prompt: &str,
        retry_of: Option<String>,
    ) -> Result<ChatEvent, ChatRunError> {
        ensure!(
            !self
                .active
                .values()
                .any(|job| job.invocation.session_id == session_id)
                && !self
                    .retained
                    .values()
                    .any(|job| job.invocation.session_id == session_id),
            SessionBusySnafu { session_id }
        );
        ensure!(
            self.active.len() + self.retained.len() < self.launch.options.max_active,
            CapacitySnafu {
                limit: self.launch.options.max_active
            }
        );
        let session = self.database.session(session_id)?;
        ensure!(
            !session.archived,
            OptionsSnafu {
                reason: "archived sessions cannot admit turns"
            }
        );
        let invocation = Invocation::new(session_id);
        let request_prompt = prompt.to_owned();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let launch = Arc::clone(&self.launch);
        let database = Arc::clone(&self.database);
        let events = self.workers_out.clone();
        let worker_invocation = invocation.clone();
        let worker = thread::Builder::new()
            .name("mf-chat-turn".into())
            .spawn(move || {
                let result = match execute_turn(
                    &launch,
                    &database,
                    worker_invocation.clone(),
                    &request_prompt,
                    retry_of,
                    worker_cancel,
                    &events,
                ) {
                    Ok(completion) => Ok(completion),
                    Err(source) => {
                        let source = match database.finalize_invocation(
                            worker_invocation.clone(),
                            TurnExit::Failed,
                            &source.to_string(),
                        ) {
                            Ok(_) => source,
                            Err(cleanup) => CleanupSnafu {
                                cleanup: Box::new(cleanup),
                            }
                            .into_error(Box::new(source)),
                        };
                        Err(source)
                    }
                };
                let _ = events.send(WorkerEvent::Finished {
                    invocation: worker_invocation,
                    result: Box::new(result),
                });
            })
            .context(IoSnafu {
                operation: "start conversation worker",
            })?;
        self.active.insert(
            invocation.call_id.clone(),
            Active {
                invocation: invocation.clone(),
                cancel: Arc::clone(&cancel),
                worker,
            },
        );
        Ok(ChatEvent::Submitted {
            prompt: prompt.into(),
            invocation,
            cancel,
        })
    }
    fn worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Observing {
                invocation,
                observation,
            } => self.outbox.push_back(ChatEvent::Observing {
                invocation,
                observation,
            }),
            WorkerEvent::Finished { invocation, result } => {
                if let Some(active) = self.active.remove(&invocation.call_id) {
                    let _ = active.worker.join();
                }
                match *result {
                    Ok(completion) => match completion.resolution {
                        Ok(resolution) => {
                            if let CompletionState::Uncommitted(pending) = &resolution.state {
                                self.retained.insert(
                                    invocation.call_id.clone(),
                                    Retained {
                                        invocation: invocation.clone(),
                                        state: RetainedState::Ready(pending.clone()),
                                    },
                                );
                            }
                            self.outbox.push_back(ChatEvent::Finished {
                                invocation,
                                resolution,
                                status: Some(completion.outcome.status),
                                stderr: completion.outcome.stderr,
                                capture_error: completion.outcome.capture_error,
                            });
                        }
                        Err(source) => {
                            let status = completion.outcome.status;
                            let stderr = completion.outcome.stderr.clone();
                            let capture_error = completion.outcome.capture_error.clone();
                            self.retained.insert(
                                invocation.call_id.clone(),
                                Retained {
                                    invocation: invocation.clone(),
                                    state: RetainedState::Exited(completion.outcome),
                                },
                            );
                            self.outbox.push_back(ChatEvent::RecoveryBlocked {
                                invocation,
                                status,
                                stderr,
                                capture_error,
                                source,
                            });
                        }
                    },
                    Err(source) => self
                        .outbox
                        .push_back(ChatEvent::Failed { invocation, source }),
                }
            }
        }
    }
    fn resolve_retained(&mut self, call_id: &str) -> Result<ReconciledCompletion, ChatRunError> {
        let retained = self
            .retained
            .get_mut(call_id)
            .context(MissingReplySnafu { call_id })?;
        let resolution = match &retained.state {
            RetainedState::Ready(pending) => ReconciledCompletion {
                state: CompletionState::Uncommitted(pending.clone()),
                diagnostics: vec![],
            },
            RetainedState::Exited(outcome) => {
                reconcile_turn(&self.database, &retained.invocation, outcome)?
            }
        };
        if let CompletionState::Uncommitted(pending) = &resolution.state {
            retained.state = RetainedState::Ready(pending.clone());
        }
        Ok(resolution)
    }
    fn shutdown(&mut self) -> Vec<ChatRunError> {
        let mut errors = Vec::new();
        for active in self.active.values() {
            active.cancel.store(true, Ordering::Release);
        }
        let active = std::mem::take(&mut self.active);
        for (_, active) in active {
            if active.worker.join().is_err() {
                errors.push(WorkerPanicSnafu.build());
            }
        }
        while let Ok(event) = self.workers_in.try_recv() {
            match event {
                WorkerEvent::Finished { result, invocation } => match *result {
                    Err(error) => errors.push(error),
                    Ok(completion) => self.worker_event(WorkerEvent::Finished {
                        invocation,
                        result: Box::new(Ok(completion)),
                    }),
                },
                WorkerEvent::Observing { .. } => {}
            }
        }
        let calls: Vec<_> = self.retained.keys().cloned().collect();
        for call in calls {
            let cleanup = (|| -> Result<(), ChatRunError> {
                if let CompletionState::Uncommitted(pending) = self.resolve_retained(&call)?.state {
                    self.database.abandon_recovered(&pending)?;
                }
                self.retained.remove(&call);
                Ok(())
            })();
            if let Err(error) = cleanup {
                errors.push(error);
            }
        }
        errors
    }
}

fn execute_turn(
    launch: &ChatLaunch,
    database: &ChatDatabase,
    invocation: Invocation,
    prompt: &str,
    retry_of: Option<String>,
    cancel: Arc<AtomicBool>,
    events: &SyncSender<WorkerEvent>,
) -> Result<WorkerCompletion, ChatRunError> {
    let started = database.begin_turn_with_invocation(
        invocation.clone(),
        prompt,
        mf_chat::ModelIdentity {
            profile: launch.options.profile.clone(),
            model: None,
        },
        retry_of,
        mf_chat::DEFAULT_HISTORY_BYTES,
    )?;
    let arguments = launch
        .contract
        .arguments(started.request, &launch.parameters)?;
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
    let mut command = Command::new(&launch.executable);
    command.arg("--inputs-file").arg(file.path());
    for _ in 0..launch.options.run.verbose.min(3) {
        command.arg("--verbose");
    }
    let description = launch
        .manifest
        .description()
        .map_err(mf_chat::ChatError::from)?;
    let mut run = LocalRun::start(
        command,
        description,
        mf_telemetry::identity::RunId::new(),
        mf_chat::executable::MAX_RECOVERY_BYTES as u64,
    )?;
    let _ = events.send(WorkerEvent::Observing {
        invocation: invocation.clone(),
        observation: run.observation(),
    });
    let outcome = loop {
        if cancel.load(Ordering::Acquire) {
            run.cancel();
        }
        match run.poll() {
            Ok(Some(outcome)) => break outcome,
            Ok(None) => {}
            Err(error) => {
                drop(run);
                let source = match database.finalize_invocation(
                    invocation,
                    TurnExit::Interrupted,
                    "runner supervision failed",
                ) {
                    Ok(_) => error.into(),
                    Err(cleanup) => CleanupSnafu {
                        cleanup: Box::new(cleanup),
                    }
                    .into_error(Box::new(error.into())),
                };
                return Err(source);
            }
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut resolution = reconcile_turn(database, &invocation, &outcome);
    if let Ok(resolution) = &mut resolution
        && let CompletionState::Uncommitted(pending) = &resolution.state
    {
        match database.complete_turn(
            pending.completion.invocation.clone(),
            pending.completion.reply.clone(),
        ) {
            Ok(completion) => resolution.state = CompletionState::Committed(completion),
            Err(source) => resolution.diagnostics.push(source),
        }
    }
    Ok(WorkerCompletion {
        resolution,
        outcome,
    })
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
        stdout: (outcome.capture_error.is_none()).then_some(outcome.stdout.as_slice()),
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
