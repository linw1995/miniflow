use mf_telemetry::{
    Count,
    description::{
        ClusterDescription, ControlEdge, DataEdge, ExecutionDescription, ExecutionMode,
        LoopBodyDescription, NodeDescription, WorkflowDescription, WorkflowDescriptionVersion,
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
use std::collections::BTreeSet;

fn description() -> WorkflowDescription {
    let nodes = ["source", "router", "left", "right", "join", "orphan"];
    WorkflowDescription {
        order_edges: Vec::new(),
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
                exclusive: false,
                from_node: "source".into(),
                from_output: "value".into(),
                to_node: "router".into(),
                to_input: "input".into(),
            },
            DataEdge {
                exclusive: false,
                from_node: "left".into(),
                from_output: "value".into(),
                to_node: "join".into(),
                to_input: "left".into(),
            },
            DataEdge {
                exclusive: false,
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
        clusters: Vec::new(),
    }
}

#[test]
fn llm_cards_reserve_stable_metric_rows_before_data_arrives() {
    let mut graph = description();
    graph.nodes[0].kind = "builtin.llm".into();
    let layout = GraphLayout::new(&graph).unwrap();
    assert_eq!(layout.nodes()[0].rect.width, 40);
    assert!(layout.nodes()[0].rect.height >= 6);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 200, 100));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    let text = buffer_text(&buffer);
    assert!(text.contains("FTT —"));
    assert!(text.contains("Gen — | E2E — tok/s"));
}

#[test]
fn clusters_render_expanded_bounds_and_collapse_to_one_node() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.clusters = vec![ClusterDescription {
        label: "left and join".into(),
        annotation: "The left branch and its convergence point.".into(),
        nodes: vec!["left".into(), "join".into()],
    }];

    let expanded = GraphLayout::with_view(&graph, mf_tui::graph::ConnectionView::Ports).unwrap();
    assert_eq!(expanded.nodes().len(), graph.nodes.len());
    assert_eq!(expanded.clusters().len(), 1);
    let cluster = &expanded.clusters()[0];
    assert!(!cluster.collapsed);
    assert_eq!(cluster.members.len(), 2);
    assert!(cluster.rect.width > expanded.nodes()[0].rect.width);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 160, 60));
    GraphView::new(&expanded).render(buffer.area, &mut buffer);
    assert!(buffer_text(&buffer).contains("left and join"));

    let collapsed = GraphLayout::with_view_and_collapsed(
        &graph,
        mf_tui::graph::ConnectionView::Ports,
        &BTreeSet::from([0]),
    )
    .unwrap();
    assert_eq!(collapsed.nodes().len(), graph.nodes.len() - 1);
    assert!(collapsed.clusters()[0].collapsed);
    assert_eq!(collapsed.hidden_cluster_edge_count(), 1);
    let cluster_node = collapsed
        .nodes()
        .iter()
        .position(|node| node.id == "left and join")
        .unwrap();
    assert!(collapsed.is_collapsed_cluster(cluster_node));
    assert_eq!(collapsed.sources(cluster_node), &[2, 4]);
    assert_eq!(collapsed.display_index_for_cluster(0), Some(cluster_node));
    assert_eq!(
        collapsed.display_index_for_node_id("join"),
        Some(cluster_node)
    );
    assert_eq!(collapsed.display_index_for_node_id("router"), Some(1));

    let incoming = collapsed
        .edges()
        .iter()
        .filter(|edge| edge.to == cluster_node)
        .count();
    assert_eq!(incoming, 2);
    assert!(
        collapsed
            .edges()
            .iter()
            .all(|edge| edge.from != cluster_node || edge.to != cluster_node)
    );
    let mut collapsed_buffer = Buffer::empty(Rect::new(0, 0, 160, 60));
    GraphView::new(&collapsed).render(collapsed_buffer.area, &mut collapsed_buffer);
    assert!(buffer_text(&collapsed_buffer).contains("2 nodes | collapsed"));
}

#[test]
fn cluster_titles_preserve_the_remaining_top_border() {
    let mut graph = fan_graph(1);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    for label in ["Computation", "Compute a\u{301} with 🦀"] {
        graph.clusters = vec![ClusterDescription {
            label: label.into(),
            annotation: "Read the source and forward its value.".into(),
            nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
        }];
        let layout = GraphLayout::new(&graph).unwrap();
        let rect = layout.clusters()[0].rect;
        let (width, height) = layout.size();
        let mut buffer = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
        GraphView::new(&layout).render(buffer.area, &mut buffer);
        let label_end = rect.x + 2 + ratatui::text::Span::raw(format!(" {label} ")).width() as u32;
        let right = rect.x + rect.width - 1;
        assert!(label_end < right - 1);
        for x in label_end..right {
            assert_eq!(
                buffer
                    .cell(Position::new(x as u16, rect.y as u16))
                    .unwrap()
                    .symbol(),
                "─"
            );
        }
        assert_eq!(
            buffer
                .cell(Position::new(right as u16, rect.y as u16))
                .unwrap()
                .symbol(),
            "┐"
        );
    }
}

