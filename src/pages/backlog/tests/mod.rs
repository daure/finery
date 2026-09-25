mod missing_filter_values;
mod saved_filter_runtime;

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::mpsc,
};

use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Modifier};
use tuicore::{
    AnimationSettings, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId,
    FocusRequest, FocusTarget, Key, KeyEvent, KeyModifiers, LayoutCtx, Propagation, RenderCtx,
    TreePath, TuiEvent, TuiNode,
};

use super::{
    components::{
        BacklogQuickMenu, BacklogQuickMenuEvent, BacklogTree, SavedFilterField,
        SavedFilterManagerEvent, backlog_tree, backlog_tree_with_issue_types,
        issue_types_in_snapshot, saved_filter_dialog, selectable_issue_types,
    },
    page::{
        BacklogPage, MAX_UNCONFIRMED_TRANSFER_REFRESHES, PendingRank, PendingRankReconciliation,
        PendingTransfer, PendingTransferReconciliation, RequestGenerations, StatusTransitionCache,
        TicketCommentsPane, apply_assignee_to_snapshot, apply_status_to_snapshot,
        apply_story_points_to_snapshot, current_user_assignment, description_width_percent,
        move_work_items_to_edge, quick_menu_labels, recalculate_capacity, reconcile_pending_rank,
        reconcile_pending_transfer, should_poll, source_transfer_highlight,
        source_transfer_highlight_key, sprint_report, strip_legacy_account_id_mentions,
        transfer_destinations, transfer_reconciliation_highlight, velocity_dialog,
        velocity_share_report,
    },
};
use crate::app_settings::BacklogRunwaySettings;
use crate::jira::JiraOption;
use crate::service::AppService;
use crate::store::work_items::content::{TicketImage, ticket_image_marker};
use crate::store::work_items::{
    BacklogSnapshot, IssueStatusTransition, RunwayCapacitySource, Sprint, StatusTransition,
    SubtaskProgress, TicketComment, TicketComments, VelocityReport, VelocitySprint, WorkItem,
    apply_capacity, rank_plan,
    release::{ReleaseVersion, parse_date},
    saved_filter::{BacklogFilterCriteria, BacklogFilterOptions, SavedBacklogFilter},
};

fn work_item(key: &str, title: &str) -> WorkItem {
    WorkItem {
        key: key.into(),
        title: title.into(),
        description: String::new(),
        kind: "Story".into(),
        status: "To Do".into(),
        status_category: crate::store::work_items::StatusCategory::Todo,
        done: false,
        priority: "High".into(),
        assignee: "Ada".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        labels: Vec::new(),
        fix_versions: Vec::new(),
        releases: Vec::new(),
        epic_name: None,
        story_points: None,
        status_changed_at: None,
    }
}

#[test]
fn sprint_report_uses_compact_ticket_points_without_dates() {
    let mut estimated = work_item("FIN-8", "Estimate this work");
    estimated.story_points = Some(8.0);
    estimated.done = true;
    estimated.status = "Done".into();
    let mut unestimated_bug = work_item("FIN-9", "Fix a bug");
    unestimated_bug.kind = "Bug".into();
    unestimated_bug.done = true;
    unestimated_bug.status = "Done".into();
    let mut unestimated_story = work_item("FIN-10", "Unestimated story");
    unestimated_story.done = true;
    unestimated_story.status = "Done".into();
    let mut in_review = work_item("FIN-12", "Review report format");
    in_review.status = "In Review".into();
    in_review.story_points = Some(5.0);
    let mut selected = work_item("FIN-13", "Select report release");
    selected.status = "Selected for Development".into();
    let mut subtask = work_item("FIN-11", "Hidden subtask");
    subtask.kind = "Sub-task".into();
    subtask.done = true;
    let sprint = Sprint {
        id: 1,
        name: "Sprint 1".into(),
        state: "active".into(),
        goal: Some("Ship it".into()),
        start_date: Some("2026-07-02T09:00:00.000Z".into()),
        end_date: Some("2026-07-16T17:00:00.000Z".into()),
        work_items: vec![
            estimated,
            unestimated_bug,
            unestimated_story,
            in_review,
            selected,
            subtask,
        ],
        capacity: None,
    };

    let report = sprint_report(&sprint, Some("https://jira.example"));

    assert!(report.starts_with("Sprint 1\n\nGoal: Ship it\nPoints: 8/13 pts completed"));
    assert!(!report.contains("2026-07-02"));
    assert!(report.contains("Points: 8/13 pts completed"));
    assert!(report.contains("Tickets: 3/5 done"));
    assert!(report.contains("Estimated stories/tasks: 1/2"));
    assert!(
        report
            .contains("✓ [S] Estimate this work - 8pts - Done - https://jira.example/browse/FIN-8")
    );
    assert!(report.contains("✓ [B] Fix a bug - Done - https://jira.example/browse/FIN-9"));
    assert!(
        report
            .contains("✓ [S] Unestimated story - ?pts - Done - https://jira.example/browse/FIN-10")
    );
    assert!(report.contains(
        "~ [S] Review report format - 5pts - In Review - https://jira.example/browse/FIN-12"
    ));
    assert!(
        report.contains(
            "· [S] Select report release - ?pts - Selected for Development - https://jira.example/browse/FIN-13"
        )
    );
    assert!(!report.contains("Hidden subtask"));
}

#[test]
fn velocity_report_uses_loaded_historical_sprint_tickets() {
    let mut item = work_item("FIN-12", "Ship the report");
    item.done = true;
    item.status = "Done".into();
    item.story_points = Some(3.0);
    let sprint = VelocitySprint {
        id: 12,
        name: "Sprint 12".into(),
        completed: 3.0,
        goal: Some("Report accurately".into()),
        work_items: Some(vec![item]),
    };

    let report = velocity_share_report(&sprint, None, Some("https://jira.example"));

    assert!(report.contains("Points: 3/3 pts completed"));
    assert!(report.contains("Tickets: 1/1 done"));
    assert!(report.contains("Estimated stories/tasks: 1/1"));
    assert!(
        report.contains("✓ [S] Ship the report - 3pts - Done - https://jira.example/browse/FIN-12")
    );
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
fn backlog_header_orders_icon_only_toolbar_focus() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 180, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let header = rendered_lines(&terminal, area).remove(0);

    for label in [
        "Web",
        "Group by",
        "Estimated",
        "User",
        "Type",
        "Status",
        "Epic",
        "Label",
        "Release",
        "Velocity",
        "Refresh",
    ] {
        assert!(!header.contains(label));
    }
    for (left, right) in [
        ("󰖟", "󰑮"),
        ("󰑮", "󰑭"),
        ("󰑭", "󰀄"),
        ("󰀄", "󰡯"),
        ("󰡯", ""),
        ("", ""),
        ("", ""),
        ("", ""),
        ("", "󰓅"),
        ("󰓅", "󰑓"),
    ] {
        assert!(cell_position(&header, left) < cell_position(&header, right));
    }

    let focus_position = |path| {
        layout
            .focus_targets()
            .iter()
            .position(|target| target.path == path)
            .unwrap_or_else(|| panic!("missing focus path: {path:?}"))
    };
    assert!(
        focus_position(TreePath::from_keys([
            ChildKey::new("web"),
            ChildKey::new("trigger")
        ])) < focus_position(TreePath::from_keys([
            ChildKey::new("group-by"),
            ChildKey::new("trigger")
        ]))
    );
    assert!(
        focus_position(TreePath::from_keys([
            ChildKey::new("group-by"),
            ChildKey::new("trigger")
        ])) < focus_position(TreePath::from_keys([ChildKey::new("estimated")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("estimated")]))
            < focus_position(TreePath::from_keys([ChildKey::new("users")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("users")]))
            < focus_position(TreePath::from_keys([ChildKey::new("issue-types")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("issue-types")]))
            < focus_position(TreePath::from_keys([ChildKey::new("statuses")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("statuses")]))
            < focus_position(TreePath::from_keys([ChildKey::new("epics")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("epics")]))
            < focus_position(TreePath::from_keys([ChildKey::new("labels")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("labels")]))
            < focus_position(TreePath::from_keys([ChildKey::new("releases")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("releases")]))
            < focus_position(TreePath::from_keys([ChildKey::new("velocity")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("velocity")]))
            < focus_position(TreePath::from_keys([ChildKey::new("refresh")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("refresh")]))
            < focus_position(TreePath::from_keys([ChildKey::new("data")]))
    );

    let group_by = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.path
                == TreePath::from_keys([ChildKey::new("group-by"), ChildKey::new("trigger")])
        })
        .unwrap();
    assert_eq!(group_by.hotkey_sequences, ["shift+p"]);
}

#[test]
fn backlog_header_uses_two_rows_for_compact_widths() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 70, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);

    assert!(lines[0].contains("󰖟"));
    assert!(lines[0].contains("|W|"));
    assert!(!lines[0].contains("Web"));
    assert!(lines[0].contains("󰑮"));
    assert!(lines[0].contains("󰓅"));
    assert!(lines[0].contains("󰑓"));
    assert!(!lines[0].contains(" G"));
    assert!(lines[1].contains("Search…"));
    assert!(lines[1].contains("󰀄"));
    assert!(lines[1].contains("󰡯"));
    assert!(lines[1].contains(""));
    assert!(lines[1].contains(""));
    assert!(lines[1].contains(""));
    assert!(lines[1].contains(""));
    assert!(!lines[1].contains("User"));
    assert!(!lines[1].contains("Type"));
    assert!(lines[1].contains("󰑭"));
    assert!(!lines[1].contains("Estimated"));
    let group_by = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.path
                == TreePath::from_keys([ChildKey::new("group-by"), ChildKey::new("trigger")])
        })
        .unwrap();
    assert!(group_by.area.x > cell_position(&lines[0], "󰖟").unwrap() as u16);
    assert!(cell_position(&lines[1], "󰑭") < cell_position(&lines[1], "󰀄"));
    assert!(cell_position(&lines[1], "󰀄") < cell_position(&lines[1], "󰡯"));
    assert!(cell_position(&lines[1], "󰡯") < cell_position(&lines[1], ""));
    assert!(cell_position(&lines[1], "") < cell_position(&lines[1], ""));
    assert!(cell_position(&lines[1], "") < cell_position(&lines[1], ""));
    assert!(cell_position(&lines[1], "") < cell_position(&lines[1], ""));
    let focus_position = |path| {
        layout
            .focus_targets()
            .iter()
            .position(|target| target.path == path)
            .unwrap_or_else(|| panic!("missing focus path: {path:?}"))
    };
    assert!(
        focus_position(TreePath::from_keys([
            ChildKey::new("web"),
            ChildKey::new("trigger")
        ])) < focus_position(TreePath::from_keys([
            ChildKey::new("group-by"),
            ChildKey::new("trigger")
        ]))
    );
    assert!(
        focus_position(TreePath::from_keys([
            ChildKey::new("group-by"),
            ChildKey::new("trigger")
        ])) < focus_position(TreePath::from_keys([ChildKey::new("velocity")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("velocity")]))
            < focus_position(TreePath::from_keys([ChildKey::new("refresh")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("refresh")]))
            < focus_position(TreePath::from_keys([ChildKey::new("estimated")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("estimated")]))
            < focus_position(TreePath::from_keys([ChildKey::new("users")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("users")]))
            < focus_position(TreePath::from_keys([ChildKey::new("issue-types")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("issue-types")]))
            < focus_position(TreePath::from_keys([ChildKey::new("statuses")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("statuses")]))
            < focus_position(TreePath::from_keys([ChildKey::new("epics")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("epics")]))
            < focus_position(TreePath::from_keys([ChildKey::new("labels")]))
    );
    assert!(
        focus_position(TreePath::from_keys([ChildKey::new("labels")]))
            < focus_position(TreePath::from_keys([ChildKey::new("releases")]))
    );
}

