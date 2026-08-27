use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventRoute, ExternalEditorResponse, FocusCtx, FocusId,
    FocusRequest, HotkeyEvent, Key, KeyEvent, KeyModifiers, LayoutCtx, RenderCtx,
    TabsBodyBorderStyle, TuiEvent, TuiNode,
};

use super::page::ComposerPage;
use super::property_fields::BoundPropertyDropdown;
use super::source::SourceController;
use super::submission::SubmissionController;
use crate::{
    jira::JiraOption,
    service::AppService,
    store::composer::{
        ComposerAction, ComposerState, ComposerViewMode, PlacementTarget, TicketKind,
    },
};

use super::title_guidance::{TitleLevel, evaluate_title, format_title};

const TEST_WIDTH: u16 = 96;

fn composer_page() -> ComposerPage {
    let service = AppService::for_tests();
    let settings = service.settings();
    ComposerPage::new(ComposerState::demo().change_sets, service, settings)
}

fn render_text(page: &mut ComposerPage) -> String {
    render_text_at(page, TEST_WIDTH)
}

fn render_text_at(page: &mut ComposerPage, width: u16) -> String {
    let area = Rect::new(0, 0, width, 40);
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

fn open_change_set(page: &mut ComposerPage, index: usize) -> EventCtx<()> {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    let target = layout.focus_targets().first().unwrap().clone();
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Home)),
        &mut EventCtx::default(),
    );
    for _ in 0..index {
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
    std::thread::sleep(Duration::from_millis(20));
    page.tick(Duration::from_millis(100), AnimationSettings::default());
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
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new(id))
        .unwrap()
        .clone()
}

fn last_target(page: &mut ComposerPage, id: &str) -> tuicore::FocusTarget {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    layout
        .focus_targets()
        .iter()
        .rev()
        .find(|target| target.id == FocusId::new(id))
        .unwrap()
        .clone()
}

fn open_new_ticket(page: &mut ComposerPage) {
    page.open_new_ticket(&mut EventCtx::default());
}

#[test]
fn composer_replaces_change_set_list_with_breadcrumb_and_ticket_detail() {
    tuicore::init();
    let mut page = composer_page();
    let change_sets = render_text(&mut page);
    assert!(change_sets.contains("Checkout reliability"));
    assert!(!change_sets.contains("+ new · - delete · Enter open"));

    open_change_set(&mut page, 1);
    let text = render_text(&mut page);

    assert!(text.contains("Change sets > Checkout reliability"));
    assert!(text.contains("Add sibling"));
    assert!(text.contains("Add child"));
    assert!(text.contains("Commit"));
    assert!(text.contains("Refresh"));
    assert!(text.contains("Changes"));
    assert!(text.contains("󰄱"));
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

    let source = page.selected_changes();
    page.set_selected_source(source);
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
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
    assert_eq!(reader.area.x + reader.area.width / 2, TEST_WIDTH / 2);
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

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Delete (d)"));
    assert!(dialog_text.contains("Remove (r)"));
    assert!(dialog_text.contains("Cancel (c)"));

    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Ticket action"));
    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    open_change_set(&mut page, 0);
    assert!(render_text(&mut page).contains("No issue selected"));

    page.create_ticket("New checkout note");
    let target = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let new_ticket_dialog = render_text(&mut page);
    assert!(new_ticket_dialog.contains("Remove (r)"));
    assert!(new_ticket_dialog.contains("Cancel (c)"));
    assert!(!new_ticket_dialog.contains("Delete (d)"));

    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
}

#[test]
fn new_ticket_dialog_shows_title_guidance() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    open_new_ticket(&mut page);

    let text = render_text(&mut page);
    assert!(text.contains("Create ticket"));
    assert!(text.contains("Issue type"));
    assert!(text.contains("Story"));
    assert!(text.contains("Starts with a verb"));
    assert!(text.contains("No second action detected"));
    assert!(text.contains("3-8 words for quick scanning"));
    assert!(text.contains("OK"));
    assert!(text.contains("Cancel"));
}

#[test]
fn ticket_action_dialogs_match_the_selected_change_kind() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    let source = page.selected_changes();
    page.set_selected_source(source);
    page.mark_selected_deleted();

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let delete_dialog = render_text(&mut page);
    assert!(!delete_dialog.contains("Delete (d)"));
    assert!(delete_dialog.contains("Remove (r)"));
    assert!(delete_dialog.contains("Cancel (c)"));

    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
        &mut EventCtx::default(),
    );

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let restore_dialog = render_text(&mut page);
    assert!(restore_dialog.contains("Restore (r)"));
    assert!(restore_dialog.contains("Cancel (c)"));
    assert!(!restore_dialog.contains("Reset (s)"));

    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    page.update_selected_kind(TicketKind::Task);

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let reset_dialog = render_text(&mut page);
    assert!(reset_dialog.contains("Reset (s)"));
    assert!(reset_dialog.contains("Cancel (c)"));
    assert!(!reset_dialog.contains("Restore (r)"));
}

