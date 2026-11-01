//! Interactive preview of the graph widget with simulated lifecycle events.

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mf_telemetry::{
    Count,
    description::{
        ClusterDescription, ControlEdge, DataEdge, ExecutionDescription, ExecutionMode,
        NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
    },
    event::{Event, LifecycleEvent, NodeIdentity, Outcome, SkipCause},
    identity::{RunId, WorkflowId},
};
use mf_tui::{
    graph::{
        ConnectionView, EdgeKind, GraphLayout, GraphView,
        routing::{crossing_cells, line_symbols},
    },
    state::SessionState,
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Layout},
    widgets::Paragraph,
};
use std::{
    error::Error,
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

type PreviewResult<T> = Result<T, Box<dyn Error>>;

/// Animation frame interval used to throttle preview redraws.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
/// Settling window after the last event. A terminal node's single-lap pulse
/// runs for 1.2s, so the animation must stay alive past that before freezing.
const SETTLING: Duration = Duration::from_millis(1_500);
/// Poll wait once the animation has settled and only keys are expected.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// Frame index for the given animation clock.
fn frame_index(animation_elapsed: Duration) -> u128 {
    animation_elapsed.as_millis() / FRAME_INTERVAL.as_millis()
}

/// Time left until the next frame deadline, measured from `started` so time
/// spent rendering is subtracted instead of accumulating into drift.
fn next_frame_wait(started: Duration, tick: u128) -> Duration {
    let deadline =
        Duration::from_millis((tick.saturating_add(1) * FRAME_INTERVAL.as_millis()) as u64);
    deadline.saturating_sub(started)
}

/// Animation clock, capped one settling window past the last event.
fn animation_clock(elapsed: Duration, final_elapsed: Duration) -> Duration {
    elapsed.min(final_elapsed + SETTLING)
}

/// Timing outcome for one preview frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    /// Animation clock for this frame, capped past the final pulse.
    animation_elapsed: Duration,
    /// Whether the frame boundary or the final pulse requires a redraw.
    redraw: bool,
}

/// Animation clock state shared by the preview loop and its tests.
struct Timing {
    final_elapsed: Duration,
    animation_end: Duration,
    frame_tick: u128,
    settled: bool,
}

impl Timing {
    fn new(final_elapsed: Duration) -> Self {
        Self {
            final_elapsed,
            animation_end: final_elapsed + SETTLING,
            frame_tick: u128::MAX,
            settled: false,
        }
    }

    /// Advance to `elapsed`, reporting whether this frame must be redrawn.
    fn advance(&mut self, elapsed: Duration) -> Frame {
        let animation_elapsed = animation_clock(elapsed, self.final_elapsed);
        let tick = frame_index(animation_elapsed);
        let mut redraw = false;
        if tick != self.frame_tick {
            self.frame_tick = tick;
            redraw = true;
        }
        if elapsed >= self.animation_end && !self.settled {
            self.settled = true;
            redraw = true;
        }
        Frame {
            animation_elapsed,
            redraw,
        }
    }

    /// Poll until the next frame deadline, the final pulse, or idle keys,
    /// subtracting time already spent rendering.
    fn wait(&self, rendered: Duration) -> Duration {
        if self.settled {
            IDLE_WAIT
        } else {
            next_frame_wait(rendered, self.frame_tick)
                .min(self.animation_end.saturating_sub(rendered))
        }
    }
}

/// Terminal event source, injectable so the preview loop can run under test.
struct Input {
    poll: fn(Duration) -> io::Result<bool>,
    read: fn() -> io::Result<TerminalEvent>,
}

/// Real terminal events used by the interactive preview.
const TERMINAL_INPUT: Input = Input {
    poll: event::poll,
    read: event::read,
};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), Show, LeaveAlternateScreen);
    }
}

