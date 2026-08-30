use std::sync::mpsc;

use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Modifier};
use tuicore::{
    AnimationSettings, ChildKey, EventCtx, EventRoute, FocusCtx, FocusId, FocusRequest,
    FocusTarget, Key, KeyEvent, KeyModifiers, LayoutCtx, RenderCtx, TreePath, TuiEvent, TuiNode,
};

use super::{
    components::backlog_tree,
    page::{
        BacklogPage, MAX_UNCONFIRMED_TRANSFER_REFRESHES, PendingRank, PendingRankReconciliation,
        PendingTransfer, PendingTransferReconciliation, move_work_items_to_edge,
        recalculate_capacity, reconcile_pending_rank, reconcile_pending_transfer, should_poll,
        source_transfer_highlight, source_transfer_highlight_key, transfer_destinations,
        transfer_reconciliation_highlight,
    },
};
use crate::app_settings::BacklogRunwaySettings;
use crate::store::work_items::{
    BacklogSnapshot, RunwayCapacitySource, Sprint, SubtaskProgress, WorkItem, apply_capacity,
    rank_plan,
};

fn work_item(key: &str, title: &str) -> WorkItem {
    WorkItem {
        key: key.into(),
        title: title.into(),
        kind: "Story".into(),
        status: "To Do".into(),
        done: false,
        priority: "High".into(),
        assignee: "Ada".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        fix_versions: Vec::new(),
        epic_name: None,
        story_points: None,
    }
}

fn snapshot() -> BacklogSnapshot {
    BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: vec![Sprint {
            id: 7,
            name: "Sprint 7".into(),
            state: "active".into(),
            goal: None,
            start_date: Some("2026-06-18T09:00:00.000Z".into()),
            end_date: Some("2026-07-02T09:00:00.000Z".into()),
            work_items: vec![work_item("FIN-7", "Ship sprint work")],
            capacity: None,
        }],
        work_items: vec![work_item("FIN-8", "Plan next sprint")],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    }
}

fn data_focus_target() -> FocusTarget {
    FocusTarget {
        id: FocusId::new("data-view"),
        path: TreePath::from_keys([ChildKey::new("data")]),
        area: Rect::default(),
        enabled: true,
        tab_stop: true,
        control: true,
        hotkey: None,
        hotkeys: Vec::new(),
        hotkey_sequences: Vec::new(),
        suppress_global_hotkeys: false,
        focused_events_before_global_hotkeys: false,
    }
}

#[test]
fn backlog_shows_loading_indicator_before_the_initial_snapshot_arrives() {
    tuicore::init();
    let mut page = BacklogPage::with_initial_loading_for_test();
    let area = Rect::new(0, 0, 80, 16);
    page.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("Loading Jira backlog…"));
}

#[test]
fn backlog_hides_the_existing_data_view_while_reloading() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_loading_for_test(snapshot());
    let area = Rect::new(0, 0, 80, 16);
    page.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();

    assert!(text.contains("Loading Jira backlog…"));
    assert!(!text.contains("FIN-8"));
}

#[test]
fn backlog_refresh_shows_a_loader_while_reloading() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    view.set_loading(true);
    let area = Rect::new(0, 0, 80, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut lines = rendered_lines(&terminal, area);
    let header = lines.remove(0);

    assert!(header.contains("⠋"));
    assert!(header.contains("Refresh"));
    assert!(cell_position(&header, "Refresh") > cell_position(&header, "⠋"));
}

#[test]
fn backlog_header_places_web_menu_before_the_right_aligned_refresh_button() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let header = rendered_lines(&terminal, area).remove(0);

    assert!(cell_position(&header, "Web") < cell_position(&header, "Velocity"));
    assert!(cell_position(&header, "Velocity") < cell_position(&header, "Refresh"));
}