#[test]
fn cluster_titles_follow_both_scroll_axes() {
    let mut graph = fan_graph(3);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    for label in ["Computation", "Compute a\u{301} with 🦀"] {
        graph.clusters = vec![ClusterDescription {
            label: label.into(),
            annotation: "Read the source and forward its value.".into(),
            nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
        }];
        let layout = GraphLayout::new(&graph).unwrap();
        let rect = layout.clusters()[0].rect;
        for x in [0, rect.x + 4] {
            for y in [0, rect.y + 1, layout.nodes()[0].rect.y + 1] {
                let mut buffer = Buffer::empty(Rect::new(0, 0, 64, 6));
                GraphView::new(&layout)
                    .offset(x, y)
                    .render(buffer.area, &mut buffer);
                let row = rect.y.saturating_sub(y) as u16;
                let rendered: String = (0..buffer.area.width)
                    .map(|column| buffer.cell(Position::new(column, row)).unwrap().symbol())
                    .collect();
                assert!(rendered.contains(label), "offset ({x}, {y}): {rendered}");
            }
        }
    }
}

#[test]
fn cluster_annotations_hide_only_while_the_label_is_repositioned() {
    let mut graph = fan_graph(1);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    let annotation = "abcdefghijklmnopqrstuv";
    graph.clusters = vec![ClusterDescription {
        label: "Group".into(),
        annotation: annotation.into(),
        nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
    }];
    let layout = GraphLayout::new(&graph).unwrap();
    let cluster = layout.clusters()[0].rect;
    let node = layout.nodes()[layout.display_index_for_node_id("source").unwrap()].rect;
    for (x, y, hidden) in [
        (0, 0, false),
        (cluster.x + 2, 0, false),
        (cluster.x + 3, 0, true),
        (0, cluster.y + 1, true),
        (cluster.x + 3, cluster.y + 1, true),
        (0, 0, false),
    ] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 64, 12));
        GraphView::new(&layout)
            .offset(x, y)
            .render(buffer.area, &mut buffer);
        assert!(buffer_text(&buffer).contains("Group"));
        let label_x = (cluster.x + 2).max(x);
        let label_y = cluster.y.max(y);
        for (column, expected) in annotation.chars().enumerate() {
            let column = node.x + 1 + column as u32;
            if column < x
                || column >= x + 64
                || (node.y == label_y && (label_x..label_x + 7).contains(&column))
            {
                continue;
            }
            let cell = buffer
                .cell(Position::new((column - x) as u16, (node.y - y) as u16))
                .unwrap();
            assert_eq!(
                cell.symbol(),
                if hidden {
                    " ".into()
                } else {
                    expected.to_string()
                },
                "offset ({x}, {y})"
            );
        }
    }
}

#[test]
fn pinned_cluster_titles_clip_to_narrow_nonzero_viewports() {
    let mut graph = fan_graph(1);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.clusters = vec![ClusterDescription {
        label: "Compute a\u{301} 🦀 result".into(),
        annotation: "Forward the source value.".into(),
        nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
    }];
    let layout = GraphLayout::new(&graph).unwrap();
    let rect = layout.clusters()[0].rect;
    for width in [0, 1, 2, 3, 4, 12, 13, 14, 24] {
        let area = Rect::new(3, 2, width, 1);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 30, 4));
        for cell in &mut buffer.content {
            cell.set_symbol("#");
        }
        GraphView::new(&layout)
            .offset(rect.x + 4, rect.y + 1)
            .render(area, &mut buffer);

        let mut expected = Buffer::empty(Rect::new(0, 0, width, 1));
        let label = if width < 3 {
            graph.clusters[0].label.clone()
        } else {
            format!(" {} ", graph.clusters[0].label)
        };
        let label_width = ratatui::text::Span::raw(&label).width().min(width as usize) as u16;
        expected.set_stringn(
            0,
            0,
            label,
            width as usize,
            ratatui::style::Style::default(),
        );
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let actual = buffer.cell(Position::new(x, y)).unwrap().symbol();
                if area.contains(Position::new(x, y)) {
                    if x - area.x < label_width {
                        assert_eq!(
                            actual,
                            expected
                                .cell(Position::new(x - area.x, 0))
                                .unwrap()
                                .symbol(),
                            "width {width}, column {x}"
                        );
                    }
                } else {
                    assert_eq!(actual, "#", "width {width}, position ({x}, {y})");
                }
            }
        }
    }
}

#[test]
fn cluster_titles_disappear_when_the_cluster_leaves_the_viewport() {
    let mut graph = fan_graph(1);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.clusters = vec![ClusterDescription {
        label: "Computation".into(),
        annotation: "Forward the source value.".into(),
        nodes: graph.nodes.iter().map(|node| node.id.clone()).collect(),
    }];
    let layout = GraphLayout::new(&graph).unwrap();
    let rect = layout.clusters()[0].rect;
    for (x, y) in [(rect.x + rect.width, 0), (0, rect.y + rect.height)] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 6));
        GraphView::new(&layout)
            .offset(x, y)
            .render(buffer.area, &mut buffer);
        assert!(!buffer_text(&buffer).contains("Computation"));
    }
}