fn main() -> PreviewResult<()> {
    if std::env::args().any(|arg| arg == "--crossings") {
        preview_crossings();
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--dump") {
        dump_graph()?;
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("graph preview requires interactive stdin and stderr".into());
    }
    enable_raw_mode()?;
    let guard = TerminalGuard;
    execute!(io::stderr(), EnterAlternateScreen, Hide)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stderr()))?;
    let result = run(&mut terminal, TERMINAL_INPUT);
    drop(terminal);
    drop(guard);
    result
}

fn run<B: Backend>(terminal: &mut Terminal<B>, input: Input) -> PreviewResult<()>
where
    B::Error: 'static,
{
    let description = preview_description(
        description(),
        std::env::args().any(|arg| arg == "--clusters"),
    );
    let layout = GraphLayout::new(&description)?;
    let run_id = RunId::new();
    let mut state = SessionState::new(description.clone(), run_id)?;
    let events = scheduled_events(&description, run_id);
    let mut next_event = 0;
    let mut offset = (0u32, 0u32);
    let mut minimap = true;
    let started = Instant::now();
    let final_elapsed = events.last().map_or(Duration::ZERO, |entry| entry.0);
    let mut timing = Timing::new(final_elapsed);
    let mut redraw = true;

    loop {
        let elapsed = started.elapsed();
        while next_event < events.len() && elapsed >= events[next_event].0 {
            state.apply(events[next_event].1.clone())?;
            next_event += 1;
            redraw = true;
        }
        let complete = next_event == events.len();
        let displayed_elapsed = if complete { final_elapsed } else { elapsed };
        let preview = timing.advance(elapsed);
        if redraw || preview.redraw {
            let snapshot = state.snapshot();
            terminal.draw(|frame| {
                let [header, graph, footer] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .areas(frame.area());
                let phase = if complete { "Complete" } else { "Running" };
                frame.render_widget(
                    Paragraph::new(format!(
                        "Workflow graph preview  |  {phase}  |  {:.1}s",
                        displayed_elapsed.as_secs_f64()
                    )),
                    header,
                );
                frame.render_widget(
                    GraphView::new(&layout)
                        .snapshot(&snapshot)
                        .offset(offset.0, offset.1)
                        .elapsed_ns(preview.animation_elapsed.as_nanos() as u64)
                        .minimap(minimap),
                    graph,
                );
                frame.render_widget(
                    Paragraph::new("Arrows: pan  |  f: origin  |  m: minimap  |  q/Esc: quit"),
                    footer,
                );
            })?;
            redraw = false;
        }

        let wait = timing.wait(started.elapsed());
        if (input.poll)(wait)? {
            match (input.read)()? {
                TerminalEvent::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('f') => offset = (0, 0),
                        KeyCode::Char('m') => minimap = !minimap,
                        KeyCode::Left => offset.0 = offset.0.saturating_sub(4),
                        KeyCode::Right => offset.0 = offset.0.saturating_add(4),
                        KeyCode::Up => offset.1 = offset.1.saturating_sub(2),
                        KeyCode::Down => offset.1 = offset.1.saturating_add(2),
                        _ => {}
                    }
                    redraw = true;
                }
                TerminalEvent::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
    }
}

fn preview_description(mut fixture: WorkflowDescription, clusters: bool) -> WorkflowDescription {
    if clusters {
        fixture.version = WorkflowDescriptionVersion::V2026_10_23;
        fixture.execution = Some(ExecutionDescription {
            mode: ExecutionMode::Single,
        });
        let middle = fixture.nodes.len().div_ceil(2);
        fixture.clusters = vec![
            ClusterDescription {
                label: "Input preparation".into(),
                annotation: "Fetch the input and transform the source before publishing.".into(),
                nodes: fixture.nodes[..middle]
                    .iter()
                    .map(|node| node.id.clone())
                    .collect(),
            },
            ClusterDescription {
                label: "Output delivery".into(),
                annotation: "Publish the result or archive the original response.".into(),
                nodes: fixture.nodes[middle..]
                    .iter()
                    .map(|node| node.id.clone())
                    .collect(),
            },
        ];
    }
    fixture
}

