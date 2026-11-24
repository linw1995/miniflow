use super::{transcript::Format, *};
use crate::run::{ObservationView, SignalGuard, TerminalGuard, terminal_area};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
};
use mf_chat::display::{Message as DisplayMessage, PreviewSegment, Text};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListState, Paragraph, Wrap},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    io::{IsTerminal, Write},
    time::Instant,
};
const MAX_EDITOR_BYTES: usize = 256 * 1024;
const LOADING_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// A role header, optionally followed by a one-cell activity indicator.
///
/// The indicator keeps its single separating space so the header keeps its width
/// when the turn settles and the spinner disappears.
fn role_label(text: &str, color: Color, indicator: Option<char>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        safe(text).replace('\n', "\\n"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    if let Some(frame) = indicator {
        spans.push(Span::styled(
            format!(" {frame}"),
            Style::default().fg(Color::Cyan),
        ));
    }
    Line::from(spans)
}

struct MessageGroup<'a> {
    index: usize,
    message: &'a DisplayMessage,
    results: BTreeMap<&'a str, (&'a Text, usize)>,
}

impl MessageGroup<'_> {
    fn tool_ids(&self) -> impl Iterator<Item = &str> {
        let (calls, standalone) = match self.message {
            DisplayMessage::Assistant { tools, .. } => (tools.as_slice(), None),
            DisplayMessage::ToolResult { id, .. } => (&[][..], Some(id.as_str())),
            _ => (&[][..], None),
        };
        calls.iter().map(|call| call.id.as_str()).chain(standalone)
    }
}

// Pair only visible results in the same assistant batch. A history page can cut
// through a batch, so unmatched calls and results remain independently visible.
fn message_groups<'a>(
    messages: impl Iterator<Item = &'a DisplayMessage>,
) -> impl Iterator<Item = MessageGroup<'a>> {
    let mut messages = messages.enumerate().peekable();
    std::iter::from_fn(move || {
        let (index, message) = messages.next()?;
        let mut results = BTreeMap::new();
        if let DisplayMessage::Assistant { tools, .. } = message {
            let mut pending: BTreeSet<_> = tools.iter().map(|call| call.id.as_str()).collect();
            while let Some((_, DisplayMessage::ToolResult { id, content, bytes })) =
                messages.peek().copied()
                && pending.remove(id.as_str())
            {
                results.insert(id.as_str(), (content, *bytes));
                messages.next();
            }
        }
        Some(MessageGroup {
            index,
            message,
            results,
        })
    })
}

fn page_turns(page: &display::Page) -> impl Iterator<Item = &[display::Record]> {
    page.messages
        .chunk_by(|left, right| left.turn_id == right.turn_id)
}

fn group_gap(transcript: &mut super::transcript::Transcript, key: &str) {
    transcript.plain(format!("{key}:gap"), vec![Line::default()]);
}

fn render_tools<'a>(
    transcript: &mut super::transcript::Transcript,
    key: &str,
    tools: impl IntoIterator<Item = ToolGroup<'a>>,
    expanded: &BTreeSet<String>,
    focused: Option<&str>,
) {
    for (index, tool) in tools.into_iter().enumerate() {
        if index > 0 {
            group_gap(transcript, &format!("{key}:call:{index}"));
        }
        tool.render(transcript, expanded, focused);
    }
}

fn render_assistant<'a>(
    transcript: &mut super::transcript::Transcript,
    key: &str,
    text: Option<&Text>,
    tools: impl IntoIterator<Item = ToolGroup<'a>>,
    indicator: Option<char>,
    expanded: &BTreeSet<String>,
    focused: Option<&str>,
) {
    transcript.plain(
        format!("{key}:label"),
        vec![role_label("Assistant", Color::Cyan, indicator)],
    );
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        transcript.deferred(format!("{key}:body"), text, Format::Markdown);
    }
    render_tools(transcript, key, tools, expanded, focused);
}

fn render_message_group(
    transcript: &mut super::transcript::Transcript,
    key: &str,
    group: &MessageGroup<'_>,
    turn: &str,
    executions: &BTreeMap<String, mf_chat::ToolExecution>,
    expanded: &BTreeSet<String>,
    focused: Option<&str>,
) {
    if group.index > 0 {
        group_gap(transcript, key);
    }
    let (label, content) = match group.message {
        DisplayMessage::User(content) => ("You", content),
        DisplayMessage::System(content) => ("System", content),
        DisplayMessage::Assistant { content, tools } => {
            let tools = tools.iter().map(|call| ToolGroup {
                key: tool_key(turn, &call.id),
                title: tool_call_label(Some(&call.name), &call.id),
                arguments: Some(&call.arguments),
                result: group.results.get(call.id.as_str()).copied(),
                execution: executions
                    .get(&call.id)
                    .filter(|execution| execution.name == call.name),
            });
            render_assistant(
                transcript,
                key,
                Some(content),
                tools,
                None,
                expanded,
                focused,
            );
            return;
        }
        DisplayMessage::ToolResult { id, content, bytes } => {
            ToolGroup::standalone_result(turn, id, (content, *bytes), executions.get(id))
                .render(transcript, expanded, focused);
            return;
        }
    };
    transcript.plain(
        format!("{key}:label"),
        vec![role_label(label, Color::Cyan, None)],
    );
    transcript.deferred(format!("{key}:body"), content, Format::Literal);
}

fn tool_key(turn: &str, id: &str) -> String {
    // A provider ID may contain the same suffix as one of the group's body keys.
    format!("tool:{turn}:{}:{id}", id.len())
}

fn preview_tool_key(turn: &str, model: &str, index: u64) -> String {
    format!("tool-preview:{turn}:{}:{model}:{index}", model.len())
}

fn tool_call_label(name: Option<&str>, reference: &str) -> String {
    match name {
        Some(name) => format!("Tool call {name} [{reference}]"),
        None => format!("Tool call [{reference}]"),
    }
}

struct ToolGroup<'a> {
    key: String,
    title: String,
    arguments: Option<&'a Text>,
    result: Option<(&'a Text, usize)>,
    execution: Option<&'a mf_chat::ToolExecution>,
}

impl<'a> ToolGroup<'a> {
    fn standalone_result(
        turn: &str,
        id: &str,
        content: (&'a Text, usize),
        execution: Option<&'a mf_chat::ToolExecution>,
    ) -> Self {
        Self {
            key: tool_key(turn, id),
            title: format!("Tool result [{id}] ({} bytes)", content.1),
            arguments: None,
            result: Some(content),
            execution,
        }
    }

    fn render(
        self,
        transcript: &mut super::transcript::Transcript,
        expanded: &BTreeSet<String>,
        focused: Option<&str>,
    ) {
        let Self {
            key,
            title,
            arguments,
            result,
            execution,
        } = self;
        let open = expanded.contains(&key);
        let marker = if open { "[-]" } else { "[+]" };
        let label = if let Some(execution) = execution {
            format!("{marker} [{}] {title}", tool_status(execution))
        } else {
            format!("{marker} {title}")
        };
        let mut header = role_label(&label, Color::Yellow, None);
        if focused == Some(key.as_str()) {
            header = header.style(Style::default().add_modifier(Modifier::REVERSED));
        }
        transcript.plain(key.clone(), vec![header]);
        if !open {
            return;
        }
        if let Some(arguments) = arguments {
            transcript.deferred(format!("{key}:arguments"), arguments, Format::Json);
        }
        if let Some((content, bytes)) = result {
            if arguments.is_some() {
                transcript.plain(
                    format!("{key}:result:label"),
                    vec![role_label(
                        &format!("Tool result ({bytes} bytes)"),
                        Color::Yellow,
                        None,
                    )],
                );
            }
            transcript.deferred(format!("{key}:result"), content, Format::Literal);
        } else if arguments.is_none() {
            transcript.plain(
                format!("{key}:arguments"),
                vec![Line::styled(
                    "No argument preview",
                    Style::default().fg(Color::DarkGray),
                )],
            );
        }
    }
}

fn unique_preview_ids(segments: &[Arc<PreviewSegment>]) -> BTreeSet<&str> {
    let mut unique = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for id in segments
        .iter()
        .filter_map(|segment| segment.tool.as_ref()?.id.as_deref())
    {
        if !unique.insert(id) {
            repeated.insert(id);
        }
    }
    unique.retain(|id| !repeated.contains(id));
    unique
}

struct LiveMessage<'a> {
    model_call_id: &'a str,
    text: Option<&'a Text>,
    tools: Vec<ToolGroup<'a>>,
}

struct LiveMessages<'a> {
    messages: Vec<LiveMessage<'a>>,
    unmatched: Vec<ToolGroup<'a>>,
}

fn live_messages<'a>(
    turn: &str,
    segments: &'a [Arc<PreviewSegment>],
    executions: &'a BTreeMap<String, mf_chat::ToolExecution>,
) -> LiveMessages<'a> {
    let unique = unique_preview_ids(segments);
    let mut messages = Vec::new();
    let mut positions = BTreeMap::new();
    for segment in segments {
        // Preserve first appearance even when segments from different calls are interleaved.
        let model_call_id = segment.model_call_id.as_str();
        let position = *positions.entry(model_call_id).or_insert_with(|| {
            let position = messages.len();
            messages.push(LiveMessage {
                model_call_id,
                text: None,
                tools: Vec::new(),
            });
            position
        });
        let message = &mut messages[position];
        let Some(tool) = &segment.tool else {
            message.text = Some(&segment.text);
            continue;
        };
        let id = tool.id.as_deref().filter(|id| unique.contains(id));
        let execution = id.and_then(|id| executions.get(id)).filter(|execution| {
            tool.name
                .as_ref()
                .is_none_or(|name| name == &execution.name)
        });
        let name = tool
            .name
            .as_deref()
            .or_else(|| execution.map(|execution| execution.name.as_str()));
        let index = format!("index {}", tool.index);
        message.tools.push(ToolGroup {
            key: id.map_or_else(
                || preview_tool_key(turn, model_call_id, tool.index),
                |id| tool_key(turn, id),
            ),
            title: tool_call_label(name, id.unwrap_or(&index)),
            arguments: (!segment.text.is_empty()).then_some(&segment.text),
            result: None,
            execution,
        });
    }
    let unmatched = executions
        .iter()
        .filter(|(id, _)| !unique.contains(id.as_str()))
        .map(|(id, execution)| ToolGroup {
            key: tool_key(turn, id),
            title: tool_call_label(Some(&execution.name), id),
            arguments: None,
            result: None,
            execution: Some(execution),
        })
        .collect();
    LiveMessages {
        messages,
        unmatched,
    }
}

