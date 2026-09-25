use super::super::App;
use super::*;
use tuicore::{EventRoute, FocusCtx, FocusTarget, HotkeyEvent, LayoutCtx};

const AREA: Rect = Rect::new(0, 0, 120, 42);

fn layout(app: &mut App) -> LayoutCtx {
    let mut ctx = LayoutCtx::new();
    ctx.with_overlay_bounds(AREA, |ctx| app.layout(AREA, ctx));
    ctx
}

fn target(app: &mut App, slot: &str, id: &str) -> FocusTarget {
    layout(app)
        .focus_targets()
        .iter()
        .find(|target| {
            target.enabled
                && target.id.as_str() == id
                && target.path.keys().iter().any(|key| key.as_str() == slot)
        })
        .unwrap_or_else(|| panic!("missing {slot}/{id}"))
        .clone()
}

fn send(app: &mut App, target: &FocusTarget, event: TuiEvent) {
    app.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &event,
        &mut EventCtx::default(),
    );
    layout(app);
}

fn key(app: &mut App, target: &FocusTarget, code: Key, modifiers: KeyModifiers) {
    send(app, target, TuiEvent::Key(KeyEvent { code, modifiers }));
}

fn open_manager(app: &mut App) {
    let filter = target(app, "saved-filter", "field");
    app.dispatch_focus(&filter, true, &mut FocusCtx::default());
    key(app, &filter, Key::Enter, KeyModifiers::NONE);
    key(app, &filter, Key::Char('j'), KeyModifiers::CONTROL);
    key(app, &filter, Key::Enter, KeyModifiers::NONE);
}

fn new_filter(app: &mut App) -> FocusTarget {
    let new = target(app, "new", "button");
    send(
        app,
        &new,
        TuiEvent::Hotkey(HotkeyEvent::Commit("shift+n".into())),
    );
    let name = target(app, "name", "input");
    app.dispatch_focus(&name, true, &mut FocusCtx::default());
    name
}

fn named_filter_manager() -> (App, AppService) {
    tuicore::init();
    let service = AppService::for_tests();
    let mut app = root(service.clone(), Vec::new());
    open_manager(&mut app);
    let name = new_filter(&mut app);
    send(&mut app, &name, TuiEvent::Paste("Triage".into()));
    key(&mut app, &name, Key::Enter, KeyModifiers::CONTROL);
    open_manager(&mut app);
    (app, service)
}

fn assert_closed(app: &mut App) {
    assert!(!layout(app).focus_targets().iter().any(|target| {
        target
            .path
            .keys()
            .iter()
            .any(|key| key.as_str() == "selector")
    }));
}

#[test]
fn manager_name_accepts_capital_h_and_ctrl_enter_saves_and_closes() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut app = root(service.clone(), Vec::new());
    open_manager(&mut app);
    let name = new_filter(&mut app);
    key(&mut app, &name, Key::Char('H'), KeyModifiers::SHIFT);
    assert!(target(&mut app, "name", "input").suppress_global_hotkeys);
    key(&mut app, &name, Key::Enter, KeyModifiers::CONTROL);
    let settings = service.settings();
    let settings = settings.read().unwrap();
    assert_eq!(settings.saved_backlog_filters.len(), 1);
    assert_eq!(settings.saved_backlog_filters[0].name, "H");
    assert_closed(&mut app);
}

#[test]
fn manager_selector_search_accepts_capital_h_and_ctrl_enter_closes() {
    tuicore::init();
    let mut app = root(AppService::for_tests(), Vec::new());
    open_manager(&mut app);
    let selector = target(&mut app, "selector", "field");
    app.dispatch_focus(&selector, true, &mut FocusCtx::default());
    key(&mut app, &selector, Key::Enter, KeyModifiers::NONE);
    let search = target(&mut app, "selector", "input");
    app.dispatch_focus(&search, true, &mut FocusCtx::default());
    key(&mut app, &search, Key::Char('H'), KeyModifiers::SHIFT);
    assert!(target(&mut app, "selector", "input").suppress_global_hotkeys);
    key(&mut app, &search, Key::Enter, KeyModifiers::CONTROL);
    assert_closed(&mut app);
}

#[test]
fn manager_list_search_accepts_capital_h_and_ctrl_enter_closes() {
    let (mut app, service) = named_filter_manager();
    let users = target(&mut app, "users", "data-view");
    app.dispatch_focus(&users, true, &mut FocusCtx::default());
    key(&mut app, &users, Key::Char('/'), KeyModifiers::NONE);
    let search = target(&mut app, "users", "input");
    app.dispatch_focus(&search, true, &mut FocusCtx::default());
    key(&mut app, &search, Key::Char('H'), KeyModifiers::SHIFT);
    assert!(target(&mut app, "users", "input").suppress_global_hotkeys);
    key(&mut app, &search, Key::Enter, KeyModifiers::CONTROL);
    assert_closed(&mut app);
    assert_eq!(
        service.settings().read().unwrap().saved_backlog_filters[0].name,
        "Triage"
    );
}

#[test]
fn manager_ctrl_enter_commits_an_open_value_picker_before_closing() {
    let (mut app, service) = named_filter_manager();
    let epics = target(&mut app, "epics", "data-view");
    app.dispatch_focus(&epics, true, &mut FocusCtx::default());
    key(&mut app, &epics, Key::Char('+'), KeyModifiers::NONE);
    let picker = target(&mut app, "add-input", "input");
    app.dispatch_focus(&picker, true, &mut FocusCtx::default());
    key(&mut app, &picker, Key::Enter, KeyModifiers::CONTROL);
    assert_closed(&mut app);
    assert_eq!(
        service.settings().read().unwrap().saved_backlog_filters[0]
            .criteria
            .epics,
        [""]
    );
}

#[test]
fn manager_ctrl_enter_closes_from_controls_without_activating_them() {
    let (mut app, service) = named_filter_manager();
    for (slot, id) in [
        ("new", "button"),
        ("delete", "button"),
        ("name", "input"),
        ("epics", "data-view"),
    ] {
        let control = target(&mut app, slot, id);
        app.dispatch_focus(&control, true, &mut FocusCtx::default());
        key(&mut app, &control, Key::Enter, KeyModifiers::CONTROL);
        assert_closed(&mut app);
        assert_eq!(
            service
                .settings()
                .read()
                .unwrap()
                .saved_backlog_filters
                .len(),
            1
        );
        open_manager(&mut app);
    }
}

#[test]
fn manager_done_shortcut_is_configurable_and_an_unnamed_draft_stays_open() {
    tuicore::init();
    let service = AppService::for_tests();
    service.settings().write().unwrap().backlog_keys =
        crate::app_settings::AppSettings::resolve(&std::collections::HashMap::from([(
            "backlog.saved_filter_done_key".into(),
            "ctrl+s".into(),
        )]))
        .unwrap()
        .backlog_keys;
    let mut app = root(service.clone(), Vec::new());
    open_manager(&mut app);
    let name = new_filter(&mut app);
    key(&mut app, &name, Key::Char('s'), KeyModifiers::CONTROL);
    let name = target(&mut app, "name", "input");
    app.dispatch_focus(&name, true, &mut FocusCtx::default());
    assert!(
        service
            .settings()
            .read()
            .unwrap()
            .saved_backlog_filters
            .is_empty()
    );
    send(&mut app, &name, TuiEvent::Paste("Named".into()));
    key(&mut app, &name, Key::Char('s'), KeyModifiers::CONTROL);
    assert_closed(&mut app);
    assert_eq!(
        service.settings().read().unwrap().saved_backlog_filters[0].name,
        "Named"
    );
}
