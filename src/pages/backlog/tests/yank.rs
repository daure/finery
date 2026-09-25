use super::*;

#[test]
fn backlog_yank_uses_the_configured_url_for_full_and_slack() {
    tuicore::init();
    let service = AppService::for_tests();
    service.settings().write().unwrap().jira_base_url = "https://jira.example/".into();
    let mut page = BacklogPage::with_snapshot_and_service_for_test(snapshot(), service.clone());
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, 160, 30), &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new("data-view"))
        .unwrap();
    page.dispatch_focus(target, true, &mut FocusCtx::default());
    page.view_for_test()
        .base_mut()
        .base_mut()
        .clear_selection_and_highlight_ticket("FIN-8");
    let route = EventRoute::new(target.path.clone());
    for (shortcut, expected) in [
        ('u', "https://jira.example/browse/FIN-8"),
        ('f', "https://jira.example/browse/FIN-8 - Plan next sprint"),
        (
            's',
            ":ticket: https://jira.example/browse/FIN-8 - Plan next sprint",
        ),
    ] {
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(shortcut))),
            &mut ctx,
        );
        assert_eq!(ctx.clipboard_request(), Some(expected));
    }

    service.settings().write().unwrap().jira_base_url.clear();
    for shortcut in ['f', 's'] {
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(shortcut))),
            &mut ctx,
        );
        assert_eq!(ctx.clipboard_request(), None);
    }
}