#[test]
fn backlog_groups_sprint_and_backlog_tickets_by_release_with_a_missing_version_group() {
    tuicore::init();
    let mut snapshot = snapshot();
    let mut released = work_item("FIN-8", "Release this work");
    released.fix_versions = vec!["v1.0".into()];
    released.story_points = Some(2.0);
    snapshot.sprints[0].work_items[0].fix_versions = vec!["v1.0".into()];
    snapshot.sprints[0].work_items[0].story_points = Some(3.0);
    let mut version_ten = work_item("FIN-9", "Plan this work");
    version_ten.fix_versions = vec!["v10.0".into()];
    snapshot.work_items = vec![
        released,
        version_ten,
        work_item("FIN-10", "Unreleased work"),
    ];
    let (sender, receiver) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.group_by_release_for_test();
    assert!(!tree.runway_markers_visible_for_test());
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

    let lines = rendered_lines(&terminal, area);
    let text = lines.concat();
    assert!(text.contains(" v1.0 •  Start and end date missing"));
    assert!(text.contains("✓ 2/2 est • 2 open • 5 pts remaining"));
    assert!(text.contains("(no release version) • 1 items"));
    assert!(text.contains("v10.0 •  Start and end date missing"));
    assert_eq!(
        lines.iter().position(|line| line.contains("v1.0 •")),
        Some(2)
    );
    assert_eq!(
        lines.iter().position(|line| line.contains("v10.0 •")),
        Some(4)
    );
    assert_eq!(
        lines
            .iter()
            .position(|line| line.contains("(no release version) • 1 items")),
        Some(6)
    );
    assert!(!text.contains("Sprint 7"));
    assert!(!text.contains("Ship sprint work"));
    assert!(!text.contains("Release this work"));
    assert!(!text.contains("Plan this work"));

    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Right)), &mut ctx);
    tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Down)), &mut ctx);
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenQuickMenu {
            section_moves_available: false,
            ..
        })
    ));
}

#[test]
fn changing_backlog_grouping_leaves_all_rows_collapsed() {
    tuicore::init();
    let mut snapshot = snapshot();
    let mut child = work_item("FIN-9", "Hidden child");
    child.kind = "Sub-task".into();
    child.parent_key = Some("FIN-8".into());
    snapshot.work_items.push(child);
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());

    tree.group_by_user_for_test();

    assert!(tree.expansion_snapshot_for_test().is_empty());
}

#[test]
fn backlog_groups_releases_by_scheduled_dates_before_version_names() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items[0].fix_versions = vec!["zeta".into()];
    snapshot.sprints[0].work_items[0].releases = vec![ReleaseVersion {
        id: "1".into(),
        name: "zeta".into(),
        start_date: parse_date("2026-09-14"),
        end_date: parse_date("2026-09-28"),
    }];
    snapshot.work_items[0].fix_versions = vec!["alpha".into()];
    snapshot.work_items[0].releases = vec![ReleaseVersion {
        id: "2".into(),
        name: "alpha".into(),
        start_date: parse_date("2026-10-12"),
        end_date: parse_date("2026-10-26"),
    }];
    let mut undated = work_item("FIN-9", "Undated release work");
    undated.fix_versions = vec!["beta".into()];
    undated.releases = vec![ReleaseVersion {
        id: "3".into(),
        name: "beta".into(),
        start_date: None,
        end_date: None,
    }];
    snapshot
        .work_items
        .extend([undated, work_item("FIN-10", "Unreleased work")]);

    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.group_by_release_for_test();
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

    let lines = rendered_lines(&terminal, area);
    let position = |label: &str| lines.iter().position(|line| line.contains(label)).unwrap();
    assert!(position("zeta •") < position("alpha •"));
    assert!(position("alpha •") < position("beta •"));
    assert!(position("beta •") < position("(no release version) •"));
}

#[test]
fn backlog_groups_sprint_and_backlog_tickets_by_epic_with_an_unassigned_group() {
    tuicore::init();
    let mut snapshot = snapshot();
    let mut assigned = work_item("FIN-8", "Build the release");
    assigned.epic_name = Some("Zulu".into());
    assigned.story_points = Some(8.0);
    assigned.done = true;
    snapshot.sprints[0].work_items[0].epic_name = Some("Zulu".into());
    snapshot.sprints[0].work_items[0].story_points = Some(13.0);
    let mut alpha = work_item("FIN-9", "Polish the release");
    alpha.epic_name = Some("Alpha".into());
    snapshot.work_items = vec![assigned, alpha, work_item("FIN-10", "Unassigned work")];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.group_by_epic_for_test();
    assert!(!tree.runway_markers_visible_for_test());
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

    let lines = rendered_lines(&terminal, area);
    let text = lines.concat();
    assert!(lines[4].trim_end().ends_with(" Zulu"));
    assert!(text.contains("✓ 2/2 est • 1 open • 13 pts remaining"));
    assert!(lines[2].trim_end().ends_with(" Alpha"));
    assert!(text.contains("(no epic assigned) • 1 items"));
    assert_eq!(
        lines.iter().position(|line| line.contains(" Alpha")),
        Some(2)
    );
    assert_eq!(
        lines.iter().position(|line| line.contains(" Zulu")),
        Some(4)
    );
    assert_eq!(
        lines
            .iter()
            .position(|line| line.contains("(no epic assigned) • 1 items")),
        Some(6)
    );
    assert!(!text.contains("Sprint 7"));
    assert!(!text.contains("Ship sprint work"));
    assert!(!text.contains("Build the release"));
    assert!(!text.contains("Polish the release"));
}

#[test]
fn backlog_groups_all_ticket_types_by_user_with_estimation_and_status_summaries() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.story_points_configured = true;
    snapshot.sprints[0].work_items[0].story_points = Some(3.0);
    snapshot.sprints[0].work_items[0].status = "Team To Do Queue".into();

    let task = WorkItem {
        kind: "Task".into(),
        status: "Product Backlog".into(),
        ..work_item("FIN-8", "Plan the work")
    };
    let in_progress_story = WorkItem {
        status: "QA Review".into(),
        status_category: crate::store::work_items::StatusCategory::InProgress,
        story_points: Some(5.0),
        ..work_item("FIN-9", "Build the work")
    };
    let bug = WorkItem {
        kind: "Bug".into(),
        status: "Work In Progress".into(),
        status_category: crate::store::work_items::StatusCategory::InProgress,
        ..work_item("FIN-10", "Fix the work")
    };
    let subtask = WorkItem {
        kind: "Sub-task".into(),
        status: "Done".into(),
        status_category: crate::store::work_items::StatusCategory::Done,
        done: true,
        parent_key: Some("FIN-8".into()),
        ..work_item("FIN-11", "Finish the work")
    };
    let todo_bug = WorkItem {
        kind: "Bug".into(),
        status: "TODO Grooming".into(),
        ..work_item("FIN-13", "Groom the work")
    };
    let unassigned = WorkItem {
        kind: "Bug".into(),
        assignee: "Unassigned".into(),
        ..work_item("FIN-12", "Assign the work")
    };
    snapshot.work_items = vec![task, in_progress_story, bug, subtask, todo_bug, unassigned];

    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.group_by_user_for_test();
    let area = Rect::new(0, 0, 160, 16);
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
    assert!(lines[2].contains("󰀆 Ada • 󰄰 2/3 est • 6 items"));
    assert_eq!(
        lines[3].trim(),
        "1 Product Backlog • 1 Team To Do Queue • 1 TODO Grooming • 1 Work In Progress • 1 QA Review • 1 Done"
    );
    let review_x = cell_position(&lines[3], "QA Review").unwrap() as u16;
    assert_eq!(
        terminal.backend().buffer().cell((review_x, 3)).unwrap().fg,
        tuicore::theme().info_fg()
    );
    let done_x = cell_position(&lines[3], "Done").unwrap() as u16;
    assert_eq!(
        terminal.backend().buffer().cell((done_x, 3)).unwrap().fg,
        tuicore::theme().success_fg()
    );
    assert!(lines[4].contains("󰀆 (no user assigned) • ✓ 0/0 est • 1 items"));
    assert_eq!(lines[5].trim(), "1 To Do");
}

#[test]
fn disabling_estimated_only_shows_unestimated_stories_and_tasks() {
    tuicore::init();
    let mut snapshot = snapshot();
    let unpointed_parent = work_item("FIN-8", "Unpointed parent");
    let unpointed_task = WorkItem {
        kind: "Task".into(),
        ..work_item("FIN-9", "Unpointed task")
    };
    let bug = WorkItem {
        kind: "Bug".into(),
        ..work_item("FIN-10", "Unpointed bug")
    };
    let subtask = WorkItem {
        kind: "Sub-task".into(),
        ..work_item("FIN-11", "Unpointed subtask")
    };
    let pointed_story = WorkItem {
        story_points: Some(8.0),
        ..work_item("FIN-12", "Pointed story")
    };
    snapshot.work_items = vec![
        unpointed_parent,
        unpointed_task,
        bug,
        subtask,
        pointed_story,
    ];
    apply_capacity(
        &mut snapshot,
        9.1,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.set_estimated(false);
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
    assert!(text.contains("Unpointed parent"));
    assert!(text.contains("Unpointed task"));
    assert!(!text.contains("Unpointed bug"));
    assert!(!text.contains("Unpointed subtask"));
    assert!(!text.contains("Pointed story"));
    assert!(!text.contains("┃"));
}

#[test]
fn board_epic_label_and_release_filters_match_selected_values() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints.clear();
    snapshot.work_items = vec![
        WorkItem {
            epic_name: Some("Delivery".into()),
            labels: vec!["ready".into()],
            fix_versions: vec!["v1.0".into()],
            ..work_item("FIN-8", "Delivery ticket")
        },
        WorkItem {
            epic_name: Some("Platform".into()),
            labels: vec!["api".into()],
            fix_versions: vec!["v2.0".into()],
            ..work_item("FIN-9", "Platform ticket")
        },
    ];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    for (selected, apply_filter) in [
        (
            "Delivery",
            BacklogTree::set_epics_filter as fn(&mut BacklogTree, Vec<String>),
        ),
        ("ready", BacklogTree::set_labels_filter),
        ("v1.0", BacklogTree::set_releases_filter),
    ] {
        apply_filter(&mut tree, vec![selected.into()]);
        terminal
            .draw(|frame| {
                let mut render = RenderCtx::new();
                tree.render(frame, area, &mut render);
                render.flush(frame);
            })
            .unwrap();
        let text = rendered_lines(&terminal, area).concat();
        assert!(text.contains("Delivery ticket"));
        assert!(!text.contains("Platform ticket"));
        tree.set_epics_filter(Vec::new());
        tree.set_labels_filter(Vec::new());
        tree.set_releases_filter(Vec::new());
    }
}