#[test]
fn ticket_action_hotkeys_open_from_other_composer_controls() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let no_action_dialog = render_text(&mut page);
    assert!(no_action_dialog.contains("No restore or reset action is available for this ticket."));
    assert!(no_action_dialog.contains("Cancel (c)"));

    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('c'))),
        &mut EventCtx::default(),
    );

    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Ticket action"));
    assert!(dialog_text.contains("Delete (d)"));

    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
        &mut EventCtx::default(),
    );
    page.update_selected_kind(TicketKind::Task);

    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Reset (s)"));
}

#[test]
fn submit_requires_confirmation() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let source = page.selected_changes();
    page.set_selected_source(source);

    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );

    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Commit changes"));
    assert!(dialog_text.contains("Commit (s)"));
    assert!(dialog_text.contains("Cancel (c)"));
}

#[test]
fn submit_confirmation_lists_local_parent_dependencies() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 0);
    page.create_ticket("Parent task");
    page.create_child_ticket("Child sub-task");

    let title = focus(&mut page, "input");
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );

    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Commit 2 ticket changes to Jira:"));
    assert!(dialog_text.contains("NEW-1 · Parent task"));
    assert!(dialog_text.contains("NEW-2 · Child sub-task"));
}

#[test]
fn remove_discloses_descendants_and_keeps_blocked_subtree_dialog_open() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 0);
    page.create_ticket("Parent task");
    page.create_child_ticket("Submitted child");
    page.submit_selected_locally();

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Home)),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("Remove blocked"));
    assert!(dialog_text.contains("NEW-2 was already submitted"));
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );

    assert!(render_text(&mut page).contains("Ticket action"));
}

#[test]
fn restore_reset_dialog_opens_without_a_selected_ticket() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 0);
    page.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    let dialog_text = render_text(&mut page);
    assert!(dialog_text.contains("No ticket is selected."));
    assert!(dialog_text.contains("Cancel (c)"));
}

