use super::*;
use crate::run::{ObservationView, SignalGuard, TerminalGuard, terminal_area};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListState, Paragraph, Wrap},
};
use std::{
    collections::BTreeMap,
    io::{IsTerminal, Write},
};
const MAX_EDITOR_BYTES: usize = 256 * 1024;

fn execution_areas(body: Rect) -> (Rect, Rect) {
    if body.width >= 120 && body.height >= 12 {
        let [chat, panel] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(body);
        (chat, panel)
    } else if body.height >= 18 {
        let [chat, panel] = Layout::vertical([
            Constraint::Min(10),
            Constraint::Length((body.height / 3).clamp(8, 14)),
        ])
        .areas(body);
        (chat, panel)
    } else {
        let [chat, panel] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(body);
        (chat, panel)
    }
}

pub fn run_chat(path: &Path, options: ChatOptions) -> Result<u8, ChatRunError> {
    let launch = ChatLaunch::prepare(path, options)?;
    ensure!(
        io::stdin().is_terminal() && io::stderr().is_terminal(),
        TerminalRequiredSnafu
    );
    let area = terminal_area().context(IoSnafu {
        operation: "read terminal size",
    })?;
    let mut view = ChatView::new(
        &launch
            .manifest
            .description()
            .map_err(mf_chat::ChatError::from)?,
    )?;
    let controller = ChatController::open(launch)?;
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
            for _ in 0..16 {
                let Some(event) = controller.try_event() else {
                    break;
                };
                view.event(event, &controller);
            }
            if signal.interrupted.swap(false, Ordering::Relaxed) {
                controller.cancel();
            }
            view.draw(&mut terminal)?;
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
                Event::Paste(text) if view.mode == Mode::Input => view
                    .editor
                    .insert(&text.replace("\r\n", "\n").replace('\r', "\n")),
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                        && view.key(key, &controller) =>
                {
                    break;
                }
                _ => {}
            }
        }
        Ok::<(), ChatRunError>(())
    })();
    *terminal.backend_mut() = CrosstermBackend::new(Box::new(io::sink()));
    let cleanup = controller.close();
    let _ = execute!(io::stderr(), DisableBracketedPaste);
    drop(terminal);
    drop(guard);
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
enum Mode {
    #[default]
    Input,
    Browse,
    Commands,
    Sessions,
    Inspect,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    New,
    Switch,
    Retry,
    Save,
    Discard,
    Inspect,
    Earlier,
    Latest,
    ToggleExecution,
    Quit,
}

