use std::time::Duration;

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{AnimationSettings, RenderCtx, TuiNode};

use crate::service::AppService;

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
