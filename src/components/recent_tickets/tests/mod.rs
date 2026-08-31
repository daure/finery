use super::{recent_ticket_row, recent_ticket_text};
use crate::{service::AppService, store::work_items::WorkItem};
use tuicore::{EventCtx, FocusId, FocusRequest};

#[test]
fn unestimated_tasks_and_stories_use_the_assumed_story_points() {
    for kind in ["Task", "Story"] {
        let row = recent_ticket_row(work_item(kind), true, 3.0, false);
        assert_eq!(row.item.story_points, Some(3.0));
        assert!(row.item.story_points_estimated);
    }
}

#[test]
fn unestimated_non_estimated_ticket_types_keep_the_dash_placeholder() {
    for kind in ["Bug", "Epic", "Sub-task"] {
        let row = recent_ticket_row(work_item(kind), true, 3.0, false);
        assert_eq!(row.item.story_points, None);
        assert!(!row.item.story_points_estimated);
    }
}

#[test]
fn ticket_releases_render_as_highlight_chips() {
    tuicore::init();
    let mut ticket = work_item("Story");
    ticket.fix_versions = vec!["1.4.0".into()];

    let text = recent_ticket_text(&recent_ticket_row(ticket, true, 3.0, false));
    let release = text.lines[1]
        .spans
        .iter()
        .find(|span| span.content == "1.4.0")
        .unwrap();

    assert_eq!(release.style.bg, Some(tuicore::theme().highlight_bg()));
    assert!(
        release
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
}

#[test]
fn opening_requests_focus_for_the_search_input() {
    let mut menu = super::RecentTicketsMenu::new(AppService::for_tests());
    let mut ctx = EventCtx::default();

    menu.open(&mut ctx);

    assert!(matches!(
        ctx.focus_request(),
        Some(FocusRequest::Target(id)) if id == &FocusId::new("input")
    ));
}

fn work_item(kind: &str) -> WorkItem {
    WorkItem {
        key: "FIN-1".into(),
        title: "Example".into(),
        kind: kind.into(),
        status: "To Do".into(),
        done: false,
        priority: String::new(),
        assignee: "Unassigned".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        fix_versions: Vec::new(),
        epic_name: None,
        story_points: None,
    }
}