#[test]
fn issue_type_filter_keeps_subtasks_of_matching_stories_and_tasks() {
    tuicore::init();
    let mut snapshot = snapshot();
    let mut task_subtask = work_item("FIN-10", "Task subtask");
    task_subtask.kind = "Sub-task".into();
    task_subtask.parent_key = Some("FIN-9".into());
    let mut story_subtask = work_item("FIN-12", "Story subtask");
    story_subtask.kind = "Sub-task".into();
    story_subtask.parent_key = Some("FIN-11".into());
    snapshot.work_items = vec![
        WorkItem {
            kind: "Bug".into(),
            ..work_item("FIN-8", "Bug ticket")
        },
        WorkItem {
            kind: "Task".into(),
            ..work_item("FIN-9", "Task ticket")
        },
        task_subtask,
        work_item("FIN-11", "Story ticket"),
        story_subtask,
    ];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.set_issue_types_filter(vec!["Task".into()]);
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
    assert!(text.contains("Task ticket"));
    assert!(text.contains("Task subtask"));
    assert!(!text.contains("Bug ticket"));
    assert!(!text.contains("Story ticket"));
    assert!(!text.contains("Story subtask"));

    tree.set_issue_types_filter(vec!["Story".into()]);
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("Story ticket"));
    assert!(text.contains("Story subtask"));
    assert!(!text.contains("Task ticket"));
    assert!(!text.contains("Task subtask"));

    tree.set_issue_types_filter(Vec::new());
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("Bug ticket"));
    assert!(text.contains("Task ticket"));
    assert!(text.contains("Story ticket"));
}

#[test]
fn user_filter_shows_only_matching_assignees() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        WorkItem {
            assignee: "Ada".into(),
            ..work_item("FIN-8", "Ada ticket")
        },
        WorkItem {
            assignee: "Maya".into(),
            ..work_item("FIN-9", "Maya ticket")
        },
    ];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.set_users_filter(vec!["Maya".into()]);
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
    assert!(!text.contains("Ada ticket"));
    assert!(text.contains("Maya ticket"));
}

#[test]
fn status_filter_shows_only_matching_ticket_statuses() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        WorkItem {
            status: "In Progress".into(),
            ..work_item("FIN-8", "Active ticket")
        },
        WorkItem {
            status: "Done".into(),
            ..work_item("FIN-9", "Completed ticket")
        },
    ];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.set_statuses_filter(vec!["In Progress".into()]);
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
    assert!(text.contains("Active ticket"));
    assert!(!text.contains("Completed ticket"));
}

#[test]
fn issue_type_dropdown_excludes_epics_and_subtasks() {
    let issue_types = selectable_issue_types(vec![
        JiraOption {
            id: "1".into(),
            label: "Story".into(),
        },
        JiraOption {
            id: "2".into(),
            label: "Epic".into(),
        },
        JiraOption {
            id: "3".into(),
            label: "Subtask".into(),
        },
        JiraOption {
            id: "4".into(),
            label: "Sub-task".into(),
        },
        JiraOption {
            id: "5".into(),
            label: "Subtasks".into(),
        },
        JiraOption {
            id: "6".into(),
            label: "Sub-tasks".into(),
        },
    ]);

    assert_eq!(
        issue_types
            .iter()
            .map(|issue_type| issue_type.label.as_str())
            .collect::<Vec<_>>(),
        ["Story"]
    );
}

#[test]
fn issue_type_dropdown_uses_types_present_in_the_backlog() {
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items[0].kind = "Bug".into();
    snapshot.work_items[0].kind = "Story".into();
    snapshot.work_items.push(WorkItem {
        kind: "bug".into(),
        ..work_item("FIN-9", "Duplicate type")
    });
    snapshot.work_items.push(WorkItem {
        kind: "Epic".into(),
        ..work_item("FIN-10", "Epic")
    });

    assert_eq!(
        issue_types_in_snapshot(&snapshot)
            .into_iter()
            .map(|issue_type| issue_type.label)
            .collect::<Vec<_>>(),
        ["Bug", "Story"]
    );
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
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("refresh")]))
        .unwrap();

    assert_eq!(
        layout.focus_targets()[0].path,
        TreePath::from_keys([ChildKey::new("web"), ChildKey::new("trigger")])
    );
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
fn backlog_data_view_retains_focus_when_unfocused() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));

    for key in [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        let mut ctx = EventCtx::new(AnimationSettings::default());

        assert_eq!(
            view.dispatch_event(&route, &TuiEvent::Key(key), &mut ctx),
            tuicore::EventOutcome::Handled
        );
        assert_eq!(ctx.propagation(), Propagation::Stopped);
        assert_eq!(ctx.focus_request(), None);
    }
}

#[test]
fn backlog_header_controls_return_focus_to_the_data_view_when_unfocused() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());

    for control in ["refresh", "velocity", "estimated", "issue-types", "web"] {
        for key in [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ] {
            let mut ctx = EventCtx::new(AnimationSettings::default());

            assert_eq!(
                view.dispatch_event(
                    &EventRoute::new(TreePath::from_keys([ChildKey::new(control)])),
                    &TuiEvent::Key(key),
                    &mut ctx,
                ),
                tuicore::EventOutcome::Handled
            );
            assert_eq!(
                ctx.focus_request(),
                Some(&FocusRequest::Target(FocusId::new("data-view")))
            );
            assert_eq!(ctx.propagation(), Propagation::Stopped);
        }
    }
}

#[test]
fn release_menu_focuses_its_search_field() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    let mut open = EventCtx::new(AnimationSettings::default());
    assert!(
        page.view_for_test()
            .base_mut()
            .layer_mut()
            .open_release_menu(
                "backlog".into(),
                vec!["FIN-8".into()],
                vec!["FIN-8".into()],
                &mut open,
            )
    );
    page.view_for_test().base_mut().set_active(true);
    let area = Rect::new(0, 0, 100, 24);
    let mut layout = LayoutCtx::new();
    page.layout(area, &mut layout);
    let mut focus = EventCtx::new(AnimationSettings::default());
    page.focus_release_menu(&mut focus);

    let Some(FocusRequest::TargetAt { path, id }) = focus.focus_request() else {
        panic!("expected release menu focus request");
    };
    assert_eq!(id, &FocusId::new("input"));
    assert!(
        layout
            .focus_targets()
            .iter()
            .any(|target| target.path == *path && target.id == *id),
        "focus request: {path:?}/{id:?}; targets: {:?}",
        layout.focus_targets()
    );
    page.queue_release_menu_focus();
    assert_eq!(
        page.take_pending_focus_request(),
        focus.focus_request().cloned()
    );
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
fn backlog_estimated_toggle_is_focusable_with_shift_d() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);

    let estimated = layout
        .focus_targets()
        .iter()
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("estimated")]))
        .unwrap();

    assert_eq!(estimated.hotkey_sequences, ["shift+d"]);
}

#[test]
fn backlog_saved_filter_dropdown_is_focusable_with_shift_f() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);

    let saved_filter = layout
        .focus_targets()
        .iter()
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("saved-filter")]))
        .unwrap();

    assert_eq!(saved_filter.hotkey_sequences, ["shift+f"]);
}

#[test]
fn backlog_status_filter_is_focusable_with_shift_s() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);

    let statuses = layout
        .focus_targets()
        .iter()
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("statuses")]))
        .unwrap();

    assert_eq!(statuses.hotkey_sequences, ["shift+s"]);
}

#[test]
fn backlog_epic_label_and_release_filters_have_unique_hotkeys() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot(), sender, Default::default());
    let area = Rect::new(0, 0, 100, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);

    for (key, sequence) in [
        ("epics", "shift+e"),
        ("labels", "shift+l"),
        ("releases", "shift+a"),
    ] {
        let filter = layout
            .focus_targets()
            .iter()
            .find(|target| target.path == TreePath::from_keys([ChildKey::new(key)]))
            .unwrap();
        assert_eq!(filter.hotkey_sequences, [sequence]);
    }
}

#[test]
fn estimated_toggle_filters_the_loaded_backlog_without_reloading() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        WorkItem {
            story_points: Some(3.0),
            ..work_item("FIN-8", "Estimated story")
        },
        work_item("FIN-9", "Unestimated story"),
    ];
    let mut page = BacklogPage::with_snapshot_for_test(snapshot);
    let area = Rect::new(0, 0, 100, 16);
    page.layout(area, &mut LayoutCtx::new());

    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("estimated"),
        ])),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('d'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    assert!(!page.is_loading_for_test());
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
    assert!(!text.contains("Estimated story"));
    assert!(text.contains("Unestimated story"));
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
fn home_reset_restores_the_default_backlog_view() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    let mut child = work_item("FIN-9", "Hidden child");
    child.kind = "Sub-task".into();
    child.parent_key = Some("FIN-8".into());
    snapshot.work_items.push(child);
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.set_estimated(false);
    tree.set_issue_types_filter(vec!["Task".into()]);
    tree.set_users_filter(vec!["Maya".into()]);
    tree.set_epics_filter(vec!["Delivery".into()]);
    tree.set_labels_filter(vec!["ready".into()]);
    tree.set_releases_filter(vec!["v1.0".into()]);
    tree.group_by_release_for_test();
    tree.highlight("ticket:FIN-8");

    tree.reset_to_home();

    assert_eq!(
        tree.highlighted_id_for_test().as_deref(),
        Some("section:backlog")
    );
    assert!(tree.runway_markers_visible_for_test());
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

    assert!(text.contains("Plan next sprint"));
    assert!(!text.contains("Ship sprint work"));
    assert!(!text.contains("Hidden child"));
}

#[test]
fn expanded_sprint_shows_subtasks_under_their_parent() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    let mut subtask = work_item("FIN-9", "Finish sprint work");
    subtask.kind = "Sub-task".into();
    subtask.parent_key = Some("FIN-7".into());
    subtask.parent_title = Some("Ship sprint work".into());
    snapshot.sprints[0].work_items.push(subtask);
    let mut view = backlog_tree(&snapshot, sender, Default::default());
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

    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("FIN-7 Ship sprint work"));
    assert!(text.contains("FIN-9 Finish sprint work"));
    assert!(text.contains("0/1 done"));
}

#[test]
fn orphaned_subtasks_are_hidden_from_the_backlog_and_sprints() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    let mut sprint_subtask = work_item("FIN-9", "Hidden sprint child");
    sprint_subtask.kind = "Sub-task".into();
    sprint_subtask.parent_key = Some("MISSING-1".into());
    snapshot.sprints[0].work_items.push(sprint_subtask);
    let mut backlog_subtask = work_item("FIN-10", "Hidden backlog child");
    backlog_subtask.kind = "Sub-task".into();
    backlog_subtask.parent_key = Some("MISSING-2".into());
    snapshot.work_items.push(backlog_subtask);
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

    let text = rendered_lines(&terminal, area).concat();
    assert!(!text.contains("Hidden sprint child"));
    assert!(!text.contains("Hidden backlog child"));
}

