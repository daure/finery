use crate::{
    app_settings::BacklogRunwaySettings,
    pages::backlog::page::{BacklogPage, velocity_dialog},
    service::AppService,
    store::work_items::{BacklogSnapshot, Sprint, VelocityReport, VelocitySprint},
};
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpListener,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tuicore::{EventCtx, EventRoute, HotkeyEvent, LayoutCtx, TuiEvent, TuiNode};

fn sprint(id: u64) -> VelocitySprint {
    VelocitySprint {
        id,
        name: format!("Sprint {id}"),
        completed: 3.0,
        goal: Some("Ship reports".into()),
        work_items: None,
    }
}

fn copy_hotkey(view: &mut impl TuiNode<()>) {
    let mut layout = LayoutCtx::new();
    view.layout(ratatui::layout::Rect::new(0, 0, 100, 30), &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .hotkey_sequences
                .iter()
                .any(|sequence| sequence == "yv")
        })
        .unwrap();
    view.dispatch_focus(
        target,
        true,
        &mut tuicore::FocusCtx::new(tuicore::AnimationSettings::default()),
    );
    view.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(tuicore::KeyEvent::from(tuicore::Key::Home)),
        &mut EventCtx::new(tuicore::AnimationSettings::default()),
    );
    let mut ctx = EventCtx::new(tuicore::AnimationSettings::default());
    view.dispatch_event(
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("yv".into())),
        &mut ctx,
    );
    assert_eq!(ctx.clipboard_request(), None);
    assert!(ctx.notifications().is_empty());
}

#[test]
fn both_report_copy_actions_fetch_only_the_selected_sprint_without_loading_notifications() {
    tuicore::init();
    for from_dialog in [true, false] {
        let service = AppService::for_tests();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        {
            let settings = service.settings();
            let mut settings = settings.write().unwrap();
            settings.jira_base_url = base_url.clone();
            settings.jira_email = "user@example.com".into();
            settings.jira_api_token = "token".into();
            settings.jira_story_points_field_id = "customfield_10016".into();
        }
        let report = VelocityReport {
            sprints: vec![sprint(12), sprint(13)],
            dynamic_capacity: Some(3.0),
            configured_sprints: 2,
        };
        let mut view: Box<dyn TuiNode<()>> = if from_dialog {
            Box::new(velocity_dialog(
                Some(&report),
                &BacklogRunwaySettings::default(),
                None,
                Rc::new(Cell::new(false)),
                Some(service.clone()),
            ))
        } else {
            Box::new(BacklogPage::with_snapshot_and_service_for_test(
                BacklogSnapshot {
                    board_name: "Finery".into(),
                    story_points_configured: true,
                    sprints: vec![Sprint {
                        id: 12,
                        name: "Sprint 12".into(),
                        state: "active".into(),
                        goal: Some("Ship reports".into()),
                        start_date: None,
                        end_date: None,
                        work_items: Vec::new(),
                        capacity: None,
                    }],
                    work_items: Vec::new(),
                    top_level_backlog_keys: Vec::new(),
                    warnings: Vec::new(),
                    runway: None,
                    velocity: Some(report),
                },
                service.clone(),
            ))
        };
        assert!(!service.clipboard_pending());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        listener.set_nonblocking(false).unwrap();
        let (release, gate) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]).into_owned();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            let body = serde_json::json!({"isLast": true, "issues": [{"key": "FIN-12", "fields": {
                "summary": "Fresh ticket", "issuetype": {"name": "Story"},
                "status": {"name": "Done"}, "customfield_10016": 3 }}]})
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            request
        });
        copy_hotkey(&mut view);
        assert!(service.clipboard_pending(), "from_dialog={from_dialog}");
        assert!(service.take_notifications().is_empty());
        assert_eq!(service.take_pending_clipboard(), None);
        release.send(()).unwrap();
        let mut app = crate::app::root(service.clone(), Vec::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        let text = loop {
            if let Some(text) = app.take_pending_clipboard_request() {
                break text;
            }
            assert!(
                Instant::now() < deadline,
                "Report copy did not complete: {:?}",
                service.take_errors()
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(text.contains("Fresh ticket"));
        assert!(text.contains("Points: 3/3 pts completed"));
        assert!(text.contains(&format!("{base_url}/browse/FIN-12")));
        assert!(!service.clipboard_pending());
        assert!(service.take_notifications().is_empty());
        assert!(service.take_errors().is_empty());
        assert!(server.join().unwrap().contains("/sprint/12/issue?"));
    }
}
