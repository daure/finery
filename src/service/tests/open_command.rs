use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{app_settings::AppSettings, service::AppService};

pub(crate) struct OpenCommandProbe {
    path: PathBuf,
}

impl OpenCommandProbe {
    pub(crate) fn new(service: &AppService) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "finery-open-command-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        let quoted_path = path.to_string_lossy().replace('\'', "'\\''");
        let mut settings = service.settings().read().unwrap().clone();
        settings.open_command = format!(
            "printf '%s\\n%s' \"$FINERY_TICKET_KEY\" \"$FINERY_TICKET_URL\" > '{quoted_path}'"
        );
        settings.jira_base_url = "https://jira.example".into();
        service.save_settings(settings);
        Self { path }
    }

    pub(crate) fn assert_opened(&self, key: &str) {
        self.assert_output(&format!("{key}\nhttps://jira.example/browse/{key}"));
    }

    fn assert_output(&self, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if fs::read_to_string(&self.path).is_ok_and(|actual| actual == expected) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&self.path).unwrap(), expected);
    }

    pub(crate) fn assert_not_opened(&self) {
        assert!(!self.path.exists());
    }
}

impl Drop for OpenCommandProbe {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn saved_open_command_passes_literal_ticket_context_without_running_on_save() {
    let service = AppService::for_tests();
    let probe = OpenCommandProbe::new(&service);
    service.flush().unwrap();
    probe.assert_not_opened();
    let stored = service
        .runtime
        .block_on(service.storage.load_settings())
        .unwrap();
    let restored = AppSettings::resolve(&stored).unwrap();
    assert_eq!(
        restored.open_command,
        service.settings().read().unwrap().open_command
    );

    let key = "FIN-42 ' ; $(exit 29)";
    assert!(service.run_open_command(key));
    probe.assert_opened(key);
    assert!(service.take_errors().is_empty());
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let notifications = service.take_notifications();
        if !notifications.is_empty() {
            assert_eq!(notifications.len(), 1);
            assert_eq!(notifications[0].title(), "Open command started");
            assert_eq!(
                notifications[0].body(),
                format!("Running open command for {key}")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "open command start was not reported"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn empty_commands_and_local_drafts_are_noops_and_command_failures_are_visible() {
    let service = AppService::for_tests();
    let probe = OpenCommandProbe::new(&service);
    assert!(!service.run_open_command("NEW-1"));
    assert!(!service.run_open_command(""));
    probe.assert_not_opened();
    for blank in ["", " \n\t"] {
        service.settings().write().unwrap().open_command = blank.into();
        assert!(!service.run_open_command("FIN-42"));
    }
    assert!(service.take_errors().is_empty());
    assert!(service.take_notifications().is_empty());

    service.settings().write().unwrap().open_command = "exit 23".into();
    service.run_open_command("FIN-42");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let errors = service.take_errors();
        if !errors.is_empty() {
            assert_eq!(errors.len(), 1);
            assert!(errors[0].contains("Open command exited with"));
            assert!(errors[0].contains("23"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "open command failure was not reported"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