pub fn description() -> WorkflowDescription {
    WorkflowDescription {
        order_edges: Vec::new(),
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "c".repeat(64)))
            .expect("fixture workflow ID is valid"),
        nodes: ["fetch", "transform", "publish", "archive"]
            .into_iter()
            .map(|id| NodeDescription {
                id: id.into(),
                kind: format!("preview.{id}"),
            })
            .collect(),
        data_edges: vec![
            DataEdge {
                exclusive: false,
                from_node: "fetch".into(),
                from_output: "value".into(),
                to_node: "transform".into(),
                to_input: "input".into(),
            },
            DataEdge {
                exclusive: false,
                from_node: "transform".into(),
                from_output: "value".into(),
                to_node: "publish".into(),
                to_input: "input".into(),
            },
        ],
        control_edges: vec![
            ControlEdge {
                from_node: "transform".into(),
                from_output: "ready".into(),
                to_node: "publish".into(),
            },
            ControlEdge {
                from_node: "transform".into(),
                from_output: "archive".into(),
                to_node: "archive".into(),
            },
        ],
        execution_order: ["fetch", "transform", "publish", "archive"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        execution: None,
        loop_bodies: Vec::new(),
        clusters: Vec::new(),
    }
}

pub fn body_description(
    fixture: &WorkflowDescription,
) -> mf_telemetry::description::LoopBodyDescription {
    mf_telemetry::description::LoopBodyDescription {
        order_edges: Vec::new(),
        path: vec!["repeat".into()],
        nodes: fixture.nodes.clone(),
        data_edges: fixture.data_edges.clone(),
        control_edges: fixture.control_edges.clone(),
        execution_order: fixture.execution_order.clone(),
        clusters: fixture.clusters.clone(),
    }
}

fn scheduled_events(
    description: &WorkflowDescription,
    run_id: RunId,
) -> Vec<(Duration, LifecycleEvent)> {
    let count = |value| Count::try_from(value).expect("fixture count is valid");
    let node = |id: &str| NodeIdentity {
        id: id.into(),
        kind: format!("preview.{id}"),
        path: Vec::new(),
    };
    let record = |at_ms: i64, sequence: i64, event| {
        (
            Duration::from_millis(at_ms as u64),
            LifecycleEvent {
                workflow_id: description.workflow_id.clone(),
                run_id,
                sequence: count(sequence),
                event,
            },
        )
    };
    vec![
        record(
            0,
            1,
            Event::WorkflowStarted {
                elapsed_ns: count(0),
            },
        ),
        record(
            0,
            2,
            Event::NodeStarted {
                selected_inputs: Vec::new(),
                node: node("fetch"),
                position: count(0),
                elapsed_ns: count(0),
            },
        ),
        record(
            800,
            3,
            Event::NodeFinished {
                node: node("fetch"),
                position: count(0),
                elapsed_ns: count(800_000_000),
                duration_ns: Some(count(800_000_000)),
                outcome: Outcome::Succeeded,
                produced_ports: vec!["value".into()],
                skipped_ports: vec![],
                failure: None,
                loop_summary: None,
            },
        ),
        record(
            800,
            4,
            Event::NodeStarted {
                selected_inputs: Vec::new(),
                node: node("transform"),
                position: count(1),
                elapsed_ns: count(800_000_000),
            },
        ),
        record(
            2500,
            5,
            Event::NodeFinished {
                node: node("transform"),
                position: count(1),
                elapsed_ns: count(2_500_000_000),
                duration_ns: Some(count(1_700_000_000)),
                outcome: Outcome::Succeeded,
                produced_ports: vec!["value".into(), "ready".into()],
                skipped_ports: vec!["archive".into()],
                failure: None,
                loop_summary: None,
            },
        ),
        record(
            2500,
            6,
            Event::NodeStarted {
                selected_inputs: Vec::new(),
                node: node("publish"),
                position: count(2),
                elapsed_ns: count(2_500_000_000),
            },
        ),
        record(
            3500,
            7,
            Event::NodeFinished {
                node: node("publish"),
                position: count(2),
                elapsed_ns: count(3_500_000_000),
                duration_ns: Some(count(1_000_000_000)),
                outcome: Outcome::Succeeded,
                produced_ports: vec![],
                skipped_ports: vec![],
                failure: None,
                loop_summary: None,
            },
        ),
        record(
            3500,
            8,
            Event::NodeSkipped {
                selected_inputs: Vec::new(),
                node: node("archive"),
                position: count(3),
                elapsed_ns: count(3_500_000_000),
                causes: vec![SkipCause::Output {
                    source_node: "transform".into(),
                    source_output: "archive".into(),
                }],
                skipped_ports: vec!["value".into()],
            },
        ),
        record(
            3500,
            9,
            Event::WorkflowFinished {
                elapsed_ns: count(3_500_000_000),
                outcome: Outcome::Succeeded,
                failure_node_id: None,
                failure: None,
            },
        ),
    ]
}