#[test]
fn subtasks_cannot_enter_reorder_mode() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut snapshot = snapshot();
    let mut subtask = work_item("FIN-9", "Finish sprint work");
    subtask.kind = "Sub-task".into();
    subtask.parent_key = Some("FIN-7".into());
    snapshot.sprints[0].work_items.push(subtask);
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    view.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    view.highlight("ticket:FIN-9");

    view.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    assert!(!view.is_reordering_for_test());
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
fn status_shortcut_starts_loading_without_another_keypress() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    page.view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-8");
    let area = Rect::new(0, 0, 80, 16);
    page.layout(area, &mut LayoutCtx::new());
    let route = EventRoute::new(TreePath::from_keys([
        ChildKey::first(),
        ChildKey::first(),
        ChildKey::new("data"),
    ]));
    let target = FocusTarget {
        id: FocusId::new("data-view"),
        path: route.path.clone(),
        area: Rect::default(),
        enabled: true,
        tab_stop: true,
        control: true,
        hotkey: None,
        hotkeys: Vec::new(),
        hotkey_sequences: Vec::new(),
        suppress_global_hotkeys: false,
        focused_events_before_global_hotkeys: false,
    };
    page.dispatch_focus(
        &target,
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );

    page.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('s'))),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    assert!(page.is_status_loading_for_test());
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
    story.labels = vec!["AB".into(), "CD".into(), "Refinery".into()];
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
    assert!(text.contains("3 • @MV • 0/2  • To Do • AB|CD|Refinery • Shopping cart • 1.4.0"));
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
    assert_eq!(metadata_cell.fg, tuicore::theme().accent_fg());
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((epic_x, metadata_y as u16))
            .unwrap()
            .fg,
        tuicore::theme().warning_fg()
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

    assert!(text.contains("󰑓"));
    assert!(text.contains("󰓅"));
    assert!(text.contains("┃"));
    assert!(text.contains("3 • @AD • To Do"));
    let lines = rendered_lines(&terminal, area);
    let capacity_line = lines.iter().find(|line| line.contains("✓ 1/1")).unwrap();
    assert!(capacity_line.contains("✓ 1/1 est • 0/1 done • 󱩿 0/18 pts (20c)"));
    assert_eq!(cell_position(capacity_line, "✓"), Some(2));
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
    assert!(text.contains("󰓅"));
    assert!(text.contains(" Sprint 7 • 18 Jun – 2 Jul"));
    assert!(text.contains("✓ 1/1 est • 0/1 done • 󰸂 ~0/5.4 pts (20v)"));
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
    assert!(text.contains("󰄰 0/1 est • 0/1 done • 󰸂 ~0/5.4 pts (20v)"));

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
    assert!(text.contains("󰄰 0/1 est • 0/1 done • 󰸂 0/5.4 pts (20c)"));
    assert!(!text.contains("~0/5.4"));
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
    assert!(text.contains("✓ 1/1 est • 0/4 done • 󰸂 ~0/7 pts (20v)"));

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
            .contains("󰄰 0/1 est • 0/1 done • 󰸂 ~0/0 pts (20v)")
    );
}

#[test]
fn active_sprint_shows_completed_points_alongside_total_and_capacity() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints = vec![
        Sprint {
            id: 202,
            name: "DICE Sprint 202".into(),
            state: "active".into(),
            goal: None,
            start_date: Some("2026-08-26T09:00:00.000Z".into()),
            end_date: Some("2026-09-09T09:00:00.000Z".into()),
            work_items: vec![
                WorkItem {
                    story_points: Some(30.0),
                    status: "Done".into(),
                    done: true,
                    ..work_item("FIN-1", "Completed story")
                },
                WorkItem {
                    story_points: Some(80.2),
                    status: "In Progress".into(),
                    done: false,
                    ..work_item("FIN-2", "In flight story")
                },
                WorkItem {
                    kind: "Subtask".into(),
                    parent_key: Some("FIN-1".into()),
                    status: "Done".into(),
                    done: true,
                    ..work_item("FIN-10", "Subtask 1")
                },
                WorkItem {
                    kind: "Sub-task".into(),
                    parent_key: Some("FIN-2".into()),
                    status: "In Progress".into(),
                    done: false,
                    ..work_item("FIN-11", "Subtask 2")
                },
            ],
            capacity: None,
        },
        Sprint {
            id: 203,
            name: "DICE Sprint 203".into(),
            state: "future".into(),
            goal: None,
            start_date: Some("2026-09-10T09:00:00.000Z".into()),
            end_date: Some("2026-09-24T09:00:00.000Z".into()),
            work_items: vec![
                WorkItem {
                    story_points: Some(15.0),
                    status: "To Do".into(),
                    done: false,
                    ..work_item("FIN-3", "Future story")
                },
                WorkItem {
                    kind: "Subtask".into(),
                    parent_key: Some("FIN-3".into()),
                    status: "To Do".into(),
                    done: false,
                    ..work_item("FIN-12", "Future subtask")
                },
            ],
            capacity: None,
        },
    ];
    snapshot.work_items = vec![
        work_item("FIN-20", "Backlog story"),
        WorkItem {
            kind: "Task".into(),
            ..work_item("FIN-21", "Backlog task")
        },
        WorkItem {
            kind: "Bug".into(),
            ..work_item("FIN-22", "Backlog bug")
        },
        WorkItem {
            kind: "Subtask".into(),
            parent_key: Some("FIN-20".into()),
            ..work_item("FIN-23", "Backlog subtask")
        },
    ];
    apply_capacity(
        &mut snapshot,
        35.0,
        Some((5.0, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    let (sender, _) = mpsc::channel();
    let mut view = backlog_tree(&snapshot, sender, Default::default());
    let area = Rect::new(0, 0, 120, 20);
    view.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("✓ 2/2 est • 1/2 done • 󰸁 ~30/110.2 pts (35v)"));
    assert!(text.contains("✓ 1/1 est • 1 planned • 󰸂 ~15/35 pts (35v)"));
    assert!(text.contains(" Backlog • 3 items"));
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        story_points_configured: false,
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
fn wrapped_backlog_cache_reuses_renderer_work_and_refreshes_after_renderer_changes() {
    fn render_backlog(tree: &BacklogTree, terminal: &mut Terminal<TestBackend>, area: Rect) {
        terminal
            .draw(|frame| {
                let mut render = RenderCtx::new();
                tree.render(frame, area, &mut render);
                render.flush(frame);
            })
            .unwrap();
    }

    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints.clear();
    snapshot.work_items = (1..=32)
        .map(|number| {
            let title = if number == 1 {
                "A backlog title that wraps across this narrow viewport ".repeat(8)
            } else {
                "Short title".into()
            };
            work_item(&format!("FIN-{number}"), &title)
        })
        .collect();
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    let area = Rect::new(0, 0, 32, 8);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    tree.layout(area, &mut LayoutCtx::new());
    render_backlog(&tree, &mut terminal, area);
    assert!(!rendered_lines(&terminal, area).concat().contains("FIN-2"));
    let initial_calls = tree.take_renderer_calls_for_test();
    assert!(initial_calls > 32);
    tree.layout(area, &mut LayoutCtx::new());
    render_backlog(&tree, &mut terminal, area);
    let cached_calls = tree.take_renderer_calls_for_test();
    assert!(cached_calls < initial_calls);

    snapshot.work_items[0].title = "Short title".into();
    tree.set_snapshot(&snapshot);
    tree.layout(area, &mut LayoutCtx::new());
    render_backlog(&tree, &mut terminal, area);
    assert!(rendered_lines(&terminal, area).concat().contains("FIN-2"));
    assert!(tree.take_renderer_calls_for_test() > cached_calls);

    let mut ctx = EventCtx::new(AnimationSettings::default());
    for key in [
        Key::Char('0'),
        Key::Char('0'),
        Key::Esc,
        Key::Char('0'),
        Key::Enter,
    ] {
        tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(key)), &mut ctx);
        tree.layout(area, &mut LayoutCtx::new());
        render_backlog(&tree, &mut terminal, area);
        assert!(tree.take_renderer_calls_for_test() > cached_calls);
    }
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
fn saved_filter_dialog_shows_every_filter_and_marks_unavailable_values() {
    tuicore::init();
    let filter = SavedBacklogFilter {
        id: 1,
        name: "Release readiness".into(),
        criteria: BacklogFilterCriteria {
            users: vec!["Ada".into(), "Former contractor".into()],
            releases: vec!["4.8".into()],
            ..BacklogFilterCriteria::default()
        },
    };
    let options = BacklogFilterOptions {
        issue_types: vec!["Story".into(), "Bug".into()],
        users: vec!["Ada".into()],
        statuses: vec!["To Do".into()],
        epics: vec!["Delivery".into()],
        labels: vec!["release".into()],
        releases: vec!["4.9".into()],
    };
    let (sender, _) = mpsc::channel();
    let mut dialog = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &options,
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 90, 46);
    dialog.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            dialog.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    let text = lines.join("\n");
    for title in ["Types", "Users", "Statuses", "Epics", "Labels", "Releases"] {
        assert!(text.contains(title), "missing {title} filter");
    }
    let unavailable_y = lines
        .iter()
        .position(|line| line.contains("Former contractor · unavailable"))
        .unwrap() as u16;
    let unavailable_x = cell_position(
        &lines[usize::from(unavailable_y)],
        "Former contractor · unavailable",
    )
    .unwrap() as u16;
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((unavailable_x, unavailable_y))
            .unwrap()
            .fg,
        tuicore::theme().error_fg()
    );
}

#[test]
fn saved_filter_manager_opens_as_a_right_dock() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    let mut open = EventCtx::default();
    page.open_saved_filter_manager_for_test(&mut open);
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    page.view_for_test().layout(area, &mut layout);
    let selector = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "selector")
        })
        .unwrap()
        .clone();
    assert!(matches!(
        open.focus_request(),
        Some(FocusRequest::TargetAt { path, id })
            if path == &selector.path && id == &selector.id
    ));
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.view_for_test().render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("Manage saved filters"))
    );
    let selector_x = usize::from(selector.area.x);
    assert_eq!(selector_x, 61, "selector must sit beside the dock border");
    assert!(
        (selector_x.saturating_sub(3)..selector_x).any(|x| {
            (0..area.height).any(|y| {
                terminal.backend().buffer().cell((x as u16, y)).unwrap().fg
                    == tuicore::theme().accent_fg()
            })
        }),
        "focused manager border did not use the accent color"
    );
}

#[test]
fn saved_filter_manager_registers_management_hotkeys() {
    tuicore::init();
    let filter = SavedBacklogFilter::new(1, "Filter");
    let (sender, _) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &BacklogFilterOptions::default(),
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let mut layout = LayoutCtx::new();
    manager.layout(Rect::new(0, 0, 80, 30), &mut layout);

    for (slot, hotkey) in [
        ("selector", "shift+f"),
        ("new", "shift+n"),
        ("delete", "shift+d"),
        ("issue-types", "shift+t"),
        ("users", "shift+u"),
        ("statuses", "shift+s"),
        ("epics", "shift+e"),
        ("labels", "shift+l"),
        ("releases", "shift+a"),
    ] {
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|key| key.as_str() == slot))
            .unwrap_or_else(|| panic!("missing {slot} control"));
        assert_eq!(target.hotkey_sequences, [hotkey]);
    }
}