#[test]
fn chat_title_usage_connections_stay_straight_only_when_the_path_is_clear() {
    let definition = mf_compiler::WorkflowDefinition::from_json(include_str!(
        "../../../examples/chat-title-flow.json"
    ))
    .unwrap();
    let graph = mf_compiler::describe_compiled(&mf_compiler::plan_definition(&definition).unwrap())
        .unwrap();
    for view in [
        mf_tui::graph::ConnectionView::Execution,
        mf_tui::graph::ConnectionView::Dependencies,
        mf_tui::graph::ConnectionView::Ports,
    ] {
        let layout = GraphLayout::with_view(&graph, view).unwrap();
        let edge = layout
            .edges()
            .iter()
            .position(|edge| {
                layout.nodes()[edge.from].id == "conversation_usages"
                    && layout.nodes()[edge.to].id == "usages"
            })
            .unwrap();
        let route = &layout.routes()[edge];
        assert!(route.unresolved.is_none());
        let start = *route.points.first().unwrap();
        let end = *route.points.last().unwrap();
        assert_eq!(start.y, end.y);
        let direct = mf_tui::graph::routing::Segment { start, end };
        if view == mf_tui::graph::ConnectionView::Execution {
            assert_eq!(route.points.len(), 2);
        } else {
            let obstacle = layout
                .nodes()
                .iter()
                .find(|node| node.id == "combined_usages")
                .unwrap();
            assert!(direct.intersects_rect(obstacle.rect));
            assert!(route.points.len() > 2);
        }
        assert_completed_geometry_is_safe(&layout);
    }
}

#[test]
fn chat_title_clusters_stay_disjoint_across_every_fold_and_connection_view() {
    let definition = mf_compiler::WorkflowDefinition::from_json(include_str!(
        "../../../examples/chat-title-flow.json"
    ))
    .unwrap();
    let plan = mf_compiler::plan_definition(&definition).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    for bits in 0..(1 << graph.clusters.len()) {
        let collapsed = (0..graph.clusters.len())
            .filter(|cluster| bits & (1 << cluster) != 0)
            .collect();
        for view in [
            mf_tui::graph::ConnectionView::Execution,
            mf_tui::graph::ConnectionView::Dependencies,
            mf_tui::graph::ConnectionView::Ports,
        ] {
            let layout = GraphLayout::with_view_and_collapsed(&graph, view, &collapsed).unwrap();
            assert_eq!(layout.unresolved_count(), 0, "{collapsed:?} {view:?}");
            let (width, height) = layout.size();
            let mut buffer = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
            GraphView::new(&layout).render(buffer.area, &mut buffer);
            for (index, cluster) in layout.clusters().iter().enumerate() {
                let rect = cluster.rect;
                for other in &layout.clusters()[index + 1..] {
                    let other = other.rect;
                    assert!(
                        rect.x + rect.width <= other.x
                            || other.x + other.width <= rect.x
                            || rect.y + rect.height <= other.y
                            || other.y + other.height <= rect.y,
                        "{collapsed:?} {view:?}: clusters overlap"
                    );
                }
                if !cluster.collapsed {
                    assert_eq!(
                        buffer
                            .cell(Position::new(
                                (rect.x + rect.width - 1) as u16,
                                rect.y as u16
                            ))
                            .unwrap()
                            .symbol(),
                        "┐",
                        "{collapsed:?} {view:?}: {}",
                        cluster.label
                    );
                }
            }
            assert_completed_geometry_is_safe(&layout);
        }
    }
}

#[test]
fn folding_noncontiguous_members_preserves_projected_cycle_connections() {
    let mut graph = fan_graph(2);
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.data_edges.push(DataEdge {
        exclusive: false,
        from_node: "sink-0000".into(),
        from_output: "value".into(),
        to_node: "sink-0001".into(),
        to_input: "extra".into(),
    });
    graph.clusters = vec![ClusterDescription {
        label: "Endpoints".into(),
        annotation: "The source and final consumer.".into(),
        nodes: vec!["source".into(), "sink-0001".into()],
    }];
    for view in [
        mf_tui::graph::ConnectionView::Execution,
        mf_tui::graph::ConnectionView::Dependencies,
        mf_tui::graph::ConnectionView::Ports,
    ] {
        let layout =
            GraphLayout::with_view_and_collapsed(&graph, view, &BTreeSet::from([0])).unwrap();
        assert_eq!(layout.nodes().len(), 2);
        assert_eq!(layout.edges().len(), 2);
        assert_eq!(layout.hidden_transitive_count(), 0);
        assert!(
            layout
                .edges()
                .iter()
                .any(|edge| edge.from == 0 && edge.to == 1)
        );
        assert!(
            layout
                .edges()
                .iter()
                .any(|edge| edge.from == 1 && edge.to == 0)
        );
        assert_completed_geometry_is_safe(&layout);
    }
}

