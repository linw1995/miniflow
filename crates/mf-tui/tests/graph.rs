use mf_telemetry::{
    Count,
    description::{
        ControlEdge, DataEdge, LoopBodyDescription, NodeDescription, WorkflowDescription,
        WorkflowDescriptionVersion,
    },
    event::{Event, LifecycleEvent, NodeIdentity, Outcome},
    identity::{RunId, WorkflowId},
};
use mf_tui::{
    graph::{EdgeKind, GraphLayout, GraphView},
    state::{NodeObservation, NodeStatus, SessionState},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Color,
    widgets::Widget,
};

fn description() -> WorkflowDescription {
    let nodes = ["source", "router", "left", "right", "join", "orphan"];
    WorkflowDescription {
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "b".repeat(64))).unwrap(),
        nodes: nodes
            .iter()
            .map(|id| NodeDescription {
                id: (*id).into(),
                kind: (*id).into(),
            })
            .collect(),
        data_edges: vec![
            DataEdge {
                from_node: "source".into(),
                from_output: "value".into(),
                to_node: "router".into(),
                to_input: "input".into(),
            },
            DataEdge {
                from_node: "left".into(),
                from_output: "value".into(),
                to_node: "join".into(),
                to_input: "left".into(),
            },
            DataEdge {
                from_node: "right".into(),
                from_output: "value".into(),
                to_node: "join".into(),
                to_input: "right".into(),
            },
        ],
        control_edges: vec![
            ControlEdge {
                from_node: "router".into(),
                from_output: "yes".into(),
                to_node: "left".into(),
            },
            ControlEdge {
                from_node: "router".into(),
                from_output: "no".into(),
                to_node: "right".into(),
            },
        ],
        execution_order: nodes.iter().map(|id| (*id).into()).collect(),
        loop_bodies: Vec::new(),
    }
}

#[test]
fn loop_body_layout_renders_the_selected_pass_independently() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_09_29;
    graph.nodes = vec![NodeDescription {
        id: "repeat".into(),
        kind: "workflow.loop".into(),
    }];
    graph.data_edges.clear();
    graph.control_edges.clear();
    graph.execution_order = vec!["repeat".into()];
    graph.loop_bodies = vec![LoopBodyDescription {
        path: vec!["repeat".into()],
        nodes: vec![
            NodeDescription {
                id: "$loop".into(),
                kind: "$loop".into(),
            },
            NodeDescription {
                id: "child".into(),
                kind: "fixture.child".into(),
            },
        ],
        data_edges: vec![DataEdge {
            from_node: "$loop".into(),
            from_output: "count".into(),
            to_node: "child".into(),
            to_input: "input".into(),
        }],
        control_edges: vec![],
        execution_order: vec!["$loop".into(), "child".into()],
    }];
    let layout = GraphLayout::from_loop_body(&graph, &["repeat".into()]).unwrap();
    assert_eq!(layout.nodes().len(), 2);
    assert_eq!(layout.edges().len(), 1);
    let mut nodes = vec![
        NodeObservation::pending("$loop", "$loop"),
        NodeObservation::pending("child", "fixture.child"),
    ];
    nodes[1].status = NodeStatus::Running;
    let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 20));
    GraphView::new(&layout)
        .nodes(&nodes)
        .render(buffer.area, &mut buffer);
    let rendered = buffer_text(&buffer);
    assert!(rendered.contains("$loop") && rendered.contains("child"));
}

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer.cell(Position::new(x, y)).unwrap().symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn layered_layout_keeps_all_nodes_and_both_edge_kinds() {
    let graph = description();
    let layout = GraphLayout::new(&graph).unwrap();
    assert_eq!(layout.nodes().len(), graph.nodes.len());
    assert_eq!(layout.edges().len(), 5);
    assert_eq!(
        layout
            .edges()
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Control)
            .count(),
        2
    );
    for left in layout.nodes() {
        for right in layout.nodes().iter().filter(|node| node.id != left.id) {
            assert!(
                left.rect.x + left.rect.width <= right.rect.x
                    || right.rect.x + right.rect.width <= left.rect.x
                    || left.rect.y + left.rect.height <= right.rect.y
                    || right.rect.y + right.rect.height <= left.rect.y,
                "{} and {} overlap",
                left.id,
                right.id
            );
        }
    }
    for edge in layout.edges() {
        assert!(layout.nodes()[edge.from].rect.x < layout.nodes()[edge.to].rect.x);
    }
    let orphan = layout
        .nodes()
        .iter()
        .find(|node| node.id == "orphan")
        .unwrap();
    assert!(orphan.rect.y > 0);
    let expected: Vec<_> = layout.nodes().iter().map(|node| node.rect).collect();
    for _ in 0..8 {
        let next = GraphLayout::new(&graph).unwrap();
        assert_eq!(
            next.nodes()
                .iter()
                .map(|node| node.rect)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn empty_workflow_has_an_empty_bounded_canvas() {
    let mut graph = description();
    graph.nodes.clear();
    graph.data_edges.clear();
    graph.control_edges.clear();
    graph.execution_order.clear();
    let layout = GraphLayout::new(&graph).unwrap();
    assert!(layout.nodes().is_empty());
    assert!(layout.edges().is_empty());
    assert_eq!(layout.size(), (4, 4));
}

#[test]
fn parallel_data_and_control_edges_use_separate_block_rows() {
    let mut graph = description();
    graph.control_edges.push(ControlEdge {
        from_node: "source".into(),
        from_output: "gate".into(),
        to_node: "router".into(),
    });
    let layout = GraphLayout::new(&graph).unwrap();
    let data = layout
        .edges()
        .iter()
        .find(|edge| edge.from == 0 && edge.to == 1 && edge.kind == EdgeKind::Data)
        .unwrap();
    let control = layout
        .edges()
        .iter()
        .find(|edge| edge.from == 0 && edge.to == 1 && edge.kind == EdgeKind::Control)
        .unwrap();
    assert_ne!(data.from_row, control.from_row);
    assert_ne!(data.to_row, control.to_row);

    let mut buffer = Buffer::empty(Rect::new(0, 0, 150, 60));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    let source = layout.nodes()[0].rect;
    let data_symbol = buffer
        .cell(Position::new(
            (source.x + source.width) as u16,
            (source.y + data.from_row) as u16,
        ))
        .unwrap()
        .symbol();
    let control_symbol = buffer
        .cell(Position::new(
            (source.x + source.width) as u16,
            (source.y + control.from_row) as u16,
        ))
        .unwrap()
        .symbol();
    assert_eq!(data_symbol, "═");
    assert_eq!(control_symbol, "─");
}

#[test]
fn opaque_node_names_stay_on_one_terminal_line() {
    let mut graph = description();
    graph.nodes[0].id = "source\nextra".into();
    graph.execution_order[0] = "source\nextra".into();
    graph.data_edges[0].from_node = "source\nextra".into();
    let layout = GraphLayout::new(&graph).unwrap();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 150, 60));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    let text = buffer_text(&buffer);
    assert!(text.contains("source extra"));
    assert!(!text.contains("source\nextra"));
}

