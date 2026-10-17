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
    style::{Color, Modifier},
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
        execution: None,
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
                id: "%loop".into(),
                kind: "%loop".into(),
            },
            NodeDescription {
                id: "child".into(),
                kind: "fixture.child".into(),
            },
        ],
        data_edges: vec![DataEdge {
            from_node: "%loop".into(),
            from_output: "count".into(),
            to_node: "child".into(),
            to_input: "input".into(),
        }],
        control_edges: vec![],
        execution_order: vec!["%loop".into(), "child".into()],
    }];
    let layout = GraphLayout::from_loop_body(&graph, &["repeat".into()]).unwrap();
    assert_eq!(layout.nodes().len(), 2);
    assert_eq!(layout.edges().len(), 1);
    let mut nodes = vec![
        NodeObservation::pending("%loop", "%loop"),
        NodeObservation::pending("child", "fixture.child"),
    ];
    nodes[1].status = NodeStatus::Running;
    let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 20));
    GraphView::new(&layout)
        .nodes(&nodes)
        .render(buffer.area, &mut buffer);
    let rendered = buffer_text(&buffer);
    assert!(rendered.contains("%loop") && rendered.contains("child"));
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
    assert_eq!(data_symbol, "─");
    assert_eq!(control_symbol, "─");
    for edge in [data, control] {
        let target = layout.nodes()[edge.to].rect;
        let color = match edge.kind {
            EdgeKind::Data => Color::Cyan,
            EdgeKind::Control => Color::Blue,
        };
        for (x, y, symbol) in [
            (source.x + source.width - 1, source.y + edge.from_row, "├"),
            (target.x - 1, target.y + edge.to_row, "▶"),
            (target.x, target.y + edge.to_row, "┤"),
        ] {
            let cell = buffer.cell(Position::new(x as u16, y as u16)).unwrap();
            assert_eq!(cell.symbol(), symbol);
            assert_eq!(cell.fg, color);
        }
    }
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
    assert!(text.contains('├'));
    assert!(text.contains("▶┤"));
    assert!(text.contains('─'));

    let mut focused = buffer.clone();
    for selected in [0, 1, 0] {
        GraphView::new(&layout)
            .snapshot(&snapshot)
            .elapsed_ns(1_200_000_100)
            .selected(selected)
            .render(focused.area, &mut focused);
        for index in [0, 1] {
            let rect = layout.nodes()[index].rect;
            let position = Position::new(rect.x as u16 + 1, rect.y as u16 + 1);
            let original = buffer.cell(position).unwrap();
            let actual = focused.cell(position).unwrap();
            assert_eq!(actual.symbol(), original.symbol());
            assert_eq!(actual.fg, original.fg);
            assert_eq!(actual.bg, original.bg);
            assert_eq!(
                actual.modifier.contains(Modifier::REVERSED),
                index == selected
            );
            if index != selected {
                assert_eq!(actual, original);
            }
        }
    }

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

#[test]
fn stored_routes_preserve_connection_and_observation_identity() {
    let graph = description();
    let layout = GraphLayout::new(&graph).unwrap();
    assert_eq!(layout.routes().len(), layout.edges().len());
    for (edge, route) in layout.edges().iter().zip(layout.routes()) {
        assert_eq!(edge.key.source, layout.nodes()[edge.from].id);
        assert_eq!(edge.key.destination, layout.nodes()[edge.to].id);
        assert_eq!(edge.key.output, edge.from_output);
        assert_eq!(edge.key.input, edge.to_input);
        let source = layout.nodes()[edge.from].rect;
        let target = layout.nodes()[edge.to].rect;
        assert_eq!(route.points.first().unwrap().x, source.x + source.width);
        assert_eq!(route.points.last().unwrap().x, target.x - 1);
        assert!(route.segments().all(|segment| segment.start.x == segment.end.x || segment.start.y == segment.end.y));
    }
    assert!(layout.crossings().is_empty());
    assert_eq!(
        layout
            .nodes()
            .iter()
            .map(|node| &node.id)
            .collect::<Vec<_>>(),
        graph.execution_order.iter().collect::<Vec<_>>()
    );
}

fn fan_graph(count: usize) -> WorkflowDescription {
    let mut graph = description();
    let names: Vec<_> = std::iter::once("source".to_owned())
        .chain((0..count).map(|index| format!("sink-{index:04}")))
        .collect();
    graph.nodes = names
        .iter()
        .map(|id| NodeDescription {
            id: id.clone(),
            kind: "fixture.node".into(),
        })
        .collect();
    graph.execution_order = names;
    graph.control_edges.clear();
    graph.data_edges = (0..count)
        .map(|index| DataEdge {
            from_node: "source".into(),
            from_output: "value".into(),
            to_node: format!("sink-{index:04}"),
            to_input: "input".into(),
        })
        .collect();
    graph
}