#[test]
fn backlog_refresh_is_focusable_with_shift_r() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 80, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);
    let refresh = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "button")
        .unwrap();

    assert_eq!(layout.focus_targets()[0].id.as_str(), "data-view");
    assert!(refresh.tab_stop);
    assert_eq!(refresh.hotkey_sequences, ["shift+r"]);
    assert_eq!(
        refresh.path,
        TreePath::from_keys([ChildKey::new("refresh")])
    );

    let mut ctx = EventCtx::new(AnimationSettings::default());
    view.dispatch_event(
        &EventRoute::new(TreePath::from_keys([ChildKey::new("refresh")])),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    );

    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::Refresh)
    ));
}

#[test]
fn backlog_velocity_is_focusable_with_shift_v() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 80, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);
    let velocity = layout
        .focus_targets()
        .iter()
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("velocity")]))
        .unwrap();

    assert_eq!(velocity.hotkey_sequences, ["shift+v"]);

    let mut ctx = EventCtx::new(AnimationSettings::default());
    view.dispatch_event(
        &EventRoute::new(TreePath::from_keys([ChildKey::new("velocity")])),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('v'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    );

    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenVelocity)
    ));
}

#[test]
fn backlog_web_menu_opens_with_shift_w_and_emits_board_event() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);

    let web = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.path == TreePath::from_keys([ChildKey::new("web"), ChildKey::new("trigger")])
        })
        .unwrap();
    assert_eq!(web.hotkey_sequences, ["shift+w"]);

    let mut ctx = EventCtx::new(AnimationSettings::default());
    view.dispatch_event(
        &EventRoute::new(TreePath::from_keys([ChildKey::new("web")])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut ctx,
    );
    view.layout(area, &mut LayoutCtx::new());
    view.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::new("web"),
            ChildKey::new("menu"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut ctx,
    );

    assert_eq!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenBoard)
    );
}

#[test]
fn selecting_a_web_menu_item_returns_focus_to_the_backlog_data_view() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    let area = Rect::new(0, 0, 100, 16);
    page.layout(area, &mut LayoutCtx::new());
    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("web"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    page.layout(area, &mut LayoutCtx::new());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("web"),
            ChildKey::new("menu"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut ctx,
    );

    assert_eq!(
        ctx.focus_request(),
        Some(&FocusRequest::TargetAt {
            path: TreePath::from_keys([
                ChildKey::first(),
                ChildKey::first(),
                ChildKey::new("data")
            ]),
            id: FocusId::new("data-view"),
        })
    );
}

#[test]
fn closing_the_web_menu_returns_focus_to_the_backlog_data_view() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    let area = Rect::new(0, 0, 100, 16);
    page.layout(area, &mut LayoutCtx::new());
    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("web"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    page.layout(area, &mut LayoutCtx::new());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("web"),
            ChildKey::new("menu"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Tab)),
        &mut ctx,
    );

    assert_eq!(
        ctx.focus_request(),
        Some(&FocusRequest::TargetAt {
            path: TreePath::from_keys([
                ChildKey::first(),
                ChildKey::first(),
                ChildKey::new("data")
            ]),
            id: FocusId::new("data-view"),
        })
    );
}

#[test]
fn unified_backlog_tree_shows_collapsed_sprints_and_expanded_backlog() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    snapshot.work_items[0].assignee = "Unassigned".into();
    snapshot.sprints.push(Sprint {
        id: 8,
        name: "Sprint 8".into(),
        state: "future".into(),
        goal: None,
        start_date: Some("2026-07-03T09:00:00.000Z".into()),
        end_date: Some("2026-07-17T09:00:00.000Z".into()),
        work_items: Vec::new(),
        capacity: None,
    });
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 80, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains(" Sprint 7 • 18 Jun – 2 Jul"));
    assert!(text.contains(" Sprint 8 • 3 Jul – 17 Jul"));
    assert!(text.contains(" Backlog • 1 items"));
    assert!(!text.contains("Finery"));
    assert!(!text.contains("Ship sprint work"));
    assert!(text.contains("Plan next sprint"));
    assert!(text.contains("FIN-8 Plan next sprint"));
    assert!(text.contains("- • @-- • To Do"));
}