#[test]
fn ctrl_enter_creates_a_ticket_from_the_title_input() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    open_new_ticket(&mut page);
    let input = last_target(&mut page, "input");
    page.dispatch_focus(&input, true, &mut FocusCtx::default());
    for character in "fix login redirect".chars() {
        page.dispatch_event(
            &EventRoute::new(input.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut EventCtx::default(),
        );
    }

    page.dispatch_event(
        &EventRoute::new(input.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    assert_eq!(page.selected_changes().title, "Fix login redirect");
    assert!(!render_text(&mut page).contains("Create ticket"));
}

#[test]
fn reopened_new_ticket_dialog_is_in_insert_mode() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    open_new_ticket(&mut page);
    assert!(render_text(&mut page).contains("Create ticket"));
    page.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Create ticket"));

    open_new_ticket(&mut page);
    assert!(render_text(&mut page).contains("Create ticket"));

    let input = last_target(&mut page, "input");
    page.dispatch_focus(&input, true, &mut FocusCtx::default());
    for character in "second ticket".chars() {
        page.dispatch_event(
            &EventRoute::new(input.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut EventCtx::default(),
        );
    }

    page.dispatch_event(
        &EventRoute::new(input.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    assert_eq!(page.selected_changes().title, "Second ticket");
}

#[test]
fn escape_in_insert_or_select_mode_exits_editing_before_closing_dialog() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    // 1. Text input in insert mode
    open_new_ticket(&mut page);
    let input = last_target(&mut page, "input");
    page.dispatch_focus(&input, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(input.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
        &mut EventCtx::default(),
    );

    // First Esc: exits insert mode, dialog stays open
    page.dispatch_event(
        &EventRoute::new(input.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(page.create_dialog_is_open());

    // Second Esc: closes dialog
    page.dispatch_event(
        &EventRoute::new(input.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(!page.create_dialog_is_open());

    // 2. Dropdown in select mode
    open_new_ticket(&mut page);
    let issue_type = last_target(&mut page, "field");
    page.dispatch_focus(&issue_type, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(issue_type.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    assert!(page.create_kind_menu_is_open());

    // First Esc: closes dropdown popup, dialog stays open
    page.dispatch_event(
        &EventRoute::new(issue_type.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(!page.create_kind_menu_is_open());
    assert!(page.create_dialog_is_open());

    // Second Esc: closes dialog
    page.dispatch_event(
        &EventRoute::new(issue_type.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(!page.create_dialog_is_open());
}

#[test]
fn ctrl_enter_creates_a_ticket_from_the_issue_type_control() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    open_new_ticket(&mut page);
    let title = last_target(&mut page, "input");
    page.dispatch_focus(&title, true, &mut FocusCtx::default());
    for character in "fix login redirect".chars() {
        page.dispatch_event(
            &EventRoute::new(title.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut EventCtx::default(),
        );
    }
    let issue_type = last_target(&mut page, "field");
    page.dispatch_focus(&issue_type, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(issue_type.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    assert_eq!(page.selected_changes().title, "Fix login redirect");
    assert!(!render_text(&mut page).contains("Create ticket"));
}

#[test]
fn new_ticket_dialog_renders_a_bordered_title_field() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    open_new_ticket(&mut page);

    assert!(render_text(&mut page).contains("Title"));
}

#[test]
fn enter_on_issue_type_opens_its_dropdown_without_creating_ticket() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    open_new_ticket(&mut page);
    let title = page.selected_changes().title;
    let issue_type = last_target(&mut page, "field");
    page.dispatch_focus(&issue_type, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(issue_type.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    assert!(page.create_dialog_is_open());
    assert!(page.create_kind_menu_is_open());
    assert_eq!(page.selected_changes().title, title);
}

#[test]
fn ticket_navigation_activates_and_marks_selected_row_when_unfocused() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    let tickets = focus(&mut page, "data-view");
    let initial_title = page.selected_changes().title;
    page.dispatch_event(
        &EventRoute::new(tickets.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Down)),
        &mut EventCtx::default(),
    );
    assert_ne!(page.selected_changes().title, initial_title);
    page.dispatch_focus(&tickets, false, &mut FocusCtx::default());

    let area = Rect::new(0, 0, TEST_WIDTH, 40);
    page.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let selected_background = tuicore::theme().selected_bg();
    assert!(
        (tickets.area.y..tickets.area.bottom()).any(|y| {
            (tickets.area.x..tickets.area.right()).any(|x| {
                terminal.backend().buffer().cell((x, y)).unwrap().bg == selected_background
            })
        }),
        "selected ticket should remain marked after ticket list loses focus"
    );
}

#[test]
fn ticket_title_guidance_detects_multiple_actions_and_normalizes_input() {
    let checks = evaluate_title("Fix login and update docs");
    assert_eq!(checks[0].level, TitleLevel::Perfect);
    assert_eq!(checks[1].level, TitleLevel::Bad);
    assert_eq!(checks[2].level, TitleLevel::Perfect);
    assert_eq!(format_title("  fix   dont crash...  "), "Fix don't crash");
}

#[test]
fn diff_mode_uses_submission_snapshots_and_submitted_rows_use_disabled_glyph() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    page.submit_selected_locally();
    page.set_view_mode(ComposerViewMode::Diff);

    let text = render_text(&mut page);

    assert!(text.contains("󱋭"));
    assert!(text.contains("Diff"));
    assert!(!text.contains("Source"));
    assert!(!text.contains("Changes"));
}

#[test]
fn refreshed_source_updates_ticket_row_title_issue_type_and_title_field() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    let mut refreshed = page.selected_changes();
    refreshed.title = "Refreshed KAN-28 title".into();
    refreshed.kind = TicketKind::Bug;
    page.set_selected_source(refreshed);
    page.set_view_mode(ComposerViewMode::Source);

    let text = render_text(&mut page);

    assert!(text.matches("Refreshed KAN-28 title").count() >= 2);
    assert!(text.contains(""));
}

#[test]
fn toolbar_hotkeys_run_without_focusing_their_buttons() {
    tuicore::init();
    let mut page = composer_page();
    let open = open_change_set(&mut page, 1);
    assert!(open.tick_requested());

    let source = page.selected_changes();
    page.set_selected_source(source);
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    assert!(layout.focus_targets().iter().any(|target| {
        target
            .hotkey_sequences
            .iter()
            .any(|hotkey| hotkey == "shift+s")
    }));
    let mut refresh_ctx = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(tickets.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('R'))),
        &mut refresh_ctx,
    );
    assert!(refresh_ctx.tick_requested());
    assert_eq!(refresh_ctx.focus_request(), None);

    let mut submit_ctx = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('M'))),
        &mut submit_ctx,
    );
    assert!(submit_ctx.layout_requested());
    assert!(submit_ctx.focus_request().is_some());

    let settings = AnimationSettings {
        enabled: false,
        ..AnimationSettings::default()
    };
    let tick = page.tick(Duration::ZERO, settings);

    assert!(
        tick.next_tick
            .is_some_and(|delay| delay <= Duration::from_millis(50))
    );
}

#[test]
fn refresh_queues_every_remote_ticket_in_the_open_change_set() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let state = std::rc::Rc::new(std::cell::RefCell::new(state));
    let mut source = SourceController::new(state, AppService::for_tests());

    assert_eq!(source.refresh_all(), 3);
}

#[test]
fn preflight_failure_clears_only_its_durable_create_attempt_marker() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "Older unresolved ticket".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::MarkCreateAttempts {
        change_set_id: "CS-2".into(),
        ids: vec!["NEW-1".into()],
    });
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    let changes = state.commit_changes(&["NEW-2".into()]).unwrap();
    let state = Rc::new(RefCell::new(state));
    let mut submission = SubmissionController::new(Rc::clone(&state), AppService::for_tests());

    submission.start(changes, &mut EventCtx::default());
    for _ in 0..100 {
        if submission.drain_results() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let state = state.borrow();
    let tickets = &state.active_set().unwrap().tickets;
    assert!(tickets[0].create_attempt);
    assert!(!tickets[1].create_attempt);
    assert!(
        state
            .commit_changes(&["NEW-1".into()])
            .unwrap_err()
            .contains("unresolved Jira create attempt")
    );
    assert!(state.commit_changes(&["NEW-2".into()]).is_ok());
}

#[test]
fn local_issue_type_change_updates_ticket_row() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    page.update_selected_kind(TicketKind::Bug);

    assert!(render_text(&mut page).contains(""));
}

#[test]
fn property_dropdown_navigation_survives_tick_before_commit() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let state = Rc::new(RefCell::new(state));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut dropdown = BoundPropertyDropdown::priority_for_test(
        state,
        Rc::clone(&pending),
        AppService::for_tests(),
        vec![
            JiraOption {
                id: "high".into(),
                label: "High".into(),
            },
            JiraOption {
                id: "highest".into(),
                label: "Highest".into(),
            },
        ],
    );
    let area = Rect::new(0, 0, TEST_WIDTH, 3);
    let mut layout = LayoutCtx::new();
    dropdown.layout(area, &mut layout);
    let target = layout.focus_targets().first().unwrap().clone();
    dropdown.dispatch_focus(&target, true, &mut FocusCtx::default());
    dropdown.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    dropdown.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    dropdown.set_priorities_for_test(vec![
        JiraOption {
            id: "high".into(),
            label: "High".into(),
        },
        JiraOption {
            id: "highest".into(),
            label: "Highest".into(),
        },
        JiraOption {
            id: "low".into(),
            label: "Low".into(),
        },
    ]);
    dropdown.tick(Duration::ZERO, AnimationSettings::default());
    dropdown.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    assert_eq!(
        pending.borrow().as_slice(),
        [ComposerAction::UpdatePriority("Highest".into())]
    );
}

#[test]
fn assignee_dropdown_selects_the_ticket_account_id() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let state = Rc::new(RefCell::new(state));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut dropdown = BoundPropertyDropdown::assignee_for_test(
        state,
        pending,
        AppService::for_tests(),
        vec![JiraOption {
            id: "mina".into(),
            label: "Mina Patel".into(),
        }],
    );

    dropdown.sync_for_test();

    assert_eq!(dropdown.selected_for_test().as_deref(), Some("mina"));
}

#[test]
fn external_property_selection_closes_open_draft_before_escape() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let state = Rc::new(RefCell::new(state));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut dropdown = BoundPropertyDropdown::priority_for_test(
        Rc::clone(&state),
        Rc::clone(&pending),
        AppService::for_tests(),
        vec![
            JiraOption {
                id: "high".into(),
                label: "High".into(),
            },
            JiraOption {
                id: "highest".into(),
                label: "Highest".into(),
            },
            JiraOption {
                id: "low".into(),
                label: "Low".into(),
            },
        ],
    );
    let area = Rect::new(0, 0, TEST_WIDTH, 3);
    let mut layout = LayoutCtx::new();
    dropdown.layout(area, &mut layout);
    let target = layout.focus_targets().first().unwrap().clone();
    dropdown.dispatch_focus(&target, true, &mut FocusCtx::default());
    dropdown.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    dropdown.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    state
        .borrow_mut()
        .dispatch(ComposerAction::UpdatePriority("Low".into()));
    dropdown.tick(Duration::ZERO, AnimationSettings::default());
    assert!(!dropdown.is_open_for_test());

    dropdown.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );

    assert_eq!(dropdown.selected_for_test().as_deref(), Some("Low"));
    assert!(pending.borrow().is_empty());
}