#[test]
fn saved_filter_delete_confirmation_keeps_the_manager_underneath() {
    tuicore::init();
    let filter = SavedBacklogFilter::new(1, "Release readiness");
    let (sender, _) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &BacklogFilterOptions::default(),
        sender,
        Rc::new(Cell::new(false)),
        Some((filter.id, filter.name.clone())),
    );
    let area = Rect::new(0, 0, 80, 30);
    manager.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            manager.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    let text = lines.join("\n");
    assert!(!text.contains("Manage saved filters"));
    assert!(text.contains("Delete saved filter?"));
    assert!(text.matches("Release readiness").count() >= 2);
    assert!(text.contains("Cancel (c)"));
    assert!(!text.contains("Keep (k)"));
    let confirmation_x = lines
        .iter()
        .find_map(|line| cell_position(line, "Delete saved filter?"))
        .unwrap();
    let manager_x = lines
        .iter()
        .filter_map(|line| cell_position(line, "Release readiness"))
        .max()
        .unwrap();
    assert!(manager_x >= 49, "manager started at column {manager_x}");
    assert!(
        confirmation_x < 40,
        "confirmation started at column {confirmation_x}"
    );
}

#[test]
fn saved_filter_selector_popup_is_visible_inside_the_right_dock() {
    tuicore::init();
    let filters = [
        SavedBacklogFilter::new(1, "First filter"),
        SavedBacklogFilter::new(2, "Second filter"),
    ];
    let (sender, _) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        &filters,
        Some(1),
        &BacklogFilterOptions::default(),
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    manager.layout(area, &mut layout);
    let selector = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|key| key.as_str() == "selector")
        })
        .unwrap()
        .clone();
    manager.dispatch_event(
        &EventRoute::new(selector.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    manager.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            manager.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    assert!(
        rendered_lines(&terminal, area)
            .iter()
            .any(|line| line.contains("Second filter"))
    );
}

#[test]
fn saved_filter_value_popup_is_visible_inside_the_right_dock() {
    tuicore::init();
    let filter = SavedBacklogFilter::new(1, "Filter");
    let options = BacklogFilterOptions {
        users: vec!["Ada".into(), "Grace".into(), "Linus".into()],
        ..BacklogFilterOptions::default()
    };
    let (sender, _) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &options,
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    manager.layout(area, &mut layout);
    let users = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && target.path.keys().iter().any(|key| key.as_str() == "users")
        })
        .unwrap()
        .clone();
    let outcome = manager.dispatch_event(
        &EventRoute::new(users.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('+'))),
        &mut EventCtx::default(),
    );
    assert_eq!(outcome, EventOutcome::Handled);
    manager.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            manager.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    let text = lines.join("\n");
    assert!(text.contains("Ada"));
    assert!(text.contains("Grace"));
    assert!(text.contains("Linus"));
    let option_rows = ["Ada", "Grace", "Linus"]
        .map(|option| lines.iter().position(|line| line.contains(option)).unwrap());
    assert_eq!(option_rows[1], option_rows[0] + 1);
    assert_eq!(option_rows[2], option_rows[1] + 1);
}

#[test]
fn new_saved_filter_is_an_empty_draft_and_focuses_its_name() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    page.open_saved_filter_manager_for_test(&mut EventCtx::default());
    let mut create_ctx = EventCtx::default();

    page.create_saved_filter_for_test(&mut create_ctx);

    assert_eq!(page.draft_saved_filter_for_test().unwrap().name, "");
    let Some(FocusRequest::TargetAt { path, id }) = create_ctx.focus_request() else {
        panic!("new filter name did not receive a targeted focus request");
    };
    assert_eq!(id.as_str(), "input");
    let mut layout = LayoutCtx::new();
    page.view_for_test()
        .layout(Rect::new(0, 0, 100, 30), &mut layout);
    let name = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "input"
                && target.path.keys().iter().any(|key| key.as_str() == "name")
        })
        .unwrap();
    assert_eq!(path, &name.path);
}

#[test]
fn naming_a_saved_filter_restores_name_focus_after_rebuild() {
    tuicore::init();
    let mut page = BacklogPage::with_snapshot_for_test(snapshot());
    page.open_saved_filter_manager_for_test(&mut EventCtx::default());
    page.create_saved_filter_for_test(&mut EventCtx::default());
    let id = page.draft_saved_filter_for_test().unwrap().id;
    let mut rename = EventCtx::default();

    page.rename_saved_filter_for_test(id, "Release readiness", &mut rename);

    let Some(FocusRequest::TargetAt { path, id }) = rename.focus_request() else {
        panic!("saved filter name did not regain focus after its dialog rebuilt");
    };
    assert_eq!(id.as_str(), "input");
    let mut layout = LayoutCtx::new();
    page.view_for_test()
        .layout(Rect::new(0, 0, 100, 30), &mut layout);
    let name = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "input"
                && target.path.keys().iter().any(|key| key.as_str() == "name")
        })
        .unwrap();
    assert_eq!(path, &name.path);
}

#[test]
fn new_saved_filter_name_starts_in_insert_mode() {
    tuicore::init();
    let filter = SavedBacklogFilter::new(1, "");
    let (sender, receiver) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &BacklogFilterOptions::default(),
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let mut layout = LayoutCtx::new();
    manager.layout(Rect::new(0, 0, 80, 30), &mut layout);
    let name = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "input"
                && target.path.keys().iter().any(|key| key.as_str() == "name")
        })
        .unwrap()
        .clone();
    manager.dispatch_focus(
        &name,
        true,
        &mut FocusCtx::new(AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        }),
    );
    let area = Rect::new(0, 0, 80, 30);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            manager.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((name.area.x.saturating_sub(1), name.area.y.saturating_sub(1)))
            .unwrap()
            .fg,
        tuicore::theme().accent_fg()
    );
    manager.dispatch_event(
        &EventRoute::new(name.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
        &mut EventCtx::default(),
    );
    manager.dispatch_event(
        &EventRoute::new(name.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    assert!(matches!(
        receiver.try_recv(),
        Ok(SavedFilterManagerEvent::Rename { id: 1, name }) if name == "x"
    ));
}

#[test]
fn saved_filter_value_lists_remove_the_highlighted_value_with_minus() {
    tuicore::init();
    let filter = SavedBacklogFilter {
        id: 1,
        name: "Release readiness".into(),
        criteria: BacklogFilterCriteria {
            users: vec!["Ada".into(), "Grace".into()],
            ..BacklogFilterCriteria::default()
        },
    };
    let options = BacklogFilterOptions {
        users: vec!["Ada".into(), "Grace".into()],
        ..BacklogFilterOptions::default()
    };
    let (sender, receiver) = mpsc::channel();
    let mut dialog = saved_filter_dialog(
        std::slice::from_ref(&filter),
        Some(filter.id),
        &options,
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 90, 46);
    let mut layout = LayoutCtx::new();
    dialog.layout(area, &mut layout);
    let users = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && target.path.keys().iter().any(|key| key.as_str() == "users")
        })
        .unwrap()
        .clone();

    dialog.dispatch_event(
        &EventRoute::new(users.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('-'))),
        &mut EventCtx::default(),
    );

    assert!(matches!(
        receiver.try_recv(),
        Ok(SavedFilterManagerEvent::SetValues {
            id: 1,
            field: SavedFilterField::Users,
            values,
        }) if values == ["Grace"]
    ));
}

#[test]
fn saved_filter_is_selectable_in_the_backlog_toolbar_and_applies_locally() {
    tuicore::init();
    let mut backlog_snapshot = snapshot();
    backlog_snapshot.work_items[0].assignee = "Bob".into();
    backlog_snapshot
        .work_items
        .push(work_item("FIN-9", "Ada backlog work"));
    let filter = SavedBacklogFilter {
        id: 1,
        name: "Ada only".into(),
        criteria: BacklogFilterCriteria {
            users: vec!["Ada".into(), "Unavailable user".into()],
            ..BacklogFilterCriteria::default()
        },
    };
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&backlog_snapshot, sender, Default::default());
    tree.set_saved_filters(std::slice::from_ref(&filter));
    tree.apply_saved_filter(&filter);
    let area = Rect::new(0, 0, 180, 20);
    tree.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let text = rendered_lines(&terminal, area).join("\n");
    assert!(text.contains("Ada only"));
    assert!(text.contains("Ada backlog work"));
    assert!(!text.contains("Plan next sprint"));
}

#[test]
fn velocity_dialog_shows_goals_in_alternating_two_line_rows() {
    tuicore::init();
    let report = VelocityReport {
        sprints: vec![
            VelocitySprint {
                id: 1,
                name: "Sprint one".into(),
                completed: 22.0,
                goal: Some("Ship release".into()),
                work_items: None,
            },
            VelocitySprint {
                id: 2,
                name: "Sprint two".into(),
                completed: 20.0,
                goal: None,
                work_items: None,
            },
            VelocitySprint {
                id: 3,
                name: "Sprint three".into(),
                completed: 18.0,
                goal: Some("Finish migration".into()),
                work_items: None,
            },
        ],
        dynamic_capacity: Some(21.0),
        configured_sprints: 2,
    };
    let mut dialog = velocity_dialog(
        Some(&report),
        &BacklogRunwaySettings::default(),
        None,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 80, 24);
    dialog.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            dialog.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    let sprint_one_y = lines
        .iter()
        .position(|line| line.contains("Sprint one"))
        .unwrap() as u16;
    let release_y = lines
        .iter()
        .position(|line| line.contains("Ship release"))
        .unwrap() as u16;
    let sprint_two_y = lines
        .iter()
        .position(|line| line.contains("Sprint two"))
        .unwrap() as u16;
    let missing_goal_y = lines
        .iter()
        .position(|line| line.contains("(no sprint goal)"))
        .unwrap() as u16;
    let migration_y = lines
        .iter()
        .position(|line| line.contains("Finish migration"))
        .unwrap() as u16;
    let sprint_one_x =
        cell_position(&lines[usize::from(sprint_one_y)], "Sprint one").unwrap() as u16;
    let sprint_two_x =
        cell_position(&lines[usize::from(sprint_two_y)], "Sprint two").unwrap() as u16;
    let missing_goal_x =
        cell_position(&lines[usize::from(missing_goal_y)], "(no sprint goal)").unwrap() as u16;
    let migration_x =
        cell_position(&lines[usize::from(migration_y)], "Finish migration").unwrap() as u16;

    assert_eq!(release_y, sprint_one_y + 1);
    assert_eq!(missing_goal_y, sprint_two_y + 1);
    assert!(
        terminal
            .backend()
            .buffer()
            .cell((sprint_one_x, sprint_one_y))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        terminal
            .backend()
            .buffer()
            .cell((sprint_two_x, sprint_two_y))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        !terminal
            .backend()
            .buffer()
            .cell((missing_goal_x, missing_goal_y))
            .unwrap()
            .modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((migration_x, migration_y))
            .unwrap()
            .bg,
        tuicore::theme().background_bg()
    );
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((missing_goal_x, missing_goal_y))
            .unwrap()
            .fg,
        tuicore::theme().muted_fg()
    );
}