fn tool_status(execution: &mf_chat::ToolExecution) -> &'static str {
    use mf_chat::ToolExecutionPhase;
    match execution.phase {
        Some(ToolExecutionPhase::Running) => "Running",
        Some(ToolExecutionPhase::Succeeded) => "Succeeded",
        Some(ToolExecutionPhase::Failed) => "Failed",
        Some(ToolExecutionPhase::Interrupted) => "Interrupted",
        None => "Unknown",
    }
}

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
    let mut view = ChatView::new(launch.runner.description())?;
    let controller = ChatController::open(launch)?;
    view.apply_controller_state(&controller);
    let signal = SignalGuard::register().context(IoSnafu {
        operation: "register interrupt handler",
    })?;
    let guard = TerminalGuard::enter().context(IoSnafu {
        operation: "enter terminal",
    })?;
    view.transcript
        .borrow_mut()
        .enable_images(
            std::env::current_dir().context(IoSnafu {
                operation: "resolve image base directory",
            })?,
            crate::images::terminal_picker(),
        )
        .context(IoSnafu {
            operation: "start image worker",
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
                    view.transcript
                        .borrow_mut()
                        .update_image_picker(crate::images::terminal_picker());
                    terminal
                        .resize(Rect::new(0, 0, width, height))
                        .context(IoSnafu {
                            operation: "resize terminal",
                        })?;
                }
                Event::Mouse(mouse) if view.mode == Mode::Inspect => {
                    view.inspector.handle_mouse(mouse);
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
    view.transcript.borrow_mut().clear_images();
    let image_cleanup = view.transcript.borrow_mut().flush_images(&mut io::stderr());
    let _ = execute!(io::stderr(), DisableBracketedPaste);
    drop(terminal);
    drop(guard);
    cleanup?;
    image_cleanup.context(IoSnafu {
        operation: "release terminal images",
    })?;
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
    Models,
    Inspect,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    New,
    Switch,
    SelectModel,
    Retry,
    Save,
    Discard,
    Inspect,
    Earlier,
    Latest,
    ToggleExecution,
    Quit,
}

fn compaction_tokens_before(messages: &[mf_chat::ChatMessage]) -> Option<u64> {
    messages.iter().find_map(|message| {
        let mf_chat::ChatMessage::User {
            metadata: Some(metadata),
            ..
        } = message
        else {
            return None;
        };
        metadata.get("compaction")?.get("tokens_before")?.as_u64()
    })
}

/// Returns `Some(true)` if all compaction calls succeeded, `Some(false)` if any failed,
/// or `None` if no compaction call was reported.
fn compaction_call_outcome(usages: &[mf_chat::CallUsage]) -> Option<bool> {
    let mut saw_compaction = false;
    let mut failed = false;
    for usage in usages {
        if usage
            .attribution
            .as_ref()
            .and_then(|attribution| attribution.purpose.as_deref())
            == Some("compaction")
        {
            saw_compaction = true;
            if usage.error.is_some() {
                failed = true;
            }
        }
    }
    saw_compaction.then_some(!failed)
}

fn compacted_label(tokens_before: Option<u64>) -> String {
    match tokens_before {
        Some(tokens) => format!(
            "[context compacted from {} tokens]",
            format_token_count(tokens)
        ),
        None => "[context compacted]".into(),
    }
}

fn committed_notice(result: &mf_chat::ConversationResult) -> String {
    if result.status == "failed" {
        return "Failed result saved; model context unchanged".into();
    }
    match &result.update {
        mf_chat::MessageUpdate::ReplaceContext {
            context_messages, ..
        } => {
            let tokens_before = compaction_tokens_before(context_messages);
            if tokens_before.is_some() || compaction_call_outcome(&result.usages).is_some() {
                match tokens_before {
                    Some(tokens) => format!(
                        "Saved; context compacted from {} tokens",
                        format_token_count(tokens)
                    ),
                    None => "Saved; context compacted".into(),
                }
            } else {
                "Saved; model context replaced".into()
            }
        }
        mf_chat::MessageUpdate::Append { .. } => {
            if compaction_call_outcome(&result.usages) == Some(false) {
                "Saved; compaction failed".into()
            } else {
                "Saved".into()
            }
        }
    }
}

struct PresentedResult {
    messages: Vec<DisplayMessage>,
    usages: Vec<mf_chat::CallUsage>,
    status: Option<String>,
}

impl PresentedResult {
    fn status(result: &mf_chat::ConversationResult, saved: bool) -> Option<String> {
        if result.status == "failed" {
            Some(
                if saved {
                    "[failed] Model context unchanged"
                } else {
                    "[failed] [not saved] Model context unchanged"
                }
                .into(),
            )
        } else if let mf_chat::MessageUpdate::ReplaceContext {
            context_messages, ..
        } = &result.update
        {
            if !saved {
                Some("[not saved] Model context replacement pending".into())
            } else {
                let tokens_before = compaction_tokens_before(context_messages);
                if tokens_before.is_some() || compaction_call_outcome(&result.usages).is_some() {
                    Some(compacted_label(tokens_before))
                } else {
                    Some("[model context replaced]".into())
                }
            }
        } else if !saved {
            Some("[not saved] Model context update pending".into())
        } else if compaction_call_outcome(&result.usages) == Some(false) {
            Some("[compaction failed; model context unchanged]".into())
        } else {
            None
        }
    }
}

struct ChatView {
    session: Option<Session>,
    page: Option<display::Page>,
    earlier: bool,
    editor: Editor,
    drafts: BTreeMap<String, Editor>,
    mode: Mode,
    selected: usize,
    sessions: Option<SessionPage>,
    sessions_after: Option<String>,
    supports_model_selection: bool,
    active_model: Option<String>,
    active_context_window: Option<u64>,
    context_tokens: Option<u64>,
    loading_default_model: bool,
    models: Option<Vec<mf_chat::ModelInfo>>,
    models_error: Option<String>,
    model_filter: String,
    pending: Option<Option<u64>>,
    /// The turn being presented until its authoritative messages reload.
    active: Option<(Invocation, String)>,
    /// Whether that turn is still executing, separate from what is presented.
    running: bool,
    host_compacting: bool,
    retained: bool,
    reply: Option<PresentedResult>,
    preview: LivePreview,
    expanded_tools: BTreeSet<String>,
    focused_tool: Option<String>,
    latest_invocation: Option<Invocation>,
    observation: Option<LocalObservation>,
    show_execution: bool,
    inspector: ObservationView,
    scroll: Cell<usize>,
    transcript: RefCell<super::transcript::Transcript>,
    notice: String,
    outcome: String,
    animation_started: Instant,
}
fn format_token_count(tokens: u64) -> String {
    if tokens >= 10_000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

fn format_context_window(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 10_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else if tokens < 1_000_000 {
        format!("{}k", (tokens as f64 / 1_000.0).round() as u64)
    } else if tokens < 10_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        format!("{}M", (tokens as f64 / 1_000_000.0).round() as u64)
    }
}

fn format_context_usage(tokens: Option<u64>, window: u64) -> (String, Color) {
    let window_label = format_context_window(window);
    match tokens {
        Some(used) => {
            let percent = (used as f64 / window as f64) * 100.0;
            let color = if percent >= 90.0 {
                Color::Red
            } else if percent >= 70.0 {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            (format!("{percent:.1}%/{window_label}"), color)
        }
        None => (format!("?/{window_label}"), Color::DarkGray),
    }
}

fn append_diagnostic(output: &mut String, diagnostic: &str) {
    let mut seen: BTreeSet<String> = output.lines().map(str::to_owned).collect();
    for line in diagnostic
        .trim()
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        if seen.insert(line.to_owned()) {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(line);
        }
    }
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
            supports_model_selection: false,
            active_model: None,
            active_context_window: None,
            context_tokens: Some(0),
            loading_default_model: false,
            models: None,
            models_error: None,
            model_filter: String::new(),
            pending: None,
            active: None,
            running: false,
            host_compacting: false,
            retained: false,
            reply: None,
            preview: LivePreview::default(),
            expanded_tools: BTreeSet::new(),
            focused_tool: None,
            latest_invocation: None,
            observation: None,
            show_execution: true,
            inspector: ObservationView::new(description)?,
            scroll: Cell::new(0),
            transcript: RefCell::default(),
            notice: "Start a conversation, or Ctrl+S to resume one".into(),
            outcome: String::new(),
            animation_started: Instant::now(),
        })
    }
    fn apply_controller_state(&mut self, controller: &ChatController) {
        self.supports_model_selection = controller.supports_model_selection();
        self.active_model = controller.initial_model().map(str::to_owned);
        self.loading_default_model = controller.loading_default_model();
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
        self.context_tokens = self.session.is_none().then_some(0);
        self.transcript.get_mut().clear();
        self.page = None;
        self.active = None;
        self.running = false;
        self.host_compacting = false;
        self.preview = LivePreview::default();
        self.expanded_tools.clear();
        self.focused_tool = None;
        self.reply = None;
        self.retained = false;
        self.latest_invocation = None;
        self.earlier = false;
        self.observation = None;
        self.scroll.set(0);
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
            ChatEvent::Models(result) => match result {
                Ok(models) => {
                    if let Some(current) = self.active_model.as_deref()
                        && let Some(found) = models.iter().find(|model| model.id == current)
                    {
                        self.active_context_window = found
                            .context_window
                            .and_then(|value| u64::try_from(value).ok())
                            .filter(|&value| value > 0);
                    }
                    self.models_error = None;
                    self.models = Some(models);
                    let filtered = self.filtered_models();
                    self.selected = self
                        .active_model
                        .as_deref()
                        .and_then(|current| filtered.iter().position(|model| model.id == current))
                        .unwrap_or(0);
                }
                Err(error) => {
                    self.models = None;
                    self.models_error = Some(error);
                    self.selected = 0;
                }
            },
            ChatEvent::ModelSelected {
                model,
                context_window,
                automatic,
                warning,
            } => {
                self.loading_default_model = false;
                self.active_model = model.clone();
                self.active_context_window = context_window.filter(|&value| value > 0);
                if let Some(warning) = warning {
                    if let Some(model) = &model {
                        self.notice = format!(
                            "Model set to {} (preferences not saved: {})",
                            safe(model).replace('\n', " "),
                            safe(&warning).replace('\n', " ")
                        );
                    }
                } else if !automatic && let Some(model) = &model {
                    self.notice = format!("Model set to {}", safe(model).replace('\n', " "));
                }
            }
            ChatEvent::ContextWindow {
                model,
                context_window,
            } => {
                if self.active_model.as_deref() == Some(model.as_str()) {
                    self.active_context_window = context_window.filter(|&value| value > 0);
                }
            }
            ChatEvent::Loaded {
                session,
                page,
                context_tokens,
            } => {
                if self.session.as_ref().is_some_and(|s| s.id == session.id) {
                    // Keep presenting the in-flight turn until the page that carries it
                    // arrives, so previews and authoritative messages swap in one frame.
                    let settled = !self.running
                        && !self.retained
                        && self.latest_invocation.as_ref().is_some_and(|invocation| {
                            page.messages.iter().any(|record| {
                                record.turn_id == invocation.turn_id
                                    && record.turn_state != "running"
                            })
                        });
                    self.session = Some(session);
                    self.page = Some(page);
                    self.context_tokens = context_tokens;
                    if settled {
                        self.active = None;
                        self.preview = LivePreview::default();
                        self.reply = None;
                    }
                    self.scroll.set(0);
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
                self.transcript.get_mut().clear_images();
                self.session = Some(session);
                self.latest_invocation = Some(invocation.clone());
                self.preview = LivePreview::default();
                self.reply = None;
                self.active = Some((invocation, prompt));
                self.running = true;
                self.host_compacting = false;
                self.observation = None;
                self.retained = false;
                self.outcome.clear();
                self.notice.clear();
                self.scroll.set(0);
            }
            ChatEvent::Preview {
                invocation,
                preview,
            } => {
                if self.running
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|(active, _)| active == &invocation)
                {
                    self.replace_preview(preview);
                }
            }
            ChatEvent::Compacting { invocation, active } => {
                if self.running
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|(current, _)| current == &invocation)
                {
                    self.host_compacting = active;
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
                self.transcript.get_mut().clear_images();
                self.preview.incomplete = true;
                if self.running {
                    let canonical = format!("tool:{}:", invocation.turn_id);
                    let anonymous = format!("tool-preview:{}:", invocation.turn_id);
                    self.expanded_tools
                        .retain(|key| !key.starts_with(&canonical) && !key.starts_with(&anonymous));
                }
                self.running = false;
                self.host_compacting = false;
                self.pending = None;
                self.retained = false;
                self.reply = None;
                self.latest_invocation = Some(invocation);
                match resolution {
                    Ok(resolution) => {
                        let mut details: Vec<_> = resolution
                            .diagnostics
                            .iter()
                            .map(ToString::to_string)
                            .collect();
                        match resolution.state {
                            CompletionState::Committed(completion) => {
                                if completion.result.status != "failed" {
                                    if matches!(
                                        completion.result.update,
                                        mf_chat::MessageUpdate::ReplaceContext { .. }
                                    ) {
                                        self.context_tokens = None;
                                    } else if let Some(tokens) =
                                        mf_chat::context_tokens(&completion.result.usages)
                                    {
                                        self.context_tokens = Some(tokens);
                                    }
                                }
                                self.notice = committed_notice(&completion.result);
                                if let Some(error) = &completion.result.error {
                                    details.push(error.clone());
                                }
                                let indexed = controller._database.display_reply(
                                    &completion.invocation,
                                    completion.result.display_messages().len(),
                                );
                                let reply = match indexed {
                                    Ok(messages) => Some(messages),
                                    Err(error) => {
                                        details.push(error.to_string());
                                        None
                                    }
                                };
                                self.present_result(completion.result, true, reply);
                            }
                            CompletionState::Uncommitted(pending) => {
                                self.retained = true;
                                self.notice =
                                    "Result not saved. Ctrl+P to save or discard it".into();
                                if let Some(error) = &pending.completion.result.error {
                                    details.push(error.clone());
                                }
                                self.present_result(pending.completion.result, false, None);
                            }
                            CompletionState::Finished(turn) => {
                                self.notice = format!("Turn {}. Ctrl+P for recovery", turn.state);
                                if let Some(error) = turn.error {
                                    details.push(error);
                                }
                            }
                            CompletionState::NotBegun => {
                                self.active = None;
                                self.notice = "Turn did not start".into();
                            }
                        }
                        self.outcome.clear();
                        for detail in details {
                            append_diagnostic(&mut self.outcome, &detail);
                        }
                    }
                    Err(error) => {
                        self.active = None;
                        self.retained = true;
                        self.notice = "Result needs recovery. Ctrl+P to save or discard it".into();
                        self.outcome.clear();
                        append_diagnostic(&mut self.outcome, &error.to_string());
                    }
                }
                if let Some(error) = capture_error {
                    append_diagnostic(&mut self.outcome, &error);
                }
                if !stderr.is_empty() {
                    append_diagnostic(&mut self.outcome, &stderr);
                }
                self.earlier = false;
                self.load(controller, None);
            }
            ChatEvent::Error { admission, source } => {
                let detail = source.to_string();
                if admission {
                    self.pending = None;
                    self.running = false;
                    if let Some((_, prompt)) = self.active.take()
                        && self.editor.text.is_empty()
                    {
                        self.editor.insert(&prompt);
                    }
                    self.outcome.clear();
                    append_diagnostic(&mut self.outcome, &detail);
                    self.load(controller, None);
                }
                self.notice = detail;
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
            .map(|m| m.turn_id.clone())
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
            .is_some_and(|m| m.sequence > 0)
        {
            actions.push((Action::Earlier, "Earlier messages"));
        }
        if self.earlier {
            actions.push((Action::Latest, "Latest messages"));
        }
        if self.supports_model_selection {
            actions.push((Action::SelectModel, "Select model (Ctrl+L)"));
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
                if controller.busy() || self.pending.is_some() || self.running || self.retained =>
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
            Action::SelectModel if !self.supports_model_selection => {
                self.notice = "This chatflow does not expose model selection".into();
            }
            Action::SelectModel
                if controller.busy() || self.pending.is_some() || self.running || self.retained =>
            {
                self.notice =
                    "Cancel the current turn or recover its reply before switching model".into();
            }
            Action::SelectModel => {
                self.models = None;
                self.models_error = None;
                self.model_filter.clear();
                self.selected = 0;
                if self.send(controller, ChatCommand::ListModels) {
                    self.mode = Mode::Models;
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
            Action::Inspect => {
                self.transcript.get_mut().evict_bodies();
                self.mode = Mode::Inspect;
            }
            Action::Earlier => {
                if let Some(first) = self.page.as_ref().and_then(|p| p.messages.first()) {
                    let before = first.sequence;
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
            self.outcome.clear();
            self.mode = Mode::Input;
            self.notice = "Starting...".into();
            self.earlier = false;
        }
    }
    fn present_result(
        &mut self,
        result: mf_chat::ConversationResult,
        saved: bool,
        indexed: Option<Vec<DisplayMessage>>,
    ) {
        if result.status != "failed" {
            self.preview = LivePreview::default();
        } else if !result.display_messages().is_empty() {
            // Keep the failed call's unfinished output alongside completed audit messages.
            let coverage = display::PreviewCoverage::new(&result);
            self.preview
                .segments
                .retain(|segment| !coverage.covers(segment));
            if self.preview.segments.is_empty() {
                self.preview = LivePreview::default();
            }
        }
        let status = PresentedResult::status(&result, saved);
        let messages = match result.update {
            mf_chat::MessageUpdate::Append { messages } => messages,
            mf_chat::MessageUpdate::ReplaceContext {
                display_messages, ..
            } => display_messages,
        };
        let (messages, warning) =
            indexed.map_or_else(|| display::stage(messages), |messages| (messages, None));
        if let Some(error) = warning {
            self.notice
                .push_str(&format!("; display cache unavailable: {error}"));
        }
        self.reply = Some(PresentedResult {
            messages,
            usages: result.usages,
            status,
        });
    }

    fn replace_preview(&mut self, preview: LivePreview) {
        let Some(invocation) = &preview.invocation else {
            return;
        };
        let turn = &invocation.turn_id;
        let current_ids = unique_preview_ids(&preview.segments);
        self.expanded_tools
            .extend(
                preview
                    .segments
                    .iter()
                    .enumerate()
                    .filter_map(|(i, segment)| {
                        let tool = segment.tool.as_ref()?;
                        // Auto-open on the first argument, not on every delta after a manual fold.
                        let was_empty = self
                            .preview
                            .segments
                            .get(i)
                            .is_none_or(|old| old.text.is_empty());
                        (was_empty && !segment.text.is_empty()).then(|| {
                            tool.id
                                .as_deref()
                                .filter(|id| current_ids.contains(id))
                                .map_or_else(
                                    || preview_tool_key(turn, &segment.model_call_id, tool.index),
                                    |id| tool_key(turn, id),
                                )
                        })
                    }),
            );
        if self.preview.segments.is_empty() {
            self.preview = preview;
            return;
        }
        let previous_ids = unique_preview_ids(&self.preview.segments);
        let mut remapped = BTreeMap::new();
        // Snapshots of a running turn append segments in first-seen order.
        for (previous, next) in self.preview.segments.iter().zip(&preview.segments) {
            let (Some(tool), Some(current)) = (&previous.tool, &next.tool) else {
                continue;
            };
            if previous.model_call_id != next.model_call_id || tool.index != current.index {
                continue;
            }
            let before = tool.id.as_deref().filter(|id| previous_ids.contains(id));
            let after = current.id.as_deref().filter(|id| current_ids.contains(id));
            // Adopt an unambiguous ID without transferring state between two different calls.
            if before == after || (before.is_some() && after.is_some()) {
                continue;
            }
            let key = |id: Option<&str>| {
                id.map_or_else(
                    || preview_tool_key(turn, &previous.model_call_id, tool.index),
                    |id| tool_key(turn, id),
                )
            };
            remapped.insert(key(before), key(after));
        }
        let moved = remapped
            .iter()
            .filter(|(old, _)| self.expanded_tools.remove(*old))
            .map(|(_, new)| new.clone())
            .collect::<Vec<_>>();
        self.expanded_tools.extend(moved);
        if let Some(focused) = &mut self.focused_tool
            && let Some(key) = remapped.get(focused)
        {
            *focused = key.clone();
        }
        self.transcript.get_mut().remap_anchor(|key| {
            remapped.get(key).cloned().or_else(|| {
                let header = key.strip_suffix(":arguments")?;
                remapped.get(header).map(|key| format!("{key}:arguments"))
            })
        });
        self.preview = preview;
    }

    fn tool_executions(&self) -> BTreeMap<String, mf_chat::ToolExecution> {
        self.observation
            .as_ref()
            .map_or_else(BTreeMap::new, |run| run.reader.tool_metrics().snapshot())
    }

    fn is_compacting(&self) -> bool {
        self.running
            && (self.host_compacting
                || self
                    .observation
                    .as_ref()
                    .is_some_and(|run| run.reader.call_events().has_active_purpose("compaction")))
    }

    fn shows_previews(&self) -> bool {
        self.reply.is_none() || (self.preview.incomplete && !self.preview.segments.is_empty())
    }

    fn tool_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for records in self.page.iter().flat_map(page_turns) {
            let turn = &records[0].turn_id;
            for group in message_groups(records.iter().map(|record| &record.message)) {
                keys.extend(group.tool_ids().map(|id| tool_key(turn, id)));
            }
            let no_executions = BTreeMap::new();
            let drafts = &records.last().unwrap().unfinished;
            keys.extend(
                live_messages(turn, drafts, &no_executions)
                    .messages
                    .into_iter()
                    .flat_map(|message| message.tools)
                    .map(|tool| tool.key),
            );
        }
        let turn = self
            .latest_invocation
            .as_ref()
            .map_or("", |i| i.turn_id.as_str());
        if let Some(reply) = &self.reply {
            for group in message_groups(reply.messages.iter()) {
                keys.extend(group.tool_ids().map(|id| tool_key(turn, id)));
            }
        }
        if self.shows_previews() {
            let executions = if self.running {
                self.tool_executions()
            } else {
                BTreeMap::new()
            };
            let LiveMessages {
                messages,
                unmatched,
            } = live_messages(turn, &self.preview.segments, &executions);
            keys.extend(
                messages
                    .into_iter()
                    .flat_map(|message| message.tools)
                    .chain(unmatched)
                    .map(|tool| tool.key),
            );
        }
        let mut seen = BTreeSet::new();
        keys.retain(|key| seen.insert(key.clone()));
        keys
    }

    fn focus_tool(&mut self, previous: bool) {
        let keys = self.tool_keys();
        if keys.is_empty() {
            self.focused_tool = None;
            return;
        }
        let current = self
            .focused_tool
            .as_ref()
            .and_then(|key| keys.iter().position(|k| k == key));
        let index = match current {
            Some(index) if previous => (index + keys.len() - 1) % keys.len(),
            Some(index) => (index + 1) % keys.len(),
            None => keys.len() - 1,
        };
        let key = keys[index].clone();
        if self.transcript.get_mut().focus(&key) {
            self.scroll.set(1);
        }
        self.focused_tool = Some(key);
    }

    fn toggle_tool(&mut self) {
        if self
            .focused_tool
            .as_ref()
            .is_none_or(|key| !self.tool_keys().contains(key))
        {
            self.focus_tool(false);
        }
        if let Some(key) = &self.focused_tool {
            if !self.expanded_tools.remove(key) {
                self.expanded_tools.insert(key.clone());
            }
            if self.transcript.get_mut().focus(key) {
                self.scroll.set(1);
            }
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
                KeyCode::Char('l') => return self.act(Action::SelectModel, controller),
                KeyCode::Char('e') => {
                    self.show_execution = !self.show_execution;
                    return false;
                }
                KeyCode::Char('t') if self.mode != Mode::Inspect => {
                    self.toggle_tool();
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
                    self.inspector.handle_key(key.code, &run.reader);
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
            Mode::Models => {
                let count = self.model_rows().len();
                match key.code {
                    KeyCode::Esc => {
                        if !self.model_filter.is_empty() {
                            self.model_filter.clear();
                            self.selected = 0;
                        } else {
                            self.mode = Mode::Input;
                        }
                    }
                    KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                    KeyCode::Down => {
                        self.selected = (self.selected + 1).min(count.saturating_sub(1))
                    }
                    KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
                    KeyCode::PageDown => {
                        self.selected = (self.selected + 10).min(count.saturating_sub(1))
                    }
                    KeyCode::Backspace => {
                        if self.model_filter.pop().is_some() {
                            self.selected = 0;
                        }
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        self.model_filter.push(c);
                        self.selected = 0;
                    }
                    KeyCode::Enter => self.choose_model(controller),
                    _ => {}
                }
            }
            Mode::Input => match key.code {
                KeyCode::Esc => self.mode = Mode::Browse,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.submit(controller)
                }
                KeyCode::Enter => self.editor.insert("\n"),
                KeyCode::PageUp => self.scroll.set(self.scroll.get().saturating_add(10)),
                KeyCode::PageDown => self.scroll.set(self.scroll.get().saturating_sub(10)),
                _ => self.editor.key(key),
            },
            Mode::Browse => match key.code {
                KeyCode::Tab | KeyCode::Char('j') => self.focus_tool(false),
                KeyCode::BackTab | KeyCode::Char('k') => self.focus_tool(true),
                KeyCode::Char(' ') => self.toggle_tool(),
                KeyCode::Enter => self.submit(controller),
                KeyCode::Char('i') | KeyCode::Esc => self.mode = Mode::Input,
                KeyCode::Up => self.scroll.set(self.scroll.get().saturating_add(1)),
                KeyCode::Down => self.scroll.set(self.scroll.get().saturating_sub(1)),
                KeyCode::PageUp => self.scroll.set(self.scroll.get().saturating_add(10)),
                KeyCode::PageDown => self.scroll.set(self.scroll.get().saturating_sub(10)),
                _ => {}
            },
        }
        false
    }
    fn filtered_models(&self) -> Vec<&mf_chat::ModelInfo> {
        let Some(models) = &self.models else {
            return Vec::new();
        };
        let query = self.model_filter.trim().to_ascii_lowercase();
        if query.is_empty() {
            return models.iter().collect();
        }
        models
            .iter()
            .filter(|model| {
                model.id.to_ascii_lowercase().contains(&query)
                    || model
                        .name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&query))
            })
            .collect()
    }
    fn model_rows(&self) -> Vec<String> {
        if let Some(error) = &self.models_error {
            return vec![format!(
                "Could not load models: {}",
                safe(error).replace('\n', " ")
            )];
        }
        let Some(models) = &self.models else {
            return vec!["Loading models...".into()];
        };
        if models.is_empty() {
            return vec!["No models returned by provider".into()];
        }
        let filtered = self.filtered_models();
        if filtered.is_empty() {
            return vec!["No matching models".into()];
        }
        filtered
            .into_iter()
            .map(|model| {
                let mut label = safe(&model.id).replace('\n', " ");
                if let Some(window) = model.context_window.and_then(|v| u64::try_from(v).ok()) {
                    label.push_str(&format!(" ({})", format_token_count(window)));
                }
                if self.active_model.as_deref() == Some(model.id.as_str()) {
                    label.push_str(" [current]");
                }
                label
            })
            .collect()
    }
    fn choose_model(&mut self, controller: &ChatController) {
        let selected_id = self
            .filtered_models()
            .get(self.selected)
            .map(|model| model.id.clone());
        let Some(model) = selected_id else {
            return;
        };
        self.mode = Mode::Input;
        self.model_filter.clear();
        if self.active_model.as_deref() == Some(model.as_str()) {
            self.notice = format!("Model unchanged ({})", safe(&model).replace('\n', " "));
            return;
        }
        self.send(controller, ChatCommand::SelectModel { model });
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
        self.transcript
            .get_mut()
            .flush_images(&mut io::stderr())
            .context(IoSnafu {
                operation: "release retired terminal images",
            })?;
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
        let header_text = if let Some(model) = &self.active_model {
            format!(
                "mf chat  /  {}  ·  {}",
                safe(title),
                safe(model).replace('\n', " ")
            )
        } else if self.loading_default_model {
            format!("mf chat  /  {}  ·  selecting model...", safe(title))
        } else {
            format!("mf chat  /  {}", safe(title))
        };
        let mut header_spans = vec![Span::styled(
            header_text,
            Style::default().fg(Color::DarkGray),
        )];
        if let Some(window) = self.active_context_window.filter(|&window| window > 0) {
            let (label, color) = format_context_usage(self.context_tokens, window);
            header_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
            header_spans.push(Span::styled(label, Style::default().fg(color)));
        }
        frame.render_widget(Paragraph::new(Line::from(header_spans)), header);
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
        let mut transcript = self.transcript.borrow_mut();
        transcript.begin(body.width);
        let current_turn = self
            .latest_invocation
            .as_ref()
            .map_or("", |invocation| invocation.turn_id.as_str());
        let executions = self.tool_executions();
        let no_executions = BTreeMap::new();
        for records in self.page.iter().flat_map(page_turns) {
            let first = &records[0];
            let turn = &first.turn_id;
            let executions = if turn == current_turn {
                &executions
            } else {
                &no_executions
            };
            for group in message_groups(records.iter().map(|record| &record.message)) {
                let key = format!("message:{}", records[group.index].id);
                render_message_group(
                    &mut transcript,
                    &key,
                    &group,
                    turn,
                    executions,
                    &self.expanded_tools,
                    self.focused_tool.as_deref(),
                );
            }
            let mut metadata = vec![];
            let drafts = &records.last().unwrap().unfinished;
            if !drafts.is_empty() {
                for message in live_messages(turn, drafts, &no_executions).messages {
                    let key = format!("turn:{turn}:unfinished:{}", message.model_call_id);
                    group_gap(&mut transcript, &key);
                    render_assistant(
                        &mut transcript,
                        &key,
                        message.text,
                        message.tools,
                        None,
                        &self.expanded_tools,
                        self.focused_tool.as_deref(),
                    );
                }
                metadata.push(Line::styled(
                    "Response unfinished",
                    Style::default().fg(Color::Yellow),
                ));
            }
            let turn_usages = records.iter().find_map(|record| record.usages.as_deref());
            if let Some(usages) = turn_usages {
                let calls = crate::performance::conversation_calls(usages);
                metadata.push(Line::styled(
                    crate::performance::footer(&calls, body.width),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let state = &first.turn_state;
            if matches!(state.as_str(), "failed" | "cancelled" | "interrupted") {
                metadata.push(Line::styled(
                    format!("[{state}; model context unchanged]"),
                    Style::default().fg(Color::Yellow),
                ));
            } else if state == "succeeded"
                && let Some(outcome) = turn_usages.and_then(compaction_call_outcome)
            {
                let label = if outcome {
                    "[context compacted]"
                } else {
                    "[compaction failed; model context unchanged]"
                };
                metadata.push(Line::styled(label, Style::default().fg(Color::Yellow)));
            }
            metadata.push(Line::default());
            transcript.plain(format!("turn:{turn}:saved:metadata"), metadata);
        }
        let turn = current_turn;
        if let Some((invocation, prompt)) = &self.active
            && !self.page.as_ref().is_some_and(|page| {
                page.messages
                    .iter()
                    .any(|record| record.turn_id == invocation.turn_id)
            })
        {
            transcript.plain(
                format!("turn:{turn}:you:label"),
                vec![role_label("You", Color::Cyan, None)],
            );
            transcript.literal(format!("turn:{turn}:you:body"), prompt, Style::default());
            transcript.plain(format!("turn:{turn}:you:metadata"), vec![Line::default()]);
        }
        if !self.outcome.is_empty() {
            // Keep diagnostics scrollable without pushing an unfinished response out of view.
            transcript.literal(
                "outcome".into(),
                &self.outcome,
                Style::default().fg(Color::Yellow),
            );
        }
        let compacting = self.is_compacting();
        if let Some(reply) = &self.reply {
            for group in message_groups(reply.messages.iter()) {
                let key = format!("turn:{turn}:display:{}", group.index);
                render_message_group(
                    &mut transcript,
                    &key,
                    &group,
                    turn,
                    &executions,
                    &self.expanded_tools,
                    self.focused_tool.as_deref(),
                );
            }
            let calls = crate::performance::conversation_calls(&reply.usages);
            let mut metadata = vec![Line::styled(
                crate::performance::footer(&calls, body.width),
                Style::default().fg(Color::DarkGray),
            )];
            if let Some(status) = &reply.status {
                metadata.push(Line::styled(
                    status.clone(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            metadata.push(Line::default());
            transcript.plain(format!("turn:{turn}:metadata"), metadata);
        }
        if self.shows_previews() {
            let loading = self.pending.is_some() || self.running;
            let has_preview = !self.preview.segments.is_empty();
            let executions = if self.running {
                &executions
            } else {
                &no_executions
            };
            let indicator = loading.then(|| {
                let frame = (self.animation_started.elapsed().as_millis() / 100
                    % LOADING_FRAMES.len() as u128) as usize;
                LOADING_FRAMES[frame]
            });
            let assistant_indicator = indicator.filter(|_| !compacting);
            let LiveMessages {
                messages,
                unmatched,
            } = live_messages(turn, &self.preview.segments, executions);
            let count = messages.len();
            if count == 0 && loading && !compacting {
                transcript.plain(
                    format!("turn:{turn}:assistant:label"),
                    vec![role_label("Assistant", Color::Cyan, assistant_indicator)],
                );
            }
            for (index, message) in messages.into_iter().enumerate() {
                let model = message.model_call_id;
                let key = format!("turn:{turn}:preview:{}:{model}", model.len());
                if index > 0 {
                    group_gap(&mut transcript, &key);
                }
                render_assistant(
                    &mut transcript,
                    &key,
                    message.text,
                    message.tools,
                    assistant_indicator.filter(|_| index + 1 == count),
                    &self.expanded_tools,
                    self.focused_tool.as_deref(),
                );
            }
            if !unmatched.is_empty() {
                let key = format!("turn:{turn}:observed-tools");
                group_gap(&mut transcript, &key);
                transcript.plain(
                    format!("{key}:label"),
                    vec![role_label("Observed tools", Color::Cyan, None)],
                );
                render_tools(
                    &mut transcript,
                    &key,
                    unmatched,
                    &self.expanded_tools,
                    self.focused_tool.as_deref(),
                );
            }
            let mut status = vec![];
            if compacting {
                let spinner = indicator.unwrap_or(LOADING_FRAMES[0]);
                status.push(Line::styled(
                    format!("{spinner} Compacting context..."),
                    Style::default().fg(Color::Yellow),
                ));
            }
            if !loading && has_preview && self.preview.incomplete {
                status.push(Line::styled(
                    "Response unfinished",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if let Some(error) = &self.preview.storage_error {
                status.push(Line::styled(
                    format!("[Live preview unavailable: {}]", safe(error)),
                    Style::default().fg(Color::Yellow),
                ));
            }
            transcript.plain(format!("turn:{turn}:status"), status);
        }
        if transcript.is_empty() {
            transcript.plain(
                "welcome".into(),
                vec![Line::styled(
                    "What would you like to work on?",
                    Style::default().fg(Color::DarkGray),
                )],
            );
        }
        self.scroll.set(transcript.render_images(
            frame,
            body,
            self.scroll.get(),
            !matches!(self.mode, Mode::Commands | Mode::Sessions | Mode::Models),
        ));
        drop(transcript);
        let hint = match self.mode {
            Mode::Input => " Alt+Enter send | Enter newline | Esc browse ",
            Mode::Browse => " Enter send | i edit | j/k tool | Space fold | arrows scroll ",
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
        let hints = if self.running || self.pending.is_some() {
            "Ctrl+C cancel | Ctrl+E flow | Ctrl+P commands"
        } else if self.retained {
            "Ctrl+P recover reply | Ctrl+E flow | Ctrl+Q quit"
        } else if self.supports_model_selection {
            "Ctrl+P commands | Ctrl+L model | Ctrl+E flow | Ctrl+S chats | Ctrl+Q quit"
        } else {
            "Ctrl+P commands | Ctrl+E flow | Ctrl+S chats | Ctrl+Q quit"
        };
        let footer_notice = if self.notice.is_empty() && compacting {
            "Compacting context..."
        } else {
            self.notice.as_str()
        };
        frame.render_widget(
            Paragraph::new(format!("{}\n{hints}", safe(footer_notice)))
                .style(Style::default().fg(Color::DarkGray)),
            footer,
        );
        if matches!(self.mode, Mode::Commands | Mode::Sessions | Mode::Models) {
            let (title, rows, bottom_hint) = match self.mode {
                Mode::Commands => (
                    "Commands".to_string(),
                    self.actions()
                        .iter()
                        .map(|(_, label)| (*label).to_string())
                        .collect::<Vec<_>>(),
                    " Up/Down select | Enter open | Esc close ",
                ),
                Mode::Sessions => (
                    "Switch chat".to_string(),
                    self.session_rows(),
                    " Up/Down select | Enter open | Esc close ",
                ),
                Mode::Models => (
                    if self.model_filter.is_empty() {
                        "Select model".to_string()
                    } else {
                        format!(
                            "Select model — {}",
                            safe(&self.model_filter).replace('\n', " ")
                        )
                    },
                    self.model_rows(),
                    " Type to filter | Up/Down select | Enter apply | Esc close ",
                ),
                _ => unreachable!(),
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
                        .title_bottom(bottom_hint),
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
    use super::super::tests::{admitted, capture, preview_turn};
    use super::*;
    use crate::run::LocalRun;
    use ratatui::backend::TestBackend;
    use std::process::Command;

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
    fn row_of(screen: &str, needle: &str) -> usize {
        screen
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing:\n{screen}"))
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
    const TOOL_RESULT_BODY: &str = "**literal tool**\n![inert](tool.png)\n\u{1b}[31m";

    fn assert_grouped_tools(screen: &str, first_open: bool, second_open: bool) {
        let headers: Vec<_> = screen
            .lines()
            .filter(|line| line.contains("Tool call") || line.contains("Tool result"))
            .map(str::trim_end)
            .collect();
        let mut expected = if first_open {
            vec!["[-] Tool call lookup [first]", "Tool result (0 bytes)"]
        } else {
            vec!["[+] Tool call lookup [first]"]
        };
        let result = format!("Tool result ({} bytes)", TOOL_RESULT_BODY.len());
        if second_open {
            expected.extend(["[-] Tool call calculate [second]", result.as_str()]);
        } else {
            expected.push("[+] Tool call calculate [second]");
        }
        assert_eq!(headers, expected, "{screen}");
    }

    fn tool_result(replace: bool, failed: bool) -> mf_chat::ConversationResult {
        let mut display = serde_json::from_value::<Vec<mf_chat::ChatMessage>>(serde_json::json!([
            {"role":"assistant","content":"","tool_calls":[
                {"id":"first","name":"lookup","arguments":{"text":"**argument**","image":"![inert](tool.png)"}},
                {"id":"second","name":"calculate","arguments":[1,true,null]}
            ]},
            {"role":"tool","tool_call_id":"second","content":TOOL_RESULT_BODY},
            {"role":"tool","tool_call_id":"first","content":""},
            {"role":"assistant","content":"**Final answer**"}
        ])).unwrap();
        if failed {
            display.truncate(2);
        }
        let mut result = mf_chat::ConversationResult::append(display.clone());
        if replace {
            result.update = mf_chat::MessageUpdate::ReplaceContext {
                context_messages: vec![mf_chat::ChatMessage::User {
                    content: "PRIVATE_CONTEXT_ONLY".into(),
                    metadata: None,
                }],
                display_messages: display,
            };
        }
        if failed {
            result.status = "failed".into();
            result.error = Some("Provider failed".into());
        }
        result.usages = serde_json::from_value(serde_json::json!([
            {"call_id":"model-one","model":"fixture","timing":{"elapsed_us":1000000,"first_token_us":100000},"usage":{"output_tokens":10}},
            {"call_id":"model-two","model":"fixture","timing":{"elapsed_us":1000000,"first_token_us":100000},"usage":{"output_tokens":10}}
        ])).unwrap();
        result.validate().unwrap();
        result
    }

    fn finish(invocation: Invocation, state: CompletionState) -> ChatEvent {
        ChatEvent::Finished {
            invocation,
            resolution: Ok(ReconciledCompletion {
                state,
                diagnostics: vec![],
            }),
            status: None,
            stderr: String::new(),
            capture_error: None,
        }
    }

    #[test]
    fn interleaved_live_messages_match_settled_tool_spacing_and_focus_order() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        view.running = true;
        let (_dir, db, invocation) = preview_turn();
        view.latest_invocation = Some(invocation.clone());
        let mut preview = capture(&db, &invocation);
        let text = |model: &str, text: &str| -> mf_chat::PreviewDelta {
            mf_chat::TextDelta {
                text: text.into(),
                model_call_id: model.into(),
            }
            .into()
        };
        let tool = |model: &str, index, id: &str| -> mf_chat::PreviewDelta {
            mf_chat::ToolCallDelta {
                model_call_id: model.into(),
                index,
                arguments: "{}".into(),
                id: Some(id.into()),
                name: Some("lookup".into()),
            }
            .into()
        };
        for delta in [
            tool("z", 0, "one"),
            text("a", "Next step"),
            tool("a", 0, "three"),
            tool("z", 1, "two"),
            text("z", "Checking"),
            text("final", "Final answer"),
        ] {
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: invocation.clone(),
                    delta,
                })
                .unwrap();
        }
        view.preview = preview.frame;
        let layout = [
            "Assistant",
            "Checking",
            "[+] Tool call lookup [one]",
            "",
            "[+] Tool call lookup [two]",
            "",
            "Assistant",
            "Next step",
            "[+] Tool call lookup [three]",
            "",
            "Assistant",
            "Final answer",
        ];
        let expected_keys = ["one", "two", "three"].map(|id| tool_key(&invocation.turn_id, id));
        let assert_layout = |screen: &str| {
            let first = row_of(screen, "Assistant");
            let last = row_of(screen, "Final answer");
            let lines: Vec<_> = screen
                .lines()
                .skip(first)
                .take(last + 1 - first)
                .map(|line| line.trim_end().trim_end_matches(LOADING_FRAMES).trim_end())
                .collect();
            assert_eq!(lines, layout, "{screen}");
        };
        let live = render(&view, 100, 30);
        assert_layout(&live);
        assert_eq!(live.matches(LOADING_FRAMES).count(), 1);
        assert!(
            live.lines()
                .nth(row_of(&live, "Final answer") - 1)
                .unwrap()
                .contains(LOADING_FRAMES)
        );
        assert_eq!(view.tool_keys(), expected_keys);

        let result = mf_chat::ConversationResult::append(serde_json::from_value(serde_json::json!([
            {"role":"assistant","content":"Checking","tool_calls":[
                {"id":"one","name":"lookup","arguments":{}},
                {"id":"two","name":"lookup","arguments":{}}
            ]},
            {"role":"tool","tool_call_id":"two","content":"second result"},
            {"role":"tool","tool_call_id":"one","content":"first result"},
            {"role":"assistant","content":"Next step","tool_calls":[{"id":"three","name":"lookup","arguments":{}}]},
            {"role":"tool","tool_call_id":"three","content":"third result"},
            {"role":"assistant","content":"Final answer"}
        ])).unwrap());
        result.validate().unwrap();
        view.running = false;
        view.present_result(result, true, None);
        let settled = render(&view, 100, 30);
        assert_layout(&settled);
        assert!(!settled.contains(LOADING_FRAMES));
        assert_eq!(view.tool_keys(), expected_keys);
    }

    #[test]
    fn many_key_deltas_keep_their_full_text_and_scroll_anchor_after_late_identity() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let (_dir, db, invocation) = preview_turn();
        let delta = |arguments: String, id: Option<String>| mf_chat::PreviewEvent {
            invocation: invocation.clone(),
            delta: mf_chat::ToolCallDelta {
                model_call_id: "model".into(),
                index: 0,
                arguments,
                id,
                name: None,
            }
            .into(),
        };
        let mut anonymous = capture(&db, &invocation);
        anonymous.apply(delta("{\n".into(), None)).unwrap();
        for index in 0..420 {
            anonymous
                .apply(delta(
                    format!("  \"key_{index:04}\": \"value_{index:04}\",\n"),
                    None,
                ))
                .unwrap();
        }
        anonymous
            .apply(delta("  \"last\": true\n}".into(), None))
            .unwrap();
        let retained = anonymous.segments[0].text.load().unwrap();
        assert!(retained.starts_with("{\n  \"key_0000\""));
        assert!(retained.contains("key_0419") && retained.ends_with("  \"last\": true\n}"));
        let first_frame = anonymous.frame.clone();
        anonymous
            .apply(delta(String::new(), Some("known".into())))
            .unwrap();
        let turn = &invocation.turn_id;
        let old = preview_tool_key(turn, "model", 0);
        let new = tool_key(turn, "known");
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        view.running = true;
        view.latest_invocation = Some(invocation.clone());
        view.preview = first_frame;
        view.expanded_tools.insert(old.clone());
        render(&view, 100, 22);
        assert!(view.transcript.get_mut().focus(&format!("{old}:arguments")));
        view.scroll.set(1);
        view.replace_preview(anonymous.frame.clone());
        let screen = render(&view, 100, 22);
        assert!(
            screen.lines().nth(2).unwrap().contains("key_0000"),
            "{screen}"
        );
        assert_eq!(view.expanded_tools, BTreeSet::from([new.clone()]));
        view.focused_tool = Some(new.clone());
        view.toggle_tool();
        anonymous
            .apply(delta("\nstream-tail".into(), None))
            .unwrap();
        view.replace_preview(anonymous.frame);
        assert!(!view.expanded_tools.contains(&new));
        assert!(!render(&view, 100, 22).contains("stream-tail"));
        view.toggle_tool();
        view.scroll.set(0);
        assert!(render(&view, 100, 22).contains("stream-tail"));
    }

    #[test]
    fn long_tool_arguments_render_in_full_and_replay_canonical_values_from_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                "Long arguments",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: begun.invocation.clone(),
                prompt: "Long arguments".into(),
            },
            &controller,
        );
        let key = tool_key(&begun.invocation.turn_id, "long");
        let mut preview = capture(&controller._database, &begun.invocation);
        for (index, arguments) in [
            "{\"query\":\"OLD_PREFIX",
            &"b".repeat(9000),
            "\",\"latest\":\"live-tail\"}",
        ]
        .into_iter()
        .enumerate()
        {
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: begun.invocation.clone(),
                    delta: mf_chat::ToolCallDelta {
                        model_call_id: "model".into(),
                        index: 0,
                        arguments: arguments.into(),
                        id: (index == 0).then(|| "long".into()),
                        name: (index == 0).then(|| "search".into()),
                    }
                    .into(),
                })
                .unwrap();
        }
        view.event(
            ChatEvent::Preview {
                invocation: begun.invocation.clone(),
                preview: preview.frame,
            },
            &controller,
        );
        assert_eq!(view.expanded_tools, BTreeSet::from([key.clone()]));
        let live = render(&view, 240, 100);
        assert!(live.contains("[-] Tool call search [long]"));
        assert!(live.contains("OLD_PREFIX") && live.contains("live-tail"));

        let canonical = serde_json::json!({
            "query": "c".repeat(9000),
            "z_latest": "persisted-tail"
        });
        let result = mf_chat::ConversationResult::append(
            serde_json::from_value(serde_json::json!([
                {"role":"assistant","content":"","tool_calls":[{
                    "id":"long","name":"search","arguments":canonical
                }]},
                {"role":"tool","tool_call_id":"long","content":"done"},
                {"role":"assistant","content":"Finished"}
            ]))
            .unwrap(),
        );
        result.validate().unwrap();
        let completion = controller
            ._database
            .complete_turn(begun.invocation.clone(), result)
            .unwrap();
        view.event(
            finish(
                begun.invocation.clone(),
                CompletionState::Committed(completion),
            ),
            &controller,
        );
        let saved = render(&view, 240, 70);
        assert!(saved.contains("[+] Tool call search [long]"), "{saved}");
        assert!(!saved.contains("live-tail") && !saved.contains("persisted-tail"));
        view.mode = Mode::Browse;
        view.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &controller);
        view.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &controller,
        );
        let full = render(&view, 240, 100);
        assert!(full.contains("persisted-tail") && full.contains("Finished"));
        assert!(!full.contains("live-tail"));
        assert!(!full.contains("truncated") && !full.contains("older deltas omitted"));
        controller.close().unwrap();

        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        view.switch(Some(session.clone()));
        let controller = ChatController::open(launch).unwrap();
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        let source = page
            .messages
            .iter()
            .find_map(|record| match &record.message {
                DisplayMessage::Assistant { tools, .. } => {
                    tools.first().map(|tool| tool.arguments.load().unwrap())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&source).unwrap(),
            canonical
        );
        let mut document = crate::markdown::Document::default();
        document.json(&source);
        let rows = (0..document.layout(240))
            .map(|row| document.row(row).unwrap().1.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rows.contains("\"query\"") && rows.matches('c').count() == 9000);
        assert!(rows.contains("persisted-tail") && !rows.contains("older deltas omitted"));
        view.event(
            ChatEvent::Loaded {
                session: controller._database.session(&session.id).unwrap(),
                page,
                context_tokens: None,
            },
            &controller,
        );
        let loaded = render(&view, 240, 100);
        assert!(loaded.contains("[+] Tool call search [long]"));
        assert!(!loaded.contains("live-tail") && !loaded.contains("persisted-tail"));
        view.mode = Mode::Browse;
        view.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &controller);
        view.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &controller,
        );
        let full = render(&view, 240, 100);
        assert!(full.contains("persisted-tail") && full.contains("Finished"));
        assert!(!full.contains("live-tail"));
        assert!(!full.contains("truncated") && !full.contains("older deltas omitted"));
        controller.close().unwrap();
    }

    #[test]
    fn full_display_replaces_previews_and_survives_pending_reload_then_save() {
        for replace in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let launch = super::super::tests::launch(root.path());
            let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                    "Original question",
                    mf_chat::ModelIdentity {
                        profile: "default".into(),
                        model: None,
                    },
                    None,
                )
                .unwrap();
            view.event(
                ChatEvent::Submitted {
                    session: session.clone(),
                    invocation: begun.invocation.clone(),
                    prompt: "Original question".into(),
                },
                &controller,
            );
            let mut preview = capture(&controller._database, &begun.invocation);
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: begun.invocation.clone(),
                    delta: mf_chat::TextDelta {
                        text: "provisional only".into(),
                        model_call_id: "preview".into(),
                    }
                    .into(),
                })
                .unwrap();
            view.mode = Mode::Browse;
            for (step, (arguments, label, hint)) in [
                ("{ \"query\": ", "Tool call [index 0]", false),
                ("", "Tool call lookup [first]", true),
                (
                    "\"**preview argument**\" }",
                    "Tool call lookup [first]",
                    false,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                preview
                    .apply(mf_chat::PreviewEvent {
                        invocation: begun.invocation.clone(),
                        delta: mf_chat::ToolCallDelta {
                            model_call_id: "preview".into(),
                            index: 0,
                            arguments: arguments.into(),
                            id: hint.then(|| "first".into()),
                            name: hint.then(|| "lookup".into()),
                        }
                        .into(),
                    })
                    .unwrap();
                view.event(
                    ChatEvent::Preview {
                        invocation: begun.invocation.clone(),
                        preview: preview.frame.clone(),
                    },
                    &controller,
                );
                if step == 0 {
                    let expanded = render(&view, 140, 70);
                    assert!(
                        expanded.contains("[-] Tool call [index 0]") && expanded.contains("query")
                    );
                }
                let screen = render(&view, 140, 70);
                assert_eq!(screen.matches("Tool call").count(), 1);
                assert!(
                    screen.contains(&format!("[-] {label}")) && screen.contains("{ \"query\":"),
                    "{screen}"
                );
                assert_eq!(screen.contains("**preview argument**"), step == 2);
                assert!(!screen.contains("Tool result") && !screen.contains("[Running]"));
            }
            let first = tool_key(&begun.invocation.turn_id, "first");
            let second = tool_key(&begun.invocation.turn_id, "second");
            assert_eq!(view.expanded_tools, BTreeSet::from([first.clone()]));
            let result = tool_result(replace, false);
            view.event(
                finish(
                    begun.invocation.clone(),
                    CompletionState::Uncommitted(UncommittedCompletion {
                        database_path: controller._database.path().to_owned(),
                        completion: mf_chat::Completion {
                            invocation: begun.invocation.clone(),
                            result: result.clone(),
                        },
                    }),
                ),
                &controller,
            );
            let settled = render(&view, 140, 70);
            assert_grouped_tools(&settled, false, false);
            assert!(
                !settled.contains("**argument**")
                    && !settled.contains("**preview argument**")
                    && !settled.contains("**literal tool**")
            );
            let toggle = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
            view.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &controller);
            view.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &controller);
            view.key(toggle, &controller);
            let settled = render(&view, 140, 70);
            assert_grouped_tools(&settled, true, false);
            assert!(settled.contains("**argument**") && !settled.contains("**preview argument**"));
            assert_eq!(view.tool_keys(), [first.clone(), second.clone()]);
            for (key, expected) in [
                (KeyCode::Char('j'), &second),
                (KeyCode::Char('k'), &first),
                (KeyCode::BackTab, &second),
                (KeyCode::Tab, &first),
                (KeyCode::Char('j'), &second),
            ] {
                view.key(KeyEvent::new(key, KeyModifiers::NONE), &controller);
                assert_eq!(view.focused_tool.as_ref(), Some(expected));
            }
            view.key(toggle, &controller);
            assert!(render(&view, 140, 70).contains("**literal tool**"));
            view.key(toggle, &controller);
            assert!(!render(&view, 140, 70).contains("**literal tool**"));
            view.key(toggle, &controller);
            for reload in [false, true] {
                if reload {
                    let page = controller
                        ._database
                        .display_page(&session.id, 50, None)
                        .unwrap();
                    view.event(
                        ChatEvent::Loaded {
                            session: session.clone(),
                            page,
                            context_tokens: None,
                        },
                        &controller,
                    );
                }
                let screen = render(&view, 140, 70);
                assert_eq!(screen.matches("Original question").count(), 1, "{screen}");
                assert_eq!(screen.matches("Assistant").count(), 2, "{screen}");
                assert_grouped_tools(&screen, true, true);
                assert!(screen.contains("**argument**") && screen.contains("**literal tool**"));
                assert!(screen.contains("![inert](tool.png)"));
                assert!(screen.contains("\\u{1b}[31m"));
                assert!(screen.contains("Final answer") && !screen.contains("**Final answer**"));
                assert!(
                    !screen.contains("provisional only")
                        && !screen.contains("PRIVATE_CONTEXT_ONLY")
                );
                assert_eq!(screen.matches("FTT").count(), 1);
                assert!(screen.contains("[not saved]"));
                assert_eq!(screen.contains("replacement pending"), replace);
                assert!(
                    view.retained
                        && view
                            .actions()
                            .iter()
                            .any(|(action, _)| *action == Action::Save)
                );
                assert_eq!(
                    controller
                        ._database
                        .model_context(&session.id)
                        .unwrap()
                        .revision,
                    0
                );
            }
            let completion = controller
                ._database
                .complete_turn(begun.invocation.clone(), result)
                .unwrap();
            view.event(
                finish(
                    begun.invocation.clone(),
                    CompletionState::Committed(completion),
                ),
                &controller,
            );
            let saved = render(&view, 140, 70);
            assert!(!saved.contains("[not saved]"));
            assert!(!saved.contains("PRIVATE_CONTEXT_ONLY"));
            let page = controller
                ._database
                .display_page(&session.id, 50, None)
                .unwrap();
            view.event(
                ChatEvent::Loaded {
                    session: controller._database.session(&session.id).unwrap(),
                    page,
                    context_tokens: None,
                },
                &controller,
            );
            let loaded = render(&view, 140, 70);
            assert_grouped_tools(&loaded, true, true);
            assert_eq!(view.tool_keys(), [first, second]);
            assert_eq!(loaded.matches("Assistant").count(), 2);
            assert_eq!(loaded.matches("FTT").count(), 1);
            assert!(!loaded.contains("PRIVATE_CONTEXT_ONLY"));
            assert!(view.reply.is_none());
            assert_eq!(
                controller
                    ._database
                    .usage(Default::default())
                    .unwrap()
                    .overall
                    .attempted_calls,
                2
            );
            controller.close().unwrap();
        }
    }

    #[test]
    fn failed_partial_display_is_visible_before_and_after_audit_save() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                "Question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: begun.invocation.clone(),
                prompt: "Question".into(),
            },
            &controller,
        );
        let result = tool_result(true, true);
        view.event(
            finish(
                begun.invocation.clone(),
                CompletionState::Uncommitted(UncommittedCompletion {
                    database_path: controller._database.path().to_owned(),
                    completion: mf_chat::Completion {
                        invocation: begun.invocation.clone(),
                        result: result.clone(),
                    },
                }),
            ),
            &controller,
        );
        let screen = render(&view, 120, 55);
        assert_grouped_tools(&screen, false, false);
        assert_eq!(
            view.tool_keys().len(),
            2,
            "retain the call whose result was missing"
        );
        assert!(screen.contains("[failed]") && screen.contains("[not saved]"));
        assert!(screen.contains("Model context unchanged"));
        assert!(!screen.contains("replacement pending"));
        assert!(!screen.contains("PRIVATE_CONTEXT_ONLY"));
        let completion = controller
            ._database
            .complete_turn(begun.invocation.clone(), result)
            .unwrap();
        view.event(
            finish(begun.invocation, CompletionState::Committed(completion)),
            &controller,
        );
        assert!(view.notice.contains("Failed result saved"));
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        let screen = render(&view, 120, 55);
        assert_grouped_tools(&screen, false, false);
        assert!(screen.contains("[failed"));
        assert!(screen.contains("Provider failed"));
        assert!(!screen.contains("not saved"));
        assert_eq!(
            controller
                ._database
                .model_context(&view.session.as_ref().unwrap().id)
                .unwrap()
                .revision,
            0
        );
        controller.close().unwrap();
    }

    #[test]
    fn pages_inside_a_tool_exchange_keep_ids_and_one_turn_summary() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                "Question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        controller
            ._database
            .complete_turn(begun.invocation, tool_result(true, false))
            .unwrap();
        view.switch(Some(session.clone()));
        let page = controller
            ._database
            .display_page(&session.id, 2, Some(1))
            .unwrap();
        let [second, first] = page.messages.as_slice() else {
            panic!("expected two results on a page without their calls");
        };
        assert!(matches!(
            &second.message,
            DisplayMessage::ToolResult { id, .. } if id == "second"
        ));
        assert!(matches!(
            &first.message,
            DisplayMessage::ToolResult { id, .. } if id == "first"
        ));
        view.event(
            ChatEvent::Loaded {
                session: session.clone(),
                page,
                context_tokens: None,
            },
            &controller,
        );
        let collapsed = render(&view, 120, 35);
        assert!(collapsed.contains("[+] Tool result [second]"));
        assert!(collapsed.contains("[+] Tool result [first] (0 bytes)"));
        assert!(!collapsed.contains("**literal tool**"));
        view.mode = Mode::Browse;
        view.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &controller);
        view.key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &controller,
        );
        view.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &controller,
        );
        let screen = render(&view, 120, 35);
        assert_eq!(screen.matches("Tool result").count(), 2);
        assert!(screen.contains("[-] Tool result [second]"));
        assert!(screen.contains("[+] Tool result [first] (0 bytes)"));
        assert!(!screen.contains("lookup") && !screen.contains("calculate"));
        assert!(!screen.contains("Assistant") && !screen.lines().any(|line| line.trim() == "You"));
        assert!(screen.contains("**literal tool**") && screen.contains("![inert](tool.png)"));
        assert_eq!(screen.matches("FTT").count(), 1);
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session: session.clone(),
                page,
                context_tokens: None,
            },
            &controller,
        );
        let screen = render(&view, 120, 70);
        assert_grouped_tools(&screen, false, true);
        assert_eq!(screen.matches("FTT").count(), 1);
        assert!(!screen.contains("PRIVATE_CONTEXT_ONLY"));
        let page = controller
            ._database
            .display_page(&session.id, 2, Some(0))
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        let screen = render(&view, 120, 35);
        assert_grouped_tools(&screen, false, true);
        assert_eq!(screen.matches("FTT").count(), 1);
        assert_eq!(
            controller
                ._database
                .usage(Default::default())
                .unwrap()
                .overall
                .attempted_calls,
            2
        );
        controller.close().unwrap();
    }

    #[test]
    fn fold_marker_is_on_the_first_row_even_when_the_result_id_wraps() {
        let id = "call_1234567890123456789012345678901234567890";
        let key = tool_key("turn", id);
        let mut terminal = Terminal::new(TestBackend::new(16, 1)).unwrap();
        let mut transcript = super::super::transcript::Transcript::default();
        let mut expanded = BTreeSet::new();
        let text = display::Fragments::open()
            .unwrap()
            .append(&Text::default(), TOOL_RESULT_BODY)
            .unwrap();
        for prefix in ["[+] Tool result", "[-] Tool result"] {
            transcript.begin(16);
            ToolGroup::standalone_result("turn", id, (&text, TOOL_RESULT_BODY.len()), None).render(
                &mut transcript,
                &expanded,
                Some(&key),
            );
            assert!(transcript.focus(&key));
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    transcript.render_images(frame, area, 1, false);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row: String = (0..16).map(|x| buffer[(x, 0)].symbol()).collect();
            assert_eq!(row.trim_end(), prefix);
            assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
            expanded.insert(key.clone());
        }
    }

    #[test]
    fn loading_an_admitted_user_keeps_subsequent_previews_without_duplicate_submission() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                "Original question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: begun.invocation.clone(),
                prompt: "Original question".into(),
            },
            &controller,
        );
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        let mut preview = capture(&controller._database, &begun.invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: begun.invocation.clone(),
                delta: mf_chat::TextDelta {
                    text: "Later preview".into(),
                    model_call_id: "preview".into(),
                }
                .into(),
            })
            .unwrap();
        view.event(
            ChatEvent::Preview {
                invocation: begun.invocation,
                preview: preview.frame,
            },
            &controller,
        );
        let screen = render(&view, 100, 25);
        assert_eq!(screen.matches("Original question").count(), 1);
        assert!(screen.contains("Later preview"));
        assert!(view.running && view.active.is_some());
        controller.close().unwrap();
    }

    #[test]
    fn failed_empty_display_has_no_invented_assistant_or_published_context() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let result = mf_chat::ConversationResult {
            status: "failed".into(),
            error: Some("No messages produced".into()),
            ..Default::default()
        };
        let invocation = Invocation::new("session");
        result.validate().unwrap();
        view.event(
            finish(
                invocation.clone(),
                CompletionState::Uncommitted(UncommittedCompletion {
                    database_path: controller._database.path().to_owned(),
                    completion: mf_chat::Completion { invocation, result },
                }),
            ),
            &controller,
        );
        let screen = render(&view, 100, 25);
        assert!(!screen.contains("Assistant"));
        assert!(screen.contains("[failed]") && screen.contains("No messages produced"));
        assert!(!screen.contains("pending"));
        assert!(view.retained);
        controller.close().unwrap();
    }

    #[test]
    fn replacing_context_keeps_prior_display_and_accounting_on_reload() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let identity = || mf_chat::ModelIdentity {
            profile: "default".into(),
            model: None,
        };
        let first = controller
            ._database
            .begin_turn(&session.id, "Previous question", identity(), None)
            .unwrap();
        controller
            ._database
            .complete_turn(
                first.invocation,
                mf_chat::ConversationResult::assistant("Previous answer"),
            )
            .unwrap();
        view.switch(Some(session.clone()));
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session: session.clone(),
                page,
                context_tokens: None,
            },
            &controller,
        );
        let next = controller
            ._database
            .begin_turn(&session.id, "Current question", identity(), None)
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: next.invocation.clone(),
                prompt: "Current question".into(),
            },
            &controller,
        );
        let completion = controller
            ._database
            .complete_turn(next.invocation.clone(), tool_result(true, false))
            .unwrap();
        view.event(
            finish(next.invocation, CompletionState::Committed(completion)),
            &controller,
        );
        for reload in [false, true] {
            if reload {
                let page = controller
                    ._database
                    .display_page(&session.id, 50, None)
                    .unwrap();
                view.event(
                    ChatEvent::Loaded {
                        session: controller._database.session(&session.id).unwrap(),
                        page,
                        context_tokens: None,
                    },
                    &controller,
                );
            }
            let screen = render(&view, 140, 80);
            for text in [
                "Previous question",
                "Previous answer",
                "Current question",
                "Final answer",
            ] {
                assert_eq!(screen.matches(text).count(), 1, "{screen}");
            }
            assert_eq!(screen.matches("FTT").count(), 2);
            assert!(!screen.contains("PRIVATE_CONTEXT_ONLY"));
        }
        assert_eq!(
            controller
                ._database
                .model_context(&session.id)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert_eq!(
            controller
                ._database
                .usage(Default::default())
                .unwrap()
                .overall
                .attempted_calls,
            2
        );
        controller.close().unwrap();
    }

    #[test]
    fn metrics_follow_saved_unsaved_and_reloaded_replies() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
            )
            .unwrap();
        let calls: Vec<mf_chat::CallUsage> = serde_json::from_value(serde_json::json!([
            {"call_id":"answer", "model":"fixture", "timing":{"elapsed_us":2000000,"first_token_us":500000},"usage":{"output_tokens":100}},
            {"call_id":"title", "model":"fixture", "timing":{"elapsed_us":10000,"first_token_us":1000},"usage":{"output_tokens":100},"attribution":{"purpose":"title"}}
        ])).unwrap();
        let response = mf_chat::ConversationResult {
            update: mf_chat::ConversationResult::assistant("canonical answer").update,
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
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        assert!(view.reply.is_none());
        let text = render(&view, 100, 30);
        assert_eq!(text.matches("FTT 500ms").count(), 1);
        assert!(text.contains("E2E 50.0 tok/s"));
        assert!(!render(&view, 45, 30).contains("E2E"));
        controller.close().unwrap();
    }

    #[test]
    fn editing_keeps_plain_letters_out_of_global_shortcuts() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        for character in ['e', 'j', 'k'] {
            view.key(
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                &controller,
            );
        }
        assert_eq!(view.editor.text, "ejk");
        assert!(view.focused_tool.is_none());
        controller.close().unwrap();
    }

    #[test]
    fn execution_panel_adapts_to_resizing_and_preserves_the_editor() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.editor.insert("Draft for the next turn");
        assert!(!render(&view, 180, 40).contains("Flow execution"));
        view.pending = Some(None);
        assert!(render(&view, 180, 40).contains("Flow: Starting..."));
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 0.1"]);
        let mut run = LocalRun::start(
            command,
            launch.runner.description().clone(),
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
                .position(|row| row.contains("Assistant"))
                .unwrap();
            assert_eq!(panel == chat, side_by_side, "{text}");
            assert!(text.contains(LOADING_FRAMES), "{text}");
            assert!(text.contains("Flow: Running"), "{text}");
            assert!(text.contains("start"), "{text}");
            assert!(text.contains("Delivery: Collecting"), "{text}");
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
    fn parallel_previews_do_not_share_folds_or_status_on_conflicting_id_hints() {
        let (_dir, db, invocation) = preview_turn();
        let turn = &invocation.turn_id;
        let mut preview = capture(&db, &invocation);
        for (index, id) in [(0, "duplicate"), (1, "duplicate"), (2, "known")] {
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: invocation.clone(),
                    delta: mf_chat::ToolCallDelta {
                        model_call_id: "model".into(),
                        index,
                        arguments: index.to_string(),
                        id: Some(id.into()),
                        name: Some("search".into()),
                    }
                    .into(),
                })
                .unwrap();
        }
        let executions = ["duplicate", "known"]
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    mf_chat::ToolExecution {
                        name: "search".into(),
                        phase: Some(mf_chat::ToolExecutionPhase::Running),
                        duration_ns: None,
                    },
                )
            })
            .collect();
        let LiveMessages {
            messages,
            unmatched,
        } = live_messages(turn, &preview.segments, &executions);
        let [message] = messages.as_slice() else {
            panic!("expected one preview batch");
        };
        let [first, second, known] = message.tools.as_slice() else {
            panic!("expected two anonymous previews and one known group");
        };
        let [observed] = unmatched.as_slice() else {
            panic!("expected one observation without a known model-call identity");
        };
        assert_ne!(first.key, second.key);
        assert!(first.execution.is_none() && second.execution.is_none());
        assert!(first.title.ends_with("[index 0]") && second.title.ends_with("[index 1]"));
        assert_eq!(known.key, tool_key(turn, "known"));
        assert!(known.execution.is_some() && known.arguments.is_some());
        assert_eq!(observed.key, tool_key(turn, "duplicate"));
        assert!(observed.execution.is_some() && observed.arguments.is_none());
    }

    #[test]
    fn tool_observations_label_live_calls_even_with_the_flow_panel_hidden() {
        use mf_runtime::ValueRef;
        use mf_telemetry::{
            event::{Event, LifecycleEvent, LoopPassOutcome, LoopPathEntry},
            identity::{RunId, WorkflowId},
            observation::LocalRecord,
        };

        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let run = LocalRun::start(
            Command::new("true"),
            launch.runner.description().clone(),
            RunId::new(),
            1024,
        )
        .unwrap();
        let observation = run.observation();
        let metrics = observation.reader.tool_metrics().clone();
        view.observation = Some(observation);
        view.show_execution = false;
        view.running = true;
        let (_dir, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation,
                delta: mf_chat::ToolCallDelta {
                    model_call_id: "model".into(),
                    index: 0,
                    arguments: "{\"query\":\"test\"}".into(),
                    id: Some("matched".into()),
                    name: Some("search".into()),
                }
                .into(),
            })
            .unwrap();
        view.preview = preview.frame;
        let screen = render(&view, 100, 25);
        assert!(screen.contains("Tool call search [matched]") && !screen.contains("[Running]"));

        let run_id = RunId::new();
        let observe = |id: &str, terminal: Option<LoopPassOutcome>| {
            let scope = vec![LoopPathEntry::Call { call_id: id.into() }];
            metrics.observe_snapshot(
                run_id,
                &scope,
                &ValueRef::from(serde_json::json!({
                    "call": {"id": id, "name": "search", "arguments": {}}
                })),
            );
            let mut path = scope;
            path.push(LoopPathEntry::Call {
                call_id: "selected".into(),
            });
            let observe = |sequence: i64, event| {
                metrics.observe(
                    LocalRecord::Finite(&LifecycleEvent {
                        workflow_id: WorkflowId::from_definition(&"test", &[]).unwrap(),
                        run_id,
                        sequence: sequence.try_into().unwrap(),
                        event,
                    }),
                    false,
                )
            };
            observe(
                1,
                Event::CallStarted {
                    path: path.clone(),
                    elapsed_ns: 10.try_into().unwrap(),
                },
            );
            if let Some(outcome) = terminal {
                observe(
                    2,
                    Event::CallFinished {
                        path,
                        elapsed_ns: 11.try_into().unwrap(),
                        outcome,
                    },
                );
            }
        };
        observe("matched", None);
        observe("failed", Some(LoopPassOutcome::Failed));
        observe("done", Some(LoopPassOutcome::Completed));
        observe("stopped", None);
        observe("ambiguous", Some(LoopPassOutcome::Failed));
        observe("ambiguous", Some(LoopPassOutcome::Completed));
        let screen = render(&view, 100, 25);
        for expected in [
            "[+] [Running] Tool call search [matched]",
            "[+] [Failed] Tool call search [failed]",
            "[+] [Succeeded] Tool call search [done]",
            "[+] [Running] Tool call search [stopped]",
            "[+] [Unknown] Tool call search [ambiguous]",
        ] {
            assert!(screen.contains(expected), "{screen}");
        }
        assert!(!screen.contains("Tool result") && !screen.contains("Flow execution"));
        assert!(!screen.contains("Tool execution"));
        assert_eq!(screen.matches("[matched]").count(), 1);
        assert_eq!(screen.matches("Assistant").count(), 1);
        assert_eq!(
            row_of(&screen, "Observed tools"),
            row_of(&screen, "[matched]") + 2
        );
        assert_eq!(
            row_of(&screen, "[done]"),
            row_of(&screen, "[ambiguous]") + 2
        );
        metrics.close(true);
        let screen = render(&view, 100, 25);
        assert!(screen.contains("[+] [Interrupted] Tool call search [matched]"));
        assert!(screen.contains("[+] [Interrupted] Tool call search [stopped]"));
        assert!(screen.contains("[+] [Succeeded] Tool call search [done]"));
        view.toggle_tool();
        let opened = render(&view, 100, 25);
        assert!(opened.contains("[-] [Interrupted] Tool call search [stopped]"));
        assert!(opened.contains("No argument preview"));
        view.running = false;
        let stopped = render(&view, 100, 25);
        assert!(!stopped.contains("[failed]") && !stopped.contains("[Running]"));
    }

    #[test]
    fn execution_panel_toggle_preserves_observation_drafts_and_preferences() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let run = LocalRun::start(
            Command::new(launch.runner.executable()),
            launch.runner.description().clone(),
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
            )
            .unwrap();
        database
            .complete_turn(
                started.invocation,
                mf_chat::ConversationResult::assistant(
                    "Keep one execution lane and move infrequent actions into the command palette.",
                ),
            )
            .unwrap();
        view.page = Some(database.display_page(&session.id, 50, None).unwrap());
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
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                        mf_chat::ConversationResult::assistant(format!("reply-{index}")),
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
                .content()
                .load()
                .unwrap(),
            "reply-27"
        );
        assert!(view.retry_turn().is_none());
        choose(&mut view, &controller, Action::Earlier);
        pump(&mut view, &controller, |v| {
            v.page.as_ref().unwrap().messages[0].sequence == 0
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
                .content()
                .load()
                .unwrap()
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
        assert_eq!(view.scroll.get(), 0);
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
        std::fs::remove_file(launch.runner.executable()).unwrap();
        launch.options.run.verbose = 1;
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
        assert!(view.outcome.contains("could not launch"));
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
        std::fs::write(launch.runner.executable().with_extension("script"), "#!/bin/sh\nsleep 0.3\nprintf '%s' '{\"append\":{\"status\":\"succeeded\",\"messages\":[{\"role\":\"assistant\",\"content\":\"paid reply\"}],\"usages\":[]}}'\necho 'runner diagnostic' >&2\nkill -TERM $$\n").unwrap();
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let invocation = admitted(&controller._database, &session.id, "hello");
        view.pending = Some(None);
        assert!(render(&view, 100, 30).contains(LOADING_FRAMES));
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: invocation.clone(),
                prompt: "hello".into(),
            },
            &controller,
        );
        assert!(render(&view, 100, 30).contains(LOADING_FRAMES));
        let mut preview = capture(&controller._database, &invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    text: "**draft prefix**".into(),
                    model_call_id: "model-call".into(),
                }
                .into(),
            })
            .unwrap();
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    text: "second call output".into(),
                    model_call_id: "second-call".into(),
                }
                .into(),
            })
            .unwrap();
        view.event(
            ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.frame.clone(),
            },
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("draft prefix"));
        assert!(screen.contains("second call output"));
        assert_eq!(screen.matches("Assistant").count(), 2);
        assert!(screen.contains(LOADING_FRAMES));
        assert!(!screen.contains("provisional"));
        assert!(!screen.contains("segment"));
        let finish = |invocation: Invocation, text: &str| ChatEvent::Finished {
            invocation: invocation.clone(),
            resolution: Ok(ReconciledCompletion {
                state: CompletionState::Committed(mf_chat::Completion {
                    invocation,
                    result: mf_chat::ConversationResult::assistant(text),
                }),
                diagnostics: vec![],
            }),
            status: None,
            stderr: String::new(),
            capture_error: None,
        };
        view.event(
            finish(invocation.clone(), "**canonical reply**"),
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("canonical reply"));
        assert!(!screen.contains("**"));
        assert_eq!(
            view.reply
                .as_ref()
                .unwrap()
                .messages
                .last()
                .unwrap()
                .content()
                .load()
                .unwrap(),
            "**canonical reply**"
        );
        assert!(!screen.contains("draft prefix"));
        assert!(!screen.contains(LOADING_FRAMES));
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
                preview: preview.frame,
            },
            &controller,
        );
        view.event(finish(invocation, "stale final"), &controller);
        assert_eq!(view.active.as_ref().unwrap().0, next);
        assert!(!render(&view, 100, 30).contains("stale final"));
        controller.close().unwrap();
    }

    #[test]
    fn previews_keep_their_rows_and_spinner_placement_across_completion() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
            )
            .unwrap();
        let invocation = begun.invocation.clone();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: invocation.clone(),
                prompt: "question".into(),
            },
            &controller,
        );
        let mut preview = capture(&controller._database, &invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    text: "streamed answer".into(),
                    model_call_id: "model-call".into(),
                }
                .into(),
            })
            .unwrap();
        view.event(
            ChatEvent::Preview {
                invocation: invocation.clone(),
                preview: preview.frame,
            },
            &controller,
        );
        let streamed = render(&view, 80, 30);
        let label = streamed
            .lines()
            .find(|line| line.contains("Assistant"))
            .unwrap()
            .trim_end();
        assert!(label.starts_with("Assistant "), "{label}");
        assert!(
            LOADING_FRAMES.contains(&label.chars().nth(10).unwrap()),
            "{label}"
        );
        assert_eq!(row_of(&streamed, "question"), 2);
        assert_eq!(row_of(&streamed, "streamed answer"), 5);
        assert!(!streamed.contains("FTT"));
        let completion = controller
            ._database
            .complete_turn(
                invocation.clone(),
                mf_chat::ConversationResult::assistant("streamed answer"),
            )
            .unwrap();
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
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
        let finished = render(&view, 80, 30);
        assert_eq!(row_of(&finished, "question"), 2);
        assert_eq!(row_of(&finished, "streamed answer"), 5);
        assert!(!finished.contains(LOADING_FRAMES));
        assert!(finished.contains("FTT"));
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        assert!(view.active.is_none());
        let loaded = render(&view, 80, 30);
        assert_eq!(row_of(&loaded, "question"), 2);
        assert_eq!(row_of(&loaded, "streamed answer"), 5);
        assert_eq!(loaded.matches("FTT").count(), 1);
        controller.close().unwrap();
    }

    #[test]
    fn repeated_terminal_diagnostics_preserve_distinct_root_causes() {
        let mut output = String::new();
        let wrapper = "Workflow ended without a terminal conversation result";
        let root = "DeepSeek fetch request failed with HTTP 404 Not Found";
        append_diagnostic(&mut output, wrapper);
        append_diagnostic(&mut output, wrapper);
        append_diagnostic(&mut output, &format!("\n{wrapper}\n{root}\n"));
        append_diagnostic(&mut output, "   ");
        assert_eq!(output, format!("{wrapper}\n{root}"));
    }

    #[test]
    fn failed_prefixes_are_marked_incomplete_and_full_recovery_replies_replace_them() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();
        let invocation = admitted(&controller._database, &session.id, "hello");
        view.event(
            ChatEvent::Submitted {
                session,
                invocation: invocation.clone(),
                prompt: "hello".into(),
            },
            &controller,
        );
        let mut preview = capture(&controller._database, &invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    model_call_id: "model-call".into(),
                    text: "partial text".into(),
                }
                .into(),
            })
            .unwrap();
        view.preview = preview.frame;
        view.preview.storage_error = Some("scratch full".into());
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::NotBegun,
                    diagnostics: vec![],
                }),
                status: None,
                stderr: (0..40).map(|i| format!("Diagnostic {i}\n")).collect(),
                capture_error: None,
            },
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("partial text"));
        assert!(screen.contains("Response unfinished"));
        assert!(screen.contains("[Live preview unavailable: scratch full]"));
        assert!(!screen.contains(LOADING_FRAMES));
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
                resolution: Ok(ReconciledCompletion {
                    state: CompletionState::Uncommitted(UncommittedCompletion {
                        database_path: controller._database.path().to_owned(),
                        completion: mf_chat::Completion {
                            invocation,
                            result: mf_chat::ConversationResult::assistant(
                                "complete unsaved reply",
                            ),
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
        assert!(!screen.contains("scratch full"));
        assert!(!screen.contains("partial text"));
        assert!(view.retained);
        controller.close().unwrap();
    }

    #[test]
    fn failed_calls_keep_unrepresented_previews_beside_audit_messages() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let (_dir, db, invocation) = preview_turn();
        let mut preview = capture(&db, &invocation);
        for (call_id, text) in [
            ("first", "first provisional"),
            ("recorded", "recorded partial"),
            ("missing", "unrecorded partial"),
        ] {
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: invocation.clone(),
                    delta: mf_chat::TextDelta {
                        model_call_id: call_id.into(),
                        text: text.into(),
                    }
                    .into(),
                })
                .unwrap();
        }
        for (index, id) in ["recorded-tool", "pending-tool"].into_iter().enumerate() {
            preview
                .apply(mf_chat::PreviewEvent {
                    invocation: invocation.clone(),
                    delta: mf_chat::ToolCallDelta {
                        model_call_id: "missing".into(),
                        index: index as u64,
                        arguments: "{}".into(),
                        id: Some(id.into()),
                        name: Some("lookup".into()),
                    }
                    .into(),
                })
                .unwrap();
        }
        let mut reply = mf_chat::ConversationResult::append(
            serde_json::from_value(serde_json::json!([
                {"role":"assistant", "content":"recorded partial", "tool_calls":[
                    {"id":"recorded-tool", "name":"lookup", "arguments":{}}
                ]}
            ]))
            .unwrap(),
        );
        reply.status = "failed".into();
        reply.error = Some("Provider failed".into());
        reply.usages = serde_json::from_value(serde_json::json!([
            {"call_id":"first", "model":"fixture"},
            {"call_id":"recorded", "model":"fixture", "error":"provider failed"},
            {"call_id":"missing", "model":"fixture", "error":"provider failed"}
        ]))
        .unwrap();
        reply.validate().unwrap();
        view.preview = preview.frame;
        view.preview.incomplete = true;
        view.present_result(reply, true, None);
        let [partial, pending] = view.preview.segments.as_slice() else {
            panic!("expected only the failed previews absent from the audit transcript");
        };
        assert_eq!(partial.model_call_id, "missing");
        assert_eq!(partial.text.load().unwrap(), "unrecorded partial");
        assert_eq!(
            pending.tool.as_ref().unwrap().id.as_deref(),
            Some("pending-tool")
        );
        assert!(view.shows_previews());
    }

    #[test]
    fn live_compacting_status_renders_for_host_and_clears_after_settlement() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
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
                "Long conversation turn",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        let invocation = begun.invocation.clone();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: invocation.clone(),
                prompt: "Long conversation turn".into(),
            },
            &controller,
        );
        let mut preview = capture(&controller._database, &invocation);
        preview
            .apply(mf_chat::PreviewEvent {
                invocation: invocation.clone(),
                delta: mf_chat::TextDelta {
                    model_call_id: "model-1".into(),
                    text: "Assistant answer".into(),
                }
                .into(),
            })
            .unwrap();
        view.preview = preview.frame;

        // Stale invocation compaction events must be ignored.
        view.event(
            ChatEvent::Compacting {
                invocation: Invocation::new(&session.id),
                active: true,
            },
            &controller,
        );
        assert!(!view.is_compacting());

        // Active host compaction renders in both transcript status and footer notice.
        view.event(
            ChatEvent::Compacting {
                invocation: invocation.clone(),
                active: true,
            },
            &controller,
        );
        assert!(view.is_compacting());
        let compacting_screen = render(&view, 100, 30);
        assert!(
            compacting_screen.contains("Assistant answer"),
            "{compacting_screen}"
        );
        assert_eq!(
            compacting_screen.matches("Compacting context...").count(),
            2,
            "expected compacting indicator in both transcript and footer:\n{compacting_screen}"
        );

        // Settled compacted completion shows token count and clears live compacting state.
        let compacted_result = mf_chat::ConversationResult {
            status: "succeeded".into(),
            update: mf_chat::MessageUpdate::ReplaceContext {
                context_messages: vec![mf_chat::ChatMessage::User {
                    content: "Summary body".into(),
                    metadata: Some(
                        serde_json::from_value(serde_json::json!({
                            "compaction": {"tokens_before": 12400}
                        }))
                        .unwrap(),
                    ),
                }],
                display_messages: vec![mf_chat::ChatMessage::Assistant {
                    content: "Assistant answer".into(),
                    tool_calls: None,
                    metadata: None,
                }],
            },
            usages: serde_json::from_value(serde_json::json!([
                {"call_id": "main-1", "model": "fixture"},
                {"call_id": "compact-1", "model": "fixture", "attribution": {"purpose": "compaction"}}
            ]))
            .unwrap(),
            error: None,
            title: None,
        };
        compacted_result.validate().unwrap();
        let completion = controller
            ._database
            .complete_turn(invocation.clone(), compacted_result)
            .unwrap();
        view.event(
            finish(invocation, CompletionState::Committed(completion)),
            &controller,
        );
        assert!(!view.is_compacting());
        let settled = render(&view, 100, 30);
        assert!(!settled.contains("Compacting context..."), "{settled}");
        assert!(
            settled.contains("[context compacted from 12.4k tokens]"),
            "{settled}"
        );
        assert!(
            settled.contains("Saved; context compacted from 12.4k tokens"),
            "{settled}"
        );

        // Persisted history reload preserves a compaction marker on the compacted turn.
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        let loaded = render(&view, 100, 30);
        assert!(loaded.contains("[context compacted]"), "{loaded}");
        controller.close().unwrap();
    }

    #[test]
    fn failed_and_custom_compaction_statuses_render_distinctly() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        view.show_execution = false;
        let controller = ChatController::open(launch).unwrap();
        let session = controller
            ._database
            .create_session("default", None, None)
            .unwrap();

        // Turn 1: failed compaction preserves primary answer and marks compaction failure.
        let begun_failed = controller
            ._database
            .begin_turn(
                &session.id,
                "First question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        let mut failed_compact = mf_chat::ConversationResult::assistant("First answer");
        failed_compact.usages = serde_json::from_value(serde_json::json!([
            {"call_id": "main-1", "model": "fixture"},
            {"call_id": "compact-1", "model": "fixture", "attribution": {"purpose": "compaction"}, "error": "HTTP 500"}
        ]))
        .unwrap();
        let completion = controller
            ._database
            .complete_turn(begun_failed.invocation.clone(), failed_compact)
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: begun_failed.invocation.clone(),
                prompt: "First question".into(),
            },
            &controller,
        );
        view.event(
            finish(
                begun_failed.invocation,
                CompletionState::Committed(completion),
            ),
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(
            screen.contains("[compaction failed; model context unchanged]"),
            "{screen}"
        );
        assert!(screen.contains("Saved; compaction failed"), "{screen}");

        // Turn 2: custom compaction without metadata.compaction.tokens_before still reports compacted.
        let begun_custom = controller
            ._database
            .begin_turn(
                &session.id,
                "Second question",
                mf_chat::ModelIdentity {
                    profile: "default".into(),
                    model: None,
                },
                None,
            )
            .unwrap();
        let custom_compact = mf_chat::ConversationResult {
            status: "succeeded".into(),
            update: mf_chat::MessageUpdate::ReplaceContext {
                context_messages: vec![mf_chat::ChatMessage::System {
                    content: "Custom summary".into(),
                    metadata: None,
                }],
                display_messages: vec![mf_chat::ChatMessage::Assistant {
                    content: "Second answer".into(),
                    tool_calls: None,
                    metadata: None,
                }],
            },
            usages: serde_json::from_value(serde_json::json!([
                {"call_id": "main-2", "model": "fixture"},
                {"call_id": "compact-2", "model": "fixture", "attribution": {"purpose": "compaction"}}
            ]))
            .unwrap(),
            error: None,
            title: None,
        };
        let completion = controller
            ._database
            .complete_turn(begun_custom.invocation.clone(), custom_compact)
            .unwrap();
        view.event(
            ChatEvent::Submitted {
                session: session.clone(),
                invocation: begun_custom.invocation.clone(),
                prompt: "Second question".into(),
            },
            &controller,
        );
        view.event(
            finish(
                begun_custom.invocation,
                CompletionState::Committed(completion),
            ),
            &controller,
        );
        let screen = render(&view, 100, 30);
        assert!(screen.contains("[context compacted]"), "{screen}");
        assert!(screen.contains("Saved; context compacted"), "{screen}");

        // Reload both turns from storage and verify persisted compaction badges.
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: None,
            },
            &controller,
        );
        let loaded = render(&view, 100, 40);
        assert!(
            loaded.contains("[compaction failed; model context unchanged]"),
            "{loaded}"
        );
        assert!(loaded.contains("[context compacted]"), "{loaded}");
        controller.close().unwrap();
    }

    #[test]
    fn model_selector_popup_filters_selects_and_updates_header() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let controller = ChatController::open(launch).unwrap();
        view.supports_model_selection = true;
        view.active_model = Some("model-alpha".into());

        let initial = render(&view, 100, 30);
        assert!(initial.contains("New chat  ·  model-alpha"), "{initial}");
        assert!(initial.contains("Ctrl+L model"), "{initial}");

        // Open model selector via Ctrl+L.
        view.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.mode == Mode::Models);
        let loading = render(&view, 100, 30);
        assert!(loading.contains("Select model"), "{loading}");
        assert!(loading.contains("Loading models..."), "{loading}");

        // Receive provider models and verify badges + current highlight.
        view.event(
            ChatEvent::Models(Ok(vec![
                mf_chat::ModelInfo {
                    id: "model-alpha".into(),
                    context_window: Some(128_000),
                    ..Default::default()
                },
                mf_chat::ModelInfo {
                    id: "model-beta".into(),
                    name: Some("Beta Fast".into()),
                    context_window: Some(64_000),
                    ..Default::default()
                },
            ])),
            &controller,
        );
        assert_eq!(view.selected, 0);
        let listed = render(&view, 100, 30);
        assert!(
            listed.contains("model-alpha (128.0k) [current]"),
            "{listed}"
        );
        assert!(listed.contains("model-beta (64.0k)"), "{listed}");

        // Type to filter by name/id, Esc clears filter first.
        for ch in "beta".chars() {
            view.key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &controller,
            );
        }
        let filtered = render(&view, 100, 30);
        assert!(filtered.contains("Select model — beta"), "{filtered}");
        assert!(filtered.contains("model-beta (64.0k)"), "{filtered}");
        assert!(!filtered.contains("model-alpha (128.0k)"), "{filtered}");

        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        assert!(view.mode == Mode::Models);
        assert!(view.model_filter.is_empty());

        // Move down to model-beta and apply with Enter.
        view.key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &controller,
        );
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        assert!(view.mode == Mode::Input);
        view.event(
            ChatEvent::ModelSelected {
                model: Some("model-beta".into()),
                context_window: None,
                automatic: false,
                warning: None,
            },
            &controller,
        );
        let updated = render(&view, 100, 30);
        assert!(updated.contains("New chat  ·  model-beta"), "{updated}");
        assert!(updated.contains("Model set to model-beta"), "{updated}");

        // Switching model is blocked while a turn is running.
        view.running = true;
        view.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.mode == Mode::Input);
        assert!(
            view.notice.contains("before switching model"),
            "{}",
            view.notice
        );
        controller.close().unwrap();
    }

    #[test]
    fn model_selector_handles_errors_empty_lists_navigation_unchanged_and_warnings() {
        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let controller = ChatController::open(launch).unwrap();

        // Unsupported chatflow rejects Ctrl+L with a clear notice.
        view.supports_model_selection = false;
        view.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.mode == Mode::Input);
        assert!(
            view.notice.contains("does not expose model selection"),
            "{}",
            view.notice
        );

        // Header shows selecting indicator while initial model discovery is in flight.
        view.supports_model_selection = true;
        view.loading_default_model = true;
        let loading_header = render(&view, 100, 30);
        assert!(
            loading_header.contains("selecting model..."),
            "{loading_header}"
        );

        // Open selector and cover provider error, empty list, filter/backspace/page navigation.
        view.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.mode == Mode::Models);
        view.event(ChatEvent::Models(Err("network down".into())), &controller);
        let errored = render(&view, 100, 30);
        assert!(
            errored.contains("Could not load models: network down"),
            "{errored}"
        );
        // Pressing Enter with no models loaded is a safe no-op.
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        assert!(view.mode == Mode::Models);

        view.event(ChatEvent::Models(Ok(Vec::new())), &controller);
        let empty = render(&view, 100, 30);
        assert!(empty.contains("No models returned by provider"), "{empty}");

        view.active_model = Some("m1".into());
        view.event(
            ChatEvent::Models(Ok(vec![
                mf_chat::ModelInfo {
                    id: "m1".into(),
                    ..Default::default()
                },
                mf_chat::ModelInfo {
                    id: "m2".into(),
                    ..Default::default()
                },
                mf_chat::ModelInfo {
                    id: "m3".into(),
                    ..Default::default()
                },
            ])),
            &controller,
        );
        view.key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &controller,
        );
        assert_eq!(view.selected, 2);
        view.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &controller);
        assert_eq!(view.selected, 1);
        view.key(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            &controller,
        );
        assert_eq!(view.selected, 0);

        // Non-matching filter shows empty-match message, Backspace trims filter.
        view.key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &controller,
        );
        let unmatched = render(&view, 100, 30);
        assert!(unmatched.contains("No matching models"), "{unmatched}");
        view.key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &controller,
        );
        assert!(view.model_filter.is_empty());

        // Choosing the already-active model reports unchanged without issuing a switch.
        view.key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &controller,
        );
        assert!(view.mode == Mode::Input);
        assert!(
            view.notice.contains("Model unchanged (m1)"),
            "{}",
            view.notice
        );

        // Esc in Mode::Models with empty filter closes the popup back to Input.
        view.key(
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &controller,
        );
        assert!(view.mode == Mode::Models);
        view.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &controller);
        assert!(view.mode == Mode::Input);

        // ModelSelected with persistence warning updates notice.
        view.event(
            ChatEvent::ModelSelected {
                model: Some("m2".into()),
                context_window: None,
                automatic: false,
                warning: Some("disk full".into()),
            },
            &controller,
        );
        assert!(
            view.notice.contains("preferences not saved: disk full"),
            "{}",
            view.notice
        );
        controller.close().unwrap();
    }

    #[test]
    fn header_renders_context_usage_percentage_colors_and_compaction_reset() {
        assert_eq!(format_context_window(512), "512");
        assert_eq!(format_context_window(4_096), "4.1k");
        assert_eq!(format_context_window(128_000), "128k");
        assert_eq!(format_context_window(500_000), "500k");
        assert_eq!(format_context_window(1_000_000), "1.0M");
        assert_eq!(format_context_window(12_000_000), "12M");

        assert_eq!(
            format_context_usage(Some(307_000), 500_000),
            ("61.4%/500k".into(), Color::DarkGray)
        );
        assert_eq!(
            format_context_usage(Some(375_000), 500_000),
            ("75.0%/500k".into(), Color::Yellow)
        );
        assert_eq!(
            format_context_usage(Some(460_000), 500_000),
            ("92.0%/500k".into(), Color::Red)
        );
        assert_eq!(
            format_context_usage(None, 500_000),
            ("?/500k".into(), Color::DarkGray)
        );

        let root = tempfile::tempdir().unwrap();
        let launch = super::super::tests::launch(root.path());
        let mut view = ChatView::new(launch.runner.description()).unwrap();
        let controller = ChatController::open(launch).unwrap();

        // Model selected before window discovery starts without a window, then updates on
        // the matching ContextWindow event while ignoring stale models.
        view.supports_model_selection = true;
        view.event(
            ChatEvent::ModelSelected {
                model: Some("model-500k".into()),
                context_window: None,
                automatic: true,
                warning: None,
            },
            &controller,
        );
        assert_eq!(view.active_context_window, None);
        view.event(
            ChatEvent::ContextWindow {
                model: "other-model".into(),
                context_window: Some(32_000),
            },
            &controller,
        );
        assert_eq!(view.active_context_window, None);
        view.event(
            ChatEvent::ContextWindow {
                model: "model-500k".into(),
                context_window: Some(500_000),
            },
            &controller,
        );
        assert_eq!(view.active_context_window, Some(500_000));
        let initial = render(&view, 100, 30);
        assert!(
            initial.contains("New chat  ·  model-500k  ·  0.0%/500k"),
            "{initial}"
        );

        // Committed append turn updates context usage to 61.4%/500k.
        let invocation = Invocation::new("session-1");
        view.latest_invocation = Some(invocation.clone());
        let mut reply = mf_chat::ConversationResult::assistant("done");
        reply.usages = vec![mf_chat::CallUsage {
            attribution: None,
            started_at: None,
            timing: None,
            call_id: "c1".into(),
            node: None,
            model: "model-500k".into(),
            usage: Some(BTreeMap::from([("total_tokens".into(), 307_000)])),
            error: None,
        }];
        view.event(
            ChatEvent::Finished {
                invocation: invocation.clone(),
                resolution: Ok(mf_chat::contract::ReconciledCompletion {
                    state: CompletionState::Committed(mf_chat::Completion {
                        invocation: invocation.clone(),
                        result: reply,
                    }),
                    diagnostics: Vec::new(),
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        let used = render(&view, 100, 30);
        assert!(used.contains("model-500k  ·  61.4%/500k"), "{used}");

        // Switching to a 1M model recomputes percentage immediately against the new window.
        view.event(
            ChatEvent::ModelSelected {
                model: Some("model-1m".into()),
                context_window: Some(1_000_000),
                automatic: false,
                warning: None,
            },
            &controller,
        );
        let switched = render(&view, 100, 30);
        assert!(switched.contains("model-1m  ·  30.7%/1.0M"), "{switched}");

        // Committed context replacement (compaction) clears stale usage to ?/1.0M.
        let replacement = mf_chat::ConversationResult {
            status: "succeeded".into(),
            update: mf_chat::MessageUpdate::ReplaceContext {
                display_messages: vec![mf_chat::ChatMessage::Assistant {
                    content: "after compact".into(),
                    tool_calls: None,
                    metadata: None,
                }],
                context_messages: vec![mf_chat::ChatMessage::Assistant {
                    content: "after compact".into(),
                    tool_calls: None,
                    metadata: None,
                }],
            },
            usages: Vec::new(),
            title: None,
            error: None,
        };
        let inv2 = Invocation::new("session-1");
        view.latest_invocation = Some(inv2.clone());
        view.event(
            ChatEvent::Finished {
                invocation: inv2.clone(),
                resolution: Ok(mf_chat::contract::ReconciledCompletion {
                    state: CompletionState::Committed(mf_chat::Completion {
                        invocation: inv2,
                        result: replacement,
                    }),
                    diagnostics: Vec::new(),
                }),
                status: None,
                stderr: String::new(),
                capture_error: None,
            },
            &controller,
        );
        let compacted = render(&view, 100, 30);
        assert!(compacted.contains("model-1m  ·  ?/1.0M"), "{compacted}");

        // Loaded session restores persisted context_tokens.
        let session = controller
            ._database
            .create_session("default", None, Some("Loaded chat".into()))
            .unwrap();
        let page = controller
            ._database
            .display_page(&session.id, 50, None)
            .unwrap();
        view.switch(Some(session.clone()));
        view.event(
            ChatEvent::Loaded {
                session,
                page,
                context_tokens: Some(750_000),
            },
            &controller,
        );
        let reloaded = render(&view, 100, 30);
        assert!(
            reloaded.contains("Loaded chat  ·  model-1m  ·  75.0%/1.0M"),
            "{reloaded}"
        );
        controller.close().unwrap();
    }
}
