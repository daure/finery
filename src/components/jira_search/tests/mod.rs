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