#[test]
fn long_cluster_labels_fit_expanded_and_collapsed_geometry() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.data_edges.push(DataEdge {
        exclusive: false,
        from_node: "source".into(),
        from_output: "value".into(),
        to_node: "join".into(),
        to_input: "direct".into(),
    });
    for label in [
        "Load and validate the complete workflow configuration",
        "Validate a\u{301} configuration with 🦀 and a complete title",
    ] {
        graph.clusters = vec![ClusterDescription {
            label: label.into(),
            annotation: "Keep the entire title visible in both cluster states.".into(),
            nodes: vec!["source".into()],
        }];
        for view in [
            mf_tui::graph::ConnectionView::Execution,
            mf_tui::graph::ConnectionView::Dependencies,
            mf_tui::graph::ConnectionView::Ports,
        ] {
            for collapsed in [BTreeSet::new(), BTreeSet::from([0])] {
                let layout =
                    GraphLayout::with_view_and_collapsed(&graph, view, &collapsed).unwrap();
                assert_eq!(layout.unresolved_count(), 0);
                let node = &layout.nodes()[layout.display_index_for_node_id("source").unwrap()];
                assert!(node.rect.width >= ratatui::text::Span::raw(label).width() as u32 + 4);
                let (width, height) = layout.size();
                let mut buffer = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
                GraphView::new(&layout).render(buffer.area, &mut buffer);
                let cluster = &layout.clusters()[0];
                let (mut x, y) = if cluster.collapsed {
                    (node.rect.x + 3, node.rect.y + 1)
                } else {
                    (cluster.rect.x + 3, cluster.rect.y)
                };
                let span = ratatui::text::Span::raw(label);
                for grapheme in span.styled_graphemes(ratatui::style::Style::default()) {
                    assert_eq!(
                        buffer
                            .cell(Position::new(x as u16, y as u16))
                            .unwrap()
                            .symbol(),
                        grapheme.symbol
                    );
                    x += ratatui::text::Span::raw(grapheme.symbol).width() as u32;
                }
                assert_completed_geometry_is_safe(&layout);
                for edge in layout.edges() {
                    let source = layout.nodes()[edge.from].rect;
                    let target = layout.nodes()[edge.to].rect;
                    assert!(source.x + source.width < target.x);
                }
            }
        }
    }
}

#[test]
fn cluster_colors_match_borders_and_labels_across_views_and_folds() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.clusters = vec![
        ClusterDescription {
            label: "Input".into(),
            annotation: "Read the source and forward its value.".into(),
            nodes: vec!["source".into(), "router".into()],
        },
        ClusterDescription {
            label: "Output".into(),
            annotation: "Merge the branch results.".into(),
            nodes: vec!["left".into(), "join".into()],
        },
    ];
    let mut observations: Vec<_> = graph
        .nodes
        .iter()
        .map(|node| NodeObservation::pending(&node.id, &node.kind))
        .collect();
    observations[0].status = NodeStatus::Failed;
    let mut expected_colors = Vec::new();
    for view in [
        mf_tui::graph::ConnectionView::Execution,
        mf_tui::graph::ConnectionView::Dependencies,
        mf_tui::graph::ConnectionView::Ports,
    ] {
        for collapsed in [
            BTreeSet::new(),
            BTreeSet::from([0]),
            BTreeSet::from([1]),
            BTreeSet::from([0, 1]),
        ] {
            let layout = GraphLayout::with_view_and_collapsed(&graph, view, &collapsed).unwrap();
            let (width, height) = layout.size();
            let mut buffer = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
            let selected = layout.display_index_for_node_id("source").unwrap();
            GraphView::new(&layout)
                .nodes(&observations)
                .selected(selected)
                .render(buffer.area, &mut buffer);
            let mut colors = Vec::new();
            for (index, cluster) in layout.clusters().iter().enumerate() {
                let label_position = if cluster.collapsed {
                    Position::new((cluster.rect.x + 3) as u16, (cluster.rect.y + 1) as u16)
                } else {
                    Position::new((cluster.rect.x + 3) as u16, cluster.rect.y as u16)
                };
                let color = buffer.cell(label_position).unwrap().fg;
                colors.push(color);
                let corner = buffer
                    .cell(Position::new(cluster.rect.x as u16, cluster.rect.y as u16))
                    .unwrap();
                assert_eq!(corner.fg, color);
                for node in 0..layout.nodes().len() {
                    if layout.cluster_of(node) != Some(index) {
                        continue;
                    }
                    let rect = layout.nodes()[node].rect;
                    let corner = buffer
                        .cell(Position::new(
                            rect.x as u16,
                            (rect.y + rect.height - 1) as u16,
                        ))
                        .unwrap();
                    assert_eq!(corner.fg, color);
                }
            }
            assert_ne!(colors[0], colors[1]);
            if expected_colors.is_empty() {
                expected_colors = colors;
            } else {
                assert_eq!(colors, expected_colors);
            }
            let rect = layout.nodes()[selected].rect;
            let title_y = if collapsed.contains(&0) {
                rect.y + 1
            } else {
                // The annotation occupies two lines plus one blank row above the node card.
                rect.y + 4
            };
            let marker = buffer
                .cell(Position::new((rect.x + 1) as u16, title_y as u16))
                .unwrap();
            assert_eq!(marker.fg, Color::Red);
            assert!(marker.modifier.contains(Modifier::REVERSED));
        }
    }
}