#[test]
fn responsive_details_use_tabs_when_narrow_and_seventy_thirty_panels_when_wide() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let mut narrow = LayoutCtx::new();
    page.layout(Rect::new(0, 0, 96, 40), &mut narrow);
    assert!(
        narrow
            .focus_targets()
            .iter()
            .any(|target| target.id == FocusId::new("tabs"))
    );

    let mut wide = LayoutCtx::new();
    page.layout(Rect::new(0, 0, 120, 40), &mut wide);
    assert!(
        !wide
            .focus_targets()
            .iter()
            .any(|target| target.id == FocusId::new("tabs"))
    );
    let (description, properties) = page.detail_panel_areas();
    assert_eq!((description.width, properties.width), (84, 36));
    for hotkey in ["it", "st", "pr", "ee"] {
        assert!(wide.focus_targets().iter().any(|target| {
            target
                .hotkey_sequences
                .iter()
                .any(|sequence| sequence == hotkey)
        }));
    }
}

#[test]
fn mode_control_is_compact_and_source_uses_dashed_narrow_border() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    page.set_view_mode(ComposerViewMode::Source);
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);

    let mode = layout
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+v"])
        .unwrap();
    assert!(mode.area.width < TEST_WIDTH / 2);
    assert_eq!(page.narrow_border_style(), TabsBodyBorderStyle::Dashed);
    assert_eq!(page.ticket_detail_areas().0.height, 9);
}