#[test]
fn refreshed_backlog_falls_back_to_the_missing_ticket_parent() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    view.highlight("ticket:FIN-8");
    let mut refreshed = snapshot();
    refreshed.work_items.clear();

    view.set_snapshot(&refreshed);

    assert_eq!(
        view.highlighted_id_for_test().as_deref(),
        Some("section:backlog")
    );
}

#[test]
fn refreshed_backlog_keeps_the_highlighted_ticket_and_expanded_sprint() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    view.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    view.highlight("section:sprint-7");
    view.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Right)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    view.highlight("ticket:FIN-7");
    view.set_snapshot(&snapshot());

    let area = Rect::new(0, 0, 80, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    assert!(rendered_lines(&terminal, area).concat().contains("FIN-7"));
    assert_eq!(
        view.highlighted_id_for_test().as_deref(),
        Some("ticket:FIN-7")
    );
}

#[test]
fn page_refresh_keeps_the_highlighted_ticket() {
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    page.view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-8");

    page.refresh_snapshot_for_test(snapshot());

    assert_eq!(
        page.view_for_test()
            .base_mut()
            .base_mut()
            .highlighted_id_for_test()
            .as_deref(),
        Some("ticket:FIN-8")
    );
}

#[test]
fn backlog_story_rows_show_identity_then_subtask_release_and_epic_metadata() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    let story = &mut snapshot.work_items[0];
    story.story_points = Some(3.0);
    story.assignee = "Maya Voss".into();
    story.subtask_progress = Some(SubtaskProgress {
        completed: 0,
        total: 2,
    });
    story.fix_versions = vec!["1.4.0".into()];
    story.epic_name = Some("Shopping cart".into());
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    let text = lines.concat();

    assert!(text.contains("FIN-8 Plan next sprint"));
    assert!(text.contains("3 • @MV • 0/2  • To Do • 1.4.0 • Shopping cart"));
    let (ticket_y, ticket_line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("FIN-8 Plan next sprint"))
        .unwrap();
    let key_x = cell_position(ticket_line, "FIN-8").unwrap() as u16;
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((key_x, ticket_y as u16))
            .unwrap()
            .fg,
        tuicore::theme().muted_fg()
    );
    assert!(
        terminal
            .backend()
            .buffer()
            .cell((key_x, ticket_y as u16))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD)
    );
    let title_x = cell_position(ticket_line, "Plan next sprint").unwrap() as u16;
    assert!(
        terminal
            .backend()
            .buffer()
            .cell((title_x, ticket_y as u16))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD)
    );
    let (metadata_y, metadata_line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("1.4.0"))
        .unwrap();
    let version_x = cell_position(metadata_line, "1.4.0").unwrap() as u16;
    let epic_x = cell_position(metadata_line, "Shopping cart").unwrap() as u16;
    let metadata_cell = terminal
        .backend()
        .buffer()
        .cell((version_x, metadata_y as u16))
        .unwrap();
    assert!(metadata_cell.modifier.contains(Modifier::BOLD));
    assert_eq!(metadata_cell.bg, tuicore::theme().highlight_bg());
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((epic_x, metadata_y as u16))
            .unwrap()
            .fg,
        tuicore::theme().accent_fg()
    );
}