fn preview_crossings() {
    let colored = std::env::args().any(|arg| arg == "--color");
    for horizontal in [EdgeKind::Data, EdgeKind::Control] {
        for vertical in [EdgeKind::Data, EdgeKind::Control] {
            for upper_horizontal in [true, false] {
                println!(
                    "Crossing: H={horizontal:?}, V={vertical:?}, upper={}",
                    if upper_horizontal {
                        "horizontal"
                    } else {
                        "vertical"
                    }
                );
                let mut cells = [[(" ", false); 9]; 9];
                let h = line_symbols(horizontal)[0];
                let v = line_symbols(vertical)[1];
                cells[4].fill((h, upper_horizontal));
                for row in &mut cells {
                    row[4] = (v, !upper_horizontal);
                }
                for cell in crossing_cells(horizontal, vertical, upper_horizontal) {
                    cells[(4 + cell.dy) as usize][(4 + cell.dx) as usize] =
                        (cell.symbol, cell.upper);
                }
                for row in cells {
                    for (symbol, upper) in row {
                        if colored {
                            print!("\x1b[{}m{symbol}\x1b[0m", if upper { "32;1" } else { "90" });
                        } else {
                            print!("{symbol}");
                        }
                    }
                    println!();
                }
            }
        }
    }
}

fn dump_graph() -> PreviewResult<()> {
    use ratatui::{
        buffer::Buffer,
        layout::{Position, Rect},
        widgets::Widget,
    };
    let mut fixture = description();
    if std::env::args().any(|arg| arg == "--crowded") {
        let names = [
            "source-a", "source-b", "middle-a", "middle-b", "sink-a", "sink-b",
        ];
        fixture.nodes = names
            .iter()
            .map(|id| NodeDescription {
                id: (*id).into(),
                kind: "preview.node".into(),
            })
            .collect();
        fixture.execution_order = names.iter().map(|id| (*id).into()).collect();
        fixture.data_edges.clear();
        fixture.control_edges.clear();
        for from in 0..4 {
            for to in if from < 2 { 2..6 } else { 4..6 } {
                fixture.data_edges.push(DataEdge {
                    exclusive: false,
                    from_node: names[from].into(),
                    from_output: "value".into(),
                    to_node: names[to].into(),
                    to_input: format!("input-{from}"),
                });
            }
        }
        for from in 0..2 {
            for to in 4..6 {
                fixture.control_edges.push(ControlEdge {
                    from_node: names[from].into(),
                    from_output: "gate".into(),
                    to_node: names[to].into(),
                });
            }
        }
    }
    if std::env::args().any(|arg| arg == "--parameters") {
        fixture.nodes = ["params", "settings"]
            .map(|id| NodeDescription {
                id: id.into(),
                kind: "preview.task".into(),
            })
            .to_vec();
        fixture.execution_order = vec!["params".into(), "settings".into()];
        fixture.control_edges.clear();
        fixture.data_edges = (0..18)
            .map(|index| DataEdge {
                exclusive: false,
                from_node: "params".into(),
                from_output: format!("field{index}"),
                to_node: "settings".into(),
                to_input: format!("input{index}"),
            })
            .collect();
    }
    let mut fixture = preview_description(fixture, std::env::args().any(|arg| arg == "--clusters"));
    let view = match std::env::args()
        .find_map(|arg| arg.strip_prefix("--view=").map(str::to_owned))
        .as_deref()
    {
        Some("execution") => ConnectionView::Execution,
        Some("dependencies") => ConnectionView::Dependencies,
        Some("ports") | None => ConnectionView::Ports,
        Some(_) => return Err("unknown connection view".into()),
    };
    let layout = if std::env::args().any(|arg| arg == "--body") {
        let body = body_description(&fixture);
        if fixture.clusters.is_empty() {
            fixture.version = WorkflowDescriptionVersion::V2026_09_29;
        }
        fixture.nodes = vec![NodeDescription {
            id: "repeat".into(),
            kind: "workflow.loop".into(),
        }];
        fixture.execution_order = vec!["repeat".into()];
        fixture.data_edges.clear();
        fixture.control_edges.clear();
        fixture.loop_bodies = vec![body];
        fixture.clusters.clear();
        GraphLayout::from_body_with_view(&fixture, &["repeat".into()], view)?
    } else {
        GraphLayout::with_view(&fixture, view)?
    };
    let mut buffer = Buffer::empty(Rect::new(
        0,
        0,
        layout.size().0.min(160) as u16,
        layout.size().1.min(90) as u16,
    ));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    println!(
        "Graph preview ({}): {} hidden data mappings, {} edges, {} crossings, {} unresolved, canvas {:?}",
        view.label(),
        layout.hidden_data_count(),
        layout.edges().len(),
        layout.crossings().len(),
        layout.unresolved_count(),
        layout.size()
    );
    for y in 0..buffer.area.height {
        let row = (0..buffer.area.width)
            .map(|x| buffer.cell(Position::new(x, y)).unwrap().symbol())
            .collect::<String>();
        println!("{}", row.trim_end());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::widgets::Widget;

    #[test]
    fn animation_clock_freezes_past_the_terminal_pulse() {
        let final_elapsed = Duration::from_millis(3_500);
        let pulse = Duration::from_millis(1_200);
        let capped = animation_clock(final_elapsed + SETTLING + IDLE_WAIT, final_elapsed);
        assert_eq!(capped, final_elapsed + SETTLING);
        assert!(capped >= final_elapsed + pulse);
        assert_eq!(
            animation_clock(Duration::from_millis(2_000), final_elapsed),
            Duration::from_millis(2_000)
        );

        let fixture = description();
        let layout = GraphLayout::new(&fixture).unwrap();
        let run_id = RunId::new();
        let mut state = SessionState::new(fixture.clone(), run_id).unwrap();
        let events = scheduled_events(&fixture, run_id);
        let last_event = events.last().unwrap().0;
        for (_, event) in events {
            state.apply(event).unwrap();
        }
        let snapshot = state.snapshot();
        let render = |elapsed: Duration| {
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 160, 60));
            GraphView::new(&layout)
                .snapshot(&snapshot)
                .elapsed_ns(elapsed.as_nanos() as u64)
                .render(buffer.area, &mut buffer);
            buffer
        };
        let pulsing = render(last_event + Duration::from_secs(1));
        let settled = render(animation_clock(
            last_event + Duration::from_secs(10),
            last_event,
        ));
        assert_ne!(
            pulsing, settled,
            "preview stopped during the terminal pulse"
        );
        assert_eq!(settled, render(last_event + Duration::from_secs(10)));
    }

    #[test]
    fn frame_index_advances_on_frame_boundaries() {
        assert_eq!(frame_index(Duration::ZERO), 0);
        assert_eq!(frame_index(Duration::from_millis(99)), 0);
        assert_eq!(frame_index(Duration::from_millis(100)), 1);
        assert_eq!(frame_index(Duration::from_millis(1_499)), 14);
    }

    #[test]
    fn next_frame_wait_subtracts_render_time() {
        let tick = frame_index(Duration::from_millis(130));
        assert_eq!(tick, 1);
        assert_eq!(
            next_frame_wait(Duration::from_millis(130), tick),
            Duration::from_millis(70)
        );
        assert_eq!(
            next_frame_wait(Duration::from_millis(205), tick),
            Duration::ZERO
        );
    }

    #[test]
    fn timing_advances_active_frames() {
        let mut timing = Timing::new(Duration::from_millis(3_500));

        let first = timing.advance(Duration::ZERO);
        assert!(first.redraw);
        assert_eq!(first.animation_elapsed, Duration::ZERO);

        let still = timing.advance(Duration::from_millis(99));
        assert!(!still.redraw);
        assert_eq!(still.animation_elapsed, Duration::from_millis(99));

        let next = timing.advance(Duration::from_millis(100));
        assert!(next.redraw);
        assert_eq!(next.animation_elapsed, Duration::from_millis(100));
    }

    #[test]
    fn timing_redraws_once_at_the_final_cap() {
        let final_elapsed = Duration::from_millis(3_500);
        let animation_end = final_elapsed + SETTLING;
        let mut timing = Timing::new(final_elapsed);
        timing.advance(animation_end - Duration::from_millis(1));

        let capped = timing.advance(animation_end);
        assert!(capped.redraw);
        assert_eq!(capped.animation_elapsed, animation_end);

        let idle = timing.advance(animation_end + IDLE_WAIT);
        assert!(!idle.redraw);
        assert_eq!(idle.animation_elapsed, animation_end);
    }

    #[test]
    fn timing_waits_for_the_frame_deadline_then_idles() {
        let final_elapsed = Duration::from_millis(3_500);
        let animation_end = final_elapsed + SETTLING;
        let mut timing = Timing::new(final_elapsed);

        timing.advance(Duration::from_millis(130));
        assert_eq!(
            timing.wait(Duration::from_millis(130)),
            Duration::from_millis(70)
        );
        assert_eq!(timing.wait(Duration::from_millis(205)), Duration::ZERO);

        let final_render = animation_end - Duration::from_millis(15);
        timing.advance(final_render);
        assert_eq!(timing.wait(final_render), Duration::from_millis(15));

        timing.advance(animation_end);
        assert_eq!(timing.wait(animation_end), IDLE_WAIT);
        assert_eq!(timing.wait(animation_end + IDLE_WAIT), IDLE_WAIT);
    }

    #[test]
    fn run_draws_a_frame_and_exits_on_quit() {
        fn poll(_: Duration) -> io::Result<bool> {
            Ok(true)
        }
        fn read() -> io::Result<TerminalEvent> {
            Ok(TerminalEvent::Key(KeyEvent::new(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
            )))
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        run(&mut terminal, Input { poll, read }).unwrap();
    }

    #[test]
    fn preview_modes_complete_with_valid_cluster_and_lifecycle_contracts() {
        for clusters in [false, true] {
            let fixture = preview_description(description(), clusters);
            let layout = GraphLayout::new(&fixture).unwrap();
            assert_eq!(layout.nodes().len(), fixture.nodes.len());
            assert_eq!(layout.clusters().len(), if clusters { 2 } else { 0 });
            assert_eq!(layout.unresolved_count(), 0);
            let body = body_description(&fixture);
            assert_eq!(body.clusters.len(), layout.clusters().len());

            let run_id = RunId::new();
            let mut state = SessionState::new(fixture.clone(), run_id).unwrap();
            for (_, event) in scheduled_events(&fixture, run_id) {
                state.apply(event).unwrap();
            }
            let snapshot = state.snapshot();
            assert_eq!(snapshot.workflow_outcome, Some(Outcome::Succeeded));
            assert_eq!(
                snapshot.nodes[0].status,
                mf_tui::state::NodeStatus::Succeeded
            );
            assert_eq!(snapshot.nodes[3].status, mf_tui::state::NodeStatus::Skipped);
        }
    }
}
