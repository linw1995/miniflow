use super::*;
use crate::run::{ObservationView, SignalGuard, TerminalGuard, terminal_area};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::io::{IsTerminal, Write};

const HELP: &str = "Enter edit | n new | r rename | a archive/restore | A archived | j/k session | p/o page | PgUp/PgDn scroll | c cancel | t model retry | s save reply | x abandon | u/U usage | g inspect | [/] turn | q quit";
const MAX_EDITOR_BYTES: usize = 256 * 1024;

pub fn run_chat(path: &Path, options: ChatOptions) -> Result<u8, ChatRunError> {
    let launch = ChatLaunch::prepare(path, options)?;
    ensure!(
        io::stdin().is_terminal() && io::stderr().is_terminal(),
        TerminalRequiredSnafu
    );
    let area = terminal_area().context(IoSnafu {
        operation: "read terminal size",
    })?;
    let mut state = ChatView::new(
        &launch
            .manifest
            .description()
            .map_err(mf_chat::ChatError::from)?,
    )?;
    let mut controller = ChatController::open(launch)?;
    let signal = SignalGuard::register().context(IoSnafu {
        operation: "register interrupt handler",
    })?;
    let guard = TerminalGuard::enter().context(IoSnafu {
        operation: "enter terminal",
    })?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(Box::new(io::stderr()) as Box<dyn Write>),
        TerminalOptions {
            viewport: Viewport::Fixed(area),
        },
    )
    .context(IoSnafu {
        operation: "initialize terminal",
    })?;
    let result = (|| {
        execute!(io::stderr(), EnableBracketedPaste).context(IoSnafu {
            operation: "enable paste",
        })?;
        loop {
            while let Some(event) = controller.try_event() {
                state.event(event, &controller);
            }
            state.trim_observations();
            if signal.interrupted.swap(false, Ordering::Relaxed) {
                break;
            }
            state.draw(&mut terminal)?;
            if !event::poll(Duration::from_millis(50)).context(IoSnafu {
                operation: "poll terminal",
            })? {
                continue;
            }
            match event::read().context(IoSnafu {
                operation: "read terminal",
            })? {
                Event::Resize(width, height) => {
                    terminal
                        .resize(Rect::new(0, 0, width, height))
                        .context(IoSnafu {
                            operation: "resize terminal",
                        })?;
                }
                Event::Paste(text) => {
                    if state.focus != Focus::Navigate {
                        state
                            .editor_mut()
                            .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    }
                }
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && state.key(key, &controller) =>
                {
                    break;
                }
                _ => {}
            }
        }
        Ok::<(), ChatRunError>(())
    })();
    *terminal.backend_mut() = CrosstermBackend::new(Box::new(io::sink()));
    let cleanup = controller.join_workers();
    let _ = execute!(io::stderr(), DisableBracketedPaste);
    drop(terminal);
    drop(guard);
    drop(controller);
    cleanup?;
    result?;
    Ok(0)
}

fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\n' {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
    revision: u64,
}
impl Editor {
    fn insert(&mut self, text: &str) {
        if self.text.len().saturating_add(text.len()) > MAX_EDITOR_BYTES {
            return;
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.revision = self.revision.wrapping_add(1);
    }
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }
    fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => self.cursor = self.previous(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor]
                    .rfind('\n')
                    .map_or(0, |index| index + 1)
            }
            KeyCode::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let previous = self.previous();
                self.text.replace_range(previous..self.cursor, "");
                self.cursor = previous;
                self.revision = self.revision.wrapping_add(1);
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.replace_range(self.cursor..self.next(), "");
                self.revision = self.revision.wrapping_add(1);
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&c.to_string())
            }
            _ => {}
        }
    }
    fn display(&self, editing: bool) -> String {
        if !editing {
            return safe(&self.text);
        }
        format!(
            "{}▏{}",
            safe(&self.text[..self.cursor]),
            safe(&self.text[self.cursor..])
        )
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    Navigate,
    Prompt,
    NewSystem,
    Rename,
}
struct ActiveTurn {
    prompt: String,
    invocation: Invocation,
    cancel: Arc<AtomicBool>,
}
struct TurnNotice {
    invocation: Invocation,
    state: String,
    detail: String,
    unsaved: Option<String>,
    retained: bool,
}
struct ChatView {
    sessions: Vec<Session>,
    selected: usize,
    archived: bool,
    sessions_after: Option<String>,
    page: Option<MessagePage>,
    page_after: Option<i64>,
    previous_pages: Vec<Option<i64>>,
    scroll: u16,
    drafts: BTreeMap<String, Editor>,
    form: Editor,
    focus: Focus,
    pending: BTreeMap<String, Option<u64>>,
    active: BTreeMap<String, ActiveTurn>,
    turns: BTreeMap<String, TurnNotice>,
    observations: VecDeque<(Invocation, LocalObservation)>,
    observation_offset: usize,
    inspector: ObservationView,
    inspecting: bool,
    evicted: usize,
    notice: String,
    usage: Option<(Option<String>, UsageStatistics)>,
    creating: bool,
}
impl ChatView {
    fn new(
        description: &mf_telemetry::description::WorkflowDescription,
    ) -> Result<Self, ChatRunError> {
        Ok(Self {
            sessions: vec![],
            selected: 0,
            archived: false,
            sessions_after: None,
            page: None,
            page_after: None,
            previous_pages: vec![],
            scroll: 0,
            drafts: BTreeMap::new(),
            form: Editor::default(),
            focus: Focus::Navigate,
            pending: BTreeMap::new(),
            active: BTreeMap::new(),
            turns: BTreeMap::new(),
            observations: VecDeque::new(),
            observation_offset: 0,
            inspector: ObservationView::new(description)?,
            inspecting: false,
            evicted: 0,
            notice: HELP.into(),
            usage: None,
            creating: false,
        })
    }
    fn session_id(&self) -> Option<String> {
        self.sessions.get(self.selected).map(|s| s.id.clone())
    }
    fn editor_mut(&mut self) -> &mut Editor {
        if self.focus == Focus::Prompt
            && let Some(id) = self.session_id()
        {
            return self.drafts.entry(id).or_default();
        }
        &mut self.form
    }
    fn send(&mut self, controller: &ChatController, command: ChatCommand) -> bool {
        match controller.send(command) {
            Ok(()) => true,
            Err(error) => {
                self.notice = error.to_string();
                false
            }
        }
    }
    fn load(&mut self, controller: &ChatController) {
        if let Some(session_id) = self.session_id() {
            self.send(
                controller,
                ChatCommand::Load {
                    session_id,
                    after: self.page_after,
                },
            );
        }
    }
    fn select(&mut self, index: usize, controller: &ChatController) {
        self.selected = index.min(self.sessions.len().saturating_sub(1));
        self.page = None;
        self.page_after = None;
        self.previous_pages.clear();
        self.scroll = 0;
        self.observation_offset = 0;
        self.usage = None;
        self.load(controller);
    }
    fn event(&mut self, event: ChatEvent, controller: &ChatController) {
        match event {
            ChatEvent::Sessions {
                archived,
                page: list,
            } => {
                if archived != self.archived {
                    return;
                }
                let selected_id = self.session_id();
                self.sessions = list.sessions;
                self.sessions_after = list.next_after;
                let index = selected_id
                    .and_then(|id| self.sessions.iter().position(|s| s.id == id))
                    .unwrap_or(0);
                self.select(index, controller);
            }
            ChatEvent::Loaded {
                session,
                page,
                after,
            } => {
                if self.session_id().as_deref() == Some(&session.id) && self.page_after == after {
                    self.sessions[self.selected] = session;
                    self.page = Some(page);
                }
            }
            ChatEvent::Created(session) => {
                self.creating = false;
                self.archived = false;
                let id = session.id.clone();
                self.sessions.push(session);
                self.sessions.sort_by(|left, right| {
                    (left.created_at, &left.id).cmp(&(right.created_at, &right.id))
                });
                let selected = self
                    .sessions
                    .iter()
                    .position(|session| session.id == id)
                    .expect("created session is listed");
                self.select(selected, controller);
                self.focus = Focus::Prompt;
                self.form = Editor::default();
            }
            ChatEvent::Changed(session) => {
                if let Some(old) = self.sessions.iter_mut().find(|s| s.id == session.id) {
                    *old = session.clone();
                }
                self.notice = format!(
                    "Session {}: {}",
                    session.title,
                    if session.archived {
                        "archived"
                    } else {
                        "active"
                    }
                );
                self.load(controller);
            }
            ChatEvent::Submitted {
                prompt,
                invocation,
                cancel,
            } => {
                if let Some(Some(revision)) = self.pending.remove(&invocation.session_id)
                    && let Some(editor) = self.drafts.get_mut(&invocation.session_id)
                    && editor.revision == revision
                {
                    *editor = Editor::default();
                }
                self.turns.remove(&invocation.session_id);
                self.active.insert(
                    invocation.session_id.clone(),
                    ActiveTurn {
                        prompt,
                        invocation,
                        cancel,
                    },
                );
                self.load(controller);
            }
            ChatEvent::Observing {
                invocation,
                observation,
            } => {
                self.observations.push_back((invocation, observation));
            }
            ChatEvent::Finished {
                invocation,
                resolution,
                status,
                stderr,
                capture_error,
            } => {
                let active = self.active.remove(&invocation.session_id);
                if matches!(&resolution.state, CompletionState::NotBegun) {
                    self.restore_prompt(&invocation.session_id, active);
                }
                let workflow_error = match &resolution.state {
                    CompletionState::Finished(turn) => turn.error.as_deref(),
                    _ => None,
                };
                let workflow_error = workflow_error.map(|error| format!("\nWorkflow: {error}"));
                let (state, unsaved) = match resolution.state {
                    CompletionState::Committed(_) => ("committed".into(), None),
                    CompletionState::Uncommitted(pending) => {
                        ("uncommitted".into(), Some(pending.completion.reply.text))
                    }
                    CompletionState::Finished(turn) => (turn.state, None),
                    CompletionState::NotBegun => ("not begun".into(), None),
                };
                let mut detail = format!(
                    "Process: {}\nOutput: {}\n{}{}",
                    status.map_or_else(|| "save-only operation".into(), |s| s.to_string()),
                    capture_error.as_deref().unwrap_or("captured"),
                    resolution
                        .diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!("\nStderr: {stderr}")
                    }
                );
                if let Some(error) = workflow_error {
                    detail.push_str(&error);
                }
                if status.is_none()
                    && let Some(previous) = self.turns.get(&invocation.session_id)
                {
                    detail = format!("{}\nPersistence operation: {state}", previous.detail);
                }
                self.notice = format!("Turn {}: {state}", invocation.turn_id);
                self.turns.insert(
                    invocation.session_id.clone(),
                    TurnNotice {
                        invocation: invocation.clone(),
                        state,
                        detail,
                        retained: unsaved.is_some(),
                        unsaved,
                    },
                );
                if self.session_id().as_deref() == Some(&invocation.session_id) {
                    self.load(controller);
                }
            }
            ChatEvent::RecoveryBlocked {
                invocation,
                status,
                stderr,
                capture_error,
                source,
            } => {
                self.active.remove(&invocation.session_id);
                self.notice =
                    "Reconciliation failed; s retries facts and saving, x reconciles and abandons"
                        .into();
                self.turns.insert(
                    invocation.session_id.clone(),
                    TurnNotice {
                        invocation,
                        state: "reconciliation pending".into(),
                        detail: format!(
                            "Process: {status}\nOutput: {}\n{source}\n{stderr}",
                            capture_error.as_deref().unwrap_or("captured")
                        ),
                        unsaved: None,
                        retained: true,
                    },
                );
            }
            ChatEvent::Failed { invocation, source } => {
                let active = self.active.remove(&invocation.session_id);
                self.restore_prompt(&invocation.session_id, active);
                self.notice = source.to_string();
                self.load(controller);
            }
            ChatEvent::Usage {
                session_id,
                statistics,
            } => {
                if session_id.is_none() || session_id == self.session_id() {
                    self.usage = Some((session_id, statistics));
                }
            }
            ChatEvent::Error {
                admission,
                session_id,
                source,
            } => {
                if admission && let Some(id) = session_id {
                    self.pending.remove(&id);
                }
                self.creating = false;
                self.notice = source.to_string();
            }
        }
    }
    fn restore_prompt(&mut self, id: &str, active: Option<ActiveTurn>) {
        if let Some(active) = active {
            let editor = self.drafts.entry(id.into()).or_default();
            if editor.text.is_empty() {
                editor.insert(&active.prompt);
            }
        }
    }
    fn trim_observations(&mut self) {
        // Supervision can fail without an exit status, so use invocation ownership.
        let active = &self.active;
        let finished = |(invocation, _): &(Invocation, LocalObservation)| {
            active
                .get(&invocation.session_id)
                .is_none_or(|turn| turn.invocation.call_id != invocation.call_id)
        };
        while self.observations.iter().filter(|run| finished(run)).count() > MAX_RETAINED_CHAT_RUNS
        {
            if let Some(index) = self.observations.iter().position(finished) {
                self.observations.remove(index);
                self.evicted += 1;
            }
        }
    }
    fn observation(&self) -> Option<&LocalObservation> {
        let id = self.session_id()?;
        self.observations
            .iter()
            .rev()
            .filter(|(invocation, _)| invocation.session_id == id)
            .nth(self.observation_offset)
            .map(|(_, run)| run)
    }
    fn key(&mut self, key: KeyEvent, controller: &ChatController) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if self.inspecting {
            if key.code == KeyCode::Esc {
                self.inspecting = false;
            } else {
                let id = self.session_id();
                let offset = self.observation_offset;
                if let Some(run) = self
                    .observations
                    .iter()
                    .rev()
                    .filter(|(inv, _)| Some(&inv.session_id) == id.as_ref())
                    .nth(offset)
                    .map(|(_, run)| run)
                {
                    self.inspector.handle_key(key.code, run);
                }
            }
            return false;
        }
        if self.focus != Focus::Navigate {
            match key.code {
                KeyCode::Esc => {
                    self.focus = Focus::Navigate;
                    self.notice = HELP.into();
                }
                KeyCode::Enter
                    if key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
                        && self.focus != Focus::Rename =>
                {
                    self.editor_mut().insert("\n")
                }
                KeyCode::Enter => match self.focus {
                    Focus::Prompt => {
                        if let Some(id) = self.session_id() {
                            let busy = self.pending.contains_key(&id)
                                || self.active.contains_key(&id)
                                || self.turns.get(&id).is_some_and(|t| t.retained);
                            if busy {
                                self.notice = "Session has unresolved work; cancel, save, or abandon it first".into();
                            } else {
                                let editor = self.drafts.entry(id.clone()).or_default();
                                let prompt = editor.text.clone();
                                let revision = editor.revision;
                                if !prompt.trim().is_empty()
                                    && self.send(
                                        controller,
                                        ChatCommand::Submit {
                                            session_id: id.clone(),
                                            prompt,
                                        },
                                    )
                                {
                                    self.pending.insert(id, Some(revision));
                                    self.notice = "Submission pending".into();
                                }
                            }
                        }
                    }
                    Focus::NewSystem => {
                        if !self.creating {
                            let system =
                                (!self.form.text.is_empty()).then(|| self.form.text.clone());
                            if self.send(
                                controller,
                                ChatCommand::Create {
                                    system,
                                    title: None,
                                },
                            ) {
                                self.creating = true;
                                self.focus = Focus::Navigate;
                            }
                        }
                    }
                    Focus::Rename => {
                        if let Some(session_id) = self.session_id()
                            && self.send(
                                controller,
                                ChatCommand::Rename {
                                    session_id,
                                    title: self.form.text.clone(),
                                },
                            )
                        {
                            self.focus = Focus::Navigate;
                        }
                    }
                    Focus::Navigate => {}
                },
                _ => self.editor_mut().key(key),
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Enter => {
                if self.session_id().is_some() {
                    self.focus = Focus::Prompt;
                    self.usage = None;
                }
            }
            KeyCode::Char('n') => {
                self.form = Editor::default();
                self.focus = Focus::NewSystem;
                self.notice =
                    "Optional system instructions: Enter create; Alt-Enter newline; Esc cancel"
                        .into();
            }
            KeyCode::Char('r') => {
                if let Some(session) = self.sessions.get(self.selected) {
                    self.form = Editor {
                        text: session.title.clone(),
                        cursor: session.title.len(),
                        revision: 0,
                    };
                    self.focus = Focus::Rename;
                }
            }
            KeyCode::Char('a') => {
                if let Some(session) = self.sessions.get(self.selected) {
                    self.send(
                        controller,
                        ChatCommand::Archive {
                            session_id: session.id.clone(),
                            archived: !session.archived,
                        },
                    );
                }
            }
            KeyCode::Char('A') => {
                self.archived = !self.archived;
                self.send(
                    controller,
                    ChatCommand::ListSessions {
                        archived: self.archived,
                        after: None,
                    },
                );
            }
            KeyCode::Char('j') | KeyCode::Down => self.select(self.selected + 1, controller),
            KeyCode::Char('k') | KeyCode::Up => {
                self.select(self.selected.saturating_sub(1), controller)
            }
            KeyCode::Char('N') => {
                if let Some(after) = &self.sessions_after {
                    self.send(
                        controller,
                        ChatCommand::ListSessions {
                            archived: self.archived,
                            after: Some(after.clone()),
                        },
                    );
                }
            }
            KeyCode::Char('B') => {
                self.send(
                    controller,
                    ChatCommand::ListSessions {
                        archived: self.archived,
                        after: None,
                    },
                );
            }
            KeyCode::Char('p') => {
                if let Some(after) = self.page.as_ref().and_then(|p| p.next_after) {
                    self.previous_pages.push(self.page_after);
                    self.page_after = Some(after);
                    self.scroll = 0;
                    self.load(controller);
                }
            }
            KeyCode::Char('o') => {
                if let Some(after) = self.previous_pages.pop() {
                    self.page_after = after;
                    self.scroll = 0;
                    self.load(controller);
                }
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('c') => {
                if let Some(active) = self.session_id().and_then(|id| self.active.get(&id)) {
                    active.cancel.store(true, Ordering::Release);
                    self.notice = "Cancellation pending".into();
                }
            }
            KeyCode::Char('t') => {
                if let Some(turn_id) = self
                    .page
                    .as_ref()
                    .and_then(|p| {
                        p.messages.iter().rev().find(|m| {
                            matches!(
                                m.turn_state.as_str(),
                                "failed" | "cancelled" | "interrupted"
                            )
                        })
                    })
                    .map(|m| m.message.turn_id.clone())
                    && let Some(id) = self.session_id()
                    && !self.pending.contains_key(&id)
                    && !self.active.contains_key(&id)
                    && self.send(
                        controller,
                        ChatCommand::Retry {
                            session_id: id.clone(),
                            turn_id,
                        },
                    )
                {
                    self.pending.insert(id, None);
                }
            }
            KeyCode::Char('s') | KeyCode::Char('x') => {
                if let Some(call_id) = self
                    .session_id()
                    .and_then(|id| self.turns.get(&id))
                    .filter(|turn| turn.retained)
                    .map(|turn| turn.invocation.call_id.clone())
                {
                    let abandon = key.code == KeyCode::Char('x');
                    if self.send(
                        controller,
                        if abandon {
                            ChatCommand::Abandon { call_id }
                        } else {
                            ChatCommand::Save { call_id }
                        },
                    ) {
                        self.notice = if abandon {
                            "Abandonment pending"
                        } else {
                            "Save-only retry pending"
                        }
                        .into();
                    }
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                self.send(
                    controller,
                    ChatCommand::Usage {
                        session_id: if key.code == KeyCode::Char('U') {
                            None
                        } else {
                            self.session_id()
                        },
                    },
                );
            }
            KeyCode::Char('g') => {
                if self.observation().is_some() {
                    self.inspecting = true;
                }
            }
            KeyCode::Char('[') => {
                self.observation_offset = self.observation_offset.saturating_add(1);
                if self.observation().is_none() {
                    self.observation_offset = self.observation_offset.saturating_sub(1);
                }
            }
            KeyCode::Char(']') => {
                self.observation_offset = self.observation_offset.saturating_sub(1)
            }
            _ => {}
        }
        false
    }
    fn draw(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Box<dyn Write>>>,
    ) -> Result<(), ChatRunError> {
        if self.inspecting {
            let id = self.session_id();
            let offset = self.observation_offset;
            if let Some(run) = self
                .observations
                .iter()
                .rev()
                .filter(|(inv, _)| Some(&inv.session_id) == id.as_ref())
                .nth(offset)
                .map(|(_, run)| run)
            {
                self.inspector.draw(terminal, run)?;
                return Ok(());
            }
            self.inspecting = false;
        }
        terminal
            .draw(|frame| {
                let [header, body, input, footer] = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Min(1),
                    Constraint::Length(6),
                    Constraint::Length(3),
                ])
                .areas(frame.area());
                let selected_id = self.session_id();
                let status = selected_id
                    .as_ref()
                    .and_then(|id| self.turns.get(id))
                    .map(|t| t.state.as_str())
                    .unwrap_or("ready");
                let title = self
                    .sessions
                    .get(self.selected)
                    .map_or("No session", |s| s.title.as_str());
                frame.render_widget(
                    Paragraph::new(format!(
                        "{} | {} | {}\nAutomatic titles: deferred; observation history evicted: {}",
                        safe(title),
                        if self.archived {
                            "Archived sessions"
                        } else {
                            "Sessions"
                        },
                        status,
                        self.evicted
                    )),
                    header,
                );
                let [list, transcript] =
                    Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)])
                        .areas(body);
                let list_rows = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(index, s)| {
                        format!(
                            "{} {}{}{}",
                            if index == self.selected { ">" } else { " " },
                            safe(&s.title).replace('\n', "\\n"),
                            if self.active.contains_key(&s.id) {
                                " [running]"
                            } else if self.pending.contains_key(&s.id) {
                                " [pending]"
                            } else {
                                ""
                            },
                            if s.archived { " [archived]" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let list_scroll = self
                    .selected
                    .saturating_sub(usize::from(list.height.saturating_sub(3)))
                    as u16;
                frame.render_widget(
                    Paragraph::new(list_rows).scroll((list_scroll, 0)).block(
                        Block::default()
                            .title("Sessions (N next, B first)")
                            .borders(Borders::ALL),
                    ),
                    list,
                );
                let text = if let Some((scope, stats)) = &self.usage {
                    usage_text(scope.as_deref(), stats)
                } else {
                    let mut text = self
                        .page
                        .as_ref()
                        .map(|p| {
                            p.messages
                                .iter()
                                .map(|m| {
                                    format!(
                                        "{} [{}] {}:\n{}",
                                        m.message.role,
                                        m.turn_state,
                                        m.message.turn_id,
                                        safe(&m.message.content)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default();
                    if let Some(turn) = selected_id.as_ref().and_then(|id| self.turns.get(id)) {
                        if let Some(reply) = &turn.unsaved {
                            text.push_str(&format!(
                                "\n\nAssistant [uncommitted]:\n{}\nSave: s | Abandon: x",
                                safe(reply)
                            ));
                        }
                        text.push_str(&format!(
                            "\n\nTurn {}: {}\n{}",
                            turn.invocation.turn_id,
                            turn.state,
                            safe(&turn.detail)
                        ));
                    }
                    if let Some(active) = selected_id.as_ref().and_then(|id| self.active.get(id)) {
                        text.push_str(&format!("\n\nRunning turn {}", active.invocation.turn_id));
                    }
                    text
                };
                frame.render_widget(
                    Paragraph::new(text)
                        .scroll((self.scroll, 0))
                        .wrap(Wrap { trim: false })
                        .block(
                            Block::default()
                                .title("Transcript / outcomes / usage")
                                .borders(Borders::ALL),
                        ),
                    transcript,
                );
                let (editor, label) = match self.focus {
                    Focus::NewSystem => (
                        Some(&self.form),
                        "New session: optional system instructions",
                    ),
                    Focus::Rename => (Some(&self.form), "Rename session"),
                    _ => (
                        selected_id.as_ref().and_then(|id| self.drafts.get(id)),
                        if self.focus == Focus::Navigate {
                            "Prompt (Enter edit)"
                        } else {
                            "Prompt: Enter send; Alt/Shift-Enter newline; Esc navigate"
                        },
                    ),
                };
                let body = editor.map_or(String::new(), |editor| {
                    editor.display(self.focus != Focus::Navigate)
                });
                let scroll = body
                    .lines()
                    .count()
                    .saturating_sub(usize::from(input.height.saturating_sub(2)))
                    as u16;
                frame.render_widget(
                    Paragraph::new(body)
                        .scroll((scroll, 0))
                        .wrap(Wrap { trim: false })
                        .style(Style::default().fg(if self.focus == Focus::Navigate {
                            Color::Gray
                        } else {
                            Color::White
                        }))
                        .block(Block::default().title(label).borders(Borders::ALL)),
                    input,
                );
                frame.render_widget(
                    Paragraph::new(format!("{}\n{HELP}", safe(&self.notice)))
                        .wrap(Wrap { trim: false }),
                    footer,
                );
            })
            .context(IoSnafu {
                operation: "draw chat",
            })?;
        Ok(())
    }
}
fn usage_text(scope: Option<&str>, statistics: &UsageStatistics) -> String {
    let summary = &statistics.overall;
    let counter = |label: &str, value: &mf_chat::TokenCounter| {
        format!(
            "{label}: {} (reported {}, missing {})",
            value.known_subtotal.map_or_else(
                || "unknown".into(),
                |count| if value.missing_calls > 0 {
                    format!("{count} partial")
                } else {
                    count.to_string()
                }
            ),
            value.reported_calls,
            value.missing_calls
        )
    };
    let purposes = statistics
        .groups
        .iter()
        .map(|group| format!("{}: {} attempted", group.key, group.summary.attempted_calls))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Usage: {}\nAttempted calls: {}\nUnreported invocations: {}\n{}\n{}\n{}\nStates: {:?}\n{}\nPending title requests are excluded until attempted.",
        scope.unwrap_or("all sessions"),
        summary.attempted_calls,
        statistics.unreported_invocations,
        counter("Input", &summary.input_tokens),
        counter("Output", &summary.output_tokens),
        counter("Total", &summary.total_tokens),
        summary.statuses,
        purposes
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_preserves_unicode_boundaries_multiline_and_literal_controls() {
        let mut editor = Editor::default();
        editor.insert("A🦀e\u{301}\nB");
        editor.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(editor.cursor, "A🦀e\u{301}\n".len());
        editor.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        editor.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.text, "A🦀e\nB");
        editor.cursor = 1;
        editor.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(editor.text, "Ae\nB");
        editor.insert("\x1b[31m");
        assert!(safe(&editor.text).contains("\\u{1b}[31m"));
        assert!(!safe(&editor.text).contains('\x1b'));
    }
    #[test]
    fn delayed_admission_and_other_session_results_preserve_edits() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        let database_path = root.path().join("chat.sqlite");
        let registry = mf_runtime::NodeRegistry::from_inventory().unwrap();
        let create = registry
            .get("mf_chat.session.create")
            .unwrap()
            .instantiate(serde_json::json!({}))
            .unwrap();
        use mf_runtime::NodeValue;
        let mut sessions = vec![];
        for index in 0..2 {
            let inputs = mf_chat::nodes::CreateSessionInputs {
                database_path: database_path.to_str().unwrap().into(),
                session_id: format!("session-{index}"),
                profile: "default".into(),
                system: None,
                title: None,
            };
            let result = create
                .execution
                .as_task_node()
                .unwrap()
                .execute(inputs.into_values().unwrap(), &mut Default::default())
                .unwrap();
            sessions.push(
                mf_chat::nodes::SessionOutputs::from_values(result.outputs)
                    .unwrap()
                    .session,
            );
        }
        view.sessions = sessions;
        view.focus = Focus::Prompt;
        view.editor_mut().insert("submitted");
        let id = view.session_id().unwrap();
        let revision = view.editor_mut().revision;
        view.pending.insert(id.clone(), Some(revision));
        view.editor_mut().insert(" + new edit");
        view.select(1, &controller);
        let invocation = Invocation {
            session_id: id.clone(),
            turn_id: "turn".into(),
            call_id: "call".into(),
        };
        view.event(
            ChatEvent::Submitted {
                prompt: "submitted".into(),
                invocation,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            &controller,
        );
        assert_eq!(view.drafts[&id].text, "submitted + new edit");
        assert!(view.active.contains_key(&id));
        assert_ne!(view.session_id().unwrap(), id);
    }
    fn key(view: &mut ChatView, controller: &ChatController, code: KeyCode) {
        assert!(!view.key(KeyEvent::new(code, KeyModifiers::NONE), controller));
    }
    fn pump(view: &mut ChatView, controller: &ChatController, ready: impl Fn(&ChatView) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            while let Some(event) = controller.try_event() {
                view.event(event, controller);
            }
            if ready(view) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{}", view.notice);
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn draw(view: &mut ChatView) {
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(Box::new(io::sink()) as Box<dyn Write>),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 100, 30)),
            },
        )
        .unwrap();
        view.draw(&mut terminal).unwrap();
    }

    #[test]
    fn abandoned_observations_close_and_do_not_escape_retention_limits() {
        use crate::state::Completeness;
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let description = launch.manifest.description().unwrap();
        let mut view = ChatView::new(&description).unwrap();
        let run = LocalRun::start(
            Command::new(&launch.executable),
            description,
            mf_telemetry::identity::RunId::new(),
            1024,
        )
        .unwrap();
        let observation = run.observation();
        drop(run);
        assert_eq!(
            observation.reader.snapshot().lifecycle.completeness,
            Completeness::UnverifiedTail
        );
        for _ in 0..MAX_RETAINED_CHAT_RUNS + 2 {
            view.observations
                .push_back((Invocation::new("session"), observation.clone()));
        }
        let active = view.observations.back().unwrap().0.clone();
        view.active.insert(
            active.session_id.clone(),
            ActiveTurn {
                invocation: active.clone(),
                prompt: "pending".into(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        view.trim_observations();
        assert_eq!(view.observations.len(), MAX_RETAINED_CHAT_RUNS + 1);
        assert_eq!(view.evicted, 1);
        assert_eq!(view.observations.back().unwrap().0, active);
    }

    #[test]
    fn stored_pages_archive_filters_and_usage_keep_session_drafts() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        let db = &controller._database;
        for index in 0..55 {
            db.create_session("default", None, Some(format!("session-{index}")))
                .unwrap();
        }
        key(&mut view, &controller, KeyCode::Char('B'));
        pump(&mut view, &controller, |v| {
            v.sessions.len() == 50 && v.page.is_some()
        });
        let id = view.session_id().unwrap();
        view.focus = Focus::Prompt;
        view.editor_mut().insert("keep 🦀 draft");
        key(&mut view, &controller, KeyCode::Esc);
        key(&mut view, &controller, KeyCode::Char('N'));
        pump(&mut view, &controller, |v| {
            v.sessions.len() == 5 && v.page.is_some()
        });
        assert_eq!(view.drafts[&id].text, "keep 🦀 draft");
        key(&mut view, &controller, KeyCode::Char('B'));
        pump(&mut view, &controller, |v| {
            v.sessions.len() == 50 && v.page.is_some()
        });
        assert_eq!(view.session_id().as_deref(), Some(id.as_str()));
        let model =
            mf_chat::ModelProfile::new("default", serde_json::json!({"model":"fixture"})).unwrap();
        for index in 0..26 {
            let start = db
                .begin_turn(
                    &id,
                    &format!("page-{index}"),
                    model.identity().clone(),
                    None,
                    mf_chat::DEFAULT_HISTORY_BYTES,
                )
                .unwrap();
            db.complete_turn(
                start.invocation,
                mf_chat::Reply {
                    text: format!("reply-{index}"),
                    usages: None,
                    metadata: None,
                    finish_reason: None,
                },
            )
            .unwrap();
        }
        view.load(&controller);
        pump(&mut view, &controller, |v| {
            v.page.as_ref().is_some_and(|p| p.messages.len() == 50)
        });
        key(&mut view, &controller, KeyCode::Char('p'));
        pump(&mut view, &controller, |v| {
            v.page.as_ref().is_some_and(|p| p.messages.len() == 2)
        });
        assert_eq!(
            view.page.as_ref().unwrap().messages[0].message.content,
            "page-25"
        );
        key(&mut view, &controller, KeyCode::Char('o'));
        pump(&mut view, &controller, |v| {
            v.page.as_ref().is_some_and(|p| p.messages.len() == 50)
        });
        key(&mut view, &controller, KeyCode::PageDown);
        key(&mut view, &controller, KeyCode::PageUp);
        key(&mut view, &controller, KeyCode::Char('U'));
        pump(&mut view, &controller, |v| v.usage.is_some());
        assert!(usage_text(None, &view.usage.as_ref().unwrap().1).contains("unknown"));
        draw(&mut view);
        db.archive_session(&id).unwrap();
        key(&mut view, &controller, KeyCode::Char('A'));
        pump(&mut view, &controller, |v| {
            v.sessions.len() == 1 && v.sessions[0].archived
        });
        draw(&mut view);
        key(&mut view, &controller, KeyCode::Char('A'));
        pump(&mut view, &controller, |v| {
            v.sessions.len() == 50 && !v.sessions[0].archived
        });
        assert_eq!(view.drafts[&id].text, "keep 🦀 draft");
        controller.close().unwrap();
    }

    #[test]
    fn launch_failure_restores_drafts_and_query_errors_clear_only_admission() {
        use std::os::unix::process::ExitStatusExt;
        let root = tempfile::tempdir().unwrap();
        let mut launch = super::super::tests::launch(root.path());
        std::fs::remove_file(&launch.executable).unwrap();
        launch.options.run.verbose = 1;
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        controller
            .send(ChatCommand::Create {
                system: None,
                title: None,
            })
            .unwrap();
        pump(&mut view, &controller, |v| {
            !v.sessions.is_empty() && v.focus == Focus::Prompt
        });
        let id = view.session_id().unwrap();
        view.editor_mut().insert("restore this prompt");
        key(&mut view, &controller, KeyCode::Enter);
        pump(&mut view, &controller, |v| {
            v.notice.contains("could not launch")
        });
        assert_eq!(view.drafts[&id].text, "restore this prompt");
        view.pending.insert(id.clone(), None);
        view.event(
            ChatEvent::Error {
                admission: false,
                session_id: Some(id.clone()),
                source: OptionsSnafu {
                    reason: "query failed",
                }
                .build(),
            },
            &controller,
        );
        assert!(view.pending.contains_key(&id));
        view.event(
            ChatEvent::Error {
                admission: true,
                session_id: Some(id.clone()),
                source: OptionsSnafu {
                    reason: "retry failed",
                }
                .build(),
            },
            &controller,
        );
        assert!(!view.pending.contains_key(&id));
        let invocation = Invocation {
            session_id: id.clone(),
            turn_id: "unresolved".into(),
            call_id: "retained".into(),
        };
        view.event(
            ChatEvent::RecoveryBlocked {
                invocation: invocation.clone(),
                status: ExitStatus::from_raw(256),
                stderr: String::new(),
                capture_error: None,
                source: controller._database.session("missing").unwrap_err(),
            },
            &controller,
        );
        key(&mut view, &controller, KeyCode::Enter);
        assert!(view.notice.contains("unresolved"));
        draw(&mut view);
        view.focus = Focus::Navigate;
        key(&mut view, &controller, KeyCode::Char('x'));
        pump(&mut view, &controller, |v| {
            v.notice.contains("no retained reply")
        });
        let database = &controller._database;
        let started = database
            .begin_turn(
                &id,
                "captured failure",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                1024,
            )
            .unwrap();
        database
            .fail_turn(
                started.invocation.clone(),
                "failed",
                "provider rejected request",
            )
            .unwrap();
        view.event(
            ChatEvent::Finished {
                invocation: started.invocation.clone(),
                resolution: ReconciledCompletion {
                    state: CompletionState::Finished(
                        database.turn(&started.invocation.turn_id).unwrap(),
                    ),
                    diagnostics: vec![],
                },
                status: Some(ExitStatus::from_raw(0)),
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        assert!(
            view.turns[&id]
                .detail
                .contains("Workflow: provider rejected request")
        );
        assert!(
            usage_text(None, &database.usage(Default::default()).unwrap())
                .contains("Unreported invocations: 2")
        );
        view.active.insert(
            id.clone(),
            ActiveTurn {
                invocation,
                prompt: "keep".into(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        key(&mut view, &controller, KeyCode::Char('c'));
        assert!(view.active[&id].cancel.load(Ordering::Acquire));
        key(&mut view, &controller, KeyCode::Char('['));
        key(&mut view, &controller, KeyCode::Char(']'));
        view.inspecting = true;
        draw(&mut view);
        assert!(!view.inspecting);
        assert!(view.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &controller
        ));
        controller.close().unwrap();
    }
}