#[test]
fn backlog_shows_capacity_markers_without_a_velocity_indicator() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        WorkItem {
            story_points: Some(8.0),
            ..work_item("FIN-8", "Plan next sprint")
        },
        WorkItem {
            story_points: Some(14.0),
            ..work_item("FIN-9", "Refine the next sprint")
        },
        work_item("FIN-10", "Estimate this ticket"),
    ];
    snapshot.sprints[0].work_items[0].story_points = Some(18.0);
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }

    assert!(text.contains("Refresh"));
    assert!(text.contains("Velocity"));
    assert!(text.contains("┃"));
    assert!(text.contains("3 • @AD • To Do"));
    let lines = rendered_lines(&terminal, area);
    let capacity_line = lines.iter().find(|line| line.contains("✓ 1/1")).unwrap();
    assert!(capacity_line.contains(" 18/20 pts • ✓ 1/1 • 1 items"));
    assert_eq!(cell_position(capacity_line, ""), Some(2));
    assert!(!text.contains("assumed"));
    let ticket_position = |key| {
        lines
            .iter()
            .enumerate()
            .find_map(|(y, line)| cell_position(line, key).map(|x| (x as u16, y as u16)))
            .unwrap()
    };
    let (first_x, first_y) = ticket_position("FIN-8");
    let (second_x, second_y) = ticket_position("FIN-9");
    let (third_x, third_y) = ticket_position("FIN-10");
    assert_eq!(first_x, third_x);
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((first_x, first_y))
            .unwrap()
            .bg,
        tuicore::theme().surface_bg()
    );
    assert_ne!(
        terminal
            .backend()
            .buffer()
            .cell((second_x, second_y))
            .unwrap()
            .bg,
        tuicore::theme().surface_bg()
    );
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((third_x, third_y))
            .unwrap()
            .bg,
        tuicore::theme().surface_bg()
    );

    snapshot.sprints[0].work_items[0].story_points = Some(5.4);
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((5.4, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    view.set_snapshot(&snapshot);
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    text.clear();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("Velocity"));
    assert!(text.contains(" Sprint 7 • 18 Jun – 2 Jul"));
    assert!(text.contains(" ~5.4/20 pts • ✓ 1/1 • 1 items"));
    assert!(text.contains("5.4 • @AD • To Do"));

    snapshot.sprints[0].work_items[0].story_points = None;
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((5.4, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    view.set_snapshot(&snapshot);
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains(" ~5.4/20 pts • 󰄰 0/1 • 1 items"));

    apply_capacity(
        &mut snapshot,
        20.0,
        Some((5.4, true)),
        RunwayCapacitySource::FixedFallback,
        20,
    );
    view.set_snapshot(&snapshot);
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains(" 5.4/20 pts • 󰄰 0/1 • 1 items"));
    assert!(!text.contains("~5.4/20 pts"));
}

#[test]
fn sprint_estimation_coverage_excludes_bugs_and_counts_all_sprint_items() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    snapshot.sprints[0].work_items = vec![
        WorkItem {
            story_points: Some(5.0),
            ..work_item("FIN-7", "Estimated story")
        },
        WorkItem {
            kind: "BUG".into(),
            ..work_item("FIN-9", "Unestimated sprint bug")
        },
        WorkItem {
            kind: "bug".into(),
            story_points: Some(2.0),
            ..work_item("FIN-10", "Estimated sprint bug")
        },
        WorkItem {
            kind: "Custom issue type".into(),
            ..work_item("FIN-11", "Unestimated custom item")
        },
    ];
    snapshot.work_items = vec![WorkItem {
        kind: "Bug".into(),
        ..work_item("FIN-8", "Unestimated backlog bug")
    }];
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains(" ~10/20 pts • ✓ 1/1 • 4 items"));

    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char(' '))),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    tree.layout(area, &mut LayoutCtx::new());
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("Unestimated sprint bug"));
    assert!(text.contains("Unestimated backlog bug"));
}

#[test]
fn sprint_load_marks_zero_valued_average_assumptions() {
    tuicore::init();
    let mut snapshot = snapshot();
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((0.0, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    assert!(
        rendered_lines(&terminal, area)
            .concat()
            .contains(" ~0/20 pts • 󰄰 0/1 • 1 items")
    );
}

#[test]
fn space_toggles_the_highlighted_backlog_section() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((5.4, true)),
        RunwayCapacitySource::Fixed,
        20,
    );
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char(' '))),
        &mut ctx,
    );

    let area = Rect::new(0, 0, 80, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("Ship sprint work"));
    assert!(text.contains("~5.4 • @AD • To Do"));
}

