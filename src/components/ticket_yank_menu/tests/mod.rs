use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Modifier};
use tuicore::{EventCtx, Key, KeyEvent, LayoutCtx, RenderCtx, TuiEvent, TuiNode};

use super::*;

fn target() -> TicketYankTarget {
    TicketYankTarget {
        key: "FIN-42".into(),
        title: "Copy this ticket".into(),
        description: "Ticket details".into(),
    }
}

#[test]
fn yank_formats_each_ticket_value() {
    let target = target();

    assert_eq!(TicketYankAction::Url.text(&target), None);
    assert_eq!(
        TicketYankAction::Title.text(&target).as_deref(),
        Some("Copy this ticket")
    );
    assert_eq!(
        TicketYankAction::Description.text(&target).as_deref(),
        Some("Ticket details")
    );
    assert_eq!(
        TicketYankAction::Key.text(&target).as_deref(),
        Some("FIN-42")
    );
    assert_eq!(
        TicketYankAction::Full.text(&target).as_deref(),
        Some("FIN-42 - Copy this ticket")
    );
    assert_eq!(
        TicketYankAction::Slack.text(&target).as_deref(),
        Some(":ticket: FIN-42 - Copy this ticket")
    );

    let image = crate::store::work_items::content::ticket_image_marker(
        &crate::store::work_items::content::TicketImage {
            url: "https://jira.example/attachment/42".into(),
            alt: "cart.png".into(),
            width: 320,
            height: 180,
        },
    );
    let target = TicketYankTarget {
        description: format!("Before\n\n{image}\n\nAfter"),
        ..target
    };
    assert_eq!(
        TicketYankAction::Description.text(&target).as_deref(),
        Some("Before\n\n![cart.png](<https://jira.example/attachment/42>)\n\nAfter")
    );
}

#[test]
fn yank_menu_is_centered_without_search_and_underlines_action_hotkeys() {
    tuicore::init();
    let mut menu = TicketYankMenu::new();
    menu.open(target(), &mut EventCtx::default());
    let area = Rect::new(0, 0, 80, 24);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| menu.layout(area, ctx));
    let popup = layout.overlays().last().unwrap();
    assert_eq!(
        popup.area.x,
        area.width.saturating_sub(popup.area.width) / 2
    );
    assert_eq!(popup.area.width, "Description".len() as u16);

    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered = buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(!rendered.contains("Search"));
    for label in ["URL", "Title", "Description", "Key", "Full", "Slack"] {
        assert!(rendered.contains(label));
    }
    for hotkey in ['U', 'T', 'D', 'K', 'F', 'S'] {
        assert!(buffer.content().iter().any(|cell| {
            cell.symbol().eq_ignore_ascii_case(&hotkey.to_string())
                && cell.modifier.contains(Modifier::UNDERLINED)
        }));
    }
}

#[test]
fn yank_menu_shortcuts_select_and_close() {
    tuicore::init();
    for (hotkey, expected) in [
        ('u', TicketYankAction::Url),
        ('t', TicketYankAction::Title),
        ('d', TicketYankAction::Description),
        ('k', TicketYankAction::Key),
        ('f', TicketYankAction::Full),
        ('s', TicketYankAction::Slack),
    ] {
        let mut menu = TicketYankMenu::new();
        menu.open(target(), &mut EventCtx::default());
        menu.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char(hotkey))),
            &mut EventCtx::default(),
        );

        assert_eq!(menu.take_selection(), Some((expected, target())));
        assert!(!menu.is_open());
    }
}
