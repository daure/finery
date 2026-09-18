use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{
    AnimationSettings, ChildKey, Dialog, DialogAction, EventCtx, EventOutcome, EventRoute,
    FocusCtx, KeyModifiers, LayoutCtx, MouseEvent, MouseEventKind, RenderCtx, TuiEvent, TuiNode,
};

use super::SettingsDialog;
use crate::service::AppService;

#[test]
fn settings_scrollbar_tracks_overflow_and_reveals_focused_fields() {
    let service = AppService::for_tests();
    let mut dialog = Dialog::new()
        .top_left("Settings")
        .actions([DialogAction::new("Close")])
        .host(SettingsDialog::new(service.settings(), service));
    let area = Rect::new(0, 0, 90, 24);
    let mut layout = LayoutCtx::new();
    dialog.layout(area, &mut layout);
    assert!(
        dialog
            .child()
            .root
            .scroll_geometry()
            .layout
            .vertical_bar
            .is_some()
    );

    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    let initial = render_text(&dialog, &mut terminal, area);
    assert!(initial.contains("Jira URL"));
    assert!(initial.contains("Close"));
    assert!(!initial.contains("Recent tickets to remember"));

    let first_field = &layout.focus_targets()[0];
    assert_eq!(
        dialog.dispatch_event(
            &EventRoute::new(first_field.path.clone()),
            &TuiEvent::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: first_field.area.x,
                row: first_field.area.y,
                modifiers: KeyModifiers::NONE,
            }),
            &mut EventCtx::default(),
        ),
        EventOutcome::Handled,
    );
    assert!(dialog.child().root.offset().y > 0);

    let last_field = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().last() == Some(&ChildKey::new("recent-tickets-limit")))
        .unwrap();
    dialog.dispatch_focus(
        last_field,
        true,
        &mut FocusCtx::new(AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        }),
    );
    dialog.layout(area, &mut LayoutCtx::new());
    assert!(dialog.child().root.offset().y > 0);
    let scrolled = render_text(&dialog, &mut terminal, area);
    assert!(scrolled.contains("Recent tickets to remember"));
    assert!(scrolled.contains("Close"));
    assert!(!scrolled.contains("Jira URL"));

    dialog.layout(Rect::new(0, 0, 90, 80), &mut LayoutCtx::new());
    assert!(
        dialog
            .child()
            .root
            .scroll_geometry()
            .layout
            .vertical_bar
            .is_none()
    );
    assert_eq!(dialog.child().root.target_offset().y, 0);
}

fn render_text(node: &impl TuiNode, terminal: &mut Terminal<TestBackend>, area: Rect) -> String {
    terminal
        .draw(|frame| {
            let mut ctx = RenderCtx::new();
            node.render(frame, area, &mut ctx);
            ctx.flush(frame);
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
