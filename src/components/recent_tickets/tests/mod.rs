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
fn ticket_releases_render_as_bold_accent_text() {
    tuicore::init();
    let mut ticket = work_item("Story");
    ticket.fix_versions = vec!["1.4.0".into()];

    let text = recent_ticket_text(&recent_ticket_row(ticket, true, 3.0, false));
    let release = text.lines[1]
        .spans
        .iter()
        .find(|span| span.content == "1.4.0")
        .unwrap();

    assert_eq!(release.style.fg, Some(tuicore::theme().accent_fg()));
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

#[test]
fn configured_jira_urls_filter_recent_tickets_by_issue_key() {
    let service = AppService::for_tests();
    service.settings().write().unwrap().jira_base_url = "https://finery.atlassian.net".into();
    let mut menu = super::RecentTicketsMenu::new(service);
    *menu.query.borrow_mut() = Some("https://finery.atlassian.net/browse/DPP-5263".into());

    assert!(menu.sync_query());
    assert_eq!(menu.input.current_value(), "DPP-5263");
}

#[test]
fn yp_copies_the_highlighted_ticket_prepare_reference_with_quoted_title() {
    use tuicore::{HotkeyEvent, TuiEvent, TuiNode};

    let mut menu = super::RecentTicketsMenu::new(AppService::for_tests());
    let mut ticket = work_item("Story");
    ticket.key = "KAN-1234".into();
    ticket.title = "Fix \"quoted\" C:\\path".into();
    menu.list
        .set_rows(vec![recent_ticket_row(ticket, true, 3.0, false)]);
    menu.list.set_highlighted_id(&"KAN-1234".into());
    let mut ctx = EventCtx::default();

    menu.event(
        &TuiEvent::Hotkey(HotkeyEvent::Commit("yp".into())),
        &mut ctx,
    );

    assert_eq!(
        ctx.clipboard_request(),
        Some(r#"finery prepare KAN-1234 "Fix \"quoted\" C:\\path""#)
    );
}

fn work_item(kind: &str) -> WorkItem {
    WorkItem {
        key: "FIN-1".into(),
        title: "Example".into(),
        description: String::new(),
        kind: kind.into(),
        status: "To Do".into(),
        done: false,
        priority: String::new(),
        assignee: "Unassigned".into(),
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
fn open_command_closes_recent_tickets_only_when_a_command_is_triggered() {
    use tuicore::{EventRoute, Key, KeyEvent, KeyModifiers, TreePath, TuiEvent, TuiNode};

    let service = AppService::for_tests();
    let probe = crate::service::OpenCommandProbe::new(&service);
    let mut menu = super::RecentTicketsMenu::new(service);
    menu.list.set_rows(vec![recent_ticket_row(
        work_item("Story"),
        true,
        3.0,
        false,
    )]);
    menu.list.set_highlighted_id(&"FIN-1".into());
    let mut ctx = EventCtx::default();
    menu.dispatch_event(
        &EventRoute::new(TreePath::default()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char(';'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );
    probe.assert_opened("FIN-1", "Example");
    assert_eq!(ctx.propagation(), tuicore::Propagation::Stopped);
    assert!(matches!(
        menu.take_events().as_slice(),
        [super::RecentTicketsMenuEvent::Closed]
    ));
    assert!(menu.input.current_value().is_empty());

    menu.service
        .settings()
        .write()
        .unwrap()
        .open_command
        .clear();
    menu.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char(';'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    assert!(menu.take_events().is_empty());
}
