use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    AnimationSettings, CheckState, EventCtx, EventOutcome, EventRoute, ExternalEditorResponse,
    FocusCtx, FocusId, FocusRequest, HotkeyEvent, Key, KeyEvent, KeyModifiers, LayoutCtx,
    LifecycleCtx, RenderCtx, TabsBodyBorderStyle, TuiEvent, TuiNode, theme,
};

use super::change_set_list::change_set_share_text;
use super::page::ComposerPage;
use super::property_fields::BoundPropertyDropdown;
use super::source::SourceController;
use super::speed_reader_text::clean_for_speed_reader;
use super::submission::SubmissionController;
use super::{
    ticket_editor::selected_ticket_ids,
    ticket_rows::{ticket_data_view, ticket_rows},
};
use crate::{
    jira::JiraOption,
    service::{AppService, composer_service::ChangeSetPatchOperation},
    store::composer::{
        AttachmentChangeKind, ChangeKind, ChangeSet, ComposerAction, ComposerState,
        ComposerViewMode, PlacementTarget, SubmissionSnapshot, Ticket, TicketAttachment,
        TicketChange, TicketKind, TicketPresentation,
    },
    store::work_items::{SubtaskProgress, WorkItem},
};

use super::title_guidance::{TitleLevel, evaluate_title, format_title};

const TEST_WIDTH: u16 = 96;

fn share_ticket(key: &str, title: &str, kind: TicketKind, parent_key: Option<&str>) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "FIN".into(),
        title: title.into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        description_overwrite_warning: None,
        kind,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: String::new(),
        assignee_account_id: String::new(),
        story_points: None,
        fix_versions: Vec::new(),
        labels: Vec::new(),
        parent_key: parent_key.map(str::to_owned),
        parent_title: None,
        parent_kind: None,
        has_children: false,
        attachments: Vec::new(),
    }
}

fn share_change(id: &str, ticket: Ticket) -> TicketChange {
    TicketChange {
        id: id.into(),
        original: None,
        updated: Some(ticket),
        kind: ChangeKind::Modified,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    }
}

fn share_set(tickets: Vec<TicketChange>) -> ChangeSet {
    ChangeSet {
        id: "CS-12".into(),
        name: "Change set name".into(),
        tickets,
        selected_ticket_ids: Vec::new(),
        closed: false,
        submission_attempt: None,
    }
}

#[test]
fn sharing_one_story_ticket_omits_the_change_set_and_story() {
    let text = change_set_share_text(
        &share_set(vec![
            share_change(
                "story",
                share_ticket("FIN-100", "Story title", TicketKind::Story, None),
            ),
            share_change(
                "ticket",
                share_ticket("FIN-101", "Ticket title", TicketKind::Task, Some("FIN-100")),
            ),
        ]),
        Some("https://jira.example"),
    );

    assert_eq!(
        text,
        "[T] Ticket title - https://jira.example/browse/FIN-101"
    );
}

#[test]
fn sharing_multiple_story_tickets_keeps_the_story_heading() {
    let mut second_ticket = share_change(
        "second-ticket",
        share_ticket(
            "FIN-102",
            "Second ticket",
            TicketKind::Task,
            Some("FIN-100"),
        ),
    );
    second_ticket.sibling_order = 1;
    let text = change_set_share_text(
        &share_set(vec![
            share_change(
                "story",
                share_ticket("FIN-100", "Story title", TicketKind::Story, None),
            ),
            share_change(
                "first-ticket",
                share_ticket("FIN-101", "First ticket", TicketKind::Task, Some("FIN-100")),
            ),
            second_ticket,
        ]),
        Some("https://jira.example"),
    );

    assert_eq!(
        text,
        "[S] Story title - https://jira.example/browse/FIN-100\n├─ [T] First ticket - https://jira.example/browse/FIN-101\n└─ [T] Second ticket - https://jira.example/browse/FIN-102"
    );
}

#[test]
fn sharing_omits_subtasks_when_a_non_subtask_changed() {
    let text = change_set_share_text(
        &share_set(vec![
            share_change(
                "story",
                share_ticket("FIN-100", "Story title", TicketKind::Story, None),
            ),
            share_change(
                "ticket",
                share_ticket("FIN-101", "Ticket title", TicketKind::Task, Some("FIN-100")),
            ),
            share_change(
                "subtask",
                share_ticket(
                    "FIN-102",
                    "Subtask title",
                    TicketKind::Subtask,
                    Some("FIN-101"),
                ),
            ),
        ]),
        Some("https://jira.example"),
    );

    assert_eq!(
        text,
        "[T] Ticket title - https://jira.example/browse/FIN-101"
    );
}

#[test]
fn sharing_multiple_story_groups_includes_the_change_set_name_and_markers() {
    let mut first_story = share_change(
        "story",
        share_ticket("FIN-100", "First story", TicketKind::Story, None),
    );
    first_story.sibling_order = 1;
    let mut second_bug = share_change(
        "bug",
        share_ticket("FIN-200", "Second bug", TicketKind::Bug, None),
    );
    second_bug.sibling_order = 2;
    let text = change_set_share_text(
        &share_set(vec![
            first_story,
            share_change(
                "ticket",
                share_ticket("FIN-101", "Its ticket", TicketKind::Task, Some("FIN-100")),
            ),
            second_bug,
        ]),
        Some("https://jira.example"),
    );

    assert_eq!(
        text,
        "Change set name\n[T] Its ticket - https://jira.example/browse/FIN-101\n[B] Second bug - https://jira.example/browse/FIN-200"
    );
}

