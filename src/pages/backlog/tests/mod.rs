use std::sync::mpsc;

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    AnimationSettings, ChildKey, EventCtx, EventRoute, FocusCtx, FocusId, FocusTarget, Key,
    KeyEvent, KeyModifiers, LayoutCtx, RenderCtx, TreePath, TuiEvent, TuiNode,
};

use super::{
    components::backlog_tree,
    page::{
        BacklogPage, MAX_UNCONFIRMED_TRANSFER_REFRESHES, PendingRank, PendingRankReconciliation,
        PendingTransfer, PendingTransferReconciliation, move_work_items, reconcile_pending_rank,
        reconcile_pending_transfer, should_poll, source_transfer_highlight,
        source_transfer_highlight_key, transfer_destinations, transfer_reconciliation_highlight,
    },
};
use crate::store::work_items::{BacklogSnapshot, Sprint, WorkItem, rank_plan};

fn work_item(key: &str, title: &str) -> WorkItem {
    WorkItem {
        key: key.into(),
        title: title.into(),
        kind: "Story".into(),
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
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
            work_items: vec![work_item("FIN-7", "Ship sprint work")],
        }],
        work_items: vec![work_item("FIN-8", "Plan next sprint")],
        warnings: Vec::new(),
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
fn unified_backlog_tree_shows_collapsed_sprints_and_expanded_backlog() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    snapshot.work_items[0].assignee = "Unassigned".into();
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
    assert!(text.contains("Finery · Sprint 7 (active)"));
    assert!(text.contains("Finery · Backlog"));
    assert!(!text.contains("Ship sprint work"));
    assert!(text.contains("Plan next sprint"));
    assert!(text.contains("FIN-8 • To Do • @-- • -"));
}

#[test]
fn space_toggles_the_highlighted_backlog_section() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot(), sender, Default::default());
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
    assert!(lines.iter().any(|line| line.contains("viewport edge")));
}

#[test]
fn backlog_search_filters_tickets_by_key_and_title() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![
            work_item("FIN-1", "Plan next sprint"),
            work_item("FIN-2", "Ship release"),
        ],
        warnings: Vec::new(),
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
fn unified_tree_uses_same_section_transient_selection_for_the_quick_menu() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![work_item("FIN-1", "First"), work_item("FIN-2", "Second")],
        warnings: Vec::new(),
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
    assert!(move_work_items(
        &mut snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into()]
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
fn transfer_refresh_keeps_optimistic_snapshot_until_destination_confirms() {
    let rollback_snapshot = snapshot();
    let mut optimistic_snapshot = rollback_snapshot.clone();
    assert!(move_work_items(
        &mut optimistic_snapshot,
        "backlog",
        "sprint-7",
        &["FIN-8".into()]
    ));
    let mut pending = Some(PendingTransfer {
        rollback_snapshot,
        source_section_id: "backlog".into(),
        destination_section_id: "sprint-7".into(),
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
    assert!(move_work_items(
        &mut optimistic,
        "backlog",
        "sprint-7",
        &["FIN-8".into()]
    ));
    let mut pending = Some(PendingTransfer {
        rollback_snapshot: refreshed.clone(),
        source_section_id: "backlog".into(),
        destination_section_id: "sprint-7".into(),
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
