use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    EventCtx, EventRoute, ExternalEditorResponse, FocusCtx, FocusId, FocusRequest, HotkeyEvent,
    Key, KeyEvent, KeyModifiers, LayoutCtx, RenderCtx, TuiEvent, TuiNode,
};

use super::page::ComposerPage;
use crate::{service::AppService, store::composer::ComposerState};

fn composer_page() -> ComposerPage {
    let service = AppService::for_tests();
    let settings = service.settings();
    ComposerPage::new(ComposerState::demo().change_sets, service, settings)
}

fn render_text(page: &mut ComposerPage) -> String {
    let area = Rect::new(0, 0, 120, 40);
    page.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect()
}

fn open_change_set(page: &mut ComposerPage, navigate_down: usize) -> EventCtx<()> {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, 120, 40), &mut layout);
    let target = layout.focus_targets().first().unwrap().clone();
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    for _ in 0..navigate_down {
        page.dispatch_event(
            &EventRoute::new(target.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Down)),
            &mut EventCtx::default(),
        );
    }
    let mut ctx = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut ctx,
    );
    focus(page, "data-view");
    ctx
}

fn focus(page: &mut ComposerPage, id: &str) -> tuicore::FocusTarget {
    let target = target(page, id);
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    target
}

fn target(page: &mut ComposerPage, id: &str) -> tuicore::FocusTarget {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, 120, 40), &mut layout);
    layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new(id))
        .unwrap()
        .clone()
}

#[test]
fn composer_replaces_change_set_list_with_breadcrumb_and_ticket_detail() {
    tuicore::init();
    let mut page = composer_page();
    let change_sets = render_text(&mut page);
    assert!(change_sets.contains("Checkout reliability"));
    assert!(!change_sets.contains("+ new · - delete · Enter open"));

    let open_ctx = open_change_set(&mut page, 0);
    assert_eq!(
        open_ctx.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );
    let text = render_text(&mut page);

    assert!(text.contains("Change sets > Checkout reliability"));
    assert!(text.contains("Keep checkout state across retries"));
    assert!(text.contains("Description"));
    assert!(text.contains("Properties"));
    assert!(text.contains("Description |D|"));
    assert!(text.contains("Properties |P|"));
    assert_eq!(text.matches("dd·do·ds").count(), 1);
    assert!(!text.contains("Jira description · Markdown"));

    let title_hotkey = target(&mut page, "input");
    assert_eq!(title_hotkey.hotkey_sequences, vec!["shift+t"]);
    let tabs_hotkeys = target(&mut page, "tabs");
    assert_eq!(
        tabs_hotkeys.hotkey_sequences,
        vec!["shift+d", "shift+p", "dd", "do", "ds"]
    );
    let description_hotkeys = target(&mut page, "textarea");
    assert!(description_hotkeys.hotkey_sequences.is_empty());

    let tickets = focus(&mut page, "data-view");
    page.dispatch_focus(&tickets, false, &mut FocusCtx::default());
    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(title.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('!'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Keep checkout state across retries!"));
    let mut title_exit = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut title_exit,
    );
    assert_eq!(
        title_exit.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );

    let title = focus(&mut page, "input");
    page.dispatch_focus(&title, false, &mut FocusCtx::default());
    let tabs = target(&mut page, "tabs");

    let mut editor = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(tabs.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("do".into())),
        &mut editor,
    );
    assert!(editor.external_editor_request().is_some());

    page.dispatch_event(
        &EventRoute::new(tabs.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("ds".into())),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("600 WPM"));
    let reader = focus(&mut page, "speed-reader");
    assert_eq!(reader.area.x + reader.area.width / 2, 60);
    assert_eq!(reader.area.y + reader.area.height / 2, 20);
    page.dispatch_event(
        &EventRoute::new(reader.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("600 WPM"));

    let tabs = target(&mut page, "tabs");
    page.dispatch_event(
        &EventRoute::new(tabs.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("dd".into())),
        &mut EventCtx::default(),
    );
    render_text(&mut page);
    let description = focus(&mut page, "textarea");
    page.dispatch_event(
        &EventRoute::new(description.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('!'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("order.!"));
    page.dispatch_focus(&description, false, &mut FocusCtx::default());

    let tabs = target(&mut page, "tabs");
    page.dispatch_event(
        &EventRoute::new(tabs.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("do".into())),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(tabs.path),
        &TuiEvent::ExternalEditor(ExternalEditorResponse {
            value: "## Editor result".into(),
            line: 1,
            col: 1,
        }),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Editor result"));

    for sequence in ["dd", "do", "ds"] {
        let tabs = focus(&mut page, "tabs");
        page.dispatch_event(
            &EventRoute::new(tabs.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(']'))),
            &mut EventCtx::default(),
        );
        assert!(render_text(&mut page).contains("Issue type"));
        let mut action = EventCtx::default();
        page.dispatch_event(
            &EventRoute::new(tabs.path),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(sequence.into())),
            &mut action,
        );
        assert_eq!(
            action.focus_request(),
            Some(&FocusRequest::Target(FocusId::new("textarea")))
        );
        assert!(!render_text(&mut page).contains("Issue type"));
    }

    let description = focus(&mut page, "textarea");
    page.dispatch_event(
        &EventRoute::new(description.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char(']'))),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Issue type"));
    page.dispatch_event(
        &EventRoute::new(description.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('['))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Issue type"));
    let mut description_exit = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(description.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut description_exit,
    );
    assert_eq!(
        description_exit.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );
    let description = focus(&mut page, "textarea");
    page.dispatch_focus(&description, false, &mut FocusCtx::default());

    let target = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('-'))),
        &mut EventCtx::default(),
    );
    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Mark for deletion (d)"));
    assert!(dialog_text.contains("Remove from change set (r)"));

    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    open_change_set(&mut page, 1);
    assert!(render_text(&mut page).contains("No issue selected"));

    page.create_ticket("New checkout note");
    let target = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('-'))),
        &mut EventCtx::default(),
    );
    let new_ticket_dialog = render_text(&mut page);
    assert!(new_ticket_dialog.contains("Remove from change set (r)"));
    assert!(!new_ticket_dialog.contains("Mark for deletion (d)"));

    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    let target = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('+'))),
        &mut EventCtx::default(),
    );
    let add_menu = render_text(&mut page);
    assert!(add_menu.contains("Add new"));
    assert!(add_menu.contains("Add existing"));
    assert!(!add_menu.contains("+ Add ticket"));
    assert!(!add_menu.contains("Select..."));
}

#[test]
fn deleted_ticket_hotkeys_and_escape_return_to_change_sets() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 0);

    page.mark_selected_deleted();
    let description = target(&mut page, "textarea");
    assert!(description.hotkey_sequences.is_empty());
    let tabs = target(&mut page, "tabs");
    assert_eq!(tabs.hotkey_sequences, vec!["shift+d", "shift+p", "ds"]);

    let tabs = target(&mut page, "tabs");
    page.dispatch_event(
        &EventRoute::new(tabs.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("ds".into())),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("600 WPM"));
    let reader = target(&mut page, "speed-reader");
    page.dispatch_event(
        &EventRoute::new(reader.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    focus(&mut page, "data-view");

    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );

    let text = render_text(&mut page);
    assert!(text.contains("Customer notifications"));
    assert!(!text.contains("Change sets > Checkout reliability"));
}
