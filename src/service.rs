use std::{
    collections::{HashMap, HashSet},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::runtime::Runtime;

use crate::{
    app_settings::{
        AppSettings, JIRA_STORY_POINTS_BOARD_ID_SETTING,
        JIRA_STORY_POINTS_DISCOVERY_COMPLETE_SETTING, JIRA_STORY_POINTS_FIELD_ID_SETTING,
    },
    jira,
    storage::{
        ConditionalDeleteChangeSetOutcome, ConditionalSaveChangeSetOutcome, Storage,
        VersionedChangeSetCatalog,
    },
    store::composer::{ChangeSet, Ticket, TicketChange, TicketPresentation},
    store::work_items::{BacklogSnapshot, RankPlan, WorkItem},
};

pub(crate) mod composer_service;

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct AppService {
    settings: Arc<RwLock<AppSettings>>,
    settings_revision: Arc<AtomicU64>,
    recent_ticket_order: Arc<AtomicU64>,
    storage: Storage,
    runtime: Arc<Runtime>,
    errors: Arc<Mutex<Vec<String>>>,
    notifications: Arc<Mutex<Vec<tuicore::Notification>>>,
    jira_reorder: Arc<Mutex<()>>,
    persistence: Sender<PersistenceCommand>,
    composer_sync: Arc<Mutex<ComposerSyncState>>,
    #[cfg(test)]
    jira_submit: Arc<Mutex<Option<TestJiraSubmit>>>,
    #[cfg(test)]
    discovery_persistence_pause: Arc<Mutex<Option<DiscoveryPersistencePause>>>,
}

pub(crate) struct RecentTickets {
    pub work_items: Vec<WorkItem>,
    pub story_points_configured: bool,
    pub assumed_story_points: f64,
}

#[derive(Clone)]
pub(crate) struct ComposerSearchTicket {
    pub ticket: Ticket,
    pub work_item: WorkItem,
    pub story_points_configured: bool,
    pub assumed_story_points: f64,
}

pub(crate) struct ComposerSourceTicket {
    pub ticket: Ticket,
    pub presentation: TicketPresentation,
}

#[cfg(test)]
pub(crate) type TestJiraSubmit =
    Arc<dyn Fn(&[TicketChange]) -> jira::SubmitBatchOutcome + Send + Sync>;

#[cfg(test)]
struct DiscoveryPersistencePause {
    started: Sender<()>,
    resume: mpsc::Receiver<()>,
}

enum PersistenceCommand {
    SaveSettings(Vec<(&'static str, String)>),
    RecordRecentTicket(String, u64, usize),
    TrimRecentTickets(usize),
    SaveChangeSet(ChangeSet, Option<i64>),
    SaveChangeSetDurably(
        ChangeSet,
        Option<i64>,
        Sender<Result<DurableSaveOutcome, String>>,
    ),
    DeleteChangeSet(String, i64),
    PollComposerCatalogRevision,
    LoadComposerCatalog(i64),
    Flush(Sender<()>),
    #[cfg(test)]
    PauseDurableChangeSetSaves(mpsc::Receiver<()>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DurableSaveOutcome {
    Saved,
    Cancelled,
    Conflict,
}

#[derive(Default)]
struct ComposerSyncState {
    revisions: HashMap<String, i64>,
    queued_revisions: HashMap<String, i64>,
    blocked_change_sets: HashSet<String>,
    catalog_revision: i64,
    pending_writes: usize,
    poll_in_flight: bool,
    load_in_flight: bool,
    polled_catalog_revision: Option<Result<i64, String>>,
    loaded_catalog: Option<Result<LoadedComposerCatalog, String>>,
    reload_required: bool,
    alerts: Vec<String>,
}

pub(crate) struct LoadedComposerCatalog {
    pub catalog: VersionedChangeSetCatalog,
    pub requested_catalog_revision: i64,
}

pub(crate) struct SubmittedComposerChangeSet {
    pub response: composer_service::SubmitChangeSetResponse,
    pub change_sets: Vec<ChangeSet>,
}

impl ComposerSyncState {
    fn from_catalog(catalog: &VersionedChangeSetCatalog) -> Self {
        let revisions: HashMap<_, _> = catalog
            .change_sets
            .iter()
            .map(|set| (set.change_set.id.clone(), set.revision))
            .collect();
        Self {
            queued_revisions: revisions.clone(),
            revisions,
            catalog_revision: catalog.catalog_revision,
            ..Self::default()
        }
    }

    fn queue_save(&mut self, id: &str) -> Result<Option<i64>, String> {
        if self.blocked_change_sets.contains(id) {
            return Err(format!(
                "cannot save {id}: canonical Composer state is reloading"
            ));
        }
        let expected = self.queued_revisions.get(id).copied();
        self.queued_revisions
            .insert(id.into(), expected.map_or(1, |revision| revision + 1));
        self.pending_writes += 1;
        Ok(expected)
    }

    fn queue_delete(&mut self, id: &str) -> Result<Option<i64>, String> {
        if self.blocked_change_sets.contains(id) {
            return Err(format!(
                "cannot delete {id}: canonical Composer state is reloading"
            ));
        }
        let expected = self.queued_revisions.get(id).copied();
        if expected.is_some() {
            self.queued_revisions.remove(id);
            self.pending_writes += 1;
        }
        Ok(expected)
    }

    fn write_succeeded(&mut self, id: &str, revision: Option<i64>, catalog_revision: i64) {
        self.pending_writes = self.pending_writes.saturating_sub(1);
        match revision {
            Some(revision) => {
                self.revisions.insert(id.into(), revision);
            }
            None => {
                self.revisions.remove(id);
            }
        }
        self.catalog_revision = self.catalog_revision.max(catalog_revision);
    }

    fn write_failed(&mut self, id: &str, message: String) {
        self.pending_writes = self.pending_writes.saturating_sub(1);
        match self.revisions.get(id).copied() {
            Some(revision) => {
                self.queued_revisions.insert(id.into(), revision);
            }
            None => {
                self.queued_revisions.remove(id);
            }
        }
        self.blocked_change_sets.insert(id.into());
        self.reload_required = true;
        self.alerts.push(message);
    }

    fn write_cancelled(&mut self, id: &str) {
        self.pending_writes = self.pending_writes.saturating_sub(1);
        match self.revisions.get(id).copied() {
            Some(revision) => {
                self.queued_revisions.insert(id.into(), revision);
            }
            None => {
                self.queued_revisions.remove(id);
            }
        }
    }

    fn write_is_blocked(&self, id: &str) -> bool {
        self.blocked_change_sets.contains(id)
    }

    fn set_catalog(&mut self, catalog: &VersionedChangeSetCatalog) -> bool {
        let revisions: HashMap<_, _> = catalog
            .change_sets
            .iter()
            .map(|set| (set.change_set.id.clone(), set.revision))
            .collect();
        if catalog.catalog_revision < self.catalog_revision
            || revisions
                .iter()
                .any(|(id, revision)| self.revisions.get(id).is_some_and(|known| revision < known))
        {
            return false;
        }
        self.revisions = revisions;
        self.queued_revisions = self.revisions.clone();
        self.catalog_revision = catalog.catalog_revision;
        self.blocked_change_sets.clear();
        self.reload_required = false;
        true
    }
}

impl AppService {
    pub(crate) fn initialize() -> Result<(Self, Vec<ChangeSet>), Box<dyn std::error::Error>> {
        let runtime = Arc::new(Runtime::new()?);
        let storage = runtime.block_on(Storage::connect_from_env())?;
        let stored_settings = runtime.block_on(storage.load_settings())?;
        let settings = AppSettings::resolve(&stored_settings)
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        let catalog = runtime.block_on(storage.load_versioned_change_sets())?;
        let change_sets = catalog
            .change_sets
            .iter()
            .map(|set| set.change_set.clone())
            .collect();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let composer_sync = Arc::new(Mutex::new(ComposerSyncState::from_catalog(&catalog)));
        let persistence = start_persistence_worker(
            storage.clone(),
            Arc::clone(&runtime),
            Arc::clone(&errors),
            Arc::clone(&composer_sync),
        )?;
        Ok((
            Self {
                settings: Arc::new(RwLock::new(settings)),
                settings_revision: Arc::new(AtomicU64::new(0)),
                recent_ticket_order: Arc::new(AtomicU64::new(recent_ticket_order_seed()?)),
                storage,
                runtime,
                errors,
                notifications,
                jira_reorder: Arc::new(Mutex::new(())),
                persistence,
                composer_sync,
                #[cfg(test)]
                jira_submit: Arc::new(Mutex::new(None)),
                #[cfg(test)]
                discovery_persistence_pause: Arc::new(Mutex::new(None)),
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
        let composer_sync = Arc::new(Mutex::new(ComposerSyncState {
            catalog_revision: 1,
            ..ComposerSyncState::default()
        }));
        let persistence = start_persistence_worker(
            storage.clone(),
            Arc::clone(&runtime),
            Arc::clone(&errors),
            Arc::clone(&composer_sync),
        )
        .unwrap();
        Self {
            settings: Arc::new(RwLock::new(AppSettings::default())),
            settings_revision: Arc::new(AtomicU64::new(0)),
            recent_ticket_order: Arc::new(AtomicU64::new(recent_ticket_order_seed().unwrap())),
            storage,
            runtime,
            errors,
            notifications,
            jira_reorder: Arc::new(Mutex::new(())),
            persistence,
            composer_sync,
            jira_submit: Arc::new(Mutex::new(None)),
            discovery_persistence_pause: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn settings(&self) -> Arc<RwLock<AppSettings>> {
        Arc::clone(&self.settings)
    }

    pub(crate) fn settings_revision(&self) -> u64 {
        self.settings_revision.load(Ordering::Relaxed)
    }

    pub(crate) fn composer_service(&self) -> composer_service::ComposerService {
        let lookup_settings = Arc::clone(&self.settings);
        let jira_lookup = Arc::new(move |key: &str| {
            let settings = lookup_settings
                .read()
                .map_err(|_| "settings lock is unavailable".to_string())?
                .clone();
            jira::fetch(&settings, key)
        });
        let submit_settings = Arc::clone(&self.settings);
        #[cfg(test)]
        let test_jira_submit = Arc::clone(&self.jira_submit);
        let jira_submit = Arc::new(move |changes: &[TicketChange]| {
            #[cfg(test)]
            if let Some(submit) = test_jira_submit
                .lock()
                .ok()
                .and_then(|submit| submit.clone())
            {
                return submit(changes);
            }
            match submit_settings.read() {
                Ok(settings) => jira::submit_changes(&settings, changes),
                Err(_) => {
                    jira::SubmitBatchOutcome::PreflightError("settings lock is unavailable".into())
                }
            }
        });
        composer_service::ComposerService::new(
            self.storage.clone(),
            Arc::clone(&self.runtime),
            jira_lookup,
            jira_submit,
        )
    }

    pub(crate) fn save_settings(&self, settings: AppSettings) {
        let mut current = self.settings.write().expect("settings lock poisoned");
        let changed_values = settings.changed_values(&current);
        let recent_ticket_limit_changed =
            settings.recent_tickets_limit != current.recent_tickets_limit;
        let recent_ticket_limit = settings.recent_tickets_limit;
        *current = settings;
        if !changed_values.is_empty() {
            self.settings_revision.fetch_add(1, Ordering::Relaxed);
            self.send(PersistenceCommand::SaveSettings(changed_values));
            if recent_ticket_limit_changed {
                self.send(PersistenceCommand::TrimRecentTickets(recent_ticket_limit));
            }
        }
        drop(current);
    }

    pub(crate) fn save_change_set(&self, set: ChangeSet) {
        let expected = match self.queue_composer_save(&set.id) {
            Ok(expected) => expected,
            Err(error) => {
                self.report_error(error);
                return;
            }
        };
        self.send(PersistenceCommand::SaveChangeSet(set, expected));
    }

    pub(crate) fn save_change_set_durably(
        &self,
        set: ChangeSet,
    ) -> Result<mpsc::Receiver<Result<DurableSaveOutcome, String>>, String> {
        let (sender, receiver) = mpsc::channel();
        let expected = self.queue_composer_save(&set.id)?;
        let id = set.id.clone();
        self.persistence
            .send(PersistenceCommand::SaveChangeSetDurably(
                set, expected, sender,
            ))
            .map_err(|_| {
                self.cancel_queued_composer_write(&id);
                "persistence worker is unavailable".to_string()
            })?;
        Ok(receiver)
    }

    pub(crate) fn submit_change_set_from_snapshot(
        &self,
        set: ChangeSet,
        selected_ticket_ids: Vec<String>,
    ) -> Result<SubmittedComposerChangeSet, String> {
        let change_set_id = set.id.clone();
        let save = self.save_change_set_durably(set)?;
        match save
            .recv()
            .map_err(|_| "persistence worker stopped before saving Composer state".to_string())?
            .map_err(|error| error.to_string())?
        {
            DurableSaveOutcome::Saved => {}
            DurableSaveOutcome::Cancelled => {
                return Err("Composer save was cancelled before Jira submission".into());
            }
            DurableSaveOutcome::Conflict => {
                return Err("Composer save conflicted before Jira submission".into());
            }
        }
        let expected_revision = self
            .composer_sync
            .lock()
            .map_err(|_| "Composer synchronization state is unavailable".to_string())?
            .revisions
            .get(&change_set_id)
            .copied()
            .ok_or_else(|| format!("change set revision is unavailable: {change_set_id}"))?;
        let response = self.composer_service().submit_change_set(
            &change_set_id,
            expected_revision,
            selected_ticket_ids,
        );
        let catalog = self
            .runtime
            .block_on(self.storage.load_versioned_change_sets())
            .map_err(|error| error.to_string())?;
        self.accept_composer_catalog(&catalog);
        let change_sets = catalog
            .change_sets
            .into_iter()
            .map(|change_set| change_set.change_set)
            .collect();
        response
            .map_err(|error| error.to_string())
            .map(|response| SubmittedComposerChangeSet {
                response,
                change_sets,
            })
    }

    pub(crate) fn delete_change_set(&self, id: String) {
        let expected = match self.queue_composer_delete(&id) {
            Ok(Some(expected)) => expected,
            Ok(None) => {
                self.report_error(format!(
                    "cannot delete {id}: change-set revision is unavailable"
                ));
                return;
            }
            Err(error) => {
                self.report_error(error);
                return;
            }
        };
        self.send(PersistenceCommand::DeleteChangeSet(id, expected));
    }

    pub(crate) fn composer_catalog_revision(&self) -> i64 {
        self.composer_sync
            .lock()
            .map(|state| state.catalog_revision)
            .unwrap_or(0)
    }

    pub(crate) fn poll_composer_catalog_revision(&self) {
        let should_send = self
            .composer_sync
            .lock()
            .map(|mut state| {
                if state.poll_in_flight {
                    false
                } else {
                    state.poll_in_flight = true;
                    true
                }
            })
            .unwrap_or(false);
        if should_send {
            self.send(PersistenceCommand::PollComposerCatalogRevision);
        }
    }

    pub(crate) fn load_composer_catalog(&self) {
        let requested_catalog_revision = self
            .composer_sync
            .lock()
            .map(|mut state| {
                if state.load_in_flight {
                    None
                } else {
                    state.load_in_flight = true;
                    Some(state.catalog_revision)
                }
            })
            .ok()
            .flatten();
        if let Some(requested_catalog_revision) = requested_catalog_revision {
            self.send(PersistenceCommand::LoadComposerCatalog(
                requested_catalog_revision,
            ));
        }
    }

    pub(crate) fn take_composer_catalog_revision(&self) -> Option<Result<i64, String>> {
        self.composer_sync
            .lock()
            .ok()
            .and_then(|mut state| state.polled_catalog_revision.take())
    }

    pub(crate) fn take_loaded_composer_catalog(
        &self,
    ) -> Option<Result<LoadedComposerCatalog, String>> {
        self.composer_sync
            .lock()
            .ok()
            .and_then(|mut state| state.loaded_catalog.take())
    }

    pub(crate) fn accept_composer_catalog(&self, catalog: &VersionedChangeSetCatalog) -> bool {
        self.composer_sync
            .lock()
            .is_ok_and(|mut state| state.set_catalog(catalog))
    }

    pub(crate) fn composer_writes_pending(&self) -> bool {
        self.composer_sync
            .lock()
            .is_ok_and(|state| state.pending_writes > 0)
    }

    pub(crate) fn composer_sync_in_flight(&self) -> bool {
        self.composer_sync
            .lock()
            .is_ok_and(|state| state.poll_in_flight || state.load_in_flight)
    }

    pub(crate) fn take_composer_reload_required(&self) -> bool {
        self.composer_sync
            .lock()
            .map(|mut state| std::mem::take(&mut state.reload_required))
            .unwrap_or(false)
    }

    pub(crate) fn take_composer_alerts(&self) -> Vec<String> {
        self.composer_sync
            .lock()
            .map(|mut state| std::mem::take(&mut state.alerts))
            .unwrap_or_default()
    }

    pub(crate) fn search_jira_work_items(&self, query: &str) -> Result<RecentTickets, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        let work_items = jira::search_work_items(&settings, query)?;
        Ok(RecentTickets {
            work_items,
            story_points_configured: !settings.jira_story_points_field_id.trim().is_empty(),
            assumed_story_points: settings.backlog_runway.fixed_ticket_size,
        })
    }

    pub(crate) fn search_jira_for_composer(
        &self,
        query: &str,
    ) -> Result<Vec<ComposerSearchTicket>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        let issues = jira::search_composer_issues(&settings, query)?;
        let story_points_configured = !settings.jira_story_points_field_id.trim().is_empty();
        let assumed_story_points = settings.backlog_runway.fixed_ticket_size;
        Ok(issues
            .into_iter()
            .map(|issue| ComposerSearchTicket {
                ticket: issue.ticket,
                work_item: issue.work_item,
                story_points_configured,
                assumed_story_points,
            })
            .collect())
    }

    pub(crate) fn jira_backlog(&self) -> Result<BacklogSnapshot, String> {
        self.with_jira_reorder(|service| service.jira_backlog_while_reorder_locked())
    }

    pub(crate) fn jira_backlog_while_reorder_locked(&self) -> Result<BacklogSnapshot, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        let backlog = jira::backlog(&settings)?;
        if let Some(discovery) = backlog.discovered_story_points {
            self.save_discovered_story_points(
                &settings.jira_story_points_board_id,
                &settings.jira_story_points_field_id,
                settings.jira_story_points_discovery_complete,
                discovery.board_id,
                discovery.field_id,
                discovery.discovery_complete,
            );
        }
        Ok(backlog.snapshot)
    }

    pub(crate) fn jira_rank(&self, plan: &RankPlan) -> Result<(), String> {
        self.with_jira_reorder(|service| service.jira_rank_while_reorder_locked(plan))
    }

    pub(crate) fn jira_rank_while_reorder_locked(&self, plan: &RankPlan) -> Result<(), String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::rank(&settings, plan)
    }

    pub(crate) fn jira_move_to_sprint_while_reorder_locked(
        &self,
        sprint_id: u64,
        issue_keys: &[String],
    ) -> Result<(), String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::move_to_sprint(&settings, sprint_id, issue_keys)
    }

    pub(crate) fn jira_move_to_backlog_while_reorder_locked(
        &self,
        issue_keys: &[String],
    ) -> Result<(), String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::move_to_backlog(&settings, issue_keys)
    }

    pub(crate) fn jira_transfer(
        &self,
        sprint_id: Option<u64>,
        issue_keys: &[String],
        rank_plan: Option<&RankPlan>,
    ) -> Result<(), String> {
        self.with_jira_reorder(|service| {
            match sprint_id {
                Some(sprint_id) => {
                    service.jira_move_to_sprint_while_reorder_locked(sprint_id, issue_keys)?
                }
                None => service.jira_move_to_backlog_while_reorder_locked(issue_keys)?,
            }
            if let Some(rank_plan) = rank_plan {
                service.jira_rank_while_reorder_locked(rank_plan)?;
            }
            Ok(())
        })
    }

    pub(crate) fn with_jira_reorder<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .jira_reorder
            .lock()
            .map_err(|_| "Jira reorder lock is unavailable".to_string())?;
        operation(self)
    }

    pub(crate) fn jira_projects(&self) -> Result<Vec<jira::JiraProject>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::projects(&settings)
    }

    pub(crate) fn fetch_jira_for_composer(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, ComposerSourceTicket>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        let story_points_configured = !settings.jira_story_points_field_id.trim().is_empty();
        let assumed_story_points = settings.backlog_runway.fixed_ticket_size;
        Ok(jira::fetch_composer_issues(&settings, keys)?
            .into_iter()
            .map(|(key, issue)| {
                (
                    key,
                    ComposerSourceTicket {
                        ticket: issue.ticket,
                        presentation: TicketPresentation {
                            work_item: issue.work_item,
                            story_points_configured,
                            assumed_story_points,
                        },
                    },
                )
            })
            .collect())
    }

    pub(crate) fn open_jira_issue(&self, key: &str) {
        let url = match self.settings.read() {
            Ok(settings) => settings.jira_issue_url(key),
            Err(_) => {
                self.report_error(
                    "Could not open Jira ticket: settings lock is unavailable".into(),
                );
                return;
            }
        };
        let Some(url) = url else {
            self.report_error("Could not open Jira ticket: Jira URL is not configured".into());
            return;
        };
        if let Err(error) = spawn_browser(browser_command(&url)) {
            #[cfg(target_os = "linux")]
            if error.kind() == std::io::ErrorKind::NotFound
                && spawn_browser(xdg_open_command(&url)).is_ok()
            {
                self.record_recent_ticket(key);
                return;
            }
            self.report_error(format!("Could not open Jira ticket in browser: {error}"));
            return;
        }
        self.record_recent_ticket(key);
    }

    pub(crate) fn load_recent_jira_tickets(&self) -> Result<RecentTickets, String> {
        let (limit, settings) = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())
            .map(|settings| (settings.recent_tickets_limit, settings.clone()))?;
        let keys = self
            .runtime
            .block_on(self.storage.load_recent_ticket_keys(limit))
            .map_err(|error| error.to_string())?;
        let story_points_configured = !settings.jira_story_points_field_id.trim().is_empty();
        let assumed_story_points = settings.backlog_runway.fixed_ticket_size;
        if keys.is_empty() {
            return Ok(RecentTickets {
                work_items: Vec::new(),
                story_points_configured,
                assumed_story_points,
            });
        }
        let tickets = jira::fetch_recent_work_items(&settings, &keys)?;
        Ok(RecentTickets {
            work_items: keys
                .into_iter()
                .filter_map(|key| tickets.get(&key).cloned())
                .collect(),
            story_points_configured,
            assumed_story_points,
        })
    }

    pub(crate) fn open_jira_board_page(&self, page: Option<&str>) {
        let url = match self.settings.read() {
            Ok(settings) => settings.jira_board_url(page),
            Err(_) => {
                self.report_error(
                    "Could not open Jira board page: settings lock is unavailable".into(),
                );
                return;
            }
        };
        let Some(url) = url else {
            self.report_error(
                "Could not open Jira board page: Jira URL, default project, and board ID must be configured"
                    .into(),
            );
            return;
        };
        if let Err(error) = spawn_browser(browser_command(&url)) {
            #[cfg(target_os = "linux")]
            if error.kind() == std::io::ErrorKind::NotFound
                && spawn_browser(xdg_open_command(&url)).is_ok()
            {
                return;
            }
            self.report_error(format!(
                "Could not open Jira board page in browser: {error}"
            ));
        }
    }

    pub(crate) fn open_jira_releases(&self) {
        let url = match self.settings.read() {
            Ok(settings) => settings.jira_releases_url(),
            Err(_) => {
                self.report_error(
                    "Could not open Jira releases: settings lock is unavailable".into(),
                );
                return;
            }
        };
        let Some(url) = url else {
            self.report_error(
                "Could not open Jira releases: Jira URL and default project must be configured"
                    .into(),
            );
            return;
        };
        if let Err(error) = spawn_browser(browser_command(&url)) {
            #[cfg(target_os = "linux")]
            if error.kind() == std::io::ErrorKind::NotFound
                && spawn_browser(xdg_open_command(&url)).is_ok()
            {
                return;
            }
            self.report_error(format!("Could not open Jira releases in browser: {error}"));
        }
    }

    pub(crate) fn fetch_jira_tickets(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, Ticket>, String> {
        let settings = self
            .settings
            .read()
            .map_err(|_| "settings lock is unavailable".to_string())?
            .clone();
        jira::fetch_tickets(&settings, keys)
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

    #[cfg(test)]
    pub(crate) fn set_jira_submit_for_tests(&self, submit: TestJiraSubmit) {
        *self.jira_submit.lock().unwrap() = Some(submit);
    }

    #[cfg(test)]
    pub(crate) fn pause_durable_change_set_saves(&self) -> Sender<()> {
        let (resume, paused) = mpsc::channel();
        self.persistence
            .send(PersistenceCommand::PauseDurableChangeSetSaves(paused))
            .unwrap();
        resume
    }

    #[cfg(test)]
    pub(crate) fn change_set_for_tests(&self, id: &str) -> Option<ChangeSet> {
        self.runtime
            .block_on(self.storage.load_change_set(id))
            .ok()
            .flatten()
            .map(|change_set| change_set.change_set)
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

    #[cfg(test)]
    pub(crate) fn pause_discovery_settings_persistence(&self) -> (mpsc::Receiver<()>, Sender<()>) {
        let (started_sender, started_receiver) = mpsc::channel();
        let (resume_sender, resume_receiver) = mpsc::channel();
        *self.discovery_persistence_pause.lock().unwrap() = Some(DiscoveryPersistencePause {
            started: started_sender,
            resume: resume_receiver,
        });
        (started_receiver, resume_sender)
    }

    fn send(&self, command: PersistenceCommand) {
        if self.persistence.send(command).is_err()
            && let Ok(mut errors) = self.errors.lock()
        {
            errors.push("persistence worker is unavailable".into());
        }
    }

    fn record_recent_ticket(&self, key: &str) {
        let limit = match self.settings.read() {
            Ok(settings) => settings.recent_tickets_limit,
            Err(_) => return,
        };
        let opened_order = self.recent_ticket_order.fetch_add(1, Ordering::Relaxed);
        self.send(PersistenceCommand::RecordRecentTicket(
            key.to_owned(),
            opened_order,
            limit,
        ));
    }

    fn save_discovered_story_points(
        &self,
        expected_board_id: &str,
        expected_field_id: &str,
        expected_discovery_complete: bool,
        board_id: String,
        field_id: String,
        discovery_complete: bool,
    ) {
        let mut settings = self.settings.write().expect("settings lock poisoned");
        if settings.jira_story_points_board_id != expected_board_id
            || settings.jira_story_points_field_id != expected_field_id
            || settings.jira_story_points_discovery_complete != expected_discovery_complete
        {
            return;
        }
        if settings.jira_story_points_board_id == board_id
            && settings.jira_story_points_field_id == field_id
            && settings.jira_story_points_discovery_complete == discovery_complete
        {
            return;
        }
        settings.jira_story_points_board_id = board_id;
        settings.jira_story_points_field_id = field_id;
        settings.jira_story_points_discovery_complete = discovery_complete;
        let values = vec![
            (
                JIRA_STORY_POINTS_BOARD_ID_SETTING,
                settings.jira_story_points_board_id.clone(),
            ),
            (
                JIRA_STORY_POINTS_FIELD_ID_SETTING,
                settings.jira_story_points_field_id.clone(),
            ),
            (
                JIRA_STORY_POINTS_DISCOVERY_COMPLETE_SETTING,
                settings.jira_story_points_discovery_complete.to_string(),
            ),
        ];
        #[cfg(test)]
        self.pause_discovery_settings_persistence_if_requested();
        self.send(PersistenceCommand::SaveSettings(values));
        drop(settings);
    }

    #[cfg(test)]
    fn pause_discovery_settings_persistence_if_requested(&self) {
        let pause = self.discovery_persistence_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            let _ = pause.started.send(());
            let _ = pause.resume.recv();
        }
    }

    fn queue_composer_save(&self, id: &str) -> Result<Option<i64>, String> {
        self.composer_sync
            .lock()
            .map(|mut state| state.queue_save(id))
            .map_err(|_| "Composer synchronization state is unavailable".to_string())?
    }

    fn queue_composer_delete(&self, id: &str) -> Result<Option<i64>, String> {
        self.composer_sync
            .lock()
            .map(|mut state| state.queue_delete(id))
            .map_err(|_| "Composer synchronization state is unavailable".to_string())?
    }

    fn cancel_queued_composer_write(&self, id: &str) {
        if let Ok(mut state) = self.composer_sync.lock() {
            state.write_cancelled(id);
        }
    }
}

fn browser_command(url: &str) -> Command {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(url);
        command
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("gio");
        command.args(["open", url]);
        command
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
    {
        xdg_open_command(url)
    }
}

#[cfg(unix)]
fn xdg_open_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

fn spawn_browser(mut command: Command) -> Result<(), std::io::Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

fn recent_ticket_order_seed() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

fn start_persistence_worker(
    storage: Storage,
    runtime: Arc<Runtime>,
    errors: Arc<Mutex<Vec<String>>>,
    composer_sync: Arc<Mutex<ComposerSyncState>>,
) -> Result<Sender<PersistenceCommand>, Box<dyn std::error::Error>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("finery-persistence".into())
        .spawn(move || {
            while let Ok(command) = receiver.recv() {
                let result = match command {
                    PersistenceCommand::SaveSettings(values) => {
                        runtime.block_on(async { storage.set_settings(&values).await })
                    }
                    PersistenceCommand::RecordRecentTicket(key, opened_order, limit) => {
                        runtime.block_on(storage.record_recent_ticket(&key, opened_order, limit))
                    }
                    PersistenceCommand::TrimRecentTickets(limit) => {
                        runtime.block_on(storage.trim_recent_tickets(limit))
                    }
                    PersistenceCommand::SaveChangeSet(set, expected) => {
                        save_change_set(&storage, &runtime, &composer_sync, set, expected)
                    }
                    PersistenceCommand::SaveChangeSetDurably(set, expected, sender) => {
                        let result = save_change_set_durably(
                            &storage,
                            &runtime,
                            &composer_sync,
                            set,
                            expected,
                        );
                        let _ = sender.send(result.map_err(|error| error.to_string()));
                        continue;
                    }
                    PersistenceCommand::DeleteChangeSet(id, expected) => {
                        delete_change_set(&storage, &runtime, &composer_sync, id, expected)
                    }
                    PersistenceCommand::PollComposerCatalogRevision => {
                        let result = runtime.block_on(storage.load_change_set_catalog_revision());
                        if let Ok(mut state) = composer_sync.lock() {
                            state.poll_in_flight = false;
                            state.polled_catalog_revision =
                                Some(result.map_err(|error| error.to_string()));
                        }
                        Ok(())
                    }
                    PersistenceCommand::LoadComposerCatalog(requested_catalog_revision) => {
                        let result = runtime.block_on(storage.load_versioned_change_sets());
                        if let Ok(mut state) = composer_sync.lock() {
                            state.load_in_flight = false;
                            state.loaded_catalog = Some(
                                result
                                    .map(|catalog| LoadedComposerCatalog {
                                        catalog,
                                        requested_catalog_revision,
                                    })
                                    .map_err(|error| error.to_string()),
                            );
                        }
                        Ok(())
                    }
                    PersistenceCommand::Flush(sender) => {
                        let _ = sender.send(());
                        Ok(())
                    }
                    #[cfg(test)]
                    PersistenceCommand::PauseDurableChangeSetSaves(resume) => {
                        let _ = resume.recv();
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

fn save_change_set_durably(
    storage: &Storage,
    runtime: &Runtime,
    composer_sync: &Mutex<ComposerSyncState>,
    set: ChangeSet,
    expected: Option<i64>,
) -> Result<DurableSaveOutcome, Box<dyn std::error::Error>> {
    let id = set.id.clone();
    if let Ok(mut state) = composer_sync.lock()
        && state.write_is_blocked(&id)
    {
        state.write_cancelled(&id);
        return Ok(DurableSaveOutcome::Cancelled);
    }
    match runtime.block_on(storage.save_change_set_if_revision(&set, expected)) {
        Ok(ConditionalSaveChangeSetOutcome::Saved {
            change_set_revision,
            catalog_revision,
        }) => {
            if let Ok(mut state) = composer_sync.lock() {
                state.write_succeeded(&id, Some(change_set_revision), catalog_revision);
            }
            Ok(DurableSaveOutcome::Saved)
        }
        Ok(ConditionalSaveChangeSetOutcome::Conflict) => {
            let message = format!("Composer save conflicted for {id}; reloading canonical state");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message);
            }
            Ok(DurableSaveOutcome::Conflict)
        }
        Err(error) => {
            let message = format!("Composer save failed for {id}: {error}");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message.clone());
            }
            Err(message.into())
        }
    }
}

fn save_change_set(
    storage: &Storage,
    runtime: &Runtime,
    composer_sync: &Mutex<ComposerSyncState>,
    set: ChangeSet,
    expected: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let id = set.id.clone();
    if let Ok(mut state) = composer_sync.lock()
        && state.write_is_blocked(&id)
    {
        state.write_cancelled(&id);
        return Ok(());
    }
    let outcome = match runtime.block_on(storage.save_change_set_if_revision(&set, expected)) {
        Ok(outcome) => outcome,
        Err(error) => {
            let message = format!("Composer save failed for {id}: {error}");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message.clone());
            }
            return Err(message.into());
        }
    };
    match outcome {
        ConditionalSaveChangeSetOutcome::Saved {
            change_set_revision,
            catalog_revision,
        } => {
            if let Ok(mut state) = composer_sync.lock() {
                state.write_succeeded(&id, Some(change_set_revision), catalog_revision);
            }
            Ok(())
        }
        ConditionalSaveChangeSetOutcome::Conflict => {
            let message = format!("Composer save conflicted for {id}; reloading canonical state");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message.clone());
            }
            Err(message.into())
        }
    }
}

fn delete_change_set(
    storage: &Storage,
    runtime: &Runtime,
    composer_sync: &Mutex<ComposerSyncState>,
    id: String,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(mut state) = composer_sync.lock()
        && state.write_is_blocked(&id)
    {
        state.write_cancelled(&id);
        return Ok(());
    }
    let outcome = match runtime.block_on(storage.delete_change_set_if_revision(&id, expected)) {
        Ok(outcome) => outcome,
        Err(error) => {
            let message = format!("Composer delete failed for {id}: {error}");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message.clone());
            }
            return Err(message.into());
        }
    };
    match outcome {
        ConditionalDeleteChangeSetOutcome::Deleted { catalog_revision } => {
            if let Ok(mut state) = composer_sync.lock() {
                state.write_succeeded(&id, None, catalog_revision);
            }
            Ok(())
        }
        ConditionalDeleteChangeSetOutcome::Conflict => {
            let message = format!("Composer delete conflicted for {id}; reloading canonical state");
            if let Ok(mut state) = composer_sync.lock() {
                state.write_failed(&id, message.clone());
            }
            Err(message.into())
        }
    }
}
