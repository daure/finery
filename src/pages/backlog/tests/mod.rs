use std::{cell::Cell, rc::Rc, sync::mpsc};

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{FocusId, Key, KeyEvent, KeyModifiers, LayoutCtx, RenderCtx, TuiEvent, TuiNode};

use super::{
    components::backlog_section,
    page::{LoadCompletion, RankRefreshRetry, RequestGenerations, should_poll, snapshot_view},
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

#[test]
fn sprint_sections_start_collapsed_and_backlog_starts_expanded() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        sprints: vec![Sprint {
            id: 7,
            name: "Sprint 7".into(),
            state: "active".into(),
            work_items: vec![work_item("FIN-7", "Ship sprint work")],
        }],
        work_items: vec![work_item("FIN-8", "Plan next sprint")],
        warnings: Vec::new(),
    };
    let mut view = snapshot_view(&snapshot);
    let area = Rect::new(0, 0, 80, 16);
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
    let buffer = terminal.backend().buffer();
    let text = (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect::<String>();

    assert!(text.contains("Finery · Sprint 7 (active)"));
    assert!(text.contains("Finery · Backlog"));
    assert!(text.contains("󰄱"));
    assert!(!text.contains("Ship sprint work"));
    assert!(text.contains("Plan next sprint"));
    assert!(layout.focus_targets().iter().any(|target| {
        target.id == FocusId::new("data-view") && target.hotkey_sequences == ["shift+b"]
    }));
}

#[test]
fn backlog_rank_plan_uses_the_next_unmoved_issue_as_the_before_anchor() {
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
    assert_eq!(plan.rank_after_issue, None);
}

#[test]
fn backlog_rank_plan_uses_the_previous_unmoved_issue_or_skips_all_selected() {
    let plan = rank_plan(
        vec!["FIN-2".into(), "FIN-3".into()],
        &["FIN-1".into(), "FIN-2".into(), "FIN-3".into()],
    )
    .unwrap()
    .unwrap();
    assert_eq!(plan.rank_before_issue, None);
    assert_eq!(plan.rank_after_issue.as_deref(), Some("FIN-1"));

    assert!(
        rank_plan(
            vec!["FIN-1".into(), "FIN-2".into()],
            &["FIN-1".into(), "FIN-2".into()],
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn backlog_rank_plan_rejects_more_than_fifty_issues() {
    let issues = (1..=51)
        .map(|number| format!("FIN-{number}"))
        .collect::<Vec<_>>();

    assert!(
        rank_plan(issues.clone(), &issues)
            .unwrap_err()
            .contains("at most 50")
    );
}

#[test]
fn rank_start_invalidates_an_older_load_result() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let initial_load = generations.start_load(false, false);
    let rank = generations.start_rank();

    assert!(!generations.complete_rank(rank + 1));
    assert!(generations.complete_rank(rank));
    let rank_reload = generations.start_load(true, true);
    assert!(rank_reload > rank);
    assert_eq!(
        generations.complete_load(initial_load, true, &move_locked),
        None
    );
    assert!(move_locked.get());
    assert_eq!(
        generations.complete_load(rank_reload, true, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: true,
        })
    );
    assert!(!move_locked.get());
}

#[test]
fn polling_continues_while_loading_or_ranking() {
    assert!(should_poll(true, false, false));
    assert!(should_poll(false, true, false));
    assert!(should_poll(false, false, true));
    assert!(!should_poll(false, false, false));
}

#[test]
fn only_matching_successful_rank_refresh_unlocks_moves() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let refresh = generations.start_load(true, false);

    assert_eq!(
        generations.complete_load(refresh + 1, true, &move_locked),
        None
    );
    assert!(move_locked.get());
    assert_eq!(
        generations.complete_load(refresh, false, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: false,
        })
    );
    assert!(move_locked.get());

    let normal_load = generations.start_load(false, false);
    assert_eq!(
        generations.complete_load(normal_load, true, &move_locked),
        Some(LoadCompletion::Normal)
    );
    assert!(move_locked.get());

    let retry = generations.start_load(true, false);
    assert_eq!(
        generations.complete_load(retry, true, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: false,
        })
    );
    assert!(!move_locked.get());
}