#[test]
fn velocity_dialog_wraps_long_sprint_goals() {
    tuicore::init();
    let report = VelocityReport {
        sprints: vec![VelocitySprint {
            id: 1,
            name: "Sprint one".into(),
            completed: 22.0,
            goal: Some("Deliver the migration with reliable error handling".into()),
            work_items: None,
        }],
        dynamic_capacity: Some(22.0),
        configured_sprints: 1,
    };
    let mut dialog = velocity_dialog(
        Some(&report),
        &BacklogRunwaySettings::default(),
        None,
        Rc::new(Cell::new(false)),
        None,
    );
    let area = Rect::new(0, 0, 46, 24);
    dialog.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            dialog.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let lines = rendered_lines(&terminal, area);
    let goal_start = lines
        .iter()
        .position(|line| line.contains("Deliver the"))
        .unwrap();
    let goal_end = lines
        .iter()
        .position(|line| line.contains("handling"))
        .unwrap();

    assert!(goal_end > goal_start);
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
        .position(|line| line.contains("Ship release"))
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
fn backlog_search_keeps_subtasks_visible_when_the_parent_matches() {
    tuicore::init();
    let mut snapshot = snapshot();
    let parent = work_item("KAN-22", "Catalog browsing supports discovery");
    let mut matching_child = work_item("KAN-34", "Frontend integration");
    matching_child.kind = "Sub-task".into();
    matching_child.parent_key = Some("KAN-22".into());
    let mut non_matching_child = work_item("KAN-35", "Schema foundations");
    non_matching_child.kind = "Sub-task".into();
    non_matching_child.parent_key = Some("KAN-22".into());
    snapshot.work_items = vec![
        parent,
        matching_child,
        non_matching_child,
        work_item("KAN-30", "Checkout"),
    ];
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
    for key in "KAN-22".chars() {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut ctx,
        );
    }

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
    assert!(text.contains("KAN-22 Catalog browsing supports discovery"));
    assert!(text.contains("KAN-34 Frontend integration"));
    assert!(text.contains("KAN-35 Schema foundations"));
    assert!(!text.contains("KAN-30 Checkout"));
}

#[test]
fn backlog_search_highlights_a_matching_subtask_instead_of_its_parent() {
    tuicore::init();
    let mut snapshot = snapshot();
    let parent = work_item("KAN-22", "Catalog browsing supports discovery");
    let mut child = work_item("KAN-34", "Frontend integration");
    child.kind = "Sub-task".into();
    child.parent_key = Some("KAN-22".into());
    snapshot.work_items = vec![parent, child];
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
    for key in "Frontend integration".chars() {
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut ctx,
        );
    }

    assert_eq!(
        tree.highlighted_id_for_test().as_deref(),
        Some("ticket:KAN-34")
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
        top_level_backlog_keys: vec!["FIN-1".into(), "FIN-2".into()],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenQuickMenu { section_id, keys, source_order, section_moves_available }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"] && section_moves_available)
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('s'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenStatusMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('p'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenStoryPointsMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenAssignMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('e'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenEpicMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::OpenReleaseMenu { section_id, keys, source_order }) if section_id == "backlog" && keys == ["FIN-1", "FIN-2"] && source_order == ["FIN-1", "FIN-2"])
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('i'))),
        &mut ctx,
    );
    assert!(
        matches!(receiver.try_recv(), Ok(super::components::BacklogSectionEvent::ToggleCurrentUser { keys }) if keys == ["FIN-1", "FIN-2"])
    );
}

#[test]
fn focused_root_ticket_move_hotkeys_target_the_section_edges() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        work_item("FIN-1", "First"),
        work_item("FIN-2", "Second"),
        work_item("FIN-3", "Third"),
    ];
    let mut subtask = work_item("FIN-4", "Child");
    subtask.kind = "Sub-task".into();
    subtask.parent_key = Some("FIN-1".into());
    snapshot.work_items.push(subtask);
    let (sender, receiver) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.dispatch_focus(
        &data_focus_target(),
        true,
        &mut FocusCtx::new(AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(AnimationSettings::default());
    tree.highlight("ticket:FIN-2");

    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('t'))),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::MoveToEdge {
            section_id,
            key,
            source_order,
            to_top: true,
        }) if section_id == "backlog" && key == "FIN-2" && source_order == ["FIN-1", "FIN-2", "FIN-3"]
    ));

    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('b'))),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::MoveToEdge {
            section_id,
            key,
            source_order,
            to_top: false,
        }) if section_id == "backlog" && key == "FIN-2" && source_order == ["FIN-1", "FIN-2", "FIN-3"]
    ));

    tree.highlight("ticket:FIN-4");
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('t'))),
        &mut ctx,
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn moving_to_an_edge_selects_the_next_ticket_or_the_previous_ticket() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        work_item("FIN-1", "First"),
        work_item("FIN-2", "Second"),
        work_item("FIN-3", "Third"),
    ];
    let order = vec!["FIN-1".into(), "FIN-2".into(), "FIN-3".into()];

    let mut top_page = BacklogPage::with_snapshot_for_test(snapshot.clone());
    top_page
        .view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-2");
    top_page.move_from_menu("backlog".into(), vec!["FIN-2".into()], order.clone(), true);
    assert_eq!(
        top_page
            .view_for_test()
            .base_mut()
            .base_mut()
            .highlighted_id_for_test()
            .as_deref(),
        Some("ticket:FIN-3")
    );

    let mut bottom_page = BacklogPage::with_snapshot_for_test(snapshot.clone());
    bottom_page
        .view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-2");
    bottom_page.move_from_menu("backlog".into(), vec!["FIN-2".into()], order.clone(), false);
    assert_eq!(
        bottom_page
            .view_for_test()
            .base_mut()
            .base_mut()
            .highlighted_id_for_test()
            .as_deref(),
        Some("ticket:FIN-3")
    );

    let mut fallback_page = BacklogPage::with_snapshot_for_test(snapshot);
    fallback_page
        .view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-3");
    fallback_page.move_from_menu("backlog".into(), vec!["FIN-3".into()], order, true);
    assert_eq!(
        fallback_page
            .view_for_test()
            .base_mut()
            .base_mut()
            .highlighted_id_for_test()
            .as_deref(),
        Some("ticket:FIN-2")
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
fn open_command_targets_the_highlighted_backlog_ticket() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot(), sender, Default::default());
    tree.highlight("ticket:FIN-8");
    let mut ctx = EventCtx::default();
    tree.dispatch_event(
        &EventRoute::new(TreePath::from_keys([ChildKey::new("data")])),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char(';'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenCommand { key }) if key == "FIN-8"
    ));
    assert_eq!(ctx.propagation(), tuicore::Propagation::Stopped);
}

#[test]
fn enter_confirms_backlog_search_and_reordering_before_opening_details() {
    tuicore::init();
    for mode in [
        KeyEvent::from(Key::Char('/')),
        KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        let (sender, receiver) = mpsc::channel();
        let mut tree = backlog_tree(&snapshot(), sender, Default::default());
        tree.highlight("ticket:FIN-8");
        tree.dispatch_focus(&data_focus_target(), true, &mut FocusCtx::default());
        let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
        tree.dispatch_event(&route, &TuiEvent::Key(mode), &mut EventCtx::default());
        if mode.code == Key::Char('m') {
            assert!(tree.is_reordering_for_test());
        }
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        assert!(receiver.try_iter().all(|event| !matches!(
            event,
            super::components::BacklogSectionEvent::OpenDescription { .. }
        )));
        assert!(!tree.is_reordering_for_test());
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        assert!(matches!(receiver.try_recv(),
            Ok(super::components::BacklogSectionEvent::OpenDescription { key }) if key == "FIN-8"
        ));
    }
}

#[test]
fn enter_opens_a_focused_ticket_detail_dialog() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items[0].title = "Ticket title".into();
    snapshot.work_items[0].description = format!(
        "## Details\n\nScrollable description {} WRAPPED-END",
        "with enough context to require wrapping ".repeat(3)
    );
    snapshot.ticket_comments.insert(
        "FIN-8".into(),
        TicketComments {
            total: 2,
            comments: vec![TicketComment {
                id: "1".into(),
                parent_id: None,
                author: "Ada".into(),
                created: "2026-09-16".into(),
                body: "First comment".into(),
            }],
            complete: true,
        },
    );
    let mut page = BacklogPage::with_snapshot_for_test(snapshot);
    page.view_for_test()
        .base_mut()
        .base_mut()
        .highlight("ticket:FIN-8");
    let area = Rect::new(0, 0, 120, 30);
    page.layout(area, &mut LayoutCtx::new());
    let mut event = EventCtx::new(AnimationSettings::default());

    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("data"),
        ])),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut event,
    );

    assert!(page.view_for_test().is_active());
    assert!(
        matches!(event.focus_request(), Some(FocusRequest::Path(path)) if path == &TreePath::from_keys([ChildKey::second()]))
    );
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| page.layout(area, ctx));
    assert!(
        layout
            .focus_targets()
            .iter()
            .any(|target| target.id == FocusId::new("ticket-content"))
    );
    let description = layout
        .overlays()
        .iter()
        .find(|entry| entry.layer == tuicore::OverlayLayer::Modal)
        .expect("description dialog should be visible");
    assert_eq!(description.area.width, 90);
    assert_eq!(description.area.height, 24);
    let dialog_area = description.area;
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    let text = lines.concat();
    assert!(text.contains("Scrollable description"));
    assert!(text.contains("WRAPPED-END"));
    assert!(text.contains("Description"));
    assert!(text.contains("Comments (2)"));
    assert!(lines[dialog_area.y as usize].contains("Description"));
    assert!(lines[dialog_area.y as usize].contains("Comments (2)"));
    assert!(lines[dialog_area.y as usize + 1].contains("## Details"));
    let border = tuicore::border_chars(tuicore::preset().border());
    for y in dialog_area.y + 1..dialog_area.bottom() {
        for x in [dialog_area.x, dialog_area.right() - 1] {
            assert_eq!(
                terminal.backend().buffer().cell((x, y)).unwrap().symbol(),
                border.vertical
            );
        }
    }

    let mobile = Rect::new(0, 0, 60, 30);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(mobile, |ctx| page.layout(mobile, ctx));
    let description = layout
        .overlays()
        .iter()
        .find(|entry| entry.layer == tuicore::OverlayLayer::Modal)
        .expect("description dialog should stay visible after resize");
    assert_eq!(description.area.width, 60);
    let mobile_dialog_area = description.area;
    let mut terminal = Terminal::new(TestBackend::new(mobile.width, mobile.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, mobile, &mut render);
            render.flush(frame);
        })
        .unwrap();
    assert!(
        rendered_lines(&terminal, mobile)
            .concat()
            .contains("WRAPPED-END")
    );
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .cell((0, mobile_dialog_area.y + 1))
            .unwrap()
            .symbol(),
        "#"
    );

    let tabs = layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new("tabs"))
        .expect("ticket-detail tabs should be focusable");
    let animation = AnimationSettings {
        enabled: false,
        ..AnimationSettings::default()
    };
    page.dispatch_focus(tabs, true, &mut FocusCtx::new(animation));
    let mut event = EventCtx::new(animation);
    page.dispatch_event(
        &EventRoute::new(tabs.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char(']'))),
        &mut event,
    );
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(mobile, |ctx| page.layout(mobile, ctx));
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, mobile, &mut render);
            render.flush(frame);
        })
        .unwrap();
    assert!(
        rendered_lines(&terminal, mobile)
            .concat()
            .contains("First comment")
    );

    page.dispatch_event(
        &EventRoute::new(TreePath::from_keys([ChildKey::second()])),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut event,
    );
    assert!(!page.view_for_test().is_active());
}

#[test]
fn comment_bodies_hide_legacy_account_id_mentions() {
    assert_eq!(
        strip_legacy_account_id_mentions("[~accountid:557058:5a74] Nested comment"),
        "Nested comment"
    );
}

