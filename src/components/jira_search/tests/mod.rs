use tuicore::{EventCtx, FocusId, FocusRequest};

use crate::service::AppService;

use super::JiraSearchMenu;

#[test]
fn remote_queries_do_not_enable_list_search_highlighting() {
    let mut menu = JiraSearchMenu::new(AppService::for_tests());
    *menu.query.borrow_mut() = Some("fin".into());

    assert!(menu.sync_query());
    assert!(menu.list.data_view().transform_state().search.is_empty());
}

#[test]
fn opening_requests_focus_for_the_search_input() {
    let mut menu = JiraSearchMenu::new(AppService::for_tests());
    let mut ctx = EventCtx::default();

    menu.open(&mut ctx);

    assert!(matches!(
        ctx.focus_request(),
        Some(FocusRequest::Target(id)) if id == &FocusId::new("input")
    ));
}

#[test]
fn configured_jira_urls_search_by_issue_key() {
    let service = AppService::for_tests();
    service.settings().write().unwrap().jira_base_url = "https://finery.atlassian.net".into();
    let mut menu = JiraSearchMenu::new(service);
    *menu.query.borrow_mut() = Some("https://finery.atlassian.net/browse/DPP-5263".into());

    assert!(menu.sync_query());
    assert_eq!(menu.last_query, "DPP-5263");
    assert_eq!(menu.input.current_value(), "DPP-5263");
}

#[test]
fn open_command_closes_search_only_when_a_command_is_triggered() {
    use crate::store::work_items::WorkItem;
    use tuicore::{EventRoute, Key, KeyEvent, KeyModifiers, TreePath, TuiEvent, TuiNode};

    let service = AppService::for_tests();
    let probe = crate::service::OpenCommandProbe::new(&service);
    let mut menu = JiraSearchMenu::new(service);
    let ticket = WorkItem {
        key: "FIN-42".into(),
        title: "Search result".into(),
        description: String::new(),
        kind: "Story".into(),
        status: "To Do".into(),
        done: false,
        priority: String::new(),
        assignee: String::new(),
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
    };
    menu.list.set_rows(vec![super::jira_search_row(
        ticket,
        true,
        3.0,
        String::new(),
        false,
    )]);
    menu.list.set_highlighted_id(&"FIN-42".into());
    let mut ctx = EventCtx::default();
    menu.dispatch_event(
        &EventRoute::new(TreePath::default()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char(';'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );
    probe.assert_opened("FIN-42");
    assert_eq!(ctx.propagation(), tuicore::Propagation::Stopped);
    assert!(matches!(
        menu.take_events().as_slice(),
        [super::JiraSearchMenuEvent::Closed]
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
