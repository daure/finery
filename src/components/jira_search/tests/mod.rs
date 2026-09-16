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