#[test]
fn comment_bodies_use_markdown_viewer_styles_and_preserve_layout() {
    tuicore::init();
    let source = "## Heading\n\n**Important** and `code`\n\n- Parent\n  - Child\n\n```rust\n    let value = 1;\n```";
    let comments = Rc::new(RefCell::new(TicketComments {
        total: 1,
        complete: true,
        comments: vec![TicketComment {
            id: "1".into(),
            parent_id: None,
            author: "Marlo Vlietstra".into(),
            created: String::new(),
            body: source.into(),
        }],
    }));
    let mut pane = TicketCommentsPane::new(comments, AppService::for_tests(), |_| {});
    let area = Rect::new(0, 0, 80, 12);
    pane.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            pane.render(frame, area, &mut RenderCtx::new());
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    for (index, line) in source.lines().enumerate() {
        assert_eq!(lines[index + 1].trim_end(), line);
    }

    let expected =
        tuicore::SyntaxHighlighter::new(source, tuicore::Language::Markdown).highlighted_text();
    let mut reference = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    reference
        .draw(|frame| {
            frame.render_widget(ratatui::widgets::Paragraph::new(expected.clone()), area);
        })
        .unwrap();
    for y in 0..source.lines().count() as u16 {
        for x in 0..area.width {
            let actual = terminal.backend().buffer().cell((x, y + 1)).unwrap();
            let expected = reference.backend().buffer().cell((x, y)).unwrap();
            assert_eq!(actual.fg, expected.fg, "foreground at ({x}, {y})");
            assert_eq!(
                actual.modifier, expected.modifier,
                "modifiers at ({x}, {y})"
            );
        }
    }
}

#[test]
fn comment_images_render_between_the_surrounding_markdown_blocks() {
    tuicore::init();
    let marker = ticket_image_marker(&TicketImage {
        url: "https://jira.example/rest/api/3/attachment/content/10248".into(),
        alt: "comment.png".into(),
        width: 300,
        height: 40,
    });
    let comments = Rc::new(RefCell::new(TicketComments {
        total: 1,
        complete: true,
        comments: vec![TicketComment {
            id: "1".into(),
            parent_id: None,
            author: "Marlo Vlietstra".into(),
            created: String::new(),
            body: format!("Before\n\n{marker}\n\nAfter"),
        }],
    }));
    let mut pane = TicketCommentsPane::new(comments, AppService::for_tests(), |_| {});
    let area = Rect::new(0, 0, 60, 10);
    pane.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            pane.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);

    assert_eq!(lines[1].trim_end(), "Before");
    assert!(lines[3].contains("Loading image: comment.png"));
    assert_eq!(lines[6].trim_end(), "After");
}

#[test]
fn comment_tree_renders_replies_and_opens_the_activated_comment() {
    tuicore::init();
    let comments = Rc::new(RefCell::new(TicketComments {
        total: 3,
        complete: true,
        comments: vec![
            TicketComment {
                id: "10047".into(),
                parent_id: None,
                author: "Marlo Vlietstra".into(),
                created: String::new(),
                body: "Root body".into(),
            },
            TicketComment {
                id: "10080".into(),
                parent_id: Some("10047".into()),
                author: "Marlo Vlietstra".into(),
                created: String::new(),
                body: "First reply".into(),
            },
            TicketComment {
                id: "10081".into(),
                parent_id: Some("10047".into()),
                author: "Marlo Vlietstra".into(),
                created: String::new(),
                body: "Second reply".into(),
            },
        ],
    }));
    let (opened, receiver) = mpsc::channel();
    let mut pane = TicketCommentsPane::new(comments, AppService::for_tests(), move |id| {
        opened.send(id).unwrap();
    });
    let area = Rect::new(0, 0, 80, 6);
    pane.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            pane.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    let root_indent = cell_position(&lines[1], "Root body").unwrap();

    assert!(cell_position(&lines[3], "First reply").unwrap() > root_indent);
    assert!(cell_position(&lines[5], "Second reply").unwrap() > root_indent);

    pane.focus(None, true, &mut FocusCtx::new(AnimationSettings::default()));
    let mut ctx = EventCtx::new(AnimationSettings {
        enabled: false,
        ..AnimationSettings::default()
    });
    let route = EventRoute::new(TreePath::default());
    pane.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Left)), &mut ctx);
    pane.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Right)), &mut ctx);
    assert!(receiver.try_recv().is_err());
    pane.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    assert_eq!(receiver.try_recv().unwrap(), "10047");
    pane.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Down)), &mut ctx);
    assert!(receiver.try_recv().is_err());
    pane.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    assert_eq!(receiver.try_recv().unwrap(), "10080");
    assert!(receiver.try_recv().is_err());
}

#[test]
fn description_snackbar_is_full_width_on_mobile_and_seventy_five_percent_on_desktop() {
    assert_eq!(description_width_percent(99), 100);
    assert_eq!(description_width_percent(100), 75);
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
fn y_opens_the_backlog_ticket_yank_menu_with_immediate_navigation() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let snapshot = snapshot();
    let key = snapshot.work_items[0].key.clone();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    tree.highlight(&format!("ticket:{key}"));
    let mut open = EventCtx::default();
    assert_eq!(
        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
            &mut open,
        ),
        EventOutcome::Handled
    );
    assert_eq!(open.propagation(), tuicore::Propagation::Stopped);
    let area = Rect::new(0, 0, 80, 16);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| tree.layout(area, ctx));
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();
    assert!(text.contains("URL"));
    assert!(text.contains("Description"));

    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let mut ctx = EventCtx::default();
    tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    assert_eq!(
        ctx.clipboard_request(),
        Some(snapshot.work_items[0].title.as_str())
    );

    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
        &mut EventCtx::default(),
    );
    let mut ctx = EventCtx::default();
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('k'))),
        &mut ctx,
    );
    assert_eq!(ctx.clipboard_request(), Some(key.as_str()));

    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
        &mut EventCtx::default(),
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('u'))),
        &mut EventCtx::default(),
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::YankTicketUrl { key: copied }) if copied == key
    ));

    tree.highlight("section:backlog");
    let mut ctx = EventCtx::default();
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
        &mut ctx,
    );
    assert_eq!(ctx.clipboard_request(), None);
}

#[test]
fn successful_direct_rank_keeps_the_optimistic_order_without_reconciliation() {
    tuicore::init();
    let rollback = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![
            work_item("FIN-1", "First"),
            work_item("FIN-2", "Second"),
            work_item("FIN-3", "Third"),
        ],
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
    };
    let final_order = vec!["FIN-3".into(), "FIN-1".into(), "FIN-2".into()];
    let mut optimistic = rollback.clone();
    optimistic.work_items.rotate_right(1);
    let plan = rank_plan(vec!["FIN-3".into()], &final_order)
        .unwrap()
        .unwrap();
    let mut page = BacklogPage::with_snapshot_for_test(optimistic);
    let generation = page.begin_rank_result_for_test(
        plan,
        PendingRank {
            rollback_snapshot: rollback,
            section_id: "backlog".into(),
            final_order,
            unconfirmed_refreshes: 0,
        },
    );

    assert!(page.apply_rank_result_for_test(generation, Ok(())));
    assert!(!page.is_loading_for_test());
    assert!(!page.is_ranking_for_test());
    assert!(!page.move_is_locked_for_test());
    assert!(!page.has_active_rank_plan_for_test());
    assert!(!page.has_pending_rank_for_test());
    assert!(!page.rank_refresh_retry_is_pending_for_test());

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
    assert!(
        cell_position(&text, "FIN-3 Third").expect("optimistic moved ticket is visible")
            < cell_position(&text, "FIN-1 First").expect("following ticket is visible")
    );

    page.tick(
        std::time::Duration::from_secs(2),
        AnimationSettings::default(),
    );
    assert!(!page.is_loading_for_test());
    assert!(!page.rank_refresh_retry_is_pending_for_test());
    assert!(!page.is_ranking_for_test());
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
fn quick_menu_labels_show_the_selected_ticket_status_and_assignee() {
    let snapshot = snapshot();

    assert_eq!(
        quick_menu_labels(Some(&snapshot), &["FIN-8".into()]),
        ("To Do".into(), "Ada".into(), String::new(), String::new())
    );
}

#[test]
fn quick_menu_opens_statuses_before_move_actions() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into(), "FIN-2".into()],
        "In progress".into(),
        "Marlo Vlietstra".into(),
        String::new(),
        String::new(),
        String::new(),
        Vec::new(),
        &mut ctx,
    ));
    assert_eq!(
        BacklogQuickMenu::main_action_labels("In progress", "Marlo Vlietstra", "", ""),
        [
            "Assign user (@MV)",
            "Set status (In progress)",
            "Set story points",
            "Set epic",
            "Set release",
            "View description",
            "Open command",
            "Move to top",
            "Move to bottom"
        ]
    );
    let area = Rect::new(0, 0, 60, 14);
    menu.layout(area, &mut LayoutCtx::new());
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::LoadStatuses { keys }] if keys.as_slice() == ["FIN-1"]
    ));

    menu.set_statuses(vec![StatusTransition {
        label: "Done".into(),
        issues: vec![IssueStatusTransition {
            issue_key: "FIN-1".into(),
            transition_id: "31".into(),
        }],
    }]);
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::SetStatus { status }]
            if status.label == "Done" && status.issues[0].transition_id == "31"
    ));
}

#[test]
fn quick_menu_right_aligns_action_hotkeys() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        "To Do".into(),
        "Ada".into(),
        "3".into(),
        "[FINERY-TC-20260906-H01] Resilient checkout delivery".into(),
        "v1.4".into(),
        Vec::new(),
        &mut ctx,
    ));
    let area = Rect::new(0, 0, 60, 14);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| menu.layout(area, ctx));
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);

    for (label, hotkey) in [
        ("Assign user (@AD)", "a"),
        ("Set status (To Do)", "s"),
        ("Set story points (3)", "p"),
        ("Set epic (", "e"),
        ("Set release (v1.4)", "r"),
        ("View description", "Enter"),
        ("Open command", "⌃;"),
        ("Move to top", "t"),
        ("Move to bottom", "b"),
    ] {
        let line = lines
            .iter()
            .find(|line| line.contains(label))
            .expect("action should be visible");
        assert!(line.trim_end().ends_with(hotkey));
    }
}

#[test]
fn quick_menu_open_command_supports_selection_and_its_configured_shortcut() {
    tuicore::init();
    for via_shortcut in [true, false] {
        let mut menu = BacklogQuickMenu::new(Default::default());
        let settings =
            crate::app_settings::AppSettings::resolve(&std::collections::HashMap::from([(
                crate::app_settings::OPEN_COMMAND_KEY_SETTING.into(),
                "alt+;".into(),
            )]))
            .unwrap();
        menu.set_open_command_key(settings.open_command_key);
        let mut ctx = EventCtx::default();
        assert!(menu.open(
            "backlog".into(),
            vec!["FIN-1".into()],
            vec!["FIN-1".into()],
            "To Do".into(),
            "Ada".into(),
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            &mut ctx,
        ));
        menu.layout(Rect::new(0, 0, 69, 18), &mut LayoutCtx::new());
        if via_shortcut {
            menu.dispatch_event(
                &EventRoute::new(TreePath::default()),
                &TuiEvent::Key(KeyEvent {
                    code: Key::Char(';'),
                    modifiers: KeyModifiers::ALT,
                }),
                &mut ctx,
            );
        } else {
            for character in "Open command".chars() {
                menu.event(
                    &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
                    &mut ctx,
                );
            }
            menu.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        }
        assert!(matches!(
            menu.take_events().as_slice(),
            [BacklogQuickMenuEvent::OpenCommand { key }] if key == "FIN-1"
        ));
        assert!(!menu.is_open_for_test());
    }
}