#[test]
fn graph_renders_blocks_edges_and_running_elapsed_time() {
    let graph = description();
    let layout = GraphLayout::new(&graph).unwrap();
    let run_id = RunId::new();
    let mut state = SessionState::new(graph.clone(), run_id).unwrap();
    state
        .apply(LifecycleEvent {
            workflow_id: graph.workflow_id.clone(),
            run_id,
            sequence: count(1),
            event: Event::WorkflowStarted {
                node_count: count(6),
                elapsed_ns: count(0),
            },
        })
        .unwrap();
    state
        .apply(LifecycleEvent {
            workflow_id: graph.workflow_id,
            run_id,
            sequence: count(2),
            event: Event::NodeStarted {
                node: NodeIdentity {
                    id: "source".into(),
                    kind: "source".into(),
                    path: Vec::new(),
                },
                position: count(0),
                elapsed_ns: count(100),
            },
        })
        .unwrap();
    let snapshot = state.snapshot();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 150, 45));
    GraphView::new(&layout)
        .snapshot(&snapshot)
        .elapsed_ns(1_200_000_100)
        .render(buffer.area, &mut buffer);
    let text = buffer_text(&buffer);
    assert!(text.contains("source"));
    assert!(text.contains("1.2s"));
    assert!(text.contains("router"));
    assert!(text.contains("yes"));
    assert!(text.contains("no"));
    assert!(text.contains('═'));
    assert!(text.contains('─'));

    let mut panned = Buffer::empty(Rect::new(0, 0, 20, 8));
    GraphView::new(&layout)
        .offset(15, 1)
        .render(panned.area, &mut panned);
    assert!(buffer_text(&panned).contains('─'));

    state
        .apply(LifecycleEvent {
            workflow_id: description().workflow_id,
            run_id,
            sequence: count(3),
            event: Event::NodeFinished {
                node: NodeIdentity {
                    id: "source".into(),
                    kind: "source".into(),
                    path: Vec::new(),
                },
                position: count(0),
                elapsed_ns: count(1_200_000_100),
                duration_ns: Some(count(1_200_000_000)),
                outcome: Outcome::Succeeded,
                produced_ports: vec!["value".into()],
                skipped_ports: vec![],
                failure: None,
                loop_summary: None,
            },
        })
        .unwrap();
    let snapshot = state.snapshot();
    let mut completed = Buffer::empty(Rect::new(0, 0, 150, 45));
    GraphView::new(&layout)
        .snapshot(&snapshot)
        .render(completed.area, &mut completed);
    let source = layout
        .nodes()
        .iter()
        .find(|node| node.id == "source")
        .unwrap();
    let edge_cell = completed
        .cell(Position::new(
            (source.rect.x + source.rect.width) as u16,
            (source.rect.y + layout.edges()[0].from_row) as u16,
        ))
        .unwrap();
    assert_eq!(edge_cell.fg, Color::Green);
}

#[test]
fn known_sequence_gap_marks_last_seen_running_state_as_uncertain() {
    let graph = description();
    let layout = GraphLayout::new(&graph).unwrap();
    let run_id = RunId::new();
    let mut state = SessionState::new(graph.clone(), run_id).unwrap();
    state
        .apply(LifecycleEvent {
            workflow_id: graph.workflow_id,
            run_id,
            sequence: count(2),
            event: Event::NodeStarted {
                node: NodeIdentity {
                    id: "source".into(),
                    kind: "source".into(),
                    path: Vec::new(),
                },
                position: count(0),
                elapsed_ns: count(100),
            },
        })
        .unwrap();
    let snapshot = state.snapshot();
    assert!(snapshot.nodes[0].possibly_missing_events);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 150, 45));
    GraphView::new(&layout)
        .snapshot(&snapshot)
        .elapsed_ns(1_200_000_100)
        .render(buffer.area, &mut buffer);
    assert!(buffer_text(&buffer).contains("? source"));
    assert!(buffer_text(&buffer).contains("missing events"));

    state.close();
    let snapshot = state.snapshot();
    let mut closed = Buffer::empty(Rect::new(0, 0, 150, 45));
    GraphView::new(&layout)
        .snapshot(&snapshot)
        .render(closed.area, &mut closed);
    assert!(buffer_text(&closed).contains("last running"));
}
