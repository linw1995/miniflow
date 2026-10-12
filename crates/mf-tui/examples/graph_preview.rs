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
        ControlEdge, DataEdge, NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
    },
    event::{Event, LifecycleEvent, NodeIdentity, Outcome, SkipCause},
    identity::{RunId, WorkflowId},
};
use mf_tui::{
    graph::{EdgeKind, GraphLayout, GraphView, routing::crossing_cells},
    state::SessionState,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::Paragraph,
};
use std::{
    error::Error,
    io::{self, IsTerminal},
    time::{Duration, Instant},
};

type PreviewResult<T> = Result<T, Box<dyn Error>>;

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
    let result = run(&mut terminal);
    drop(terminal);
    drop(guard);
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stderr>>) -> PreviewResult<()> {
    let description = description();
    let layout = GraphLayout::new(&description)?;
    let run_id = RunId::new();
    let mut state = SessionState::new(description.clone(), run_id)?;
    let events = scheduled_events(&description, run_id);
    let mut next_event = 0;
    let mut offset = (0u32, 0u32);
    let started = Instant::now();
    let final_elapsed = events.last().map_or(Duration::ZERO, |entry| entry.0);
    let mut frame_tick = u128::MAX;
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
        let tick = displayed_elapsed.as_millis() / 200;
        if tick != frame_tick {
            frame_tick = tick;
            redraw = true;
        }
        if redraw {
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
                        .elapsed_ns(displayed_elapsed.as_nanos() as u64),
                    graph,
                );
                frame.render_widget(
                    Paragraph::new("Arrows: pan  |  f: origin  |  q/Esc: quit"),
                    footer,
                );
            })?;
            redraw = false;
        }

        let wait = if complete {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(80)
        };
        if event::poll(wait)? {
            match event::read()? {
                TerminalEvent::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('f') => offset = (0, 0),
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

fn description() -> WorkflowDescription {
    WorkflowDescription {
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
                from_node: "fetch".into(),
                from_output: "value".into(),
                to_node: "transform".into(),
                to_input: "input".into(),
            },
            DataEdge {
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
                node_count: count(4),
                elapsed_ns: count(0),
            },
        ),
        record(
            0,
            2,
            Event::NodeStarted {
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
                node: node("archive"),
                position: count(3),
                elapsed_ns: count(3_500_000_000),
                causes: vec![SkipCause {
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
                final_sequence: count(9),
                elapsed_ns: count(3_500_000_000),
                visited_node_count: count(4),
                outcome: Outcome::Succeeded,
                failure_node_id: None,
                failure: None,
                top_level_visited_count: None,
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
                    "Underpass: H={horizontal:?}, V={vertical:?}, upper={}",
                    if upper_horizontal {
                        "horizontal"
                    } else {
                        "vertical"
                    }
                );
                let mut cells = [[(" ", false); 9]; 9];
                let h = if horizontal == EdgeKind::Data {
                    "═"
                } else {
                    "─"
                };
                let v = if vertical == EdgeKind::Data {
                    "║"
                } else {
                    "│"
                };
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
    let layout = if std::env::args().any(|arg| arg == "--body") {
        let body = mf_telemetry::description::LoopBodyDescription {
            path: vec!["repeat".into()],
            nodes: fixture.nodes.clone(),
            data_edges: fixture.data_edges.clone(),
            control_edges: fixture.control_edges.clone(),
            execution_order: fixture.execution_order.clone(),
        };
        fixture.version = WorkflowDescriptionVersion::V2026_09_29;
        fixture.nodes = vec![NodeDescription {
            id: "repeat".into(),
            kind: "workflow.loop".into(),
        }];
        fixture.execution_order = vec!["repeat".into()];
        fixture.data_edges.clear();
        fixture.control_edges.clear();
        fixture.loop_bodies = vec![body];
        GraphLayout::from_loop_body(&fixture, &["repeat".into()])?
    } else {
        GraphLayout::new(&fixture)?
    };
    let mut buffer = Buffer::empty(Rect::new(
        0,
        0,
        layout.size().0.min(160) as u16,
        layout.size().1.min(90) as u16,
    ));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    println!(
        "Graph preview: {} edges, {} crossings, {} unresolved, canvas {:?}",
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
