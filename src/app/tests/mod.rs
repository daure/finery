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

#[test]
fn background_copy_schedules_ticks_and_a_direct_copy_supersedes_it() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut app = root(service.clone(), Vec::new());
    let (release, gate) = std::sync::mpsc::channel();
    service.copy_in_background(move || {
        gate.recv_timeout(Duration::from_secs(5))
            .map_err(|error| error.to_string())?;
        Ok("Sprint report".into())
    });
    let mut ctx = tuicore::EventCtx::new(AnimationSettings::default());
    app.apply_dialog_signals(&mut ctx);
    assert!(ctx.tick_requested());
    assert!(ctx.notifications().is_empty());
    assert!(
        app.tick(Duration::ZERO, AnimationSettings::default())
            .next_tick
            .is_some()
    );
    assert_eq!(app.take_pending_clipboard_request(), None);

    ctx.copy_to_clipboard("A later direct copy");
    app.apply_dialog_signals(&mut ctx);
    assert!(!service.clipboard_pending());
    release.send(()).unwrap();
    assert_eq!(app.take_pending_clipboard_request(), None);
}
