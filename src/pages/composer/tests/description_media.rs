use ratatui::{Terminal, backend::TestBackend};
use tuicore::{Key, KeyEvent, KeyModifiers, MouseEvent};

use super::*;
use crate::store::composer::{
    ChangeKind, ChangeSet, TicketChange, description_media::tests::image_ticket,
};

fn bound(mode: ComposerViewMode) -> (BoundDescription, PendingActions) {
    let ticket = image_ticket();
    let mut state = ComposerState::from_change_sets(vec![ChangeSet {
        id: "CS-1".into(),
        name: "Images".into(),
        tickets: vec![TicketChange {
            id: ticket.key.clone(),
            original: Some(ticket.clone()),
            updated: Some(ticket.clone()),
            kind: ChangeKind::Synced,
            submitted: None,
            retry_blocked: false,
            create_attempt: false,
            sibling_order: 0,
        }],
        selected_ticket_ids: Vec::new(),
        closed: false,
        archive_outcome: None,
        closed_at: None,
        submission_attempt: None,
    }]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state.dispatch(ComposerAction::SelectTicket(Some(ticket.key)));
    state.dispatch(ComposerAction::SetViewMode(mode));
    let pending = Rc::new(RefCell::new(Vec::new()));
    let bound = BoundDescription::new(
        Rc::new(RefCell::new(state)),
        Rc::clone(&pending),
        Rc::new(Cell::new(false)),
        &ComposerKeyBindings::default(),
        Rc::new(RefCell::new(Vec::new())),
    );
    (bound, pending)
}

fn image_cell(bound: &BoundDescription, area: Rect) -> (u16, u16) {
    let mut terminal = Terminal::new(TestBackend::new(area.right(), area.bottom())).unwrap();
    terminal
        .draw(|frame| bound.render(frame, area, &mut RenderCtx::new()))
        .unwrap();
    for y in area.y..area.bottom() {
        let line = (area.x..area.right())
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect::<String>();
        if let Some(x) = line.find("Image:") {
            return (area.x + x as u16, y);
        }
    }
    panic!("image reference should be visible");
}

fn click(x: u16, y: u16) -> TuiEvent {
    TuiEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn description_image_references_open_only_on_double_click_in_all_views() {
    for (mode, split) in [
        (ComposerViewMode::Source, false),
        (ComposerViewMode::Changes, false),
        (ComposerViewMode::Diff, false),
        (ComposerViewMode::Diff, true),
    ] {
        let (mut bound, pending) = bound(mode);
        bound.state.borrow_mut().description_diff_side_by_side = split;
        let original = bound.state.borrow().selected_ticket().unwrap().clone();
        let area = Rect::new(2, 1, 70, 12);
        bound.layout(area, &mut LayoutCtx::new());
        let (x, y) = image_cell(&bound, area);
        let mut ctx = EventCtx::default();
        assert_eq!(bound.event(&click(x, y), &mut ctx), EventOutcome::Handled);
        assert!(bound.description_actions.borrow().is_empty());
        assert!(ctx.focus_request().is_none());
        assert!(!bound.input.insert_mode());
        bound.event(&click(x, y), &mut EventCtx::default());
        let actions = bound
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        assert!(
            matches!(&actions[..], [DescriptionAction::OpenImage(attachment)] if attachment.id == "42")
        );
        let filename_click = click(x + "Image: `".len() as u16, y);
        bound.event(&filename_click, &mut EventCtx::default());
        assert!(bound.description_actions.borrow().is_empty());
        bound.event(&filename_click, &mut EventCtx::default());
        assert!(matches!(
            &bound.description_actions.borrow()[..],
            [DescriptionAction::OpenImage(attachment)] if attachment.id == "42"
        ));
        assert!(pending.borrow().is_empty());
        assert_eq!(bound.state.borrow().selected_ticket().unwrap(), &original);
    }
}

#[test]
fn description_editors_receive_canonical_text_and_media_guards_survive_noop_edits() {
    let (mut bound, pending) = bound(ComposerViewMode::Changes);
    let original = bound
        .state
        .borrow()
        .selected_ticket()
        .unwrap()
        .description
        .clone();
    bound.layout(Rect::new(0, 0, 70, 12), &mut LayoutCtx::new());
    bound.focus(None, true, &mut FocusCtx::default());
    bound.event(
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    assert!(bound.input.insert_mode());
    assert_eq!(bound.input.current_value(), original);
    bound.event(
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        &mut EventCtx::default(),
    );
    assert!(
        matches!(&pending.borrow()[..], [ComposerAction::UpdateDescription(value)] if value == &original)
    );
    assert!(
        !bound
            .state
            .borrow()
            .selected_ticket()
            .unwrap()
            .description_safe_to_overwrite
    );
    bound.event(
        &TuiEvent::Hotkey(HotkeyEvent::Commit(bound.editor_hotkey.clone())),
        &mut EventCtx::default(),
    );
    assert!(
        matches!(&bound.description_actions.borrow()[..], [DescriptionAction::OpenExternalEditor(value)] if value == &original)
    );
}