#[test]
fn cluster_annotations_wrap_in_the_graph_and_stay_clear_of_connections() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.clusters = vec![ClusterDescription {
        label: "Input".into(),
        annotation: "01234567890123456789012345678901234567890123456789\n\tKeep a\u{301} and 🦀."
            .into(),
        nodes: vec!["source".into(), "router".into()],
    }];
    let expected = [
        "0123456789012345678901",
        "2345678901234567890123",
        "456789",
        "    Keep a\u{301} and 🦀.",
    ];
    for view in [
        mf_tui::graph::ConnectionView::Execution,
        mf_tui::graph::ConnectionView::Dependencies,
        mf_tui::graph::ConnectionView::Ports,
    ] {
        for collapsed in [BTreeSet::new(), BTreeSet::from([0])] {
            let layout = GraphLayout::with_view_and_collapsed(&graph, view, &collapsed).unwrap();
            let node = &layout.nodes()[layout.display_index_for_node_id("source").unwrap()];
            let start_y = node.rect.y + if collapsed.is_empty() { 0 } else { 3 };
            let (width, height) = layout.size();
            let mut full = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
            GraphView::new(&layout).render(full.area, &mut full);
            for (row, text) in expected.iter().enumerate() {
                let mut x = node.rect.x + 1;
                let span = ratatui::text::Span::raw(*text);
                for grapheme in span.styled_graphemes(ratatui::style::Style::default()) {
                    assert_eq!(
                        full.cell(Position::new(x as u16, start_y as u16 + row as u16))
                            .unwrap()
                            .symbol(),
                        grapheme.symbol
                    );
                    x += ratatui::text::Span::raw(grapheme.symbol).width() as u32;
                }
            }
            let mut clipped = Buffer::empty(Rect::new(0, 0, width as u16, 2));
            GraphView::new(&layout)
                .offset(0, start_y + 1)
                .render(clipped.area, &mut clipped);
            let cluster = &layout.clusters()[0];
            let label_width = ratatui::text::Span::raw(format!(" {} ", cluster.label)).width();
            for y in 0..2 {
                for x in 0..width as u16 {
                    // The expanded cluster title overlays only its cells on the first visible row.
                    if collapsed.is_empty()
                        && y == 0
                        && (cluster.rect.x + 2..cluster.rect.x + 2 + label_width as u32)
                            .contains(&u32::from(x))
                    {
                        continue;
                    }
                    if collapsed.is_empty()
                        && (node.rect.x + 1..node.rect.x + node.rect.width - 1)
                            .contains(&u32::from(x))
                    {
                        assert_eq!(clipped.cell(Position::new(x, y)).unwrap().symbol(), " ");
                        continue;
                    }
                    assert_eq!(
                        clipped.cell(Position::new(x, y)),
                        full.cell(Position::new(x, start_y as u16 + 1 + y))
                    );
                }
            }
            assert_eq!(layout.unresolved_count(), 0);
            assert_completed_geometry_is_safe(&layout);
        }
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
        call_targets: Vec::new(),
        order_edges: Vec::new(),
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
            exclusive: false,
            from_node: "%loop".into(),
            from_output: "count".into(),
            to_node: "child".into(),
            to_input: "input".into(),
        }],
        control_edges: vec![],
        execution_order: vec!["%loop".into(), "child".into()],
        clusters: Vec::new(),
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
            EdgeKind::Exclusive => Color::Magenta,
            EdgeKind::Control => Color::Blue,
            EdgeKind::Success => Color::Yellow,
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
                selected_inputs: Vec::new(),
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
                selected_inputs: Vec::new(),
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
fn body_clusters_collapse_within_their_scope() {
    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_23;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.nodes = vec![NodeDescription {
        id: "repeat".into(),
        kind: "workflow.loop".into(),
    }];
    graph.data_edges.clear();
    graph.control_edges.clear();
    graph.execution_order = vec!["repeat".into()];
    graph.loop_bodies = vec![LoopBodyDescription {
        call_targets: Vec::new(),
        path: vec!["repeat".into()],
        nodes: vec![
            NodeDescription {
                id: "%loop".into(),
                kind: "%loop".into(),
            },
            NodeDescription {
                id: "step".into(),
                kind: "fixture.task".into(),
            },
            NodeDescription {
                id: "next".into(),
                kind: "fixture.task".into(),
            },
        ],
        data_edges: vec![
            DataEdge {
                exclusive: false,
                from_node: "%loop".into(),
                from_output: "count".into(),
                to_node: "step".into(),
                to_input: "input".into(),
            },
            DataEdge {
                exclusive: false,
                from_node: "step".into(),
                from_output: "value".into(),
                to_node: "next".into(),
                to_input: "input".into(),
            },
        ],
        control_edges: Vec::new(),
        order_edges: Vec::new(),
        execution_order: vec!["%loop".into(), "step".into(), "next".into()],
        clusters: vec![ClusterDescription {
            label: "Body work".into(),
            annotation: "The body transformation and its successor.".into(),
            nodes: vec!["step".into(), "next".into()],
        }],
    }];
    let collapsed = GraphLayout::from_body_with_view_and_collapsed(
        &graph,
        &["repeat".into()],
        mf_tui::graph::ConnectionView::Ports,
        &BTreeSet::from([0]),
    )
    .unwrap();
    assert_eq!(collapsed.nodes().len(), 2);
    assert_eq!(collapsed.hidden_cluster_edge_count(), 1);
    assert_eq!(
        collapsed.display_index_for_node_id("next"),
        collapsed
            .nodes()
            .iter()
            .position(|node| node.id == "Body work")
    );
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
            exclusive: false,
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
        exclusive: false,
        from_node: "sink-0000".into(),
        from_output: "value".into(),
        to_node: "sink-0001".into(),
        to_input: "extra".into(),
    });
    graph.data_edges.push(DataEdge {
        exclusive: false,
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
            exclusive: false,
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
            exclusive: false,
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
                exclusive: false,
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
                exclusive: false,
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
        call_targets: Vec::new(),
        order_edges: Vec::new(),
        path: vec!["repeat".into()],
        nodes: body_graph.nodes.clone(),
        data_edges: body_graph.data_edges.clone(),
        control_edges: body_graph.control_edges.clone(),
        execution_order: body_graph.execution_order.clone(),
        clusters: Vec::new(),
    }];
    for view in [
        mf_tui::graph::ConnectionView::Execution,
        mf_tui::graph::ConnectionView::Dependencies,
        mf_tui::graph::ConnectionView::Ports,
    ] {
        let root = GraphLayout::with_view(&body_graph, view).unwrap();
        let body =
            GraphLayout::from_body_with_view(&description, &["repeat".into()], view).unwrap();
        assert_eq!(body.nodes(), root.nodes());
        assert_eq!(body.edges(), root.edges());
        assert_eq!(body.routes(), root.routes());
        assert_eq!(body.crossings(), root.crossings());
        assert_eq!(
            body.hidden_transitive_count(),
            root.hidden_transitive_count()
        );
        if view == mf_tui::graph::ConnectionView::Execution {
            assert_eq!(body.hidden_transitive_count(), 16);
        }
    }
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
    let wide = label.text.find('🦀').unwrap();
    let prefix_width = ratatui::text::Span::raw(&label.text[..wide]).width() as u32;
    GraphView::new(&layout)
        .offset(label.rect.x + prefix_width, label.rect.y)
        .render(Rect::new(0, 0, 1, 1), &mut buffer);
    assert_eq!(buffer.cell(Position::new(0, 0)).unwrap().symbol(), " ");
    assert_eq!(buffer.cell(Position::new(1, 0)).unwrap().symbol(), "#");
}