#[test]
fn long_backlog_titles_wrap_to_the_available_viewport_width() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        sprints: Vec::new(),
        work_items: vec![work_item(
            "FIN-1",
            "A backlog title that wraps at the viewport edge",
        )],
        warnings: Vec::new(),
        story_points_configured: false,
        runway: None,
        velocity: None,
    };
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 40, 8);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| terminal.backend().buffer().cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let title_line = lines
        .iter()
        .find(|line| line.contains("A backlog title"))
        .unwrap();
    let title_start = cell_position(title_line, "A backlog title").unwrap();
    let continuation = lines.iter().find(|line| line.contains("viewport")).unwrap();
    assert_eq!(
        continuation.chars().position(|character| character != ' '),
        Some(title_start),
    );
}

#[test]
fn ticket_number_prefixes_wait_for_enter_and_underline_each_matching_number() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items = vec![work_item("KAN-34", "Sprint ticket")];
    snapshot.work_items = vec![work_item("KAN-342", "Backlog ticket")];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    for digit in ['3', '4'] {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(digit))),
            &mut ctx,
        );
    }

    let area = Rect::new(0, 0, 80, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    let (y, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("KAN-34 Sprint ticket"))
        .unwrap();
    let key_x = cell_position(line, "KAN-34").unwrap() as u16;
    assert!(
        !terminal
            .backend()
            .buffer()
            .cell((key_x, y as u16))
            .unwrap()
            .modifier
            .contains(Modifier::UNDERLINED)
    );
    assert!((key_x + 4..key_x + 6).all(|x| {
        terminal
            .backend()
            .buffer()
            .cell((x, y as u16))
            .unwrap()
            .modifier
            .contains(Modifier::UNDERLINED)
    }));

    tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    assert!(
        !terminal
            .backend()
            .buffer()
            .cell((key_x + 4, y as u16))
            .unwrap()
            .modifier
            .contains(Modifier::UNDERLINED)
    );
}

fn cell_position(line: &str, content: &str) -> Option<usize> {
    line.find(content)
        .map(|position| line[..position].chars().count())
}

fn rendered_lines(terminal: &Terminal<TestBackend>, area: Rect) -> Vec<String> {
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| terminal.backend().buffer().cell((x, y)).unwrap().symbol())
                .collect()
        })
        .collect()
}

#[test]
fn backlog_search_filters_tickets_and_hides_runway_bands() {
    tuicore::init();
    let mut snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![
            work_item("FIN-1", "Plan next sprint"),
            work_item("FIN-2", "Ship release"),
        ],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    apply_capacity(
        &mut snapshot,
        9.1,
        Some((3.0, false)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut ctx,
    );
    for key in "FIN-2".chars() {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut ctx,
        );
    }

    let area = Rect::new(0, 0, 80, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("FIN-2"));
    assert!(!text.contains("FIN-1"));
    assert!(!text.contains("┃"));
    let matching_y = rendered_lines(&terminal, area)
        .iter()
        .position(|line| line.contains("FIN-2"))
        .unwrap() as u16;
    assert!(
        (matching_y..matching_y + 2).all(|y| {
            (0..area.width).all(|x| {
                terminal.backend().buffer().cell((x, y)).unwrap().bg
                    != tuicore::theme().surface_bg()
            })
        }),
        "search results must not retain virtual sprint background bands"
    );
}

#[test]
fn backlog_search_requires_contiguous_text() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![work_item("FIN-1", "A shopper can narrow products")],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut ctx,
    );
    for key in "shoppersa".chars() {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut ctx,
        );
    }

    let area = Rect::new(0, 0, 80, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("No stories"));
}

#[test]
fn backlog_search_matches_epic_names() {
    tuicore::init();
    let mut item = work_item("FIN-1", "Improve deployment reporting");
    item.epic_name = Some("Operations".into());
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![item],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut ctx,
    );
    for key in "operations".chars() {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut ctx,
        );
    }

    let area = Rect::new(0, 0, 80, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    assert!(
        rendered_lines(&terminal, area)
            .concat()
            .contains("Improve deployment reporting")
    );
}