struct ChatView {
    session: Option<Session>,
    page: Option<MessagePage>,
    earlier: bool,
    editor: Editor,
    drafts: BTreeMap<String, Editor>,
    mode: Mode,
    selected: usize,
    sessions: Option<SessionPage>,
    sessions_after: Option<String>,
    pending: Option<Option<u64>>,
    active: Option<(Invocation, String)>,
    retained: bool,
    unsaved: Option<String>,
    final_reply: Option<String>,
    reply_usages: Vec<mf_chat::CallUsage>,
    preview: LivePreview,
    latest_invocation: Option<Invocation>,
    observation: Option<LocalObservation>,
    show_execution: bool,
    inspector: ObservationView,
    scroll: u16,
    notice: String,
    outcome: String,
}
impl ChatView {
    fn new(
        description: &mf_telemetry::description::WorkflowDescription,
    ) -> Result<Self, ChatRunError> {
        Ok(Self {
            session: None,
            page: None,
            earlier: false,
            editor: Editor::default(),
            drafts: BTreeMap::new(),
            mode: Mode::Input,
            selected: 0,
            sessions: None,
            sessions_after: None,
            pending: None,
            active: None,
            retained: false,
            unsaved: None,
            final_reply: None,
            reply_usages: vec![],
            preview: LivePreview::default(),
            latest_invocation: None,
            observation: None,
            show_execution: true,
            inspector: ObservationView::new(description)?,
            scroll: 0,
            notice: "Start a conversation, or Ctrl+S to resume one".into(),
            outcome: String::new(),
        })
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
    fn load(&mut self, controller: &ChatController, before: Option<i64>) {
        if let Some(session) = &self.session {
            self.send(
                controller,
                ChatCommand::Load {
                    session_id: session.id.clone(),
                    before,
                },
            );
        }
    }
    fn switch(&mut self, session: Option<Session>) {
        let old = self
            .session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        self.drafts.insert(old, std::mem::take(&mut self.editor));
        let new = session.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        self.editor = self.drafts.remove(&new).unwrap_or_default();
        self.session = session;
        self.page = None;
        self.preview = LivePreview::default();
        self.final_reply = None;
        self.reply_usages.clear();
        self.latest_invocation = None;
        self.earlier = false;
        self.observation = None;
        self.scroll = 0;
        self.outcome.clear();
        self.notice.clear();
        self.mode = Mode::Input;
    }
    fn event(&mut self, event: ChatEvent, controller: &ChatController) {
        let selected_action = (self.mode == Mode::Commands)
            .then(|| self.actions().get(self.selected).map(|(action, _)| *action))
            .flatten();
        match event {
            ChatEvent::Sessions(page) => {
                self.sessions = Some(page);
                self.selected = 0;
            }
            ChatEvent::Loaded { session, page } => {
                if self.session.as_ref().is_some_and(|s| s.id == session.id) {
                    self.session = Some(session);
                    self.page = Some(page);
                    self.final_reply = None;
                    self.scroll = 0;
                }
            }
            ChatEvent::Submitted {
                session,
                invocation,
                prompt,
            } => {
                if self.pending.take() == Some(Some(self.editor.revision)) {
                    self.editor = Editor::default();
                }
                self.session = Some(session);
                self.latest_invocation = Some(invocation.clone());
                self.preview = LivePreview::default();
                self.final_reply = None;
                self.reply_usages.clear();
                self.active = Some((invocation, prompt));
                self.observation = None;
                self.unsaved = None;
                self.outcome.clear();
                self.notice = "Working... Ctrl+C to cancel".into();
                self.scroll = 0;
            }
            ChatEvent::Preview {
                invocation,
                preview,
            } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|(active, _)| active == &invocation)
                {
                    self.preview = preview;
                }
            }
            ChatEvent::Observing(observation) => self.observation = Some(observation),
            ChatEvent::Finished {
                invocation,
                resolution,
                stderr,
                capture_error,
                ..
            } => {
                if self
                    .latest_invocation
                    .as_ref()
                    .is_some_and(|latest| latest != &invocation)
                {
                    return;
                }
                self.preview.incomplete = true;
                self.active = None;
                self.pending = None;
                self.retained = false;
                self.unsaved = None;
                match resolution {
                    Ok(resolution) => {
                        let mut details: Vec<_> = resolution
                            .diagnostics
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        match resolution.state {
                            CompletionState::Committed(completion) => {
                                self.preview = LivePreview::default();
                                self.reply_usages = completion.result.usages;
                                self.final_reply = Some(completion.result.text);
                                self.notice = "Saved".into();
                            }
                            CompletionState::Uncommitted(pending) => {
                                self.preview = LivePreview::default();
                                self.retained = true;
                                self.reply_usages = pending.completion.result.usages;
                                self.unsaved = Some(pending.completion.result.text);
                                self.notice =
                                    "Reply not saved. Ctrl+P to save or discard it".into();
                            }
                            CompletionState::Finished(turn) => {
                                self.notice = format!("Turn {}. Ctrl+P for recovery", turn.state);
                                if let Some(error) = turn.error {
                                    details.push(error);
                                }
                            }
                            CompletionState::NotBegun => self.notice = "Turn did not start".into(),
                        }
                        self.outcome = details.join("\n");
                    }
                    Err(error) => {
                        self.retained = true;
                        self.notice = "Result needs recovery. Ctrl+P to save or discard it".into();
                        self.outcome = error.to_string();
                    }
                }
                if let Some(error) = capture_error {
                    self.outcome.push_str(&format!("\n{error}"));
                }
                if !stderr.is_empty() {
                    self.outcome.push_str(&format!("\n{stderr}"));
                }
                self.earlier = false;
                self.load(controller, None);
            }
            ChatEvent::Error { admission, source } => {
                if admission {
                    self.pending = None;
                    if let Some((_, prompt)) = self.active.take()
                        && self.editor.text.is_empty()
                    {
                        self.editor.insert(&prompt);
                    }
                    self.load(controller, None);
                }
                self.notice = source.to_string();
            }
        }
        if self.mode == Mode::Commands {
            self.selected = selected_action
                .and_then(|selected| {
                    self.actions()
                        .iter()
                        .position(|(action, _)| *action == selected)
                })
                .unwrap_or(0);
        }
    }
    fn retry_turn(&self) -> Option<String> {
        if self.earlier {
            return None;
        }
        self.page
            .as_ref()?
            .messages
            .last()
            .filter(|m| {
                matches!(
                    m.turn_state.as_str(),
                    "failed" | "cancelled" | "interrupted"
                )
            })
            .map(|m| m.message.turn_id.clone())
    }
    fn actions(&self) -> Vec<(Action, &'static str)> {
        let mut actions = vec![(Action::New, "New chat"), (Action::Switch, "Switch chat")];
        if self.retained {
            actions.push((Action::Save, "Save reply without running again"));
            actions.push((Action::Discard, "Discard unsaved reply"));
        } else if self.retry_turn().is_some() {
            actions.push((Action::Retry, "Retry failed turn"));
        }
        if self.observation.is_some() {
            actions.push((Action::Inspect, "Inspect current run"));
        }
        if self
            .page
            .as_ref()
            .and_then(|p| p.messages.first())
            .is_some_and(|m| m.message.sequence > 0)
        {
            actions.push((Action::Earlier, "Earlier messages"));
        }
        if self.earlier {
            actions.push((Action::Latest, "Latest messages"));
        }
        actions.push((
            Action::ToggleExecution,
            if self.show_execution {
                "Hide execution panel (Ctrl+E)"
            } else {
                "Show execution panel (Ctrl+E)"
            },
        ));
        actions.push((Action::Quit, "Quit"));
        actions
    }
    fn act(&mut self, action: Action, controller: &ChatController) -> bool {
        self.mode = Mode::Input;
        match action {
            Action::New | Action::Switch
                if controller.busy()
                    || self.pending.is_some()
                    || self.active.is_some()
                    || self.retained =>
            {
                self.notice =
                    "Cancel the current turn or recover its reply before switching".into();
            }
            Action::New => self.switch(None),
            Action::Switch => {
                self.sessions = None;
                self.sessions_after = None;
                self.selected = 0;
                if self.send(controller, ChatCommand::ListSessions { after: None }) {
                    self.mode = Mode::Sessions;
                }
            }
            Action::Retry => {
                if let Some(turn_id) = self.retry_turn()
                    && self.send(controller, ChatCommand::Retry { turn_id })
                {
                    self.pending = Some(None);
                    self.observation = None;
                    self.notice = "Starting retry...".into();
                }
            }
            Action::Save => {
                self.send(controller, ChatCommand::Save);
            }
            Action::Discard => {
                self.send(controller, ChatCommand::Abandon);
            }
            Action::Inspect => self.mode = Mode::Inspect,
            Action::Earlier => {
                if let Some(first) = self.page.as_ref().and_then(|p| p.messages.first()) {
                    let before = first.message.sequence;
                    self.earlier = true;
                    self.load(controller, Some(before));
                }
            }
            Action::Latest => {
                self.earlier = false;
                self.load(controller, None);
            }
            Action::ToggleExecution => self.show_execution = !self.show_execution,
            Action::Quit => return true,
        }
        false
    }
    fn submit(&mut self, controller: &ChatController) {
        if self.editor.text.trim().is_empty() {
            return;
        }
        if self.send(
            controller,
            ChatCommand::Submit {
                session_id: self.session.as_ref().map(|s| s.id.clone()),
                prompt: self.editor.text.clone(),
            },
        ) {
            self.pending = Some(Some(self.editor.revision));
            self.observation = None;
            self.mode = Mode::Input;
            self.notice = "Starting...".into();
            self.earlier = false;
        }
    }
    fn key(&mut self, key: KeyEvent, controller: &ChatController) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => return true,
                KeyCode::Char('c') => {
                    controller.cancel();
                    self.notice = "Cancellation requested".into();
                    return false;
                }
                KeyCode::Char('p') => {
                    self.mode = Mode::Commands;
                    self.selected = 0;
                    return false;
                }
                KeyCode::Char('n') => return self.act(Action::New, controller),
                KeyCode::Char('s') => return self.act(Action::Switch, controller),
                KeyCode::Char('e') => {
                    self.show_execution = !self.show_execution;
                    return false;
                }
                KeyCode::Char('u') if self.mode == Mode::Input => {
                    self.editor.text.clear();
                    self.editor.cursor = 0;
                    self.editor.revision = self.editor.revision.wrapping_add(1);
                    return false;
                }
                _ => {}
            }
        }
        match self.mode {
            Mode::Inspect => {
                if key.code == KeyCode::Esc {
                    if !self.inspector.dismiss_overlay() {
                        self.mode = Mode::Input;
                    }
                } else if let Some(run) = &self.observation {
                    self.inspector.handle_key(key.code, run);
                }
            }
            Mode::Commands | Mode::Sessions => {
                let count = if self.mode == Mode::Commands {
                    self.actions().len()
                } else {
                    self.session_rows().len()
                };
                match key.code {
                    KeyCode::Esc => self.mode = Mode::Input,
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.selected = self.selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.selected = (self.selected + 1).min(count.saturating_sub(1))
                    }
                    KeyCode::Enter if self.mode == Mode::Commands => {
                        if let Some((action, _)) = self.actions().get(self.selected) {
                            return self.act(*action, controller);
                        }
                    }
                    KeyCode::Enter => self.choose_session(controller),
                    _ => {}
                }
            }
            Mode::Input => match key.code {
                KeyCode::Esc => self.mode = Mode::Browse,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.submit(controller)
                }
                KeyCode::Enter => self.editor.insert("\n"),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                _ => self.editor.key(key),
            },
            Mode::Browse => match key.code {
                KeyCode::Enter => self.submit(controller),
                KeyCode::Char('i') | KeyCode::Esc => self.mode = Mode::Input,
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
                _ => {}
            },
        }
        false
    }
    fn session_rows(&self) -> Vec<String> {
        let Some(page) = &self.sessions else {
            return vec!["Loading...".into()];
        };
        let mut rows: Vec<_> = page
            .sessions
            .iter()
            .map(|session| safe(&session.title).replace('\n', " "))
            .collect();
        if page.next_after.is_some() {
            rows.push("More chats...".into());
        }
        if self.sessions_after.is_some() {
            rows.push("First page".into());
        }
        if rows.is_empty() {
            rows.push("No saved chats yet".into());
        }
        rows
    }
    fn choose_session(&mut self, controller: &ChatController) {
        let Some(page) = &self.sessions else {
            return;
        };
        if let Some(session) = page.sessions.get(self.selected).cloned() {
            self.switch(Some(session));
            self.load(controller, None);
        } else {
            let next = if self.selected == page.sessions.len() {
                page.next_after.clone()
            } else {
                None
            };
            self.sessions_after = next.clone();
            self.sessions = None;
            self.send(controller, ChatCommand::ListSessions { after: next });
        }
    }
    fn draw(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Box<dyn Write>>>,
    ) -> Result<(), ChatRunError> {
        if self.mode == Mode::Inspect {
            if let Some(observation) = &self.observation {
                return self
                    .inspector
                    .draw(terminal, observation)
                    .map_err(Into::into);
            }
            self.mode = Mode::Input;
        }
        terminal.draw(|frame| self.render(frame)).context(IoSnafu {
            operation: "draw chat",
        })?;
        Ok(())
    }
    fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let input = self.editor.display(self.mode == Mode::Input);
        let input_lines = Paragraph::new(input.clone())
            .wrap(Wrap { trim: false })
            .line_count(area.width.saturating_sub(2));
        let input_height = (input_lines as u16)
            .saturating_add(2)
            .clamp(3, (area.height / 3).max(3));
        let [header, body, input_area, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ])
        .areas(area);
        let title = self
            .session
            .as_ref()
            .map_or("New chat", |s| s.title.as_str());
        frame.render_widget(
            Paragraph::new(format!("mf chat  /  {}", safe(title)))
                .style(Style::default().fg(Color::DarkGray)),
            header,
        );
        let body = if self.show_execution
            && (self.pending.is_some() || self.active.is_some() || self.observation.is_some())
        {
            let (chat, panel) = execution_areas(body);
            let inner = if panel.height > 1 {
                let block = Block::bordered().title(" Flow execution ");
                let inner = block.inner(panel);
                frame.render_widget(block, panel);
                inner
            } else {
                panel
            };
            if let Some(observation) = &self.observation {
                self.inspector.render_execution(frame, inner, observation);
            } else {
                frame.render_widget(Paragraph::new("Flow: Starting..."), inner);
            }
            chat
        } else {
            body
        };
        let mut lines = vec![];
        if let Some(page) = &self.page {
            for record in &page.messages {
                let label = if record.message.role == "assistant" {
                    "Assistant"
                } else {
                    "You"
                };
                lines.push(Line::styled(
                    label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
                for line in safe(&record.message.content).split('\n') {
                    lines.push(Line::raw(line.to_owned()));
                }
                if record.message.role == "assistant" {
                    let calls = crate::performance::conversation_calls(
                        record.usages.as_deref().unwrap_or_default(),
                    );
                    lines.push(Line::styled(
                        crate::performance::footer(&calls, body.width),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                if matches!(
                    record.turn_state.as_str(),
                    "failed" | "cancelled" | "interrupted"
                ) {
                    lines.push(Line::styled(
                        format!("[{}]", record.turn_state),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                lines.push(Line::raw(""));
            }
        }
        if let Some((_, prompt)) = &self.active {
            lines.push(Line::styled("You", Style::default().fg(Color::Cyan)));
            for line in safe(prompt).split('\n') {
                lines.push(Line::raw(line.to_owned()));
            }
            lines.push(Line::styled(
                "Working...",
                Style::default().fg(Color::DarkGray),
            ));
        }
        for (index, segment) in self.preview.segments.iter().enumerate() {
            if segment.text.is_empty() {
                continue;
            }
            let state = if self.preview.incomplete {
                "incomplete"
            } else {
                "provisional"
            };
            lines.push(Line::styled(
                format!("Assistant [{state}, segment {}]", index + 1),
                Style::default().fg(Color::Yellow),
            ));
            for line in safe(&segment.text).split('\n') {
                lines.push(Line::raw(line.to_owned()));
            }
            lines.push(Line::raw(""));
        }
        if self.active.is_some()
            && self
                .preview
                .segments
                .iter()
                .any(|segment| !segment.text.is_empty())
        {
            lines.push(Line::styled(
                "FTT / Gen / E2E: Measuring...",
                Style::default().fg(Color::DarkGray),
            ));
        }
        if self.preview.omitted {
            lines.push(Line::styled(
                "[Preview limit reached; remaining output is still being received]",
                Style::default().fg(Color::Yellow),
            ));
        }
        if let Some(reply) = &self.final_reply {
            lines.push(Line::styled("Assistant", Style::default().fg(Color::Cyan)));
            for line in safe(reply).split('\n') {
                lines.push(Line::raw(line.to_owned()));
            }
        }
        if let Some(reply) = &self.unsaved {
            lines.push(Line::styled(
                "Assistant [not saved]",
                Style::default().fg(Color::Yellow),
            ));
            for line in safe(reply).split('\n') {
                lines.push(Line::raw(line.to_owned()));
            }
        }
        if self.final_reply.is_some() || self.unsaved.is_some() {
            let calls = crate::performance::conversation_calls(&self.reply_usages);
            lines.push(Line::styled(
                crate::performance::footer(&calls, body.width),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if !self.outcome.is_empty() {
            for line in safe(&self.outcome).split('\n') {
                lines.push(Line::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        if lines.is_empty() {
            lines.push(Line::styled(
                "What would you like to work on?",
                Style::default().fg(Color::DarkGray),
            ));
        }
        let transcript = Paragraph::new(lines).wrap(Wrap { trim: false });
        let tail = transcript
            .line_count(body.width)
            .saturating_sub(body.height as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(
            transcript.scroll((tail.saturating_sub(self.scroll), 0)),
            body,
        );
        let hint = match self.mode {
            Mode::Input => " Alt+Enter send | Enter newline | Esc browse ",
            Mode::Browse => " Enter send | i edit | arrows scroll ",
            _ => "",
        };
        let editor = Paragraph::new(input).wrap(Wrap { trim: false });
        let offset = input_lines
            .saturating_sub(input_area.height.saturating_sub(2) as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(
            editor.scroll((offset, 0)).block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .title(hint),
            ),
            input_area,
        );
        let hints = if self.active.is_some() || self.pending.is_some() {
            "Ctrl+C cancel | Ctrl+E flow | Ctrl+P commands"
        } else if self.retained {
            "Ctrl+P recover reply | Ctrl+E flow | Ctrl+Q quit"
        } else {
            "Ctrl+P commands | Ctrl+E flow | Ctrl+S chats | Ctrl+Q quit"
        };
        frame.render_widget(
            Paragraph::new(format!("{}\n{hints}", safe(&self.notice)))
                .style(Style::default().fg(Color::DarkGray)),
            footer,
        );
        if matches!(self.mode, Mode::Commands | Mode::Sessions) {
            let (title, rows) = if self.mode == Mode::Commands {
                (
                    "Commands",
                    self.actions()
                        .iter()
                        .map(|(_, label)| (*label).to_string())
                        .collect::<Vec<_>>(),
                )
            } else {
                ("Switch chat", self.session_rows())
            };
            let width = area.width.saturating_sub(4).min(64);
            let height = (rows.len() as u16)
                .saturating_add(3)
                .min(area.height.saturating_sub(2));
            let popup = Rect::new(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            );
            frame.render_widget(Clear, popup);
            let list = List::new(rows)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(title)
                        .title_bottom(" Up/Down select | Enter open | Esc close "),
                )
                .highlight_symbol("› ")
                .highlight_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            frame.render_stateful_widget(
                list,
                popup,
                &mut ListState::default().with_selected(Some(self.selected)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

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
    fn render(view: &ChatView, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn metrics_follow_saved_unsaved_and_reloaded_replies() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let begun = controller
            ._database
            .begin_turn(
                &session.id,
                "question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                1024,
            )
            .unwrap();
        let calls: Vec<mf_chat::CallUsage> = serde_json::from_value(serde_json::json!([
            {"call_id":"answer", "model":"fixture", "timing":{"elapsed_us":2000000,"first_token_us":500000},"usage":{"output_tokens":100}},
            {"call_id":"title", "model":"fixture", "timing":{"elapsed_us":10000,"first_token_us":1000},"usage":{"output_tokens":100},"attribution":{"purpose":"title"}}
        ])).unwrap();
        let response = mf_chat::ConversationResult {
            text: "canonical answer".into(),
            usages: calls,
            ..Default::default()
        };
        view.session = Some(session.clone());
        view.event(
            ChatEvent::Finished {
                invocation: begun.invocation.clone(),
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(mf_chat::UncommittedCompletion {
                        completion: mf_chat::Completion {
                            invocation: begun.invocation.clone(),
                            result: response.clone(),
                        },
                        database_path: controller._database.path().to_owned(),
                    }),
                    diagnostics: vec![],
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        let text = render(&view, 100, 30);
        assert!(text.contains("[not saved]"));
        assert!(text.contains("FTT 500ms · Gen 66.7 | E2E 50.0 tok/s"));
        assert!(!text.contains("Avg FTT"));
        let completion = controller
            ._database
            .complete_turn(begun.invocation.clone(), response)
            .unwrap();
        view.event(
            ChatEvent::Finished {
                invocation: begun.invocation,
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::Committed(completion),
                    diagnostics: vec![],
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        assert!(render(&view, 100, 30).contains("FTT 500ms · Gen 66.7 | E2E 50.0 tok/s"));
        let page = controller
            ._database
            .messages(&session.id, 50, None)
            .unwrap();
        view.event(ChatEvent::Loaded { session, page }, &controller);
        assert!(view.final_reply.is_none());
        let text = render(&view, 100, 30);
        assert_eq!(text.matches("FTT 500ms").count(), 1);
        assert!(text.contains("E2E 50.0 tok/s"));
        assert!(!render(&view, 45, 30).contains("E2E"));
        controller.close().unwrap();
    }

    #[test]
    fn connection_shortcut_does_not_intercept_chat_prompt_editing() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &controller,
        );
        assert_eq!(view.editor.text, "e");
        controller.close().unwrap();
    }

    #[test]
    fn execution_panel_adapts_to_resizing_and_preserves_the_editor() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        view.editor.insert("Draft for the next turn");
        assert!(!render(&view, 180, 40).contains("Flow execution"));
        view.pending = Some(None);
        assert!(render(&view, 180, 40).contains("Flow: Starting..."));
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 0.1"]);
        let mut run = LocalRun::start(
            command,
            launch.manifest.description().unwrap(),
            mf_telemetry::identity::RunId::new(),
            1024,
        )
        .unwrap();
        view.observation = Some(run.observation());
        for (width, height, side_by_side) in [(180, 40, true), (80, 32, false)] {
            let text = render(&view, width, height);
            let rows: Vec<_> = text.lines().collect();
            let panel = rows
                .iter()
                .position(|row| row.contains("Flow execution"))
                .unwrap();
            let chat = rows
                .iter()
                .position(|row| row.contains("What would you like"))
                .unwrap();
            assert_eq!(panel == chat, side_by_side, "{text}");
            assert!(text.contains("Flow: Running"), "{text}");
            assert!(text.contains("start"), "{text}");
            assert!(text.contains("Observation: Collecting"), "{text}");
            assert!(text.contains("Draft for the next turn"), "{text}");
        }
        let compact = render(&view, 60, 18);
        assert!(compact.contains("Flow: Running"), "{compact}");
        assert!(!compact.contains("Flow execution"), "{compact}");
        assert!(compact.contains("Draft for the next turn"), "{compact}");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while run.poll().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        view.pending = None;
        let finished = render(&view, 180, 40);
        assert!(finished.contains("Flow: Exited 0"), "{finished}");
        assert!(finished.contains("UnverifiedTail"), "{finished}");
        for (width, height) in [(1, 1), (20, 6), (180, 10)] {
            render(&view, width, height);
        }
        view.switch(None);
        assert!(!render(&view, 180, 40).contains("Flow execution"));
    }

    #[test]
    fn execution_panel_toggle_preserves_observation_drafts_and_preferences() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        assert!(view.show_execution);
        view.editor.insert("first prompt");
        view.submit(&controller);
        pump(&mut view, &controller, |view| view.observation.is_some());
        let invocation = view.active.as_ref().unwrap().0.call_id.clone();
        view.editor.insert("next draft");
        view.mode = Mode::Browse;
        let toggle = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        view.key(toggle, &controller);
        assert!(!view.show_execution);
        assert!(view.mode == Mode::Browse);
        for (width, height) in [(180, 40), (80, 32), (60, 18)] {
            let text = render(&view, width, height);
            assert!(
                !text.contains("Flow execution") && !text.contains("Flow:"),
                "{text}"
            );
            assert!(text.contains("next draft"), "{text}");
        }
        view.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &controller,
        );
        view.selected = view
            .actions()
            .iter()
            .position(|(action, label)| {
                *action == Action::ToggleExecution && label.starts_with("Show")
            })
            .unwrap();
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        assert!(view.show_execution);
        assert!(render(&view, 180, 40).contains("Flow: Running"));
        assert!(controller.busy());
        assert_eq!(view.active.as_ref().unwrap().0.call_id, invocation);
        assert_eq!(view.editor.text, "next draft");
        assert!(view.actions().iter().any(|(action, label)| {
            *action == Action::ToggleExecution && label.starts_with("Hide")
        }));
        view.key(toggle, &controller);
        controller.cancel();
        pump(&mut view, &controller, |view| {
            view.active.is_none() && view.page.is_some()
        });
        assert!(view.observation.is_some());
        assert!(!render(&view, 180, 40).contains("Flow execution"));
        view.act(Action::New, &controller);
        assert!(!view.show_execution);
        view.editor.insert("another turn");
        view.submit(&controller);
        pump(&mut view, &controller, |view| view.observation.is_some());
        assert!(!view.show_execution);
        view.key(toggle, &controller);
        assert!(render(&view, 180, 40).contains("Flow: Running"));
        controller.close().unwrap();
    }

    #[test]
    fn multiline_submission_preserves_new_edits_and_blocks_switching_until_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.submit(&controller);
        assert!(!controller.busy());
        view.editor.insert("hello 🦀");
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        assert_eq!(view.editor.text, "hello 🦀\n");
        assert!(!controller.busy());
        view.editor.insert("second line");
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &controller,
        );
        view.editor.insert(" + new draft");
        pump(&mut view, &controller, |view| view.active.is_some());
        assert!(view.editor.text.ends_with(" + new draft"));
        view.submit(&controller);
        assert!(view.notice.contains("current turn"));
        let session = view.session.as_ref().unwrap().id.clone();
        view.key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &controller,
        );
        assert_eq!(view.session.as_ref().unwrap().id, session);
        assert!(view.notice.contains("before switching"));
        view.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &controller,
        );
        view.selected = view
            .actions()
            .iter()
            .position(|(action, _)| *action == Action::Quit)
            .unwrap();
        view.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &controller,
        );
        pump(&mut view, &controller, |view| {
            view.active.is_none() && view.page.is_some()
        });
        assert!(matches!(view.actions()[view.selected].0, Action::Quit));
        view.key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.session.is_none());
        view.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &controller,
        );
        pump(&mut view, &controller, |view| view.sessions.is_some());
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        pump(&mut view, &controller, |view| view.page.is_some());
        assert_eq!(view.session.as_ref().unwrap().id, session);
        assert!(view.editor.text.ends_with(" + new draft"));
        controller.close().unwrap();
    }

    #[test]
    fn conversation_and_commands_remain_readable_without_management_panels() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let run = LocalRun::start(
            Command::new(&launch.executable),
            launch.manifest.description().unwrap(),
            mf_telemetry::identity::RunId::new(),
            1024,
        )
        .unwrap();
        let observation = run.observation();
        drop(run);
        assert_eq!(
            observation.reader.snapshot().lifecycle.completeness,
            crate::state::Completeness::UnverifiedTail
        );
        view.observation = Some(observation);
        let database = ChatDatabase::open(&launch.options.database_path).unwrap();
        let session = database
            .create_session("default", None, Some("Review a workflow".into()))
            .unwrap();
        let started = database
            .begin_turn(
                &session.id,
                "How should we simplify this workflow?",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
                4096,
            )
            .unwrap();
        database.complete_turn(started.invocation, mf_chat::ConversationResult {
            text: "Keep one execution lane and move infrequent actions into the command palette.".into(),
            metadata: None, usages: vec![], finish_reason: None,
        ..Default::default()
}).unwrap();
        view.page = Some(database.messages(&session.id, 50, None).unwrap());
        view.session = Some(session);
        view.notice.clear();
        view.editor
            .insert("Show me the next step.\nKeep the changes focused.");
        let chat = render(&view, 100, 30);
        assert!(chat.contains("Alt+Enter send"));
        assert!(chat.contains("How should we simplify"));
        assert!(!chat.contains("Automatic titles"));
        assert!(!chat.contains("Transcript / outcomes / usage"));
        assert!(!chat.contains(&view.session.as_ref().unwrap().id));
        view.mode = Mode::Commands;
        let commands = render(&view, 100, 30);
        assert!(commands.contains("New chat") && commands.contains("Switch chat"));
        assert!(!commands.contains("Archive") && !commands.contains("Usage"));
        let compact = render(&view, 60, 18);
        assert!(compact.contains("Switch chat"));
        if let Some(directory) = std::env::var_os("MF_CHAT_PREVIEW_DIR") {
            let directory = PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            for (name, text) in [
                ("chat.txt", chat),
                ("commands.txt", commands),
                ("compact.txt", compact),
            ] {
                std::fs::write(directory.join(name), text).unwrap();
            }
        }
    }

    #[test]
    fn unicode_editing_and_clearing_keep_admission_revisions_distinct() {
        let mut editor = Editor::default();
        editor.insert("A🦀e\u{301}\nB");
        editor.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(editor.cursor, "A🦀e\u{301}\n".len());
        editor.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        editor.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.text, "A🦀e\nB");
        editor.cursor = 1;
        editor.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(editor.cursor, "A🦀".len());
        editor.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        editor.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        editor.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(editor.text, "A🦀\nB");
        let before = editor.text.clone();
        editor.insert(&"x".repeat(MAX_EDITOR_BYTES));
        assert_eq!(editor.text, before);
        editor.insert("\x1b[31m");
        assert!(!safe(&editor.text).contains('\x1b'));
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.editor.insert("submitted");
        let submitted = view.editor.revision;
        view.key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &controller,
        );
        view.editor.insert("replacement");
        assert_ne!(view.editor.revision, submitted);
        controller.close().unwrap();
    }

    fn choose(view: &mut ChatView, controller: &ChatController, action: Action) -> bool {
        view.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            controller,
        );
        view.selected = view
            .actions()
            .iter()
            .position(|(candidate, _)| *candidate == action)
            .unwrap();
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            controller,
        )
    }

    #[test]
    fn session_and_history_navigation_preserve_drafts_and_exclude_stale_retries() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.act(Action::Switch, &controller);
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        pump(&mut view, &controller, |v| v.sessions.is_some());
        assert!(
            view.session_rows()
                .iter()
                .any(|row| row == "No saved chats yet")
        );
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        let database = &controller._database;
        let first = database
            .create_session("default", None, Some("History".into()))
            .unwrap();
        for index in 0..55 {
            database
                .create_session("default", None, Some(format!("chat-{index}")))
                .unwrap();
        }
        for index in 0..28 {
            let started = database
                .begin_turn(
                    &first.id,
                    &format!("prompt-{index}"),
                    mf_chat::ModelIdentity {
                        profile: "default".into(),
                        model: None,
                    },
                    None,
                    8192,
                )
                .unwrap();
            if index == 0 {
                database
                    .fail_turn(started.invocation, "failed", "old failure")
                    .unwrap();
            } else {
                database
                    .complete_turn(
                        started.invocation,
                        mf_chat::ConversationResult {
                            text: format!("reply-{index}"),
                            metadata: None,
                            usages: vec![],
                            finish_reason: None,
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
        }
        view.editor.insert("new chat draft");
        choose(&mut view, &controller, Action::Switch);
        pump(&mut view, &controller, |v| v.sessions.is_some());
        assert!(view.session_rows().iter().any(|row| row == "More chats..."));
        view.selected = view.sessions.as_ref().unwrap().sessions.len();
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        pump(&mut view, &controller, |v| v.sessions.is_some());
        assert!(view.session_rows().iter().any(|row| row == "First page"));
        view.selected = view.session_rows().len() - 1;
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        pump(&mut view, &controller, |v| v.sessions.is_some());
        view.selected = view
            .sessions
            .as_ref()
            .unwrap()
            .sessions
            .iter()
            .position(|s| s.id == first.id)
            .unwrap();
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        pump(&mut view, &controller, |v| v.page.is_some());
        assert_eq!(
            view.page
                .as_ref()
                .unwrap()
                .messages
                .last()
                .unwrap()
                .message
                .content,
            "reply-27"
        );
        assert!(view.retry_turn().is_none());
        choose(&mut view, &controller, Action::Earlier);
        pump(&mut view, &controller, |v| {
            v.page.as_ref().unwrap().messages[0].message.sequence == 0
        });
        assert!(view.earlier && view.retry_turn().is_none());
        choose(&mut view, &controller, Action::Latest);
        pump(&mut view, &controller, |v| {
            v.page
                .as_ref()
                .unwrap()
                .messages
                .last()
                .unwrap()
                .message
                .content
                == "reply-27"
        });
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        for key in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            view.key(KeyEvent::new(key, KeyModifiers::NONE), &controller);
        }
        assert_eq!(view.scroll, 0);
        assert!(render(&view, 80, 24).contains("Enter send"));
        view.key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
            &controller,
        );
        for key in [KeyCode::PageUp, KeyCode::PageDown] {
            view.key(KeyEvent::new(key, KeyModifiers::NONE), &controller);
        }
        choose(&mut view, &controller, Action::New);
        assert_eq!(view.editor.text, "new chat draft");
        view.key(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &controller,
        );
        view.key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &controller,
        );
        view.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &controller);
        assert_eq!(view.selected, 0);
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        assert!(choose(&mut view, &controller, Action::Quit));
        controller.close().unwrap();
    }

    #[test]
    fn failed_launch_restores_the_prompt_and_query_errors_do_not_clear_pending_admission() {
        let root = tempfile::tempdir().unwrap();
        let mut launch = super::super::tests::launch(root.path());
        std::fs::remove_file(&launch.executable).unwrap();
        launch.options.run.verbose = 1;
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        controller
            .send(ChatCommand::Load {
                session_id: "missing".into(),
                before: None,
            })
            .unwrap();
        view.editor.insert("keep this prompt");
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        let event = super::super::tests::next(&controller);
        assert!(matches!(
            &event,
            ChatEvent::Error {
                admission: false,
                ..
            }
        ));
        view.event(event, &controller);
        assert!(view.pending.is_some());
        pump(&mut view, &controller, |v| {
            v.notice.contains("could not launch") && v.page.is_some()
        });
        assert_eq!(view.editor.text, "keep this prompt");
        assert!(view.retry_turn().is_some());
        assert!(!controller.busy());
        controller
            .send(ChatCommand::Submit {
                session_id: None,
                prompt: "  ".into(),
            })
            .unwrap();
        pump(&mut view, &controller, |v| {
            v.notice.contains("prompt must not be empty")
        });
        assert_eq!(
            controller
                ._database
                .list_sessions(false, 50, None)
                .unwrap()
                .sessions
                .len(),
            1
        );
        assert_eq!(view.editor.text, "keep this prompt");
        controller.close().unwrap();
    }

    #[test]
    fn raw_result_recovery_keeps_discard_available_after_storage_queries_fail() {
        let root = tempfile::tempdir().unwrap();
        let mut launch = super::super::tests::launch(root.path());
        let state = root.path().join("state");
        let offline = root.path().join("offline");
        launch.options.database_path = state.join("chat.sqlite");
        let path = launch.options.database_path.clone();
        std::fs::write(&launch.executable, "#!/bin/sh\nsleep 0.3\nprintf '%s' '{\"status\":\"succeeded\",\"text\":\"paid reply\",\"usages\":[]}'\necho 'runner diagnostic' >&2\nkill -TERM $$\n").unwrap();
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.editor.insert("question");
        view.submit(&controller);
        pump(&mut view, &controller, |v| v.observation.is_some());
        std::fs::rename(&state, &offline).unwrap();
        std::fs::write(&state, "unavailable").unwrap();
        pump(&mut view, &controller, |v| v.retained);
        assert!(controller.busy());
        assert!(view.outcome.contains("runner diagnostic"));
        assert!(render(&view, 80, 24).contains("recover reply"));
        choose(&mut view, &controller, Action::New);
        assert!(view.session.is_some());
        std::fs::remove_file(&state).unwrap();
        std::fs::rename(&offline, &state).unwrap();
        choose(&mut view, &controller, Action::Discard);
        pump(&mut view, &controller, |v| {
            !v.retained && v.notice.contains("cancelled")
        });
        let session = view.session.as_ref().unwrap().id.clone();
        controller.close().unwrap();
        let database = ChatDatabase::open(path).unwrap();
        let messages = database.messages(&session, 50, None).unwrap().messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].turn_state, "cancelled");
    }
    #[test]
    fn canonical_results_replace_previews_and_stale_turn_events_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let invocation = Invocation::new(&session.id);
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: invocation.clone(),
                prompt: "hello".into(),
            },
            &controller,
        );
        let mut preview = LivePreview::default();
        preview.begin(invocation.clone());
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    text: "draft prefix".into(),
                    model_call_id: "model-call".into(),
                },
            })
            .unwrap();
        view.event(
            ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.clone(),
            },
            &controller,
        );
        assert!(render(&view, 100, 30).contains("draft prefix"));
        let finish = |invocation: Invocation, text: &str| ChatEvent::Finished {
            invocation: invocation.clone(),
            resolution: Ok(ReconciledCompletion {
                state: CompletionState::Committed(mf_chat::Completion {
                    invocation,
                    result: mf_chat::ConversationResult {
                        text: text.into(),
                        ..Default::default()
                    },
                }),
                diagnostics: vec![],
            }),
            status: None,
            stderr: String::new(),
            capture_error: None,
        };
        view.event(finish(invocation.clone(), "canonical reply"), &controller);
        let screen = render(&view, 100, 30);
        assert!(screen.contains("canonical reply"));
        assert!(!screen.contains("draft prefix"));
        let next = Invocation::new(&session.id);
        view.event(
            ChatEvent::Submitted {
                session,
                invocation: next.clone(),
                prompt: "next".into(),
            },
            &controller,
        );
        view.event(
            ChatEvent::Preview {
                invocation: invocation.clone(),
                preview,
            },
            &controller,
        );
        view.event(finish(invocation, "stale final"), &controller);
        assert_eq!(view.active.as_ref().unwrap().0, next);
        assert!(!render(&view, 100, 30).contains("stale final"));
        controller.close().unwrap();
    }

    #[test]
    fn failed_prefixes_are_marked_incomplete_and_full_recovery_replies_replace_them() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(&launch.manifest.description().unwrap()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let invocation = Invocation::new(&session.id);
        view.event(
            ChatEvent::Submitted {
                session,
                invocation: invocation.clone(),
                prompt: "hello".into(),
            },
            &controller,
        );
        view.preview.begin(invocation.clone());
        view.preview
            .segments
            .push(super::super::preview::PreviewSegment {
                model_call_id: "model-call".into(),
                text: "partial text".into(),
            });
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::NotBegun,
                    diagnostics: vec![],
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("partial text"));
        assert!(screen.contains("incomplete"));
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(UncommittedCompletion {
                        database_path: controller._database.path().to_owned(),
                        completion: mf_chat::Completion {
                            invocation,
                            result: mf_chat::ConversationResult {
                                text: "complete unsaved reply".into(),
                                ..Default::default()
                            },
                        },
                    }),
                    diagnostics: vec![],
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("complete unsaved reply"));
        assert!(screen.contains("not saved"));
        assert!(!screen.contains("partial text"));
        assert!(view.retained);
        controller.close().unwrap();
    }
}