#[test]
fn exclusive_candidates_have_distinct_edges_and_visible_labels() {
    let mut graph = description();
    for edge in graph
        .data_edges
        .iter_mut()
        .filter(|edge| edge.to_node == "join")
    {
        edge.exclusive = true;
        edge.to_input = "input".into();
    }
    graph.validate().unwrap();
    let layout = GraphLayout::new(&graph).unwrap();
    assert_eq!(layout.nodes().len(), graph.nodes.len());
    assert_eq!(
        layout
            .edges()
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Exclusive)
            .count(),
        2
    );
    assert_eq!(
        layout
            .routes()
            .iter()
            .filter_map(|route| route.label.as_ref())
            .filter(|label| label.text.contains("exclusive input"))
            .count(),
        2
    );
    graph.data_edges.last_mut().unwrap().exclusive = false;
    assert!(graph.validate().is_err());
}

fn parameter_graph() -> WorkflowDescription {
    WorkflowDescription {
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "d".repeat(64))).unwrap(),
        execution: None,
        nodes: ["params", "settings"]
            .map(|id| NodeDescription {
                id: id.into(),
                kind: "fixture.task".into(),
            })
            .to_vec(),
        data_edges: (0..18)
            .map(|index| DataEdge {
                exclusive: false,
                from_node: "params".into(),
                from_output: format!("field{index}"),
                to_node: "settings".into(),
                to_input: format!("p{index}"),
            })
            .collect(),
        control_edges: vec![],
        order_edges: vec![],
        execution_order: vec!["params".into(), "settings".into()],
        loop_bodies: vec![],
        clusters: Vec::new(),
    }
}

#[test]
fn execution_view_connects_every_dependency_pair_without_ordering_independent_nodes() {
    use mf_tui::graph::ConnectionView;

    let mut graph = description();
    graph.data_edges.push(DataEdge {
        exclusive: false,
        from_node: "router".into(),
        from_output: "yes".into(),
        to_node: "left".into(),
        to_input: "input".into(),
    });
    let layout = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(layout.edges().len(), 5);
    assert_eq!(layout.hidden_data_count(), 1);
    for (source, destination) in graph
        .data_edges
        .iter()
        .map(|edge| (&edge.from_node, &edge.to_node))
        .chain(
            graph
                .control_edges
                .iter()
                .map(|edge| (&edge.from_node, &edge.to_node)),
        )
    {
        assert_eq!(
            layout
                .edges()
                .iter()
                .filter(|edge| &edge.key.source == source && &edge.key.destination == destination)
                .count(),
            1,
            "missing or duplicated dependency: {source} -> {destination}"
        );
    }
    assert!(
        layout
            .edges()
            .iter()
            .all(|edge| edge.key.source != "orphan" && edge.key.destination != "orphan")
    );
    assert_completed_geometry_is_safe(&layout);
    let (width, height) = layout.size();
    let mut buffer = Buffer::empty(Rect::new(0, 0, width as u16, height as u16));
    GraphView::new(&layout).render(buffer.area, &mut buffer);
    for edge in layout.edges() {
        let target = layout.nodes()[edge.to].rect;
        assert_eq!(
            buffer
                .cell(Position::new(
                    (target.x - 1) as u16,
                    (target.y + edge.to_row) as u16,
                ))
                .unwrap()
                .symbol(),
            "▶"
        );
    }
}