#[test]
fn unified_tree_uses_same_section_transient_selection_for_the_quick_menu() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![work_item("FIN-1", "First"), work_item("FIN-2", "Second")],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    let (sender, receiver) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Down)), &mut ctx);
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenQuickMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
}

#[test]
fn ctrl_enter_opens_the_highlighted_ticket() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let mut snapshot = snapshot();
    snapshot.work_items[0].kind = "Bug".into();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.highlight("ticket:FIN-8");
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenTicket { key }) if key == "FIN-8"
    ));
}

#[test]
fn backlog_rank_plan_uses_section_order_anchors() {
    let plan = rank_plan(
        vec!["FIN-2".into(), "FIN-3".into()],
        &[
            "FIN-1".into(),
            "FIN-2".into(),
            "FIN-3".into(),
            "FIN-4".into(),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.issues, ["FIN-2", "FIN-3"]);
    assert_eq!(plan.rank_before_issue.as_deref(), Some("FIN-4"));
}

#[test]
fn quick_menu_omits_its_current_section_from_transfer_destinations() {
    let snapshot = snapshot();
    assert_eq!(
        transfer_destinations(Some(&snapshot), "backlog")
            .iter()
            .map(|destination| destination.section_id.as_str())
            .collect::<Vec<_>>(),
        ["sprint-7"]
    );
    assert_eq!(
        transfer_destinations(Some(&snapshot), "sprint-7")
            .iter()
            .map(|destination| destination.section_id.as_str())
            .collect::<Vec<_>>(),
        ["backlog"]
    );
}

#[test]
fn stale_rank_refresh_keeps_the_optimistic_order() {
    let rollback = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![
            work_item("FIN-1", "First"),
            work_item("FIN-2", "Second"),
            work_item("FIN-3", "Third"),
        ],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    let mut optimistic = rollback.clone();
    optimistic.work_items.swap(0, 1);
    let mut pending = Some(PendingRank {
        rollback_snapshot: rollback.clone(),
        section_id: "backlog".into(),
        final_order: vec!["FIN-2".into(), "FIN-1".into(), "FIN-3".into()],
        unconfirmed_refreshes: 0,
    });

    assert_eq!(
        reconcile_pending_rank(&mut optimistic, &mut pending, rollback),
        PendingRankReconciliation::Unconfirmed
    );
    assert_eq!(
        optimistic
            .work_items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-2", "FIN-1", "FIN-3"]
    );
}

#[test]
fn optimistic_transfer_moves_selected_items_between_sections() {
    let mut snapshot = snapshot();
    assert!(move_work_items_to_edge(
        &mut snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into()],
        false,
    ));
    assert!(snapshot.work_items.is_empty());
    assert_eq!(
        snapshot.sprints[0]
            .work_items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-7", "FIN-8"]
    );
}

#[test]
fn optimistic_transfer_recalculates_destination_sprint_capacity() {
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items[0].story_points = Some(3.0);
    snapshot.work_items[0].story_points = Some(6.0);
    let mut nine_point_ticket = work_item("FIN-9", "Nine-point ticket");
    nine_point_ticket.story_points = Some(9.0);
    snapshot.work_items.push(nine_point_ticket);
    apply_capacity(
        &mut snapshot,
        9.1,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        10,
    );

    assert!(move_work_items_to_edge(
        &mut snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into(), "FIN-9".into()],
        false,
    ));
    recalculate_capacity(&mut snapshot, &BacklogRunwaySettings::default());

    assert_eq!(
        snapshot.sprints[0]
            .capacity
            .as_ref()
            .unwrap()
            .effective_points,
        18.0
    );
}

#[test]
fn optimistic_transfer_places_items_at_the_selected_destination_edge() {
    let mut snapshot = snapshot();
    snapshot.sprints[0]
        .work_items
        .push(work_item("FIN-9", "Existing sprint work"));

    assert!(move_work_items_to_edge(
        &mut snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into()],
        true,
    ));

    assert_eq!(
        snapshot.sprints[0]
            .work_items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-8", "FIN-7", "FIN-9"]
    );
}

