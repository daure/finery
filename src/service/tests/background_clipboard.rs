use super::*;

#[test]
fn latest_background_copy_wins_and_failures_leave_clipboard_untouched() {
    let service = AppService::for_tests();
    let (old_sender, receiver) = mpsc::channel();
    service.background_clipboard.lock().unwrap().pending = Some(receiver);
    service.copy_in_background(|| Ok("Newest report".into()));
    assert!(old_sender.send(Ok("Old report".into())).is_err());
    assert!(service.take_notifications().is_empty());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let text = loop {
        if let Some(text) = service.take_pending_clipboard() {
            break text;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(text, "Newest report");
    assert_eq!(service.take_pending_clipboard(), None);
    assert!(service.take_notifications().is_empty());

    let (sender, receiver) = mpsc::channel();
    service.background_clipboard.lock().unwrap().pending = Some(receiver);
    sender.send(Err("Jira unavailable".into())).unwrap();
    assert_eq!(service.take_pending_clipboard(), None);
    assert!(!service.clipboard_pending());
    assert_eq!(
        service.take_errors(),
        ["Could not copy sprint report: Jira unavailable"]
    );

    let (sender, receiver) = mpsc::channel();
    service.background_clipboard.lock().unwrap().pending = Some(receiver);
    service.cancel_pending_clipboard();
    assert!(sender.send(Ok("Superseded by direct copy".into())).is_err());
}
