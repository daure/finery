use std::{
    sync::{
        Arc, Mutex, RwLock,
        mpsc::{self, Sender},
    },
    thread,
};

use tokio::runtime::Runtime;

use crate::{
    app_settings::AppSettings,
    jira,
    storage::Storage,
    store::composer::{ChangeSet, Ticket, TicketChange},
    store::work_items::BacklogSnapshot,
};

#[derive(Clone)]
pub(crate) struct AppService {
    settings: Arc<RwLock<AppSettings>>,
    errors: Arc<Mutex<Vec<String>>>,
    notifications: Arc<Mutex<Vec<tuicore::Notification>>>,
    persistence: Sender<PersistenceCommand>,
}

enum PersistenceCommand {
    SaveSettings(Vec<(&'static str, String)>),
    SaveChangeSet(ChangeSet),
    DeleteChangeSet(String),
    Flush(Sender<()>),
}

impl AppService {
    pub(crate) fn initialize() -> Result<(Self, Vec<ChangeSet>), Box<dyn std::error::Error>> {
        let runtime = Arc::new(Runtime::new()?);
        let storage = runtime.block_on(Storage::connect_from_env())?;
        let stored_settings = runtime.block_on(storage.load_settings())?;
        let settings = AppSettings::resolve(&stored_settings)
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        let change_sets = runtime.block_on(storage.load_change_sets())?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let persistence =
            start_persistence_worker(storage, Arc::clone(&runtime), Arc::clone(&errors))?;
        Ok((
            Self {
                settings: Arc::new(RwLock::new(settings)),
                errors,
                notifications,
                persistence,
            },
            change_sets,
        ))
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        let runtime = Arc::new(Runtime::new().unwrap());
        let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let persistence =
            start_persistence_worker(storage, Arc::clone(&runtime), Arc::clone(&errors)).unwrap();
        Self {
            settings: Arc::new(RwLock::new(AppSettings::default())),
            errors,
            notifications,
            persistence,
        }
    }

    pub(crate) fn settings(&self) -> Arc<RwLock<AppSettings>> {
        Arc::clone(&self.settings)
    }

    pub(crate) fn save_settings(&self, settings: AppSettings) {
        let mut current = self.settings.write().expect("settings lock poisoned");
        let changed_values = settings.changed_values(&current);
        *current = settings;
        drop(current);
        if !changed_values.is_empty() {
            self.send(PersistenceCommand::SaveSettings(changed_values));
        }
    }

    pub(crate) fn save_change_set(&self, set: ChangeSet) {
        self.send(PersistenceCommand::SaveChangeSet(set));
    }

    pub(crate) fn delete_change_set(&self, id: String) {
        self.send(PersistenceCommand::DeleteChangeSet(id));
    }

    pub(crate) fn search_jira(&self, query: &str) -> Result<Vec<Ticket>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::search(&settings, query)
    }

    pub(crate) fn jira_backlog(&self) -> Result<BacklogSnapshot, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::backlog(&settings)
    }

    pub(crate) fn jira_projects(&self) -> Result<Vec<jira::JiraProject>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::projects(&settings)
    }

    pub(crate) fn fetch_jira(&self, key: &str) -> Result<Ticket, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::fetch(&settings, key)
    }

    pub(crate) fn jira_field_options(
        &self,
        ticket: &Ticket,
    ) -> Result<jira::JiraFieldOptions, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::field_options(&settings, ticket)
    }

    pub(crate) fn search_jira_assignees(
        &self,
        project_key: &str,
        query: &str,
    ) -> Result<Vec<jira::JiraAssignee>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::assignees(&settings, project_key, query)
    }

    pub(crate) fn submit_ticket_changes(
        &self,
        changes: &[TicketChange],
    ) -> Result<jira::SubmitBatchOutcome, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::submit_changes(&settings, changes)
    }

    pub(crate) fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut errors| std::mem::take(&mut *errors))
            .unwrap_or_else(|_| vec!["background service error lock is unavailable".into()])
    }

    pub(crate) fn report_error(&self, error: String) {
        if let Ok(mut errors) = self.errors.lock() {
            errors.push(error);
        }
    }

    pub(crate) fn report_notification(&self, notification: tuicore::Notification) {
        if let Ok(mut notifications) = self.notifications.lock() {
            notifications.push(notification);
        }
    }

    pub(crate) fn take_notifications(&self) -> Vec<tuicore::Notification> {
        self.notifications
            .lock()
            .map(|mut notifications| std::mem::take(&mut *notifications))
            .unwrap_or_default()
    }

    pub(crate) fn flush(&self) -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = mpsc::channel();
        self.persistence
            .send(PersistenceCommand::Flush(sender))
            .map_err(|_| "persistence worker is unavailable")?;
        receiver
            .recv()
            .map_err(|_| "persistence worker stopped before flushing")?;
        let errors = self.take_errors();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; ").into())
        }
    }

    fn send(&self, command: PersistenceCommand) {
        if self.persistence.send(command).is_err()
            && let Ok(mut errors) = self.errors.lock()
        {
            errors.push("persistence worker is unavailable".into());
        }
    }
}

fn start_persistence_worker(
    storage: Storage,
    runtime: Arc<Runtime>,
    errors: Arc<Mutex<Vec<String>>>,
) -> Result<Sender<PersistenceCommand>, Box<dyn std::error::Error>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("finery-persistence".into())
        .spawn(move || {
            while let Ok(command) = receiver.recv() {
                let result = match command {
                    PersistenceCommand::SaveSettings(values) => runtime.block_on(async {
                        for (key, value) in values {
                            storage.set_setting(key, &value).await?;
                        }
                        Ok(())
                    }),
                    PersistenceCommand::SaveChangeSet(set) => {
                        runtime.block_on(storage.save_change_set(&set))
                    }
                    PersistenceCommand::DeleteChangeSet(id) => {
                        runtime.block_on(storage.delete_change_set(&id))
                    }
                    PersistenceCommand::Flush(sender) => {
                        let _ = sender.send(());
                        Ok(())
                    }
                };
                if let Err(error) = result
                    && let Ok(mut errors) = errors.lock()
                {
                    errors.push(error.to_string());
                }
            }
        })?;
    Ok(sender)
}