fn assert_completed_geometry_is_safe(layout: &GraphLayout) {
    use mf_tui::graph::routing::Intersection;
    for edge in layout.edges() {
        assert!(edge.from_row > 0 && edge.from_row < layout.nodes()[edge.from].rect.height - 1);
        assert!(edge.to_row > 0 && edge.to_row < layout.nodes()[edge.to].rect.height - 1);
    }
    for (index, route) in layout.routes().iter().enumerate() {
        if route.unresolved.is_some() {
            assert!(route.points.is_empty());
            continue;
        }
        for segment in route.segments() {
            for node in layout.nodes() {
                assert!(
                    !segment.intersects_rect(node.rect),
                    "edge {index} intersects {}",
                    node.id
                );
            }
            for (other_index, other) in layout.routes().iter().enumerate().skip(index + 1) {
                for other_segment in other.segments() {
                    match segment.intersection(other_segment) {
                        None => {}
                        Some(Intersection::Crossing(point)) => {
                            assert!(layout.crossings().iter().any(|crossing| crossing.point
                                == point
                                && [crossing.horizontal, crossing.vertical].contains(&index)
                                && [crossing.horizontal, crossing.vertical].contains(&other_index)))
                        }
                        collision => panic!("edges {index}/{other_index} collide: {collision:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn compact_chain_aligns_ports_without_vertical_detours() {
    let layout = GraphLayout::new(&fan_graph(1)).unwrap();
    assert!(layout.size().1 <= 8);
    let route = &layout.routes()[0];
    assert_eq!(route.points.len(), 2);
    assert_eq!(route.points[0].y, route.points[1].y);
    assert_completed_geometry_is_safe(&layout);
}

#[test]
fn crowded_attachments_expand_nodes_and_channels_without_reusing_rows() {
    let graph = fan_graph(9);
    let layout = GraphLayout::new(&graph).unwrap();
    let rows: std::collections::BTreeSet<_> =
        layout.edges().iter().map(|edge| edge.from_row).collect();
    assert_eq!(rows.len(), 9);
    assert!(layout.nodes()[0].rect.height > 6);
    assert!(
        layout.nodes()[1].rect.x - (layout.nodes()[0].rect.x + layout.nodes()[0].rect.width) > 10
    );
    assert_completed_geometry_is_safe(&layout);
    assert!(
        layout
            .routes()
            .iter()
            .all(|route| route.unresolved.is_none())
    );
}

#[test]
fn long_edges_use_passages_and_keep_isolated_components_disjoint() {
    let mut graph = fan_graph(3);
    graph.data_edges.push(DataEdge {
        from_node: "sink-0000".into(),
        from_output: "value".into(),
        to_node: "sink-0001".into(),
        to_input: "extra".into(),
    });
    graph.data_edges.push(DataEdge {
        from_node: "sink-0001".into(),
        from_output: "value".into(),
        to_node: "sink-0002".into(),
        to_input: "extra".into(),
    });
    graph.nodes.push(NodeDescription {
        id: "orphan".into(),
        kind: "fixture.orphan".into(),
    });
    graph.execution_order.push("orphan".into());
    let layout = GraphLayout::new(&graph).unwrap();
    assert_completed_geometry_is_safe(&layout);
    for route in layout.routes() {
        for point in &route.points {
            assert!(point.x < layout.size().0 && point.y < layout.size().1);
        }
    }
    assert!(layout.routes().iter().any(|route| route.points.len() > 4));
}

#[test]
fn control_labels_are_sanitized_truncated_and_reserved_in_cell_units() {
    let mut graph = fan_graph(1);
    graph.control_edges.push(ControlEdge {
        from_node: "source".into(),
        from_output: "wide-🦀-combining-e\u{301}-tail\nextra".into(),
        to_node: "sink-0000".into(),
    });
    let layout = GraphLayout::new(&graph).unwrap();
    let label = layout
        .routes()
        .iter()
        .find_map(|route| route.label.as_ref())
        .unwrap();
    assert!(!label.text.contains('\n'));
    assert!(label.rect.width <= 12);
    assert_eq!(
        label.rect.width as usize,
        ratatui::text::Span::raw(&label.text).width()
    );
    assert_completed_geometry_is_safe(&layout);
}

#[test]
fn crowded_inputs_have_distinct_rows_and_disjoint_boxes() {
    let mut graph = fan_graph(9);
    graph.execution_order.rotate_left(1);
    graph.data_edges = (0..9)
        .map(|index| DataEdge {
            from_node: format!("sink-{index:04}"),
            from_output: "value".into(),
            to_node: "source".into(),
            to_input: format!("input-{index}"),
        })
        .collect();
    let layout = GraphLayout::new(&graph).unwrap();
    let rows: std::collections::BTreeSet<_> =
        layout.edges().iter().map(|edge| edge.to_row).collect();
    assert_eq!(rows.len(), 9);
    assert!(layout.nodes().last().unwrap().rect.height > 6);
    for (index, node) in layout.nodes().iter().enumerate() {
        for other in layout.nodes().iter().skip(index + 1) {
            assert!(!mf_tui::graph::routing::rects_intersect(
                node.rect, other.rect
            ));
        }
    }
    assert_completed_geometry_is_safe(&layout);
    assert!(
        layout
            .routes()
            .iter()
            .all(|route| route.unresolved.is_none())
    );
}

#[test]
fn routing_budgets_retain_all_connections_without_unsafe_paths() {
    use mf_tui::graph::routing::{RoutingLimits, UnresolvedReason};
    let graph = fan_graph(9);
    for (limits, reason) in [
        (
            RoutingLimits {
                max_geometry_items: 0,
                ..RoutingLimits::default()
            },
            UnresolvedReason::GeometryLimit,
        ),
        (
            RoutingLimits {
                max_conflict_checks: 0,
                ..RoutingLimits::default()
            },
            UnresolvedReason::ConflictLimit,
        ),
        (
            RoutingLimits {
                max_extent: 0,
                ..RoutingLimits::default()
            },
            UnresolvedReason::GeometryLimit,
        ),
    ] {
        let layout = GraphLayout::with_routing_limits(&graph, limits).unwrap();
        assert_eq!(layout.edges().len(), graph.data_edges.len());
        assert!(
            layout
                .routes()
                .iter()
                .all(|route| route.points.is_empty() && route.unresolved == Some(reason))
        );
        assert!(layout.routing_stats().conflict_checks <= limits.max_conflict_checks);
        assert!(layout.routing_stats().geometry_items <= limits.max_geometry_items);
        assert_completed_geometry_is_safe(&layout);
    }
    let graph = crowded_graph(4, true);
    for limits in [
        RoutingLimits {
            max_geometry_items: 1_000,
            ..RoutingLimits::default()
        },
        RoutingLimits {
            max_conflict_checks: 5_000,
            ..RoutingLimits::default()
        },
        RoutingLimits {
            max_total_expansions: 16,
            retry_rounds: 1,
            ..RoutingLimits::default()
        },
    ] {
        let layout = GraphLayout::with_routing_limits(&graph, limits).unwrap();
        let stats = layout.routing_stats();
        assert_eq!(
            layout.edges().len(),
            graph.data_edges.len() + graph.control_edges.len()
        );
        assert!(stats.geometry_items <= limits.max_geometry_items);
        assert!(stats.conflict_checks <= limits.max_conflict_checks);
        assert!(stats.search_expansions <= limits.max_total_expansions);
        assert!(stats.retry_rounds <= limits.retry_rounds);
        assert!(layout.crossings().len() <= limits.max_crossings);
        assert_completed_geometry_is_safe(&layout);
    }
}

#[test]
fn sparse_ten_thousand_node_chain_uses_linear_conflict_work() {
    let mut graph = fan_graph(9_999);
    graph.data_edges = (1..graph.execution_order.len())
        .map(|index| DataEdge {
            from_node: graph.execution_order[index - 1].clone(),
            from_output: "value".into(),
            to_node: graph.execution_order[index].clone(),
            to_input: "input".into(),
        })
        .collect();
    let layout = GraphLayout::new(&graph).unwrap();
    assert_eq!(layout.nodes().len(), 10_000);
    assert!(
        layout
            .routes()
            .iter()
            .all(|route| route.unresolved.is_none())
    );
    assert!(layout.routing_stats().conflict_checks < 10 * layout.edges().len());
    assert_eq!(layout.routing_stats().search_expansions, 0);
}

#[test]
fn local_rerouting_never_replaces_a_better_valid_arrangement() {
    use mf_tui::graph::routing::RoutingLimits;
    let mut graph = fan_graph(8);
    graph.data_edges.clear();
    for from in 0..6 {
        let first = if from < 3 { 3 } else { 6 };
        for to in first..9 {
            graph.data_edges.push(DataEdge {
                from_node: graph.execution_order[from].clone(),
                from_output: "value".into(),
                to_node: graph.execution_order[to].clone(),
                to_input: format!("input-{from}"),
            });
        }
    }
    let limits = RoutingLimits {
        max_crossings: 10,
        max_search_vertices: 0,
        ..RoutingLimits::default()
    };
    let initial = GraphLayout::with_routing_limits(
        &graph,
        RoutingLimits {
            retry_rounds: 0,
            ..limits
        },
    )
    .unwrap();
    let improved = GraphLayout::with_routing_limits(&graph, limits).unwrap();
    assert!(
        improved
            .routes()
            .iter()
            .filter(|route| route.unresolved.is_some())
            .count()
            <= initial
                .routes()
                .iter()
                .filter(|route| route.unresolved.is_some())
                .count()
    );
    assert!(improved.routing_stats().retry_rounds > 0);
    assert!(improved.routing_stats().retry_rounds <= limits.retry_rounds);
    assert_completed_geometry_is_safe(&improved);
}

#[test]
fn clipped_tall_nodes_and_wide_labels_match_the_full_canvas() {
    let mut graph = fan_graph(9);
    graph.control_edges.push(ControlEdge {
        from_node: "source".into(),
        from_output: "wide-🦀-e\u{301}".into(),
        to_node: "sink-0000".into(),
    });
    let layout = GraphLayout::new(&graph).unwrap();
    let mut full = Buffer::empty(Rect::new(
        0,
        0,
        layout.size().0 as u16,
        layout.size().1 as u16,
    ));
    GraphView::new(&layout).render(full.area, &mut full);
    let source = layout.nodes()[0].rect;
    let label = layout
        .routes()
        .iter()
        .find_map(|route| route.label.as_ref())
        .unwrap();
    for offset in [
        (source.x + 2, source.y),
        (source.x + 1, source.y + 1),
        (label.rect.x + 5, label.rect.y),
        (label.rect.x + 6, label.rect.y),
        (source.x + 20, source.y + source.height - 2),
    ] {
        let mut clipped = Buffer::empty(Rect::new(5, 3, 8, 4));
        GraphView::new(&layout)
            .offset(offset.0, offset.1)
            .render(clipped.area, &mut clipped);
        for y in 0..4 {
            for x in 0..8 {
                let expected = full.cell(Position::new(offset.0 as u16 + x, offset.1 as u16 + y));
                if let Some(expected) = expected {
                    assert_eq!(clipped.cell(Position::new(5 + x, 3 + y)).unwrap(), expected);
                }
            }
        }
    }
}

#[test]
fn unresolved_connections_keep_inspectable_kind_ports_and_reason() {
    use mf_tui::graph::routing::RoutingLimits;
    let mut graph = fan_graph(1);
    graph.control_edges.push(ControlEdge {
        from_node: "source".into(),
        from_output: "gate".into(),
        to_node: "sink-0000".into(),
    });
    let layout = GraphLayout::with_routing_limits(
        &graph,
        RoutingLimits {
            max_conflict_checks: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(layout.unresolved_count(), 2);
    let details = layout.routing_details(0);
    assert!(details.contains("Data: source.value -> sink-0000.input"));
    assert!(details.contains("Control: source.gate -> sink-0000"));
    assert!(details.contains("routing conflict budget exhausted"));
    assert_eq!(details, layout.routing_details(1));
}

fn crowded_graph(width: usize, control: bool) -> WorkflowDescription {
    let mut graph = fan_graph(width * 3 - 1);
    for (index, node) in graph.nodes.iter_mut().enumerate() {
        node.id = format!("node-{index:05}");
    }
    if width == 2 {
        let names = [
            "source-a", "source-b", "middle-a", "middle-b", "sink-a", "sink-b",
        ];
        for (node, name) in graph.nodes.iter_mut().zip(names) {
            node.id = name.into();
        }
    }
    graph.execution_order = graph.nodes.iter().map(|node| node.id.clone()).collect();
    graph.data_edges.clear();
    for from in 0..width * 2 {
        for to in if from < width {
            width..width * 3
        } else {
            width * 2..width * 3
        } {
            graph.data_edges.push(DataEdge {
                from_node: graph.execution_order[from].clone(),
                from_output: "value".into(),
                to_node: graph.execution_order[to].clone(),
                to_input: format!("input-{from}"),
            });
        }
    }
    if control {
        for from in 0..width {
            for to in width * 2..width * 3 {
                graph.control_edges.push(ControlEdge {
                    from_node: graph.execution_order[from].clone(),
                    from_output: "gate".into(),
                    to_node: graph.execution_order[to].clone(),
                });
            }
        }
    }
    graph
}

#[test]
fn input_permutations_keep_geometry_crossing_layers_and_rendered_styles() {
    use std::collections::{BTreeMap, BTreeSet};
    for (mut graph, height_bound) in [
        (crowded_graph(2, true), 100),
        (crowded_graph(4, false), 240),
        (crowded_graph(4, true), 500),
    ] {
        let baseline = GraphLayout::new(&graph).unwrap();
        assert!(!baseline.crossings().is_empty());
        assert_eq!(baseline.unresolved_count(), 0);
        assert!(baseline.size().1 < height_bound);
        assert_completed_geometry_is_safe(&baseline);
        let signature = |layout: &GraphLayout| {
            let routes = layout
                .edges()
                .iter()
                .zip(layout.routes())
                .map(|(edge, route)| (edge.key.clone(), route.clone()))
                .collect::<BTreeMap<_, _>>();
            let crossings = layout
                .crossings()
                .iter()
                .map(|crossing| {
                    (
                        crossing.point,
                        layout.edges()[crossing.horizontal].key.clone(),
                        layout.edges()[crossing.vertical].key.clone(),
                        layout.edges()[crossing.upper].key.clone(),
                    )
                })
                .collect::<BTreeSet<_>>();
            (layout.nodes().to_vec(), routes, crossings)
        };
        let expected = signature(&baseline);
        let mut observations: Vec<_> = baseline
            .nodes()
            .iter()
            .map(|node| NodeObservation::pending(&node.id, &node.kind))
            .collect();
        for (index, node) in observations.iter_mut().enumerate() {
            if index % 2 == 0 {
                node.status = NodeStatus::Succeeded;
                node.produced_ports.push("value".into());
                node.skipped_ports.push("gate".into());
            } else {
                node.status = NodeStatus::Unknown;
            }
        }
        let area = Rect::new(0, 0, baseline.size().0 as u16, baseline.size().1 as u16);
        let mut reference = Buffer::empty(area);
        GraphView::new(&baseline)
            .nodes(&observations)
            .render(area, &mut reference);
        for index in 0..8 {
            graph.data_edges.rotate_left(1);
            if !graph.control_edges.is_empty() {
                graph.control_edges.rotate_right(1);
            }
            if index % 2 == 0 {
                graph.data_edges.reverse();
            }
            let layout = GraphLayout::new(&graph).unwrap();
            assert_eq!(signature(&layout), expected);
            let mut rendered = Buffer::empty(area);
            GraphView::new(&layout)
                .nodes(&observations)
                .render(area, &mut rendered);
            assert_eq!(rendered, reference);
        }
    }
}

#[test]
fn root_and_loop_body_share_routing_geometry() {
    let body_graph = crowded_graph(4, true);
    let root = GraphLayout::new(&body_graph).unwrap();
    let mut description = body_graph.clone();
    description.version = WorkflowDescriptionVersion::V2026_09_29;
    description.nodes = vec![NodeDescription {
        id: "repeat".into(),
        kind: "workflow.loop".into(),
    }];
    description.execution_order = vec!["repeat".into()];
    description.data_edges.clear();
    description.control_edges.clear();
    description.loop_bodies = vec![LoopBodyDescription {
        path: vec!["repeat".into()],
        nodes: body_graph.nodes,
        data_edges: body_graph.data_edges,
        control_edges: body_graph.control_edges,
        execution_order: body_graph.execution_order,
    }];
    let body = GraphLayout::from_loop_body(&description, &["repeat".into()]).unwrap();
    assert_eq!(body.nodes(), root.nodes());
    assert_eq!(body.routes(), root.routes());
    assert_eq!(body.crossings(), root.crossings());
}

#[test]
fn a_wide_label_fragment_cannot_overwrite_the_next_pane() {
    let mut graph = fan_graph(1);
    graph.control_edges.push(ControlEdge {
        from_node: "source".into(),
        from_output: "🦀".into(),
        to_node: "sink-0000".into(),
    });
    let layout = GraphLayout::new(&graph).unwrap();
    let label = layout
        .routes()
        .iter()
        .find_map(|route| route.label.as_ref())
        .unwrap();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
    buffer
        .cell_mut(Position::new(1, 0))
        .unwrap()
        .set_symbol("#");
    GraphView::new(&layout)
        .offset(label.rect.x, label.rect.y)
        .render(Rect::new(0, 0, 1, 1), &mut buffer);
    assert_eq!(buffer.cell(Position::new(0, 0)).unwrap().symbol(), " ");
    assert_eq!(buffer.cell(Position::new(1, 0)).unwrap().symbol(), "#");
}
