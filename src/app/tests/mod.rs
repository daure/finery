mod saved_filter_keys;

use std::time::Duration;

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    AnimationSettings, ChildKey, EventCtx, FocusManager, FocusRequest, Key, KeyEvent, KeyModifiers,
    Propagation, RenderCtx, TreePath, TuiEvent, TuiNode,
};

use crate::{service::AppService, store::composer::ComposerState};

use super::root;

#[test]
fn background_notifications_render_on_the_next_tick() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut app = root(service.clone(), Vec::new());
    service.report_notification(tuicore::Notification::success(
        "Refresh complete",
        "1 ticket refreshed",
    ));

    app.tick(
        Duration::ZERO,
        AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        },
    );
    let area = Rect::new(0, 0, 96, 30);
    app.layout(area, &mut tuicore::LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            app.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect();

    assert!(text.contains("Refresh complete"));
}

#[test]
fn background_copy_schedules_ticks_and_a_direct_copy_supersedes_it() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut app = root(service.clone(), Vec::new());
    let (release, gate) = std::sync::mpsc::channel();
    service.copy_in_background(move || {
        gate.recv_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        Ok("Sprint report".into())
    });
    let mut ctx = tuicore::EventCtx::new(AnimationSettings::default());
    app.apply_dialog_signals(&mut ctx);
    assert!(ctx.tick_requested());
    assert!(ctx.notifications().is_empty());
    assert!(
        app.tick(Duration::ZERO, AnimationSettings::default())
            .next_tick
            .is_some()
    );
    assert_eq!(app.take_pending_clipboard_request(), None);

    ctx.copy_to_clipboard("A later direct copy");
    app.apply_dialog_signals(&mut ctx);
    assert!(!service.clipboard_pending());
    release.send(()).unwrap();
    assert_eq!(app.take_pending_clipboard_request(), None);
}

#[test]
fn configured_command_values_activate_the_global_open_command_menu() {
    tuicore::init();
    let service = AppService::for_tests();
    let _probe = crate::service::OpenCommandProbe::new(&service);
    service.settings().write().unwrap().open_command_enum = vec!["editor".into()];
    let mut app = root(service.clone(), Vec::new());
    app.layout(Rect::new(0, 0, 96, 30), &mut tuicore::LayoutCtx::new());

    assert!(service.open_command("FIN-42", "Ticket title"));
    let mut ctx = EventCtx::default();
    app.apply_dialog_signals(&mut ctx);
    let mut layout = tuicore::LayoutCtx::new();
    app.layout(Rect::new(0, 0, 96, 30), &mut layout);
    let mut focus = FocusManager::new();
    focus.apply_request(ctx.focus_request().unwrap(), layout.focus_targets());

    assert!(app.view.is_active());
    assert_eq!(focus.current().unwrap().id, tuicore::FocusId::new("input"));
}

#[test]
fn home_shortcut_closes_global_dialogs() {
    tuicore::init();
    let mut app = root(AppService::for_tests(), Vec::new());
    app.view.set_active(true);
    app.view.base_mut().set_active(true);
    app.view.base_mut().base_mut().set_active(true);
    app.view.base_mut().base_mut().base_mut().set_active(true);
    let mut ctx = EventCtx::new(AnimationSettings::default());

    assert!(app.go_home(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('h'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    ));

    assert!(!app.view.is_active());
    assert!(!app.view.base().is_active());
    assert!(!app.view.base().base().is_active());
    assert!(!app.view.base().base().base().is_active());
    assert_eq!(ctx.propagation(), Propagation::Stopped);
}

#[test]
fn home_shortcut_resets_composer_to_its_overview() {
    tuicore::init();
    let mut app = root(AppService::for_tests(), ComposerState::demo().change_sets);
    let area = Rect::new(0, 0, 96, 30);
    app.selected_page.set(Some(1));
    let mut layout = tuicore::LayoutCtx::new();
    app.layout(area, &mut layout);
    let tickets = layout
        .focus_targets()
        .iter()
        .find(|target| target.id == tuicore::FocusId::new("data-view"))
        .unwrap()
        .clone();
    app.dispatch_focus(&tickets, true, &mut tuicore::FocusCtx::default());
    app.dispatch_event(
        &tuicore::EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    let mut ctx = EventCtx::default();
    assert!(app.go_home(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('h'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    ));

    app.layout(area, &mut tuicore::LayoutCtx::new());
    let tabs = tuicore::EventRoute::new(TreePath::from_keys([
        ChildKey::first(),
        ChildKey::first(),
        ChildKey::first(),
        ChildKey::first(),
        ChildKey::new("pages"),
    ]));
    let mut select_composer = EventCtx::default();
    app.view.dispatch_event(
        &tabs,
        &TuiEvent::Key(KeyEvent::from(Key::Char(']'))),
        &mut select_composer,
    );
    assert!(matches!(
        select_composer.focus_request(),
        Some(FocusRequest::FirstChildOf { .. })
    ));

    app.selected_page.set(Some(1));
    app.layout(area, &mut tuicore::LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            app.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect();

    assert!(text.contains("New change set"));
}