#[test]
fn transfer_refresh_keeps_optimistic_snapshot_until_destination_confirms() {
    let rollback_snapshot = snapshot();
    let mut optimistic_snapshot = rollback_snapshot.clone();
    assert!(move_work_items_to_edge(
        &mut optimistic_snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into()],
        false,
    ));
    let mut pending = Some(PendingTransfer {
        rollback_snapshot,
        source_section_id: "backlog".into(),
        destination_section_id: "sprint-7".into(),
        destination_order: vec!["FIN-7".into(), "FIN-8".into()],
        keys: vec!["FIN-8".into()],
        source_highlight_key: None,
        ambiguous: false,
        unconfirmed_refreshes: 0,
    });
    let stale = pending.as_ref().unwrap().rollback_snapshot.clone();
    assert_eq!(
        reconcile_pending_transfer(&mut optimistic_snapshot, &mut pending, stale),
        PendingTransferReconciliation::Unconfirmed
    );
    assert!(pending.is_some());
    let confirmed = optimistic_snapshot.clone();
    assert_eq!(
        reconcile_pending_transfer(&mut optimistic_snapshot, &mut pending, confirmed),
        PendingTransferReconciliation::ConfirmedDestination
    );
    assert!(pending.is_none());
}

#[test]
fn transfer_highlight_prefers_remaining_source_ticket_then_section() {
    assert_eq!(
        source_transfer_highlight_key(
            &["FIN-1".into(), "FIN-2".into(), "FIN-3".into()],
            &["FIN-2".into()]
        ),
        Some("FIN-3".into())
    );
    assert_eq!(
        source_transfer_highlight("backlog", Some("FIN-3")),
        ("backlog".into(), "ticket:FIN-3".into())
    );
    assert_eq!(
        source_transfer_highlight("sprint-7", None),
        ("sprint-7".into(), "section:sprint-7".into())
    );
}

#[test]
fn unconfirmed_transfer_refreshes_exhaust() {
    let refreshed = snapshot();
    let mut optimistic = refreshed.clone();
    assert!(move_work_items_to_edge(
        &mut optimistic,
        "backlog",
        "sprint-7",
        &["FIN-8".into()],
        false,
    ));
    let mut pending = Some(PendingTransfer {
        rollback_snapshot: refreshed.clone(),
        source_section_id: "backlog".into(),
        destination_section_id: "sprint-7".into(),
        destination_order: vec!["FIN-7".into(), "FIN-8".into()],
        keys: vec!["FIN-8".into()],
        source_highlight_key: None,
        ambiguous: false,
        unconfirmed_refreshes: 0,
    });
    for _ in 1..MAX_UNCONFIRMED_TRANSFER_REFRESHES {
        assert_eq!(
            reconcile_pending_transfer(&mut optimistic, &mut pending, refreshed.clone()),
            PendingTransferReconciliation::Unconfirmed
        );
    }
    assert_eq!(
        reconcile_pending_transfer(&mut optimistic, &mut pending, refreshed),
        PendingTransferReconciliation::Exhausted
    );
}

#[test]
fn polling_runs_only_while_work_is_pending() {
    assert!(should_poll(true, false, false));
    assert!(should_poll(false, true, false));
    assert!(!should_poll(false, false, false));
}

#[test]
fn confirmed_transfer_highlight_remains_available() {
    let transfer = PendingTransfer {
        rollback_snapshot: snapshot(),
        source_section_id: "backlog".into(),
        destination_section_id: "sprint-7".into(),
        destination_order: vec!["FIN-7".into(), "FIN-8".into()],
        keys: vec!["FIN-8".into()],
        source_highlight_key: None,
        ambiguous: false,
        unconfirmed_refreshes: 0,
    };
    assert_eq!(
        transfer_reconciliation_highlight(
            PendingTransferReconciliation::ConfirmedDestination,
            &transfer
        ),
        Some(("backlog".into(), "section:backlog".into()))
    );
}
