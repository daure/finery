use super::*;

fn overview() -> (ComposerPage, AppService) {
    let service = AppService::for_tests();
    let set = ComposerState::demo().change_sets.remove(0);
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let mut page = ComposerPage::new(vec![set], service.clone(), service.settings());
    page.init(&mut LifecycleCtx::default());
    (page, service)
}

fn open_menu(page: &mut ComposerPage, event: TuiEvent) {
    let list = focus(page, "data-view");
    assert_eq!(
        page.dispatch_event(
            &EventRoute::new(list.path),
            &event,
            &mut EventCtx::default()
        ),
        EventOutcome::Handled
    );
}

fn popup_key(page: &mut ComposerPage, key: KeyEvent) {
    let area = Rect::new(0, 0, TEST_WIDTH, 40);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| page.layout(area, ctx));
    let path = layout.overlays().last().unwrap().route_path.clone();
    page.dispatch_event(
        &EventRoute::new(path),
        &TuiEvent::Key(key),
        &mut EventCtx::default(),
    );
}

fn show_archived_change_sets(page: &mut ComposerPage) {
    let target = filter_button(page);
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+f".into())),
        &mut EventCtx::default(),
    );
    for key in "Archived".chars().map(Key::Char).chain([Key::Enter]) {
        popup_key(page, KeyEvent::from(key));
    }
}

#[test]
fn dot_menu_matches_backlog_sizing_and_archive_shortcut_opens_confirmation() {
    tuicore::init();
    let (mut page, service) = overview();
    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    let text = render_text(&mut page);
    assert!(text.contains("Delete"));
    assert!(text.contains("Archive"));
    assert!(text.contains(&ComposerKeyBindings::default().delete_change_set.label()));
    assert!(text.contains(&ComposerKeyBindings::default().archive.label()));
    let mut layout = LayoutCtx::new();
    let area = Rect::new(0, 0, TEST_WIDTH, 40);
    layout.with_overlay_bounds(area, |ctx| page.layout(area, ctx));
    let host = layout
        .overlays()
        .iter()
        .find(|entry| entry.layer == tuicore::OverlayLayer::Modal)
        .unwrap();
    assert_eq!((host.area.width, host.area.height), (69, 18));
    let popup = layout.overlays().last().unwrap();
    assert_eq!(popup.anchor.width, 54);
    assert!(popup.area.height <= 16);
    popup_key(&mut page, KeyEvent::from(Key::Char('a')));
    assert!(render_text(&mut page).contains("Complete change set?"));
    assert!(!service.change_set_for_tests("CS-1").unwrap().closed);
    popup_key(&mut page, KeyEvent::from(Key::Char('r')));
    service.flush().unwrap();
    assert_eq!(
        service
            .change_set_for_tests("CS-1")
            .unwrap()
            .archive_outcome,
        Some(crate::store::composer::ArchiveOutcome::Cancelled)
    );
}

#[test]
fn dot_menu_opens_the_prefilled_rename_dialog() {
    tuicore::init();
    let (mut page, service) = overview();
    let name = service.change_set_for_tests("CS-1").unwrap().name;

    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    let menu = render_text(&mut page);
    assert!(menu.contains("Rename"));
    assert!(menu.contains(&ComposerKeyBindings::default().rename_change_set.label()));
    popup_key(&mut page, KeyEvent::from(Key::Char('r')));
    let dialog = render_text(&mut page);
    assert!(dialog.contains("Rename change set"));
    assert!(dialog.contains(&name));
}