#[test]
fn parent_dropdown_uses_change_ids_not_decorated_parent_labels() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });
    let state = Rc::new(RefCell::new(state));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let mut dropdown =
        BoundPropertyDropdown::parent_for_test(Rc::clone(&state), pending, AppService::for_tests());

    dropdown.sync_for_test();

    assert_eq!(dropdown.selected_for_test().as_deref(), Some("NEW-1"));
}

#[test]
fn deleted_ticket_hotkeys_and_escape_return_to_change_sets() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

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

#[test]
fn change_set_delete_uses_ctrl_x() {
    tuicore::init();
    let mut page = composer_page();
    let change_sets = target(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(change_sets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    assert!(render_text(&mut page).contains("Delete change set?"));
}

#[test]
fn submitting_tickets_keeps_user_in_change_set() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    assert!(render_text(&mut page).contains("Change sets > Checkout reliability"));

    page.submit_selected_locally();
    let text = render_text(&mut page);
    assert!(text.contains("Change sets > Checkout reliability"));
}

#[test]
fn opening_change_set_with_remote_tickets_shows_loader_before_content() {
    tuicore::init();
    let mut page = composer_page();
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    let target = layout.focus_targets().first().unwrap().clone();
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Home)),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Down)),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    let loader_text = render_text(&mut page);
    assert!(loader_text.contains("Loading Jira tickets…"));

    std::thread::sleep(Duration::from_millis(20));
    page.tick(Duration::from_millis(100), AnimationSettings::default());
    let content_text = render_text(&mut page);
    assert!(content_text.contains("Change sets > Checkout reliability"));
}

#[test]
fn submitted_tickets_allow_adding_children() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    page.submit_selected_locally();
    assert!(render_text(&mut page).contains("Add child (C)"));
}

#[test]
fn adding_child_expands_parent_and_focuses_new_child() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    page.create_ticket("Child of story");

    let text = render_text(&mut page);
    assert!(text.contains("Child of story"));
    assert_eq!(page.selected_changes().title, "Child of story");
}

#[test]
fn editing_description_does_not_leak_hotkeys() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let textarea = target(&mut page, "textarea");
    page.dispatch_focus(&textarea, true, &mut FocusCtx::default());

    page.dispatch_event(
        &EventRoute::new(textarea.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    let mut ctx = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(textarea.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('R'))),
        &mut ctx,
    );
    assert!(!ctx.tick_requested());

    page.dispatch_event(
        &EventRoute::new(textarea.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('M'))),
        &mut ctx,
    );
    assert!(!page.create_dialog_is_open());
}

#[test]
fn editing_description_preserves_exact_line_formatting() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let textarea = target(&mut page, "textarea");
    page.dispatch_focus(&textarea, true, &mut FocusCtx::default());

    page.dispatch_event(
        &EventRoute::new(textarea.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );

    page.dispatch_event(
        &EventRoute::new(textarea.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );

    let description = page.selected_changes().description;
    assert!(!description.contains("### Acceptance criteria\n\n- "));
}