#[test]
fn quick_menu_view_description_targets_the_selected_ticket() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        "To Do".into(),
        "Ada".into(),
        String::new(),
        String::new(),
        String::new(),
        Vec::new(),
        &mut ctx,
    ));
    for _ in 0..5 {
        menu.event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut EventCtx::new(AnimationSettings::default()),
        );
    }

    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::ViewDescription { key }] if key == "FIN-1"
    ));
}

#[test]
fn story_points_menu_lists_standard_values_and_sets_three_points() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open_story_points_menu(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        &mut ctx,
    ));
    let area = Rect::new(0, 0, 60, 16);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| menu.layout(area, ctx));
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    for value in ["None", "1", "2", "3", "5", "8", "13", "20"] {
        assert!(lines.iter().any(|line| line.contains(value)));
    }
    let none_y = lines
        .iter()
        .enumerate()
        .find_map(|(y, line)| cell_position(line, "None").map(|_| y as u16))
        .expect("None option is visible");
    let none_x = cell_position(&lines[none_y as usize], "None").unwrap();
    for x in none_x..none_x + 4 {
        assert_eq!(
            terminal
                .backend()
                .buffer()
                .cell((x as u16, none_y))
                .unwrap()
                .fg,
            tuicore::theme().muted_fg()
        );
    }
    for _ in 0..2 {
        menu.event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut EventCtx::new(AnimationSettings::default()),
        );
    }
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::SetStoryPoints { keys, story_points }]
            if keys.as_slice() == ["FIN-1"] && *story_points == Some(3.0)
    ));
}

#[test]
fn optimistic_story_point_changes_update_selected_tickets() {
    let mut snapshot = snapshot();
    assert!(apply_story_points_to_snapshot(
        &mut snapshot,
        &["FIN-7".into(), "FIN-8".into()],
        Some(5.0),
    ));
    assert_eq!(snapshot.sprints[0].work_items[0].story_points, Some(5.0));
    assert_eq!(snapshot.work_items[0].story_points, Some(5.0));
    assert!(apply_story_points_to_snapshot(
        &mut snapshot,
        &["FIN-8".into()],
        None,
    ));
    assert_eq!(snapshot.work_items[0].story_points, None);
}

#[test]
fn quick_menu_assigns_a_user_or_unassigns_tickets() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        "To do".into(),
        "Unassigned".into(),
        String::new(),
        String::new(),
        String::new(),
        Vec::new(),
        &mut ctx,
    ));
    let area = Rect::new(0, 0, 60, 14);
    menu.layout(area, &mut LayoutCtx::new());
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::LoadAssignees]
    ));

    menu.set_assignees(vec![
        super::components::BacklogAssignee {
            account_id: String::new(),
            display_name: "Unassigned".into(),
        },
        super::components::BacklogAssignee {
            account_id: "ada".into(),
            display_name: "Ada".into(),
        },
    ]);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| menu.layout(area, ctx));
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let lines = rendered_lines(&terminal, area);
    let (none_y, none_x) = lines
        .iter()
        .enumerate()
        .find_map(|(y, line)| cell_position(line, "None").map(|x| (y as u16, x as u16)))
        .expect("None option is visible");
    for x in none_x..none_x + 4 {
        assert_eq!(
            terminal.backend().buffer().cell((x, none_y)).unwrap().fg,
            tuicore::theme().muted_fg()
        );
    }
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('k'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::AssignUser { keys, assignee }]
            if keys.as_slice() == ["FIN-1"]
                && assignee.account_id.is_empty()
                && assignee.display_name == "Unassigned"
    ));
}

#[test]
fn quick_menu_selects_multiple_releases_with_ctrl_enter() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open_release_menu(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        &mut ctx,
    ));
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::LoadReleases]
    ));
    menu.set_releases(vec![
        super::components::BacklogRelease {
            id: "10001".into(),
            name: "v2.0".into(),
        },
        super::components::BacklogRelease {
            id: "10000".into(),
            name: "v1.0".into(),
        },
    ]);
    let area = Rect::new(0, 0, 90, 14);
    menu.layout(area, &mut LayoutCtx::new());
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(menu.take_events().is_empty());
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    let events = menu.take_events();
    let [BacklogQuickMenuEvent::SetReleases { keys, releases }] = events.as_slice() else {
        panic!("expected selected releases, got {events:?}");
    };
    assert_eq!(keys.as_slice(), ["FIN-1"]);
    assert_eq!(
        releases
            .iter()
            .map(|release| release.id.as_str())
            .collect::<Vec<_>>(),
        ["10001", "10000"]
    );
}

#[test]
fn release_menu_uses_a_spinner_while_loading() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    menu.open_release_menu(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        &mut EventCtx::new(AnimationSettings::default()),
    );
    let area = Rect::new(0, 0, 90, 14);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| menu.layout(area, ctx));
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, area).concat();

    assert!(text.contains("Loading releases…"));
    assert!(!text.contains("□ Loading releases"));
}

#[test]
fn release_menu_selects_versions_already_set_on_the_ticket() {
    let mut menu = BacklogQuickMenu::new(Default::default());
    menu.set_current_release_names(vec!["v1.0".into()]);
    menu.set_releases(vec![
        super::components::BacklogRelease {
            id: "10001".into(),
            name: "v2.0".into(),
        },
        super::components::BacklogRelease {
            id: "10000".into(),
            name: "v1.0".into(),
        },
    ]);

    assert_eq!(menu.selected_release_ids_for_test(), ["10000"]);
}

#[test]
fn release_menu_clears_versions_when_all_selected_versions_are_toggled_off() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    menu.set_current_release_names(vec!["v1.0".into()]);
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open_release_menu(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        &mut ctx,
    ));
    menu.take_events();
    menu.set_releases(vec![super::components::BacklogRelease {
        id: "10000".into(),
        name: "v1.0".into(),
    }]);
    menu.layout(Rect::new(0, 0, 90, 14), &mut LayoutCtx::new());
    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::new(AnimationSettings::default()),
    );

    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::SetReleases { releases, .. }] if releases.is_empty()
    ));
}

#[test]
fn assignable_user_labels_include_their_avatar() {
    assert_eq!(
        BacklogQuickMenu::assignee_label(super::components::BacklogAssignee {
            account_id: "marlo".into(),
            display_name: "Marlo Vlietstra".into(),
        }),
        "Marlo Vlietstra (@MV)"
    );
}

#[test]
fn quick_menu_can_open_and_close_the_assign_menu_directly() {
    tuicore::init();
    let mut menu = BacklogQuickMenu::new(Default::default());
    let mut ctx = EventCtx::new(AnimationSettings::default());
    assert!(menu.open_assign_menu(
        "backlog".into(),
        vec!["FIN-1".into()],
        vec!["FIN-1".into()],
        &mut ctx,
    ));
    assert!(menu.is_open_for_test());
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::LoadAssignees]
    ));

    menu.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::new(AnimationSettings::default()),
    );
    assert!(!menu.is_open_for_test());
    assert!(matches!(
        menu.take_events().as_slice(),
        [BacklogQuickMenuEvent::Closed]
    ));
}

#[test]
fn current_user_assignment_toggles_the_selected_ticket_assignee() {
    let snapshot = snapshot();
    let current = crate::jira::JiraAssignee {
        account_id: "ada".into(),
        display_name: "Ada".into(),
    };

    assert_eq!(
        current_user_assignment(Some(&snapshot), &["FIN-8".into()], &current),
        super::components::BacklogAssignee {
            account_id: String::new(),
            display_name: "Unassigned".into(),
        }
    );
}

#[test]
fn status_transitions_are_cached_per_ticket_until_the_ticket_changes_status() {
    let keys = vec!["FIN-1".into(), "FIN-2".into()];
    let mut cache = StatusTransitionCache::default();
    assert_eq!(cache.missing_keys(&keys), keys);

    cache.insert(vec![
        (
            "FIN-1".into(),
            vec![JiraOption {
                id: "31".into(),
                label: "Done".into(),
            }],
        ),
        (
            "FIN-2".into(),
            vec![JiraOption {
                id: "42".into(),
                label: "done".into(),
            }],
        ),
    ]);

    assert!(cache.missing_keys(&keys).is_empty());
    let statuses = cache.common(&keys).unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].issues[0].transition_id, "31");
    assert_eq!(statuses[0].issues[1].transition_id, "42");

    cache.invalidate(&statuses[0]);
    assert_eq!(cache.missing_keys(&keys), keys);
}

#[test]
fn status_change_updates_active_sprint_done_totals() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items[0].story_points = Some(5.0);
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );

    assert!(apply_status_to_snapshot(
        &mut snapshot,
        &["FIN-7".into()],
        "Done",
    ));
    recalculate_capacity(&mut snapshot, &BacklogRunwaySettings::default());

    let item = &snapshot.sprints[0].work_items[0];
    assert_eq!(item.status, "Done");
    assert!(item.done);
    assert!(item.status_changed_at.is_some());
    assert_eq!(
        snapshot.sprints[0]
            .capacity
            .as_ref()
            .unwrap()
            .completed_points,
        5.0
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
    assert!(
        rendered_lines(&terminal, area)
            .concat()
            .contains("✓ 1/1 est • 1/1 done • 󰸂 5/5 pts (20c)")
    );
}

#[test]
fn assignment_updates_the_selected_ticket_without_refreshing_the_backlog() {
    let mut snapshot = snapshot();

    assert!(apply_assignee_to_snapshot(
        &mut snapshot,
        &["FIN-8".into()],
        "Marlo Vlietstra",
    ));
    assert_eq!(snapshot.work_items[0].assignee, "Marlo Vlietstra");
}

#[test]
fn separate_ticket_updates_complete_independently() {
    let mut generations = RequestGenerations::default();
    let assignee_update = generations.start_users_assign();
    let status_update = generations.start_status_set();

    assert!(generations.complete_status_set(status_update));
    assert!(generations.complete_users_assign(assignee_update));
}

#[test]
fn assignee_updates_remain_available_for_other_tickets_while_one_syncs() {
    tuicore::init();
    let (sender, receiver) = mpsc::channel();
    let syncing = Rc::new(RefCell::new(HashSet::from(["FIN-7".to_owned()])));
    let mut tree = backlog_tree_with_issue_types(
        &snapshot(),
        sender,
        Default::default(),
        Rc::clone(&syncing),
        Vec::new(),
    );
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
        &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::OpenAssignMenu { keys, .. }) if keys == ["FIN-8"]
    ));

    *syncing.borrow_mut() = HashSet::from(["FIN-8".to_owned()]);
    tree.highlight("ticket:FIN-8");
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
        &mut ctx,
    );
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::components::BacklogSectionEvent::TicketsSyncing { keys }) if keys == ["FIN-8"]
    ));
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
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
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
    assert!(should_poll(true, false, false, false));
    assert!(should_poll(false, true, false, false));
    assert!(should_poll(false, false, true, false));
    assert!(should_poll(false, false, false, true));
    assert!(!should_poll(false, false, false, false));
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
