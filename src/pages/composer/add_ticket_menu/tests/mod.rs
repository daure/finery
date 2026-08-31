use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{EventCtx, FocusCtx, FocusId, FocusRequest, LayoutCtx, RenderCtx, TuiNode};

use super::{AddTicketMenu, EXISTING_WIDTH};
use crate::{
    service::{AppService, ComposerSearchTicket},
    store::composer::{PlacementTarget, Ticket, TicketKind},
    store::work_items::WorkItem,
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
    menu.open_existing(
        None,
        PlacementTarget::Root,
        vec![TicketKind::Story],
        &mut open,
    );
    assert!(matches!(
        open.focus_request(),
        Some(FocusRequest::TargetAt { id, .. }) if id == &FocusId::new("input")
    ));
    menu.dropdown.set_search_query("kan");
    menu.last_query = "kan".into();
    menu.apply_search_result(Ok(vec![search_ticket(Ticket {
        key: "KAN-28".into(),
        project_key: "KAN".into(),
        title: "Cart quantity updates preserve item".into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        kind: TicketKind::Story,
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
        assignee_account_id: "ada".into(),
        parent_key: None,
        parent_title: None,
        parent_kind: None,
        has_children: false,
    })]));
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
    assert!(text.contains("KAN-28"), "rendered: {text:?}");
    assert!(
        text.contains("Cart quantity updates preserve item"),
        "rendered: {text:?}"
    );
    assert!(text.contains("To Do"), "rendered: {text:?}");
    assert!(!text.contains("Select..."), "rendered: {text:?}");
    assert!(!text.contains("Search Jira"), "rendered: {text:?}");
    assert!(!text.contains("No results"), "rendered: {text:?}");
}

#[test]
fn existing_search_caps_its_popup_against_the_overlay_height() {
    let mut menu = AddTicketMenu::new(AppService::for_tests());
    let host = Rect::new(0, 44, EXISTING_WIDTH, 12);
    let overlay = Rect::new(0, 0, EXISTING_WIDTH, 100);
    let mut layout = LayoutCtx::new();

    layout.with_overlay_bounds(overlay, |ctx| menu.layout(host, ctx));

    assert_eq!(menu.dropdown.configured_max_popup_height(), Some(60));
}

#[test]
fn existing_search_keeps_legal_result_beyond_first_ten() {
    let mut menu = AddTicketMenu::new(AppService::for_tests());
    menu.legal_kinds = vec![TicketKind::Story];
    let mut tickets = (0..10)
        .map(|index| Ticket {
            key: format!("FIN-{index}"),
            project_key: "FIN".into(),
            title: "Task".into(),
            description: String::new(),
            description_safe_to_overwrite: true,
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            assignee_account_id: String::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        })
        .collect::<Vec<_>>();
    tickets.push(Ticket {
        key: "FIN-legal".into(),
        project_key: "FIN".into(),
        title: "Story".into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        kind: TicketKind::Story,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
        parent_key: None,
        parent_title: None,
        parent_kind: None,
        has_children: false,
    });

    menu.apply_search_result(Ok(tickets.into_iter().map(search_ticket).collect()));

    assert_eq!(menu.tickets.len(), 1);
    assert_eq!(menu.tickets[0].ticket.key, "FIN-legal");
}

fn search_ticket(ticket: Ticket) -> ComposerSearchTicket {
    ComposerSearchTicket {
        work_item: WorkItem {
            key: ticket.key.clone(),
            title: ticket.title.clone(),
            kind: format!("{:?}", ticket.kind),
            status: ticket.status.clone(),
            done: false,
            priority: ticket.priority.clone(),
            assignee: ticket.assignee.clone(),
            parent_key: ticket.parent_key.clone(),
            parent_title: ticket.parent_title.clone(),
            has_children: ticket.has_children,
            subtask_progress: None,
            labels: Vec::new(),
            fix_versions: Vec::new(),
            epic_name: None,
            story_points: None,
        },
        ticket,
        story_points_configured: false,
        assumed_story_points: 3.0,
    }
}
