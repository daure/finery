use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{FocusId, LayoutCtx, RenderCtx, TuiNode};

use super::page::snapshot_view;
use crate::store::work_items::{BacklogSnapshot, Sprint, WorkItem};

fn work_item(key: &str, title: &str) -> WorkItem {
    WorkItem {
        key: key.into(),
        title: title.into(),
        kind: "Story".into(),
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
    }
}

#[test]
fn sprint_sections_start_collapsed_and_backlog_starts_expanded() {
    tuicore::init();
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        sprints: vec![Sprint {
            id: 7,
            name: "Sprint 7".into(),
            state: "active".into(),
            work_items: vec![work_item("FIN-7", "Ship sprint work")],
        }],
        work_items: vec![work_item("FIN-8", "Plan next sprint")],
    };
    let mut view = snapshot_view(&snapshot);
    let area = Rect::new(0, 0, 80, 16);
    let mut layout = LayoutCtx::new();
    view.layout(area, &mut layout);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            view.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = (0..area.height)
        .flat_map(|y| {
            (0..area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol().to_owned())
        })
        .collect::<String>();

    assert!(text.contains("Finery · Sprint 7 (active)"));
    assert!(text.contains("Finery · Backlog"));
    assert!(text.contains("󰄱"));
    assert!(!text.contains("Ship sprint work"));
    assert!(text.contains("Plan next sprint"));
    assert!(layout.focus_targets().iter().any(|target| {
        target.id == FocusId::new("data-view") && target.hotkey_sequences == ["shift+b"]
    }));
}