#[test]
fn sharing_only_subtasks_keeps_them_and_uses_the_subtask_marker() {
    let mut story = share_change(
        "story",
        share_ticket("FIN-100", "Story title", TicketKind::Story, None),
    );
    story.kind = ChangeKind::Synced;
    let text = change_set_share_text(
        &share_set(vec![
            story,
            share_change(
                "subtask",
                share_ticket(
                    "FIN-102",
                    "Subtask title",
                    TicketKind::Subtask,
                    Some("FIN-100"),
                ),
            ),
        ]),
        Some("https://jira.example"),
    );

    assert_eq!(
        text,
        "[ST] Subtask title - https://jira.example/browse/FIN-102"
    );
}

#[test]
fn speed_reader_removes_jira_adf_syntax_and_preserves_its_meaning() {
    let source = "{{jira:panel {\"panelType\":\"info\"}}}\nAsk @mention(\"@Ada\", \"account-1\") to review @card(https://example.com/design) by @date(2026-02-28). @status(\"Ready\", green) :rocket: {color:#112233}++Styled++{/color}\n{{/jira:panel}}\n\n{{jira:task-list}}\n- [ ] Plan\n- [x] Ship\n{{/jira:task-list}}\n\n{{jira:decision-list}}\n- Use ADF\n{{/jira:decision-list}}\n\nLiteral: \\{\\{not a tag, \\@date(2026-02-28), \\:smile:, and \\++underlined\\++.\n\n`:rocket: @mention(\"@Ada\", \"account-1\")`";

    assert_eq!(
        clean_for_speed_reader(source),
        "Info:\nAsk @Ada to review https://example.com/design by 2026-02-28. Ready rocket Styled\n\n- To do: Plan\n- Done: Ship\n\n- Decision: Use ADF\n\nLiteral: {{not a tag, @date(2026-02-28), :smile:, and ++underlined++.\n\n`:rocket: @mention(\"@Ada\", \"account-1\")`"
    );
}

fn composer_page() -> ComposerPage {
    let service = AppService::for_tests();
    let settings = service.settings();
    let mut page = ComposerPage::new(ComposerState::demo().change_sets, service, settings);
    page.init(&mut LifecycleCtx::default());
    page
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

fn render_text_after_syntax(page: &mut ComposerPage, width: u16) -> String {
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(20));
        page.tick(Duration::from_millis(100), AnimationSettings::default());
    }
    render_text_at(page, width)
}

fn render_property_dropdown(dropdown: &mut BoundPropertyDropdown) -> String {
    let area = Rect::new(0, 0, 32, 3);
    dropdown.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            dropdown.render(frame, area, &mut render);
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
    let target = target(page, "data-view");
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
    target_at(page, id, TEST_WIDTH)
}

fn target_at(page: &mut ComposerPage, id: &str, width: u16) -> tuicore::FocusTarget {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, width, 40), &mut layout);
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

    assert!(text.contains("Checkout reliability"));
    assert!(text.contains("Add (A)"));
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
    let description_text = render_text(&mut page);
    assert!(description_text.contains('!'));
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
    assert!(render_text_after_syntax(&mut page, TEST_WIDTH).contains("Editor result"));

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
    assert!(render_text(&mut page).contains("No tickets added"));

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
fn desktop_description_shortcuts_open_the_editor_and_speed_reader() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let description = target_at(&mut page, "textarea", 120);
    assert_eq!(description.hotkey_sequences, ["dd", "do", "ds"]);
    let desktop = render_text_at(&mut page, 120);
    assert!(desktop.contains("dd·do·ds"));
    assert!(!desktop.contains("shift+d"));
    page.dispatch_focus(&description, true, &mut FocusCtx::default());

    let mut editor = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(description.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("do".into())),
        &mut editor,
    );
    assert!(editor.external_editor_request().is_some());

    page.dispatch_event(
        &EventRoute::new(description.path.clone()),
        &TuiEvent::ExternalEditor(ExternalEditorResponse {
            value: "## Desktop editor result".into(),
            line: 1,
            col: 1,
        }),
        &mut EventCtx::default(),
    );
    assert!(render_text_after_syntax(&mut page, 120).contains("Desktop editor result"));

    page.dispatch_event(
        &EventRoute::new(description.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("ds".into())),
        &mut EventCtx::default(),
    );
    assert!(render_text_at(&mut page, 120).contains("600 WPM"));
}

#[test]
fn description_hotkeys_follow_the_active_view() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    page.set_view_mode(ComposerViewMode::Source);
    let source = render_text_at(&mut page, 120);
    assert!(source.contains("dd·ds"));
    assert!(!source.contains("dd·do"));
    let source_description = target_at(&mut page, "textarea", 120);
    assert_eq!(source_description.hotkey_sequences, ["dd", "ds"]);

    let mut source_focus = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(source_description.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("dd".into())),
        &mut source_focus,
    );
    assert_eq!(
        source_focus.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("textarea")))
    );

    page.dispatch_event(
        &EventRoute::new(source_description.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("ds".into())),
        &mut EventCtx::default(),
    );
    assert!(render_text_at(&mut page, 120).contains("600 WPM"));
    let reader = target_at(&mut page, "speed-reader", 120);
    page.dispatch_event(
        &EventRoute::new(reader.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );

    page.set_view_mode(ComposerViewMode::Diff);
    let diff = render_text_at(&mut page, 120);
    assert!(diff.contains("dd"));
    assert!(!diff.contains("dd·do"));
    assert!(!diff.contains("dd·ds"));
    let diff_panel = target_at(&mut page, "diff-viewer", 120);
    assert_eq!(diff_panel.hotkey_sequences, ["dd"]);

    let mut diff_focus = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(diff_panel.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("dd".into())),
        &mut diff_focus,
    );
    assert!(diff_focus.focus_request().is_none());
}