#[test]
fn overview_shortcuts_open_rename_and_clone_dialogs() {
    tuicore::init();
    let (mut page, _) = overview();
    let list = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(list.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Rename change set"));

    let service = AppService::for_tests();
    let mut set = ComposerState::demo().change_sets.remove(0);
    set.closed = true;
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let mut page = ComposerPage::new(vec![set], service.clone(), service.settings());
    page.init(&mut LifecycleCtx::default());
    show_archived_change_sets(&mut page);
    let list = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(list.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('c'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Clone change set"));
}

#[test]
fn delete_menu_action_uses_confirmation_and_escape_dismisses_the_menu() {
    tuicore::init();
    let (mut page, service) = overview();
    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    popup_key(&mut page, KeyEvent::from(Key::Esc));
    assert!(!render_text(&mut page).contains("Archive"));
    assert!(service.change_set_for_tests("CS-1").is_some());
    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    popup_key(&mut page, KeyEvent::from(Key::Char('x')));
    assert!(render_text(&mut page).contains("Delete change set?"));
    assert!(service.change_set_for_tests("CS-1").is_some());
    popup_key(&mut page, KeyEvent::from(Key::Char('c')));
    assert!(service.change_set_for_tests("CS-1").is_some());
    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    for key in "delete".chars() {
        popup_key(&mut page, KeyEvent::from(Key::Char(key)));
    }
    popup_key(&mut page, KeyEvent::from(Key::Enter));
    assert!(render_text(&mut page).contains("Delete change set?"));
    popup_key(&mut page, KeyEvent::from(Key::Char('d')));
    service.flush().unwrap();
    assert!(service.change_set_for_tests("CS-1").is_none());
}

#[test]
fn configured_menu_shortcuts_are_displayed_and_trigger_actions() {
    tuicore::init();
    let service = AppService::for_tests();
    let values = std::collections::HashMap::from([
        ("composer.change_set_actions_key".into(), ",".into()),
        ("composer.rename_change_set_key".into(), "ctrl+r".into()),
        ("composer.clone_change_set_key".into(), "ctrl+o".into()),
        ("composer.delete_change_set_key".into(), "ctrl+d".into()),
        ("composer.archive_key".into(), "ctrl+e".into()),
    ]);
    let settings = AppSettings::resolve(&values).unwrap();
    let persisted = settings
        .values()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    assert_eq!(
        AppSettings::resolve(&persisted).unwrap().composer_keys,
        settings.composer_keys
    );
    *service.settings().write().unwrap() = settings;
    let mut page = ComposerPage::new(
        ComposerState::demo().change_sets,
        service.clone(),
        service.settings(),
    );
    page.init(&mut LifecycleCtx::default());
    open_menu(&mut page, TuiEvent::Hotkey(HotkeyEvent::Commit(",".into())));
    let text = render_text(&mut page);
    let keys = service.settings().read().unwrap().composer_keys.clone();
    assert!(text.contains(&keys.rename_change_set.label()));
    assert!(text.contains(&keys.delete_change_set.label()));
    assert!(text.contains(&keys.archive.label()));
    popup_key(
        &mut page,
        KeyEvent {
            code: Key::Char('e'),
            modifiers: KeyModifiers::CONTROL,
        },
    );
    assert!(render_text(&mut page).contains("Complete change set?"));
}

#[test]
fn archived_change_set_menu_opens_a_prefilled_clone_dialog() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut set = ComposerState::demo().change_sets.remove(0);
    set.closed = true;
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let mut page = ComposerPage::new(vec![set], service.clone(), service.settings());
    page.init(&mut LifecycleCtx::default());
    show_archived_change_sets(&mut page);

    open_menu(&mut page, TuiEvent::Key(KeyEvent::from(Key::Char('.'))));
    let menu = render_text(&mut page);
    assert!(menu.contains("Clone"));
    assert!(menu.contains(&ComposerKeyBindings::default().clone_change_set.label()));
    assert!(menu.contains("Rename"));
    assert!(menu.contains("Delete"));
    popup_key(&mut page, KeyEvent::from(Key::Char('c')));

    let dialog = render_text(&mut page);
    assert!(dialog.contains("Clone change set"));
    assert!(dialog.contains("Clone of Checkout reliability"));

    let title = last_target(&mut page, "input");
    page.dispatch_focus(&title, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('!'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Clone of Checkout reliability!"));
}