#[test]
fn execution_view_reduces_transitive_bundles_and_preserves_the_diamond() {
    use mf_tui::graph::ConnectionView;

    let mut graph = description();
    let expected: std::collections::BTreeSet<_> = graph
        .data_edges
        .iter()
        .map(|edge| (edge.from_node.clone(), edge.to_node.clone()))
        .chain(
            graph
                .control_edges
                .iter()
                .map(|edge| (edge.from_node.clone(), edge.to_node.clone())),
        )
        .collect();
    for (from, to, input) in [
        ("source", "join", "direct-a"),
        ("source", "join", "direct-b"),
        ("source", "left", "direct"),
        ("router", "join", "direct-router"),
    ] {
        graph.data_edges.push(DataEdge {
            exclusive: false,
            from_node: from.into(),
            from_output: "value".into(),
            to_node: to.into(),
            to_input: input.into(),
        });
    }
    let layout = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(layout.hidden_transitive_count(), 3);
    assert_eq!(layout.hidden_data_count(), 0);
    assert_eq!(
        layout
            .edges()
            .iter()
            .map(|edge| (edge.key.source.clone(), edge.key.destination.clone()))
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert_completed_geometry_is_safe(&layout);
    for (view, count) in [
        (ConnectionView::Dependencies, 8),
        (ConnectionView::Ports, 9),
    ] {
        let full = GraphLayout::with_view(&graph, view).unwrap();
        assert_eq!(full.edges().len(), count);
        assert_eq!(full.hidden_transitive_count(), 0);
        for (index, reduced) in layout.nodes().iter().enumerate() {
            for (other, node) in layout.nodes().iter().enumerate() {
                assert_eq!(
                    reduced.rect.x.cmp(&node.rect.x),
                    full.nodes()[index].rect.x.cmp(&full.nodes()[other].rect.x)
                );
            }
        }
    }
    graph.nodes.reverse();
    graph.data_edges.reverse();
    graph.control_edges.reverse();
    let reordered = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(layout.nodes(), reordered.nodes());
    assert_eq!(layout.edges(), reordered.edges());
    assert_eq!(layout.routes(), reordered.routes());
    assert_eq!(layout.crossings(), reordered.crossings());
}

#[test]
fn execution_view_reduces_transitive_pairs_across_all_connection_kinds() {
    use mf_telemetry::description::{ExecutionDescription, ExecutionMode, OrderEdge};
    use mf_tui::graph::ConnectionView;

    let mut graph = description();
    graph.version = WorkflowDescriptionVersion::V2026_10_19;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    for output in ["yes", "no"] {
        graph.control_edges.push(ControlEdge {
            from_node: "source".into(),
            from_output: output.into(),
            to_node: "join".into(),
        });
        graph.data_edges.push(DataEdge {
            exclusive: true,
            from_node: "router".into(),
            from_output: output.into(),
            to_node: "join".into(),
            to_input: "candidate".into(),
        });
        graph.data_edges.push(DataEdge {
            exclusive: false,
            from_node: "source".into(),
            from_output: output.into(),
            to_node: "join".into(),
            to_input: format!("direct-{output}"),
        });
    }
    graph.order_edges.push(OrderEdge {
        from_node: "source".into(),
        to_node: "join".into(),
    });
    let layout = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(layout.hidden_transitive_count(), 2);
    assert_eq!(layout.hidden_data_count(), 2);
    assert_eq!(layout.edges().len(), 5);
    assert!(layout.edges().iter().all(|edge| {
        !(edge.key.destination == "join" && matches!(edge.key.source.as_str(), "source" | "router"))
    }));
    let full = GraphLayout::with_view(&graph, ConnectionView::Dependencies).unwrap();
    assert_eq!(full.edges().len(), 10);
    assert_eq!(full.hidden_transitive_count(), 0);
    assert_completed_geometry_is_safe(&layout);
}

#[test]
fn transitive_reduction_preserves_long_chains_across_reachability_words() {
    use mf_tui::graph::{ConnectionView, routing::RoutingLimits};

    let mut graph = fan_graph(129);
    for index in 1..129 {
        graph.data_edges.push(DataEdge {
            exclusive: false,
            from_node: graph.execution_order[index].clone(),
            from_output: "value".into(),
            to_node: graph.execution_order[index + 1].clone(),
            to_input: "previous".into(),
        });
    }
    let layout = GraphLayout::with_view_and_limits(
        &graph,
        ConnectionView::Execution,
        RoutingLimits {
            max_geometry_items: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(layout.hidden_transitive_count(), 128);
    assert_eq!(layout.edges().len(), 129);
    assert!(layout.edges().iter().all(|edge| edge.to == edge.from + 1));
}

#[test]
fn connection_views_reduce_attachments_without_losing_dependency_placement() {
    use mf_tui::graph::ConnectionView;
    let graph = parameter_graph();
    let execution = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(execution.edges().len(), 1);
    assert_eq!(execution.edges()[0].members.len(), 18);
    assert_eq!(execution.routes().len(), 1);
    assert_eq!(execution.hidden_data_count(), 0);
    assert!(execution.nodes().iter().all(|node| node.rect.height == 4));
    assert!(execution.nodes()[0].rect.x < execution.nodes()[1].rect.x);
    let dependencies = GraphLayout::with_view(&graph, ConnectionView::Dependencies).unwrap();
    assert_eq!(dependencies.edges().len(), 1);
    assert_eq!(dependencies.edges()[0].members.len(), 18);
    assert!(
        dependencies
            .nodes()
            .iter()
            .all(|node| node.rect.height == 4)
    );
    assert!(
        dependencies.routes()[0]
            .label
            .as_ref()
            .unwrap()
            .text
            .contains("18")
    );
    let ports = GraphLayout::with_view(&graph, ConnectionView::Ports).unwrap();
    assert_eq!(ports.edges().len(), 18);
    assert!(ports.nodes()[0].rect.height > dependencies.nodes()[0].rect.height);
    assert_completed_geometry_is_safe(&execution);
    assert_completed_geometry_is_safe(&dependencies);
    assert_completed_geometry_is_safe(&ports);
    let mut reordered = graph.clone();
    reordered.data_edges.reverse();
    for view in [
        ConnectionView::Execution,
        ConnectionView::Dependencies,
        ConnectionView::Ports,
    ] {
        let first = GraphLayout::with_view(&graph, view).unwrap();
        let second = GraphLayout::with_view(&reordered, view).unwrap();
        assert_eq!(first.nodes(), second.nodes());
        assert_eq!(first.edges(), second.edges());
        assert_eq!(first.routes(), second.routes());
        assert_eq!(first.crossings(), second.crossings());
    }
}

#[test]
fn success_gates_and_data_bundles_have_distinct_inspectable_identities() {
    use mf_telemetry::description::{ExecutionDescription, ExecutionMode, OrderEdge};
    use mf_tui::graph::ConnectionView;
    let mut graph = parameter_graph();
    graph.version = WorkflowDescriptionVersion::V2026_10_19;
    graph.execution = Some(ExecutionDescription {
        mode: ExecutionMode::Single,
    });
    graph.control_edges.push(ControlEdge {
        from_node: "params".into(),
        from_output: "success".into(),
        to_node: "settings".into(),
    });
    graph.order_edges.push(OrderEdge {
        from_node: "params".into(),
        to_node: "settings".into(),
    });
    let layout = GraphLayout::with_view(&graph, ConnectionView::Dependencies).unwrap();
    assert_eq!(layout.edges().len(), 3);
    let success = layout
        .edges()
        .iter()
        .position(|edge| edge.kind == EdgeKind::Success)
        .unwrap();
    let gate = layout
        .edges()
        .iter()
        .position(|edge| edge.kind == EdgeKind::Control)
        .unwrap();
    assert_eq!(layout.edges()[success].from_output, None);
    assert_eq!(layout.edges()[success].key.output, None);
    assert_eq!(layout.edges()[gate].from_output.as_deref(), Some("success"));
    assert_ne!(
        layout.routes()[success].label.as_ref().unwrap().text,
        layout.routes()[gate].label.as_ref().unwrap().text
    );
    let execution = GraphLayout::with_view(&graph, ConnectionView::Execution).unwrap();
    assert_eq!(execution.edges().len(), 2);
    assert_eq!(execution.hidden_data_count(), 18);
    assert_completed_geometry_is_safe(&layout);
}

#[test]
fn unresolved_bundles_retain_all_members_and_mixed_evidence_is_not_success() {
    use mf_tui::graph::{ConnectionView, routing::RoutingLimits};
    let graph = parameter_graph();
    let limited = GraphLayout::with_view_and_limits(
        &graph,
        ConnectionView::Dependencies,
        RoutingLimits {
            max_geometry_items: 0,
            max_conflict_checks: 0,
            max_total_expansions: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(limited.unresolved_count(), 1);
    let details = limited.routing_details(0);
    assert!(details.contains("18 mappings"));
    for index in 0..18 {
        assert!(details.contains(&format!("params.field{index} -> settings.p{index}")));
    }
    let layout = GraphLayout::with_view(&graph, ConnectionView::Dependencies).unwrap();
    let mut nodes = vec![
        NodeObservation::pending("params", "fixture.task"),
        NodeObservation::pending("settings", "fixture.task"),
    ];
    nodes[0].status = NodeStatus::Succeeded;
    nodes[0].produced_ports = (0..17).map(|index| format!("field{index}")).collect();
    nodes[0].skipped_ports = vec!["field17".into()];
    let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 20));
    GraphView::new(&layout)
        .nodes(&nodes)
        .render(buffer.area, &mut buffer);
    let route = &layout.routes()[0];
    let point = route.points[0];
    assert_eq!(
        buffer
            .cell(Position::new(point.x as u16, point.y as u16))
            .unwrap()
            .fg,
        Color::Magenta
    );
    nodes[0].produced_ports.push("field17".into());
    nodes[0].skipped_ports.clear();
    let mut complete = Buffer::empty(buffer.area);
    GraphView::new(&layout)
        .nodes(&nodes)
        .render(complete.area, &mut complete);
    assert_eq!(
        complete
            .cell(Position::new(point.x as u16, point.y as u16))
            .unwrap()
            .fg,
        Color::Green
    );
}
