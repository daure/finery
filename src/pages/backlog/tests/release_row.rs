use super::*;
use crate::store::work_items::{
    RunwayCapacitySource, Sprint, WorkItem, apply_capacity,
    release::{ReleaseVersion, parse_date},
};
use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

fn release_snapshot() -> BacklogSnapshot {
    let release = ReleaseVersion {
        id: "1".into(),
        name: "v1.0".into(),
        start_date: parse_date("2026-09-14"),
        end_date: parse_date("2026-10-02"),
    };
    let story = WorkItem {
        key: "FIN-1".into(),
        title: "Deliver release".into(),
        description: String::new(),
        kind: "Story".into(),
        status: "To Do".into(),
        status_category: crate::store::work_items::StatusCategory::Todo,
        done: false,
        priority: String::new(),
        assignee: String::new(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        labels: Vec::new(),
        fix_versions: vec!["v1.0".into()],
        releases: vec![release],
        epic_name: None,
        story_points: Some(30.0),
        status_changed_at: None,
    };
    let mut snapshot = BacklogSnapshot {
        board_name: String::new(),
        story_points_configured: true,
        sprints: vec![Sprint {
            id: 1,
            name: "Sprint 1".into(),
            state: "active".into(),
            goal: None,
            start_date: Some("2026-08-31T09:00:00Z".into()),
            end_date: Some("2026-09-14T09:00:00Z".into()),
            work_items: Vec::new(),
            capacity: None,
        }],
        work_items: vec![story],
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
        ticket_comments: Default::default(),
    };
    apply_capacity(
        &mut snapshot,
        10.0,
        Some((3.0, false)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    snapshot
}

#[test]
fn release_and_sprint_thermometers_share_semantic_capacity_colors() {
    use super::super::{BacklogRowContent, sprint_section_row};

    tuicore::init();
    let theme = tuicore::theme();
    for (points, icon, color) in [
        (5.0, "󰸂", theme.warning_fg()),
        (10.0, "󱩿", theme.success_fg()),
        (20.0, "󰸁", theme.error_fg()),
    ] {
        let mut snapshot = release_snapshot();
        snapshot.work_items[0].story_points = Some(points);
        snapshot.work_items[0].releases[0].end_date = parse_date("2026-09-25");
        snapshot.sprints[0].work_items = std::mem::take(&mut snapshot.work_items);
        apply_capacity(
            &mut snapshot,
            10.0,
            Some((3.0, false)),
            RunwayCapacitySource::JiraVelocity,
            20,
        );
        let group = WorkItemGroup {
            label: "v1.0".into(),
            items: snapshot.sprints[0].work_items.iter().collect(),
            root_count: 1,
        };
        let release = title(&snapshot, &group, parse_date("2026-09-12").unwrap(), false);
        let BacklogRowContent::Section { title: sprint, .. } =
            sprint_section_row("sprint-1", &snapshot.sprints[0]).content
        else {
            panic!("Sprint header must be a section");
        };
        for text in [release, sprint] {
            let span = text
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .find(|span| span.content == icon)
                .unwrap();
            assert_eq!(span.style.fg, Some(color));
        }
    }
}

#[test]
fn release_header_renders_dates_forecast_and_ordered_statistics() {
    tuicore::init();
    let mut snapshot = release_snapshot();
    let group = WorkItemGroup {
        label: "v1.0".into(),
        items: snapshot.work_items.iter().collect(),
        root_count: 1,
    };
    let text = title(&snapshot, &group, parse_date("2026-09-12").unwrap(), false);
    let mut terminal = Terminal::new(TestBackend::new(80, 2)).unwrap();
    terminal
        .draw(|frame| frame.render_widget(Paragraph::new(text.clone()), frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let lines = (0..2)
        .map(|y| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            " v1.0 • 14 Sep – 2 Oct • 󰑮 3 planned 󰸁 1.5 available",
            "✓ 1/1 est • 1 open • 30 pts remaining",
        ]
    );

    snapshot.work_items[0].story_points = None;
    let group = WorkItemGroup {
        label: "v1.0".into(),
        items: snapshot.work_items.iter().collect(),
        root_count: 1,
    };
    let text = title(&snapshot, &group, parse_date("2026-09-12").unwrap(), false);
    assert_eq!(
        text.lines[0].to_string(),
        " v1.0 • 14 Sep – 2 Oct • 󰑮 0.3 planned 󰸂 1.5 available"
    );
    assert_eq!(
        text.lines[1].to_string(),
        "󰄰 0/1 est • 1 open • ~3 pts remaining"
    );

    for (start, end, warning) in [
        (None, parse_date("2026-10-02"), "Start date missing"),
        (parse_date("2026-09-14"), None, "End date missing"),
        (None, None, "Start and end date missing"),
    ] {
        snapshot.work_items[0].releases[0].start_date = start;
        snapshot.work_items[0].releases[0].end_date = end;
        let group = WorkItemGroup {
            label: "v1.0".into(),
            items: snapshot.work_items.iter().collect(),
            root_count: 1,
        };
        let text = title(&snapshot, &group, parse_date("2026-09-12").unwrap(), false);
        assert_eq!(text.lines[0].to_string(), format!(" v1.0 •  {warning}"));
        assert_eq!(
            text.lines[1].to_string(),
            "󰄰 0/1 est • 1 open • ~3 pts remaining"
        );
    }
}

#[test]
fn release_header_distinguishes_planning_active_overdue_and_delivered_work() {
    tuicore::init();
    let mut snapshot = release_snapshot();
    let mut done = snapshot.work_items[0].clone();
    done.key = "FIN-2".into();
    done.story_points = Some(10.0);
    done.done = true;
    snapshot.work_items.push(done);
    let group = WorkItemGroup {
        label: "v1.0".into(),
        items: snapshot.work_items.iter().collect(),
        root_count: 2,
    };
    for (today, summary) in [
        ("2026-09-13", "󰑮 4 planned 󰸁 1.5 available"),
        ("2026-09-14", "󰑮 3 todo 󰸁 1.5 left"),
        ("2026-09-28", "󰑮 3 todo 󰸁 0.5 left"),
        ("2026-10-02", "󰑮 3 todo 󰸁 0.1 left"),
        ("2026-10-03", "󰑮 3 unfinished 󰸁 1d overdue"),
        ("2026-10-05", "󰑮 3 unfinished 󰸁 3d overdue"),
    ] {
        let text = title(&snapshot, &group, parse_date(today).unwrap(), false);
        assert_eq!(
            text.lines[0].to_string(),
            format!(" v1.0 • 14 Sep – 2 Oct • {summary}")
        );
        assert_eq!(
            text.lines[1].to_string(),
            "✓ 2/2 est • 1 open • 30 pts remaining"
        );
        let filtered = title(&snapshot, &group, parse_date(today).unwrap(), true);
        assert_eq!(
            filtered.lines[0].to_string(),
            format!("{} (some tickets hidden by filters)", text.lines[0])
        );
        assert_eq!(filtered.lines[1], text.lines[1]);
    }
    snapshot.work_items[0].done = true;
    let group = WorkItemGroup {
        label: "v1.0".into(),
        items: snapshot.work_items.iter().collect(),
        root_count: 2,
    };
    for today in ["2026-09-13", "2026-09-28", "2026-10-05"] {
        let text = title(&snapshot, &group, parse_date(today).unwrap(), false);
        assert_eq!(
            text.lines[0].to_string(),
            " v1.0 • 14 Sep – 2 Oct •  Delivered"
        );
        assert_eq!(
            text.lines[1].to_string(),
            "✓ 2/2 est • 0 open • 0 pts remaining"
        );
    }

    snapshot.work_items[0].done = false;
    snapshot.runway = None;
    let group = WorkItemGroup {
        label: "v1.0".into(),
        items: snapshot.work_items.iter().collect(),
        root_count: 2,
    };
    let text = title(&snapshot, &group, parse_date("2026-10-05").unwrap(), false);
    assert_eq!(
        text.lines[0].to_string(),
        " v1.0 • 14 Sep – 2 Oct • 󰸁 3d overdue"
    );
}

#[test]
fn release_filters_preserve_full_metrics_while_filtering_children() {
    use super::super::{BacklogRowContent, BacklogTree, backlog_tree};
    use tuicore::{AnimationSettings, EventCtx, EventOutcome, Key, KeyEvent, TuiEvent};

    tuicore::init();
    let mut snapshot = release_snapshot();
    snapshot.work_items[0].assignee = "Ada".into();
    snapshot.work_items[0].done = true;
    let mut task = snapshot.work_items[0].clone();
    task.key = "FIN-2".into();
    task.kind = "Task".into();
    task.title = "Prepare rollout".into();
    task.assignee = "Maya".into();
    task.done = false;
    task.story_points = None;
    snapshot.sprints[0].work_items.push(task);
    let mut other_release = snapshot.work_items[0].clone();
    other_release.key = "FIN-3".into();
    other_release.assignee = "Grace".into();
    other_release.fix_versions = vec!["v2.0".into()];
    other_release.releases[0].id = "2".into();
    other_release.releases[0].name = "v2.0".into();
    snapshot.work_items.push(other_release);
    let (sender, _) = std::sync::mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.group_by_release_for_test();
    let group_title = |tree: &BacklogTree| {
        tree.control
            .items()
            .iter()
            .find_map(|row| match &row.content {
                BacklogRowContent::Group { title, search_text } if search_text == "v1.0" => {
                    Some(title.clone())
                }
                _ => None,
            })
            .unwrap()
    };
    let original = group_title(&tree);
    let mut expected = original.clone();
    expected.lines[0].spans.push(Span::styled(
        " (some tickets hidden by filters)",
        Style::default().fg(tuicore::theme().muted_fg()),
    ));
    let ticket_keys = |tree: &BacklogTree| {
        tree.control
            .items()
            .iter()
            .filter_map(|row| match &row.content {
                BacklogRowContent::WorkItem(work_item) => Some(work_item.item.key.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    tree.set_users_filter(vec!["Ada".into(), "Maya".into()]);
    assert_eq!(group_title(&tree), original);
    assert_eq!(ticket_keys(&tree), ["FIN-2", "FIN-1"]);
    tree.set_users_filter(vec!["Ada".into()]);
    assert_eq!(group_title(&tree), expected);
    assert_eq!(ticket_keys(&tree), ["FIN-1"]);
    tree.set_users_filter(Vec::new());
    assert_eq!(group_title(&tree), original);

    tree.set_estimated(false);
    assert_eq!(group_title(&tree), expected);
    assert_eq!(ticket_keys(&tree), ["FIN-2"]);
    tree.set_estimated(true);
    assert_eq!(group_title(&tree), original);

    tree.set_issue_types_filter(vec!["Story".into()]);
    assert_eq!(group_title(&tree), expected);
    assert_eq!(ticket_keys(&tree), ["FIN-1", "FIN-3"]);
    tree.set_issue_types_filter(Vec::new());
    assert_eq!(group_title(&tree), original);

    tree.set_releases_filter(vec!["v1.0".into()]);
    assert_eq!(group_title(&tree), original);
    tree.set_releases_filter(Vec::new());

    for (query, hides_tickets) in [
        ("FIN", false),
        ("Deliver", true),
        ("v1.0", false),
        ("Deliver release", true),
        ("", false),
    ] {
        tree.handle_event(
            &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
            &mut EventCtx::new(AnimationSettings::default()),
            |control, _| {
                control.data_view_mut().set_search_query(query);
                EventOutcome::Handled
            },
        );
        if hides_tickets {
            assert_eq!(group_title(&tree), expected);
        } else {
            assert_eq!(group_title(&tree), original);
        }
    }
}
