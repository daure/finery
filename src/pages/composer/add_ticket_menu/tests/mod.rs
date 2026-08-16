use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{EventCtx, FocusCtx, FocusId, FocusRequest, LayoutCtx, RenderCtx, TuiNode};

use super::{AddTicketMenu, EXISTING_WIDTH};
use crate::{
    service::AppService,
    store::composer::{Ticket, TicketKind},
};

#[test]
fn existing_jira_search_uses_centered_dropdown_popup_without_trigger_field() {
    tuicore::init();
    let mut menu = AddTicketMenu::new(AppService::for_tests());
    let area = Rect::new(0, 0, EXISTING_WIDTH, 12);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| {
        menu.layout(area, ctx);
    });
    let mut open = EventCtx::default();
    menu.open_existing(&mut open);
    assert!(matches!(
        open.focus_request(),
        Some(FocusRequest::TargetAt { id, .. }) if id == &FocusId::new("input")
    ));
    menu.dropdown.set_search_query("kan");
    menu.last_query = "kan".into();
    menu.apply_search_result(Ok(vec![Ticket {
        key: "KAN-28".into(),
        title: "Cart quantity updates preserve item".into(),
        description: String::new(),
        kind: TicketKind::Story,
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
    }]));
    let mut open_layout = LayoutCtx::new();
    open_layout.with_overlay_bounds(area, |ctx| {
        menu.layout(area, ctx);
    });
    let input = open_layout
        .focus_targets()
        .iter()
        .find(|target| target.id == FocusId::new("input"))
        .unwrap()
        .clone();
    let Some(FocusRequest::TargetAt { path, .. }) = open.focus_request() else {
        unreachable!();
    };
    assert_eq!(&input.path, path);
    menu.dispatch_focus(&input, true, &mut FocusCtx::default());
    assert!(menu.dropdown.is_focused());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            menu.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();

    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("kan"), "rendered: {text:?}");
    assert!(
        text.contains("KAN-28 · Cart quantity updates preserve item"),
        "rendered: {text:?}"
    );
    assert!(!text.contains("Select..."), "rendered: {text:?}");
    assert!(!text.contains("Search Jira"), "rendered: {text:?}");
    assert!(!text.contains("No results"), "rendered: {text:?}");
}