#[test]
fn failed_rank_refresh_stays_locked_and_schedules_a_retry() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let refresh = generations.start_load(true, true);
    let mut retry = RankRefreshRetry::default();

    assert_eq!(
        generations.complete_load(refresh, false, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: true,
        })
    );
    retry.schedule(true);

    assert!(move_locked.get());
    assert!(retry.pending());
    assert!(should_poll(false, false, retry.pending()));
    assert_eq!(retry.elapse(std::time::Duration::from_millis(999)), None);
    assert_eq!(
        retry.elapse(std::time::Duration::from_millis(1)),
        Some(true)
    );
    assert!(!retry.pending());
}

#[test]
fn failed_optimistic_rank_refresh_retries_preserving_live_view() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let refresh = generations.start_load(true, true);
    let mut retry = RankRefreshRetry::default();

    assert_eq!(
        generations.complete_load(refresh, false, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: true,
        })
    );

    retry.schedule(true);
    let retry = generations.start_load(
        true,
        retry.elapse(std::time::Duration::from_secs(1)).unwrap(),
    );
    assert_eq!(
        generations.complete_load(retry, true, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: true,
        })
    );
}

#[test]
fn failed_rank_error_refresh_retries_with_a_rebuilt_view() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let refresh = generations.start_load(true, false);
    let mut retry = RankRefreshRetry::default();

    assert_eq!(
        generations.complete_load(refresh, false, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: false,
        })
    );

    retry.schedule(false);
    let retry = generations.start_load(
        true,
        retry.elapse(std::time::Duration::from_secs(1)).unwrap(),
    );
    assert_eq!(
        generations.complete_load(retry, true, &move_locked),
        Some(LoadCompletion::RankRefresh {
            preserve_optimistic_view: false,
        })
    );
}

#[test]
fn newer_load_invalidates_optimistic_rank_refresh_preservation() {
    let mut generations = RequestGenerations::default();
    let move_locked = Cell::new(true);
    let optimistic_refresh = generations.start_load(true, true);
    let recovery_load = generations.start_load(false, false);

    assert_eq!(
        generations.complete_load(optimistic_refresh, true, &move_locked),
        None
    );
    assert_eq!(
        generations.complete_load(recovery_load, true, &move_locked),
        Some(LoadCompletion::Normal)
    );
}

#[test]
fn locked_backlog_blocks_move_keys_and_multi_ticket_space() {
    let (sender, _) = mpsc::channel();
    let section = backlog_section(
        "backlog",
        "Backlog",
        &[],
        None,
        true,
        sender,
        Rc::new(Cell::new(true)),
    );
    let ctrl_shift_m = TuiEvent::Key(KeyEvent {
        code: Key::Char('m'),
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    });
    let shifted_less_than = TuiEvent::Key(KeyEvent {
        code: Key::Char('<'),
        modifiers: KeyModifiers::SHIFT,
    });
    let shifted_greater_than = TuiEvent::Key(KeyEvent {
        code: Key::Char('>'),
        modifiers: KeyModifiers::SHIFT,
    });

    assert!(section.blocks_move_gesture(&ctrl_shift_m));
    assert!(section.blocks_move_gesture(&shifted_less_than));
    assert!(section.blocks_move_gesture(&shifted_greater_than));
    assert!(!section.blocks_move_gesture(&TuiEvent::Key(KeyEvent::from(Key::Down))));

    let space = TuiEvent::Key(KeyEvent::from(Key::Char(' ')));
    assert!(!section.blocks_move_gesture_with_selection_count(0, &space));
    assert!(!section.blocks_move_gesture_with_selection_count(1, &space));
    assert!(section.blocks_move_gesture_with_selection_count(2, &space));
}