#[test]
fn new_ticket_dialog_shows_title_guidance() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    open_new_ticket(&mut page);

    let text = render_text_after_syntax(&mut page, TEST_WIDTH);
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
fn change_set_list_is_borderless_and_opens_the_new_change_set_dialog() {
    tuicore::init();
    let mut page = composer_page();

    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    assert_eq!(
        layout.focus_targets().first().unwrap().id,
        FocusId::new("data-view")
    );

    let list = render_text(&mut page);
    assert!(list.contains("New change set"));
    assert!(list.contains("Open"));
    assert!(list.contains("Search..."));
    assert!(!list.contains("Change sets"));

    let new_change_set = last_target(&mut page, "button");
    page.dispatch_focus(&new_change_set, true, &mut FocusCtx::default());
    let mut escape = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(new_change_set.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut escape,
    );
    assert_eq!(
        escape.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );
    page.dispatch_focus(&new_change_set, true, &mut FocusCtx::default());
    let mut control_bracket = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(new_change_set.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut control_bracket,
    );
    assert_eq!(
        control_bracket.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );

    let filter = filter_button(&mut page);
    page.dispatch_focus(&filter, true, &mut FocusCtx::default());
    for event in [
        TuiEvent::Key(KeyEvent::from(Key::Esc)),
        TuiEvent::Key(KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        }),
    ] {
        let mut ctx = EventCtx::default();
        let outcome = page.dispatch_event(&EventRoute::new(filter.path.clone()), &event, &mut ctx);
        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(
            ctx.focus_request(),
            Some(&FocusRequest::Target(FocusId::new("data-view")))
        );
    }

    let data_view = focus(&mut page, "data-view");
    for event in [
        TuiEvent::Key(KeyEvent::from(Key::Esc)),
        TuiEvent::Key(KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        }),
    ] {
        let mut ctx = EventCtx::default();
        let outcome =
            page.dispatch_event(&EventRoute::new(data_view.path.clone()), &event, &mut ctx);
        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(ctx.focus_request(), None);
    }

    let data_view = target(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(data_view.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+n".into())),
        &mut EventCtx::default(),
    );
    let dialog = render_text(&mut page);
    assert!(dialog.contains("New change set"));
    let title = last_target(&mut page, "input");
    assert!(title.area.width >= 48);
    assert_eq!(title.area.height, 1);
    assert!(!dialog.contains("Issue type"));
    assert!(!dialog.contains("Starts with a verb"));
    page.dispatch_focus(&title, true, &mut FocusCtx::default());

    page.dispatch_event(
        &EventRoute::new(title.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(title.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    assert_eq!(page.active_change_set_name().as_deref(), Some("r"));

    page.tick(Duration::from_millis(100), AnimationSettings::default());
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert_eq!(page.active_change_set_name(), None);
    assert_eq!(
        page.overview_highlighted_change_set().as_deref(),
        Some("CS-3")
    );
}

fn filter_button(page: &mut ComposerPage) -> tuicore::FocusTarget {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new("button") && target.area.x > TEST_WIDTH / 2)
        .unwrap()
        .clone()
}

#[test]
fn change_set_search_matches_keys_and_titles() {
    tuicore::init();
    let mut page = composer_page();
    let data_view = focus(&mut page, "data-view");

    page.dispatch_event(
        &EventRoute::new(data_view.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut EventCtx::default(),
    );
    for key in ['c', 's', '2'] {
        page.dispatch_event(
            &EventRoute::new(data_view.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut EventCtx::default(),
        );
    }
    let key_match = render_text(&mut page);
    assert!(key_match.contains("CS-2"));
    assert!(!key_match.contains("CS-1"));

    page.dispatch_event(
        &EventRoute::new(data_view.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    page.dispatch_event(
        &EventRoute::new(data_view.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut EventCtx::default(),
    );
    for key in ['c', 'o', 'r'] {
        page.dispatch_event(
            &EventRoute::new(data_view.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
            &mut EventCtx::default(),
        );
    }
    let title_match = render_text(&mut page);
    assert!(title_match.contains("Checkout reliability"));
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
fn attachment_actions_close_the_dialog_and_restore_selected_attachments() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    page.add_selected_attachment_for_test(AttachmentChangeKind::Added);
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let action_dialog = render_text(&mut page);
    assert!(action_dialog.contains("Attachment action"));
    assert!(action_dialog.contains("Remove (r)"));
    assert!(!action_dialog.contains("Open"));
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Attachment action"));
    assert_eq!(page.selected_attachment_change(), None);

    page.add_selected_attachment_for_test(AttachmentChangeKind::Synced);
    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('d'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Attachment action"));
    assert_eq!(
        page.selected_attachment_change(),
        Some(AttachmentChangeKind::Deleted)
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
    assert!(render_text(&mut page).contains("Restore (r)"));
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Restore or reset"));
    assert_eq!(
        page.selected_attachment_change(),
        Some(AttachmentChangeKind::Synced)
    );
}

#[test]
fn ticket_delete_and_restore_actions_close_the_dialog() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('d'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Ticket action"));

    let tickets = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(tickets.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('r'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut EventCtx::default(),
    );
    let dialog = target(&mut page, "dialog");
    page.dispatch_event(
        &EventRoute::new(dialog.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('r'))),
        &mut EventCtx::default(),
    );
    assert!(!render_text(&mut page).contains("Restore or reset"));
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
    assert!(dialog_text.contains("Commit (m)"));
    assert!(dialog_text.contains("Cancel (c)"));
}

#[test]
fn submit_confirmation_uses_generic_text() {
    tuicore::init();
    let mut page = composer_page();
    page.open_change_set_for_test("CS-1");
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
    assert!(dialog_text.contains("Commit 2 selected changes to Jira?"));
    assert!(!dialog_text.contains("NEW-1"));
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
    assert!(dialog_text.contains("NEW-2"));
    assert!(dialog_text.contains("was already"));
    assert!(dialog_text.contains("submitted"));
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

    let mut created = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(input.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut created,
    );

    assert_eq!(page.selected_changes().title, "Fix login redirect");
    assert!(!render_text(&mut page).contains("Create ticket"));
    assert_eq!(
        created.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );
}

#[test]
fn ctrl_enter_opens_the_selected_existing_ticket() {
    tuicore::init();
    let mut page = composer_page();
    page.open_change_set_for_test("CS-1");
    page.update_selected_kind(TicketKind::Task);
    assert_eq!(page.selected_changes().kind, TicketKind::Task);
    let mut ctx = EventCtx::default();

    let outcome = page.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert_eq!(outcome, EventOutcome::Handled);
    assert_eq!(ctx.propagation(), tuicore::Propagation::Stopped);
}

#[test]
fn ctrl_enter_creates_a_label_instead_of_opening_the_ticket() {
    tuicore::init();
    let mut page = composer_page();
    page.open_change_set_for_test("CS-1");
    let labels = target_at(&mut page, "tag-input", 120);
    page.dispatch_focus(&labels, true, &mut FocusCtx::default());

    page.dispatch_event(
        &EventRoute::new(labels.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    for character in "frontend".chars() {
        page.dispatch_event(
            &EventRoute::new(labels.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut EventCtx::default(),
        );
    }
    let mut ctx = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(labels.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert_eq!(page.selected_changes().labels, ["checkout", "frontend"]);
    assert_eq!(ctx.propagation(), tuicore::Propagation::Stopped);
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
fn property_unfocus_first_exits_the_control_then_returns_to_tickets() {
    tuicore::init();
    let mut page = composer_page();
    open_change_set(&mut page, 1);
    let tabs = target(&mut page, "tabs");
    page.dispatch_event(
        &EventRoute::new(tabs.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+p".into())),
        &mut EventCtx::default(),
    );
    let tickets = focus(&mut page, "data-view");
    page.dispatch_focus(&tickets, false, &mut FocusCtx::default());

    let story_points = last_target(&mut page, "input");
    page.dispatch_focus(&story_points, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(story_points.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let mut story_points_exit = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(story_points.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut story_points_exit,
    );
    assert_eq!(story_points_exit.focus_request(), None);
    let mut story_points_leave = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(story_points.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut story_points_leave,
    );
    assert_eq!(
        story_points_leave.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );

    let labels = last_target(&mut page, "tag-input");
    page.dispatch_focus(&labels, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(labels.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let mut labels_exit = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(labels.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut labels_exit,
    );
    assert_eq!(labels_exit.focus_request(), None);
    let mut labels_leave = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(labels.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut labels_leave,
    );
    assert_eq!(
        labels_leave.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );

    let fix_versions = last_target(&mut page, "field");
    page.dispatch_focus(&fix_versions, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(fix_versions.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    let mut fix_versions_exit = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(fix_versions.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut fix_versions_exit,
    );
    assert_eq!(fix_versions_exit.focus_request(), None);
    let mut fix_versions_leave = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(fix_versions.path),
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut fix_versions_leave,
    );
    assert_eq!(
        fix_versions_leave.focus_request(),
        Some(&FocusRequest::Target(FocusId::new("data-view")))
    );
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
fn typing_a_ticket_number_selects_an_exact_composer_ticket_while_search_is_active() {
    tuicore::init();
    let mut page = composer_page();
    page.open_change_set_for_test("CS-1");
    let tickets = focus(&mut page, "data-view");
    let route = EventRoute::new(tickets.path);
    let mut ctx = EventCtx::default();
    page.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
        &mut ctx,
    );
    for character in "FIN".chars() {
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut ctx,
        );
    }
    page.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    for digit in ['1', '4', '2'] {
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char(digit))),
            &mut ctx,
        );
    }

    assert_eq!(page.selected_changes().key, "FIN-142");
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
fn composer_rows_show_current_ticket_properties_with_presentation_only_details() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state
        .dispatch(ComposerAction::SetPresentation {
            change_set_id: "CS-1".into(),
            id: "FIN-142".into(),
            presentation: TicketPresentation {
                work_item: WorkItem {
                    key: "FIN-142".into(),
                    title: "Keep checkout state across retries".into(),
                    kind: "Story".into(),
                    status: "In Progress".into(),
                    done: false,
                    priority: "High".into(),
                    assignee: "Ada".into(),
                    parent_key: None,
                    parent_title: None,
                    has_children: true,
                    subtask_progress: Some(SubtaskProgress {
                        completed: 1,
                        total: 2,
                    }),
                    labels: Vec::new(),
                    fix_versions: Vec::new(),
                    epic_name: Some("Checkout reliability".into()),
                    story_points: None,
                },
                story_points_configured: true,
                assumed_story_points: 3.0,
            },
        })
        .unwrap();
    state.dispatch(ComposerAction::UpdateStoryPoints(Some(8.0)));
    state.dispatch(ComposerAction::UpdateFixVersions(vec!["2026.9".into()]));
    state.dispatch(ComposerAction::UpdateLabels(vec!["checkout".into()]));
    let mut tickets = ticket_data_view(&state);
    let area = Rect::new(0, 0, 120, 8);
    TuiNode::<()>::layout(&mut tickets, area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            tickets.render(frame, area);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect::<String>();

    assert!(text.contains("8"), "rendered: {text:?}");
    assert!(text.contains("1/2"), "rendered: {text:?}");
    assert!(text.contains("2026.9"), "rendered: {text:?}");
    assert!(text.contains("checkout"), "rendered: {text:?}");
    assert!(text.contains("Checkout reliability"), "rendered: {text:?}");
}

#[test]
fn composer_page_reloads_an_externally_changed_catalog() {
    tuicore::init();
    let service = AppService::for_tests();
    let set = ComposerState::demo().change_sets.remove(0);
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let settings = service.settings();
    let mut page = ComposerPage::new(vec![set], service.clone(), settings);
    open_change_set(&mut page, 0);

    service
        .composer_service()
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-142".into(),
                title: "Changed outside the TUI".into(),
            }],
        )
        .unwrap();

    for _ in 0..10 {
        page.tick(Duration::from_millis(500), AnimationSettings::default());
        if page.selected_changes().title == "Changed outside the TUI" {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("external Composer catalog was not applied");
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
            .any(|hotkey| hotkey == "shift+a")
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
    let service = AppService::for_tests();
    let mut submission = SubmissionController::new(Rc::clone(&state), service.clone());

    submission.start(changes, &mut EventCtx::default());
    for _ in 0..100 {
        submission.drain_results();
        if !submission.is_submitting() {
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
    assert!(
        service
            .change_set_for_tests("CS-2")
            .unwrap()
            .submission_attempt
            .is_none()
    );
}

#[test]
fn delayed_marker_persistence_does_not_block_submission_polling() {
    let service = AppService::for_tests();
    let resume = service.pause_durable_change_set_saves();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    let changes = state.commit_changes(&["NEW-1".into()]).unwrap();
    let state = Rc::new(RefCell::new(state));
    let mut submission = SubmissionController::new(state, service);

    submission.start(changes, &mut EventCtx::default());
    let started = Instant::now();

    assert!(!submission.drain_results());
    assert!(started.elapsed() < Duration::from_millis(50));
    assert!(submission.is_submitting());

    resume.send(()).unwrap();
}

#[test]
fn jira_submission_waits_for_durable_create_marker_confirmation() {
    let service = AppService::for_tests();
    let marker_was_durable = Arc::new(AtomicBool::new(false));
    let jira_called = Arc::new(AtomicBool::new(false));
    let marker_service = service.clone();
    let observed_marker = Arc::clone(&marker_was_durable);
    let observed_jira_call = Arc::clone(&jira_called);
    service.set_jira_submit_for_tests(Arc::new(move |_, _| {
        observed_jira_call.store(true, Ordering::SeqCst);
        observed_marker.store(
            marker_service
                .change_set_for_tests("CS-2")
                .is_some_and(|set| {
                    set.tickets
                        .iter()
                        .any(|change| change.id == "NEW-1" && change.create_attempt)
                }),
            Ordering::SeqCst,
        );
        crate::jira::SubmitBatchOutcome::PreflightError("test preflight failure".into())
    }));
    let resume = service.pause_durable_change_set_saves();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    let changes = state.commit_changes(&["NEW-1".into()]).unwrap();
    let state = Rc::new(RefCell::new(state));
    let mut submission = SubmissionController::new(state, service);

    submission.start(changes, &mut EventCtx::default());
    assert!(!submission.drain_results());
    assert!(!jira_called.load(Ordering::SeqCst));

    resume.send(()).unwrap();
    for _ in 0..100 {
        submission.drain_results();
        if !submission.is_submitting() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(jira_called.load(Ordering::SeqCst));
    assert!(marker_was_durable.load(Ordering::SeqCst));
    assert_eq!(
        submission.take_preflight_error().as_deref(),
        Some("test preflight failure")
    );
    assert!(submission.take_preflight_error().is_none());
}

#[test]
fn cancelled_durable_claim_never_contacts_jira() {
    let service = AppService::for_tests();
    let jira_called = Arc::new(AtomicBool::new(false));
    let observed_jira_call = Arc::clone(&jira_called);
    service.set_jira_submit_for_tests(Arc::new(move |_, _| {
        observed_jira_call.store(true, Ordering::SeqCst);
        crate::jira::SubmitBatchOutcome::PreflightError("must not submit".into())
    }));
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    service.save_change_set(state.active_set().unwrap().clone());
    service.flush().unwrap();
    let changes = state.commit_changes(&["NEW-1".into()]).unwrap();
    let state = Rc::new(RefCell::new(state));
    let resume = service.pause_durable_change_set_saves();
    let mut submission = SubmissionController::new(Rc::clone(&state), service.clone());

    submission.start(changes, &mut EventCtx::default());
    service
        .composer_service()
        .apply_change_set_patch(
            "CS-2",
            1,
            vec![ChangeSetPatchOperation::UpdateDescription {
                ticket_id: "NEW-1".into(),
                description: "external write wins".into(),
            }],
        )
        .unwrap();
    resume.send(()).unwrap();
    for _ in 0..100 {
        submission.drain_results();
        if !submission.is_submitting() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(!jira_called.load(Ordering::SeqCst));
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
fn diff_property_dropdowns_show_previous_values() {
    tuicore::init();
    let priorities = vec![
        JiraOption {
            id: "High".into(),
            label: "High".into(),
        },
        JiraOption {
            id: "Low".into(),
            label: "Low".into(),
        },
        JiraOption {
            id: "Medium".into(),
            label: "Medium".into(),
        },
    ];

    let mut unchanged = ComposerState::demo();
    unchanged.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    unchanged.selected_ticket = Some("FIN-142".into());
    unchanged.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    let mut unchanged = BoundPropertyDropdown::priority_for_test(
        Rc::new(RefCell::new(unchanged)),
        Rc::new(RefCell::new(Vec::new())),
        AppService::for_tests(),
        priorities.clone(),
    );
    assert!(render_property_dropdown(&mut unchanged).contains("(unchanged)"));

    let mut changed = ComposerState::demo();
    changed.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    changed.selected_ticket = Some("FIN-142".into());
    changed.dispatch(ComposerAction::UpdatePriority("Low".into()));
    changed.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    let mut changed = BoundPropertyDropdown::priority_for_test(
        Rc::new(RefCell::new(changed)),
        Rc::new(RefCell::new(Vec::new())),
        AppService::for_tests(),
        priorities.clone(),
    );
    let changed_text = render_property_dropdown(&mut changed);
    assert!(changed_text.contains("Low"));
    assert!(changed_text.contains("High"));

    let mut added = ComposerState::demo();
    added.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    added.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    added.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    let mut added = BoundPropertyDropdown::priority_for_test(
        Rc::new(RefCell::new(added)),
        Rc::new(RefCell::new(Vec::new())),
        AppService::for_tests(),
        priorities,
    );
    assert!(render_property_dropdown(&mut added).contains("(none)"));
}

#[test]
fn changed_diff_property_values_use_diff_colors() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state.selected_ticket = Some("FIN-142".into());
    state.dispatch(ComposerAction::UpdatePriority("Low".into()));
    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    let mut dropdown = BoundPropertyDropdown::priority_for_test(
        Rc::new(RefCell::new(state)),
        Rc::new(RefCell::new(Vec::new())),
        AppService::for_tests(),
        vec![
            JiraOption {
                id: "High".into(),
                label: "High".into(),
            },
            JiraOption {
                id: "Low".into(),
                label: "Low".into(),
            },
        ],
    );
    let area = Rect::new(0, 0, 32, 3);
    dropdown.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            dropdown.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let theme = theme();
    let current = buffer.cell((1, 1)).unwrap();
    assert_eq!(current.fg, theme.diff_added_fg());
    assert_eq!(current.bg, theme.diff_added_bg());
    let previous = buffer.cell((3, 2)).unwrap();
    assert_eq!(previous.fg, theme.diff_removed_fg());
    assert_eq!(previous.bg, theme.diff_removed_bg());
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
fn mode_controls_disable_inline_outside_diffs_and_source_uses_dashed_narrow_border() {
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
    assert!(render_text(&mut page).contains("Inline"));
    let inline = layout
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+i"])
        .unwrap()
        .clone();
    let mut disabled_hotkey = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(inline.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+i".into())),
        &mut disabled_hotkey,
    );
    assert_eq!(disabled_hotkey.focus_request(), Some(&FocusRequest::Keep));

    page.set_view_mode(ComposerViewMode::Diff);
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    assert!(render_text(&mut page).contains("Inline"));
    let inline = layout
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+i"])
        .unwrap()
        .clone();
    let mut toggled = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(inline.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+i".into())),
        &mut toggled,
    );
    assert_eq!(toggled.focus_request(), Some(&FocusRequest::Keep));
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
fn unchanged_parent_with_a_local_change_id_is_not_shown_as_a_diff() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let changes = &mut state.change_sets[0].tickets;
    changes
        .iter_mut()
        .find(|change| change.id == "FIN-157")
        .unwrap()
        .id = "local-parent-id".into();
    changes
        .iter_mut()
        .find(|change| change.id == "FIN-142")
        .unwrap()
        .original
        .as_mut()
        .unwrap()
        .parent_key = Some("FIN-157".into());
    state.selected_ticket = Some("FIN-142".into());
    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    let mut dropdown = BoundPropertyDropdown::parent_for_test(
        Rc::new(RefCell::new(state)),
        Rc::new(RefCell::new(Vec::new())),
        AppService::for_tests(),
    );

    assert!(render_property_dropdown(&mut dropdown).contains("(unchanged)"));
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
    assert!(render_text(&mut page).contains("Checkout reliability"));

    page.submit_selected_locally();
    let text = render_text(&mut page);
    assert!(text.contains("Checkout reliability"));
}

#[test]
fn opening_change_set_with_remote_tickets_shows_loader_before_content() {
    tuicore::init();
    let mut page = composer_page();
    let target = target(&mut page, "data-view");
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
    assert!(content_text.contains("Checkout reliability"));
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
fn added_subtask_uses_project_temporary_key_until_submission() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state
        .dispatch(ComposerAction::CreateTicketAt {
            title: "Parent task".into(),
            project_key: "FIN".into(),
            kind: TicketKind::Task,
            placement: PlacementTarget::Root,
        })
        .unwrap();
    state
        .dispatch(ComposerAction::CreateTicketAt {
            title: "Added subtask".into(),
            project_key: "FIN".into(),
            kind: TicketKind::Subtask,
            placement: PlacementTarget::ChildOf("NEW-1".into()),
        })
        .unwrap();
    let mut tickets = ticket_data_view(&state);
    let area = Rect::new(0, 0, TEST_WIDTH, 8);
    TuiNode::<()>::layout(&mut tickets, area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            tickets.render(frame, area);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }

    assert!(text.contains("FIN-TMP-1"));
    assert!(text.contains("FIN-TMP-2"));
    assert!(text.contains("A • @-- • To Do"));
    assert!(!text.contains("Root -> NEW-1"));

    let mut submitted = state.selected_changes().unwrap().clone();
    submitted.key = "FIN-200".into();
    state
        .dispatch(ComposerAction::CompleteSubmission {
            change_set_id: "CS-2".into(),
            id: "NEW-2".into(),
            snapshot: SubmissionSnapshot {
                original: None,
                updated: Some(submitted),
            },
        })
        .unwrap();
    let mut tickets = ticket_data_view(&state);
    TuiNode::<()>::layout(&mut tickets, area, &mut LayoutCtx::new());
    terminal
        .draw(|frame| {
            tickets.render(frame, area);
        })
        .unwrap();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }

    assert!(text.contains("FIN-200"));
    assert!(text.contains("A • @-- • To Do"));
    assert_eq!(
        state.selected_existing_ticket_key().as_deref(),
        Some("FIN-200")
    );
}

#[test]
fn attachments_are_tree_children_before_ticket_children() {
    let mut parent = share_ticket("FIN-1", "Parent", TicketKind::Task, None);
    parent.attachments = vec![TicketAttachment {
        id: "10000".into(),
        filename: "design.png".into(),
        created: "2026-09-04T16:14:04.000+0000".into(),
        size: 21_504,
        mime_type: Some("image/png".into()),
        content_url: Some("https://jira.example/attachment/design.png".into()),
        change: crate::store::composer::AttachmentChangeKind::Synced,
        local_data: None,
    }];
    let child = share_ticket("FIN-2", "Child", TicketKind::Subtask, Some("FIN-1"));
    let mut state = ComposerState::from_change_sets(vec![share_set(vec![
        share_change("FIN-1", parent),
        share_change("FIN-2", child),
    ])]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-12".into()));

    let rows = ticket_rows(&state);

    assert_eq!(
        rows.iter()
            .map(|row| row.item.id.as_str())
            .collect::<Vec<_>>(),
        ["FIN-1", "FIN-1:attachment:0", "FIN-2"]
    );

    state.dispatch(ComposerAction::SelectTicket(Some(
        "FIN-1:attachment:0".into(),
    )));
    assert_eq!(
        state
            .selected_attachment()
            .as_ref()
            .map(|attachment| &attachment.filename),
        Some(&"design.png".into())
    );

    let mut tickets = ticket_data_view(&state);
    assert!(tickets.toggle_selected("FIN-1".into()));
    assert!(tickets.selected_ids().contains(&"FIN-1".into()));
    let area = Rect::new(0, 0, TEST_WIDTH, 5);
    TuiNode::<()>::layout(&mut tickets, area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal.draw(|frame| tickets.render(frame, area)).unwrap();
    let text = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| (x, y)))
        .map(|(x, y)| terminal.backend().buffer().cell((x, y)).unwrap().symbol())
        .collect::<String>();

    assert!(!text.contains("󱋭"));
}

#[test]
fn attachment_tree_ids_are_not_sent_as_submission_ticket_ids() {
    assert_eq!(
        selected_ticket_ids(vec![
            "FIN-1".into(),
            "FIN-1:attachment:0".into(),
            "FIN-2".into(),
        ]),
        ["FIN-1", "FIN-2"]
    );
}

#[test]
fn pasted_attachment_is_staged_with_local_data_and_can_be_renamed() {
    let ticket = share_ticket("FIN-1", "Parent", TicketKind::Task, None);
    let mut change = share_change("FIN-1", ticket.clone());
    change.original = Some(ticket);
    let mut state = ComposerState::from_change_sets(vec![share_set(vec![change])]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-12".into()));
    state.dispatch(ComposerAction::SelectTicket(Some("FIN-1".into())));

    state
        .dispatch(ComposerAction::AddAttachment {
            filename: "clipboard.png".into(),
            mime_type: Some("image/png".into()),
            data: vec![1, 2, 3],
        })
        .unwrap();

    let attachment = state.selected_attachment().unwrap();
    assert!(attachment.id.starts_with("local-"));
    assert_eq!(attachment.filename, "clipboard.png");
    assert_eq!(attachment.change, AttachmentChangeKind::Added);
    assert_eq!(attachment.local_data, Some(vec![1, 2, 3]));
    assert!(state.selected_attachment_is_editable());
    assert!(state.selected_can_add_attachment());

    state
        .dispatch(ComposerAction::RenameSelectedAttachment(
            "renamed.png".into(),
        ))
        .unwrap();
    assert_eq!(state.selected_attachment().unwrap().filename, "renamed.png");
    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Source));
    assert!(!state.selected_can_add_attachment());
}

#[test]
fn synced_attachment_delete_is_staged_and_can_be_restored() {
    let mut ticket = share_ticket("FIN-1", "Parent", TicketKind::Task, None);
    ticket.attachments.push(TicketAttachment {
        id: "10000".into(),
        filename: "design.png".into(),
        created: "2026-09-04T16:14:04.000+0000".into(),
        size: 21_504,
        mime_type: Some("image/png".into()),
        content_url: Some("https://jira.example/attachment/design.png".into()),
        change: AttachmentChangeKind::Synced,
        local_data: None,
    });
    let mut change = share_change("FIN-1", ticket.clone());
    change.original = Some(ticket);
    let mut state = ComposerState::from_change_sets(vec![share_set(vec![change])]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-12".into()));
    state.dispatch(ComposerAction::SelectTicket(Some(
        "FIN-1:attachment:0".into(),
    )));

    state
        .dispatch(ComposerAction::DeleteSelectedAttachment)
        .unwrap();
    assert_eq!(
        state.selected_attachment().unwrap().change,
        AttachmentChangeKind::Deleted
    );
    assert!(!state.selected_attachment_is_editable());

    state
        .dispatch(ComposerAction::RestoreSelectedAttachment)
        .unwrap();
    assert_eq!(
        state.selected_attachment().unwrap().change,
        AttachmentChangeKind::Synced
    );
}

#[test]
fn removing_a_new_attachment_drops_it_from_the_change_set() {
    let ticket = share_ticket("FIN-1", "Parent", TicketKind::Task, None);
    let mut change = share_change("FIN-1", ticket.clone());
    change.original = Some(ticket);
    let mut state = ComposerState::from_change_sets(vec![share_set(vec![change])]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-12".into()));
    state.dispatch(ComposerAction::SelectTicket(Some("FIN-1".into())));
    state
        .dispatch(ComposerAction::AddAttachment {
            filename: "clipboard.png".into(),
            mime_type: Some("image/png".into()),
            data: vec![1, 2, 3],
        })
        .unwrap();

    state
        .dispatch(ComposerAction::RemoveSelectedAttachment)
        .unwrap();

    assert!(state.selected_attachment().is_none());
    assert_eq!(state.selected_ticket().unwrap().attachments, Vec::new());
}

#[test]
fn local_attachment_bytes_survive_change_set_persistence() {
    let service = AppService::for_tests();
    let mut ticket = share_ticket("FIN-1", "Parent", TicketKind::Task, None);
    ticket.attachments.push(TicketAttachment {
        id: "local-1".into(),
        filename: "clipboard.png".into(),
        created: "2026-09-04T16:14:04Z".into(),
        size: 4,
        mime_type: Some("image/png".into()),
        content_url: None,
        change: AttachmentChangeKind::Added,
        local_data: Some(vec![1, 2, 3, 4]),
    });
    service.save_change_set(share_set(vec![share_change("FIN-1", ticket)]));
    service.flush().unwrap();

    let persisted = service.change_set_for_tests("CS-12").unwrap();
    assert_eq!(
        persisted.tickets[0].updated.as_ref().unwrap().attachments[0].local_data,
        Some(vec![1, 2, 3, 4])
    );
}

#[test]
fn selecting_a_parent_ticket_selects_descendants_and_marks_partial_parents() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state
        .dispatch(ComposerAction::CreateTicketAt {
            title: "Parent task".into(),
            project_key: "FIN".into(),
            kind: TicketKind::Task,
            placement: PlacementTarget::Root,
        })
        .unwrap();
    state
        .dispatch(ComposerAction::CreateTicketAt {
            title: "First child".into(),
            project_key: "FIN".into(),
            kind: TicketKind::Subtask,
            placement: PlacementTarget::ChildOf("NEW-1".into()),
        })
        .unwrap();
    state
        .dispatch(ComposerAction::CreateTicketAt {
            title: "Second child".into(),
            project_key: "FIN".into(),
            kind: TicketKind::Subtask,
            placement: PlacementTarget::ChildOf("NEW-1".into()),
        })
        .unwrap();
    state.dispatch(ComposerAction::SetSelectedTickets(Vec::new()));

    let mut tickets = ticket_data_view(&state);
    assert!(tickets.toggle_selected("NEW-1".into()));
    assert_eq!(tickets.selected_ids(), ["NEW-1", "NEW-2", "NEW-3"]);

    tickets.toggle_selected("NEW-2".into());
    assert_eq!(
        tickets.check_state(&"NEW-1".into()),
        CheckState::Indeterminate
    );
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

    page.dispatch_event(
        &EventRoute::new(textarea.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('M'))),
        &mut ctx,
    );
    let text = render_text_after_syntax(&mut page, TEST_WIDTH);
    assert!(text.contains("RM"));
    assert!(!text.contains("Commit changes"));
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
