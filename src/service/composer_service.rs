use std::{collections::HashMap, fmt, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::{
    storage::{
        ConditionalDeleteChangeSetOutcome, ConditionalSaveChangeSetOutcome, Storage,
        VersionedChangeSet,
    },
    store::composer::{
        ChangeKind, ChangeSet, ComposerAction, ComposerState, PlacementError, PlacementTarget,
        SubmissionAttemptPhase, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
        rebase_ticket, submission_attempt_owner,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Versioned<T> {
    pub revision: i64,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetCatalogView {
    pub change_sets: Vec<Versioned<ChangeSetView>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetView {
    pub id: String,
    pub name: String,
    pub closed: bool,
    pub selected_ticket_ids: Vec<String>,
    pub tickets: Vec<TicketChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TicketChangeView {
    pub id: String,
    pub kind: ChangeKindView,
    pub original: Option<TicketView>,
    pub updated: Option<TicketView>,
    pub submitted: bool,
    pub selected_for_commit: bool,
    pub retry_blocked: bool,
    pub create_attempt: bool,
    pub submission_claimed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TicketView {
    pub key: String,
    pub project_key: String,
    pub title: String,
    pub description: String,
    pub description_safe_to_overwrite: bool,
    pub description_overwrite_warning: Option<String>,
    pub kind: TicketKindView,
    pub status: String,
    pub priority: String,
    pub assignee: String,
    pub parent_key: Option<String>,
    pub parent_title: Option<String>,
    pub parent_kind: Option<TicketKindView>,
    pub has_children: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketKindView {
    Epic,
    Story,
    Task,
    Bug,
    Subtask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKindView {
    Added,
    Modified,
    Deleted,
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftTicketInput {
    pub title: String,
    pub project_key: String,
    pub kind: TicketKindView,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChangeSetPatchOperation {
    AddDraftTicket {
        ticket_id: String,
        draft: DraftTicketInput,
        parent_ticket_id: Option<String>,
    },
    IncludeJiraTicket {
        jira_key: String,
        parent_ticket_id: Option<String>,
    },
    SyncJiraTicket {
        ticket_id: String,
    },
    UpdateTitle {
        ticket_id: String,
        title: String,
    },
    UpdateDescription {
        ticket_id: String,
        description: String,
    },
    MoveTicket {
        ticket_id: String,
        parent_ticket_id: Option<String>,
    },
    RemoveLocalSubtree {
        ticket_id: String,
    },
    StageJiraDeletion {
        ticket_id: String,
    },
    RestoreTicket {
        ticket_id: String,
    },
    ResetTicket {
        ticket_id: String,
    },
    SetCommitSelection {
        ticket_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppliedOperation {
    pub operation_index: i64,
    pub ticket_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetPatchResponse {
    pub change_set: Versioned<ChangeSetView>,
    pub catalog_revision: i64,
    pub applied: Vec<AppliedOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetMutationResponse {
    pub change_set: Versioned<ChangeSetView>,
    pub catalog_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeleteChangeSetResponse {
    pub change_set_id: String,
    pub catalog_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveredCreate {
    pub ticket_id: String,
    pub jira_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitChangeSetResponse {
    pub change_set: Versioned<ChangeSetView>,
    pub catalog_revision: i64,
    pub outcome: SubmitChangeSetOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubmitChangeSetOutcome {
    PreflightError {
        message: String,
    },
    Conflict {
        ticket_ids: Vec<String>,
    },
    Completed {
        tickets: Vec<TicketSubmissionResult>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketSubmissionResult {
    pub ticket_id: String,
    pub submitted: bool,
    pub retry_blocked: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ServiceError {
    NotFound { resource: String, id: String },
    ClosedChangeSet { change_set_id: String },
    SubmittedTicket { ticket_id: String },
    SubmissionClaimed { ticket_id: String },
    InvalidOperation { message: String },
    StaleRevision { change_set_id: String },
    JiraLookup { jira_key: String, message: String },
    Storage { message: String },
    Submission { message: String },
    AlreadyExists { resource: String, id: String },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { resource, id } => write!(formatter, "{resource} not found: {id}"),
            Self::ClosedChangeSet { change_set_id } => {
                write!(formatter, "change set is closed: {change_set_id}")
            }
            Self::SubmittedTicket { ticket_id } => {
                write!(formatter, "ticket was submitted: {ticket_id}")
            }
            Self::SubmissionClaimed { ticket_id } => {
                write!(formatter, "ticket submission is in progress: {ticket_id}")
            }
            Self::InvalidOperation { message } => formatter.write_str(message),
            Self::StaleRevision { change_set_id } => {
                write!(formatter, "stale change set revision: {change_set_id}")
            }
            Self::JiraLookup { jira_key, message } => {
                write!(formatter, "Jira lookup failed for {jira_key}: {message}")
            }
            Self::Storage { message } => formatter.write_str(message),
            Self::Submission { message } => formatter.write_str(message),
            Self::AlreadyExists { resource, id } => {
                write!(formatter, "{resource} already exists: {id}")
            }
        }
    }
}

impl std::error::Error for ServiceError {}

pub(crate) trait JiraTicketLookup: Send + Sync {
    fn fetch_ticket(&self, jira_key: &str) -> Result<Ticket, String>;
}

pub(crate) trait JiraTicketSubmit: Send + Sync {
    fn submit_changes(
        &self,
        changes: &[TicketChange],
        allow_unsafe_description_overwrite: bool,
    ) -> crate::jira::SubmitBatchOutcome;
}

impl<F> JiraTicketSubmit for F
where
    F: Fn(&[TicketChange], bool) -> crate::jira::SubmitBatchOutcome + Send + Sync,
{
    fn submit_changes(
        &self,
        changes: &[TicketChange],
        allow_unsafe_description_overwrite: bool,
    ) -> crate::jira::SubmitBatchOutcome {
        self(changes, allow_unsafe_description_overwrite)
    }
}

impl<F> JiraTicketLookup for F
where
    F: Fn(&str) -> Result<Ticket, String> + Send + Sync,
{
    fn fetch_ticket(&self, jira_key: &str) -> Result<Ticket, String> {
        self(jira_key)
    }
}

#[derive(Clone)]
pub struct ComposerService {
    storage: Storage,
    runtime: Arc<Runtime>,
    jira: Arc<dyn JiraTicketLookup>,
    jira_submit: Arc<dyn JiraTicketSubmit>,
}

impl ComposerService {
    pub(crate) fn new(
        storage: Storage,
        runtime: Arc<Runtime>,
        jira: Arc<dyn JiraTicketLookup>,
        jira_submit: Arc<dyn JiraTicketSubmit>,
    ) -> Self {
        Self {
            storage,
            runtime,
            jira,
            jira_submit,
        }
    }

    pub fn change_set_catalog(&self) -> Result<Versioned<ChangeSetCatalogView>, ServiceError> {
        let catalog = self
            .runtime
            .block_on(self.storage.load_versioned_change_sets())
            .map_err(storage_error)?;
        Ok(Versioned {
            revision: catalog.catalog_revision,
            value: ChangeSetCatalogView {
                change_sets: catalog
                    .change_sets
                    .into_iter()
                    .map(versioned_change_set_view)
                    .collect(),
            },
        })
    }

    pub fn change_set(
        &self,
        change_set_id: &str,
    ) -> Result<Versioned<ChangeSetView>, ServiceError> {
        let change_set = self.load_change_set(change_set_id)?;
        Ok(versioned_change_set_view(change_set))
    }

    pub fn create_change_set(
        &self,
        name: String,
    ) -> Result<ChangeSetMutationResponse, ServiceError> {
        if name.trim().is_empty() {
            return Err(invalid("change set name must not be empty"));
        }
        let change_set_id = self.next_change_set_id()?;
        let change_set = ChangeSet {
            id: change_set_id.clone(),
            name,
            tickets: Vec::new(),
            selected_ticket_ids: Vec::new(),
            closed: false,
            submission_attempt: None,
        };
        match self
            .runtime
            .block_on(self.storage.save_change_set_if_revision(&change_set, None))
            .map_err(storage_error)?
        {
            ConditionalSaveChangeSetOutcome::Saved {
                change_set_revision,
                catalog_revision,
            } => Ok(ChangeSetMutationResponse {
                change_set: Versioned {
                    revision: change_set_revision,
                    value: change_set_view(change_set),
                },
                catalog_revision,
            }),
            ConditionalSaveChangeSetOutcome::Conflict => Err(ServiceError::AlreadyExists {
                resource: "change set".into(),
                id: change_set_id,
            }),
        }
    }

    fn next_change_set_id(&self) -> Result<String, ServiceError> {
        let catalog = self
            .runtime
            .block_on(self.storage.load_versioned_change_sets())
            .map_err(storage_error)?;
        let next = catalog
            .change_sets
            .iter()
            .filter_map(|change_set| {
                change_set
                    .change_set
                    .id
                    .strip_prefix("CS-")?
                    .parse::<usize>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        Ok(format!("CS-{next}"))
    }

    pub fn delete_change_set(
        &self,
        change_set_id: &str,
        expected_revision: i64,
    ) -> Result<DeleteChangeSetResponse, ServiceError> {
        let change_set = self.load_change_set(change_set_id)?;
        if change_set.revision != expected_revision {
            return Err(ServiceError::StaleRevision {
                change_set_id: change_set_id.into(),
            });
        }
        if let Some(ticket_id) = change_set.change_set.submission_attempt_ticket_id() {
            return Err(ServiceError::SubmissionClaimed {
                ticket_id: ticket_id.into(),
            });
        }
        match self
            .runtime
            .block_on(
                self.storage
                    .delete_change_set_if_revision(change_set_id, expected_revision),
            )
            .map_err(storage_error)?
        {
            ConditionalDeleteChangeSetOutcome::Deleted { catalog_revision } => {
                Ok(DeleteChangeSetResponse {
                    change_set_id: change_set_id.into(),
                    catalog_revision,
                })
            }
            ConditionalDeleteChangeSetOutcome::Conflict => Err(ServiceError::StaleRevision {
                change_set_id: change_set_id.into(),
            }),
        }
    }

    pub fn open_change_set_jira_ticket_keys(&self) -> Result<Vec<String>, ServiceError> {
        let catalog = self
            .runtime
            .block_on(self.storage.load_versioned_change_sets())
            .map_err(storage_error)?;
        let mut seen = std::collections::HashSet::new();
        Ok(catalog
            .change_sets
            .into_iter()
            .filter(|change_set| !change_set.change_set.closed)
            .flat_map(|change_set| change_set.change_set.tickets)
            .filter_map(|change| change.original.map(|ticket| ticket.key))
            .filter(|key| seen.insert(key.clone()))
            .collect())
    }

    pub fn refresh_open_change_set_baselines(
        &self,
        refreshed_tickets: &HashMap<String, Ticket>,
    ) -> Result<Versioned<ChangeSetCatalogView>, ServiceError> {
        let catalog = self
            .runtime
            .block_on(self.storage.load_versioned_change_sets())
            .map_err(storage_error)?;
        for versioned in catalog
            .change_sets
            .into_iter()
            .filter(|change_set| !change_set.change_set.closed)
        {
            let mut change_set = versioned.change_set;
            if change_set.submission_attempt_ticket_id().is_some() {
                continue;
            }
            let mut changed = false;
            for change in &mut change_set.tickets {
                let Some(original) = change.original.as_ref() else {
                    continue;
                };
                let Some(refreshed) = refreshed_tickets.get(&original.key) else {
                    continue;
                };
                if original != refreshed {
                    change.updated = change
                        .updated
                        .as_ref()
                        .map(|updated| rebase_ticket(original, updated, refreshed));
                    change.original = Some(refreshed.clone());
                    changed = true;
                }
            }
            if changed {
                self.save(&change_set.id, &change_set, Some(versioned.revision))?;
            }
        }
        self.change_set_catalog()
    }

    pub fn apply_change_set_patch(
        &self,
        change_set_id: &str,
        expected_revision: i64,
        operations: Vec<ChangeSetPatchOperation>,
    ) -> Result<ChangeSetPatchResponse, ServiceError> {
        if operations.is_empty() {
            return Err(invalid("a patch must contain at least one operation"));
        }
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let mut state = open_state(versioned.change_set.clone());
        if let Some(ticket_id) = state
            .active_set()
            .and_then(ChangeSet::submission_attempt_ticket_id)
        {
            return Err(ServiceError::SubmissionClaimed {
                ticket_id: ticket_id.into(),
            });
        }
        let mut applied = Vec::with_capacity(operations.len());
        for (operation_index, operation) in operations.into_iter().enumerate() {
            let ticket_ids = self.apply_operation(&mut state, operation)?;
            applied.push(AppliedOperation {
                operation_index: operation_index as i64,
                ticket_ids,
            });
        }
        let edited = state
            .active_set()
            .expect("opened change set is present")
            .clone();
        let (revision, catalog_revision) =
            self.save(change_set_id, &edited, Some(expected_revision))?;
        Ok(ChangeSetPatchResponse {
            change_set: Versioned {
                revision,
                value: change_set_view(edited),
            },
            catalog_revision,
            applied,
        })
    }

    pub fn refresh_change_set(
        &self,
        change_set_id: &str,
        expected_revision: i64,
    ) -> Result<ChangeSetPatchResponse, ServiceError> {
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let mut edited = versioned.change_set;
        if let Some(ticket_id) = edited.submission_attempt_ticket_id() {
            return Err(ServiceError::SubmissionClaimed {
                ticket_id: ticket_id.into(),
            });
        }
        let targets = edited
            .tickets
            .iter()
            .filter(|change| !change.is_submitted())
            .filter_map(|change| {
                change
                    .original
                    .as_ref()
                    .map(|ticket| (change.id.clone(), ticket.key.clone()))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Err(invalid("change set has no refreshable Jira tickets"));
        }
        for (id, jira_key) in targets {
            let ticket = self.fetch_jira(&jira_key)?;
            let change = edited
                .tickets
                .iter_mut()
                .find(|change| change.id == id)
                .expect("refresh target is present");
            change.updated = change.updated.as_ref().map(|updated| {
                rebase_ticket(
                    change.original.as_ref().expect("original exists"),
                    updated,
                    &ticket,
                )
            });
            change.original = Some(ticket);
        }
        let (revision, catalog_revision) =
            self.save(change_set_id, &edited, Some(expected_revision))?;
        Ok(ChangeSetPatchResponse {
            change_set: Versioned {
                revision,
                value: change_set_view(edited),
            },
            catalog_revision,
            applied: Vec::new(),
        })
    }

    pub fn recover_submission_attempt(
        &self,
        change_set_id: &str,
        expected_revision: i64,
        recovered_creates: Vec<RecoveredCreate>,
    ) -> Result<ChangeSetPatchResponse, ServiceError> {
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let Some(attempt) = versioned.change_set.submission_attempt.clone() else {
            return Err(invalid("change set has no submission attempt to recover"));
        };
        if attempt.phase != SubmissionAttemptPhase::JiraSubmissionStarted {
            if !recovered_creates.is_empty() {
                return Err(invalid(
                    "Jira submission did not start; recover without Jira confirmations",
                ));
            }
            return self.release_recovered_claim(
                change_set_id,
                expected_revision,
                attempt.owner_id,
            );
        }
        let mut state = open_state(versioned.change_set);
        let claimed = state
            .active_set()
            .expect("opened change set is present")
            .tickets
            .iter()
            .filter(|change| attempt.ticket_ids.contains(&change.id))
            .cloned()
            .collect::<Vec<_>>();
        let creates = claimed
            .iter()
            .filter(|change| change.kind == ChangeKind::Added)
            .map(|change| change.id.clone())
            .collect::<Vec<_>>();
        if recovered_creates.len() != creates.len()
            || recovered_creates.iter().any(|recovered| {
                !creates.contains(&recovered.ticket_id) || recovered.jira_key.trim().is_empty()
            })
            || recovered_creates
                .iter()
                .map(|recovered| &recovered.ticket_id)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != recovered_creates.len()
        {
            return Err(invalid(
                "confirm every claimed draft create with its Jira key before recovery",
            ));
        }
        let recovered_creates = recovered_creates
            .into_iter()
            .map(|recovered| (recovered.ticket_id, recovered.jira_key))
            .collect::<HashMap<_, _>>();
        for change in claimed {
            let snapshot = match change.kind {
                ChangeKind::Added => SubmissionSnapshot {
                    original: None,
                    updated: Some(
                        self.fetch_jira(
                            recovered_creates
                                .get(&change.id)
                                .expect("validated recovered create"),
                        )?,
                    ),
                },
                ChangeKind::Deleted => {
                    let key = change
                        .original
                        .as_ref()
                        .map(|ticket| ticket.key.as_str())
                        .ok_or_else(|| invalid("deleted ticket is missing its Jira key"))?;
                    if self.fetch_jira(key).is_ok() {
                        return Err(invalid(format!(
                            "Jira deletion is not confirmed for {}",
                            change.id
                        )));
                    }
                    SubmissionSnapshot {
                        original: change.original,
                        updated: None,
                    }
                }
                ChangeKind::Modified | ChangeKind::Synced => {
                    let desired = change
                        .updated
                        .as_ref()
                        .or(change.original.as_ref())
                        .ok_or_else(|| invalid("modified ticket is missing its desired state"))?;
                    let remote = self.fetch_jira(&desired.key)?;
                    if remote != *desired {
                        return Err(invalid(format!(
                            "Jira update is not confirmed for {}",
                            change.id
                        )));
                    }
                    SubmissionSnapshot {
                        original: change.original,
                        updated: Some(remote),
                    }
                }
            };
            dispatch(
                &mut state,
                ComposerAction::CompleteSubmission {
                    change_set_id: change_set_id.into(),
                    id: change.id,
                    snapshot,
                },
            )?;
        }
        if !recovered_creates.is_empty() && creates.is_empty() {
            return Err(invalid("claimed attempts do not contain draft creates"));
        }
        dispatch(
            &mut state,
            ComposerAction::ReleaseSubmissionClaim {
                change_set_id: change_set_id.into(),
                owner_id: attempt.owner_id,
            },
        )?;
        let reconciled = state.active_set().expect("opened change set is present");
        let (revision, catalog_revision) =
            self.save(change_set_id, reconciled, Some(expected_revision))?;
        Ok(ChangeSetPatchResponse {
            change_set: Versioned {
                revision,
                value: change_set_view(reconciled.clone()),
            },
            catalog_revision,
            applied: Vec::new(),
        })
    }

    pub fn submit_change_set(
        &self,
        change_set_id: &str,
        expected_revision: i64,
        selected_ticket_ids: Vec<String>,
        allow_unsafe_description_overwrite: bool,
    ) -> Result<SubmitChangeSetResponse, ServiceError> {
        validate_submit_selection(&selected_ticket_ids)?;
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let mut state = open_state(versioned.change_set);
        for ticket_id in &selected_ticket_ids {
            if change(&state, ticket_id)?.is_submitted() {
                return Err(ServiceError::SubmittedTicket {
                    ticket_id: ticket_id.clone(),
                });
            }
            if state
                .active_set()
                .is_some_and(|set| set.submission_attempt.is_some())
            {
                return Err(ServiceError::SubmissionClaimed {
                    ticket_id: ticket_id.clone(),
                });
            }
        }
        let changes = state
            .commit_changes(&selected_ticket_ids)
            .map_err(invalid)?;
        if changes.is_empty() {
            return Err(invalid("submission selection contains no pending tickets"));
        }

        let create_attempts = changes
            .iter()
            .filter(|change| change.kind == ChangeKind::Added)
            .map(|change| change.id.clone())
            .collect::<Vec<_>>();
        let claimed_ids = changes
            .iter()
            .map(|change| change.id.clone())
            .collect::<Vec<_>>();
        let attempt_owner = submission_attempt_owner();
        dispatch(
            &mut state,
            ComposerAction::ClaimSubmission {
                change_set_id: change_set_id.into(),
                ids: claimed_ids.clone(),
                owner_id: attempt_owner.clone(),
            },
        )?;
        let claimed = state.active_set().expect("opened change set is present");
        let (claimed_revision, _) = self.save(change_set_id, claimed, Some(expected_revision))?;
        let marked_revision = if create_attempts.is_empty() {
            claimed_revision
        } else {
            dispatch(
                &mut state,
                ComposerAction::MarkCreateAttempts {
                    change_set_id: change_set_id.into(),
                    ids: create_attempts.clone(),
                },
            )?;
            dispatch(
                &mut state,
                ComposerAction::MarkSubmissionCreateAttempts {
                    change_set_id: change_set_id.into(),
                    owner_id: attempt_owner.clone(),
                },
            )?;
            let marked = state.active_set().expect("opened change set is present");
            self.save(change_set_id, marked, Some(claimed_revision))?.0
        };
        dispatch(
            &mut state,
            ComposerAction::MarkSubmissionJiraStarted {
                change_set_id: change_set_id.into(),
                owner_id: attempt_owner.clone(),
            },
        )?;
        let persisted_revision = self
            .save(
                change_set_id,
                state.active_set().expect("opened change set is present"),
                Some(marked_revision),
            )?
            .0;

        let outcome = self
            .jira_submit
            .submit_changes(&changes, allow_unsafe_description_overwrite);
        match outcome {
            crate::jira::SubmitBatchOutcome::PreflightError(message) => {
                self.clear_create_attempts(
                    change_set_id,
                    &mut state,
                    &create_attempts,
                    &attempt_owner,
                    persisted_revision,
                )?;
                let change_set = self.change_set(change_set_id)?;
                let catalog = self.change_set_catalog()?.revision;
                Ok(SubmitChangeSetResponse {
                    change_set,
                    catalog_revision: catalog,
                    outcome: SubmitChangeSetOutcome::PreflightError { message },
                })
            }
            crate::jira::SubmitBatchOutcome::Conflict(ticket_ids) => {
                self.clear_create_attempts(
                    change_set_id,
                    &mut state,
                    &create_attempts,
                    &attempt_owner,
                    persisted_revision,
                )?;
                let change_set = self.change_set(change_set_id)?;
                let catalog = self.change_set_catalog()?.revision;
                Ok(SubmitChangeSetResponse {
                    change_set,
                    catalog_revision: catalog,
                    outcome: SubmitChangeSetOutcome::Conflict { ticket_ids },
                })
            }
            crate::jira::SubmitBatchOutcome::Completed(outcomes) => {
                if let Err(error) = validate_submission_outcomes(&changes, &outcomes) {
                    for ticket_id in &create_attempts {
                        dispatch(
                            &mut state,
                            ComposerAction::BlockTicketRetry {
                                change_set_id: change_set_id.into(),
                                id: ticket_id.clone(),
                            },
                        )?;
                    }
                    dispatch(
                        &mut state,
                        ComposerAction::ReleaseSubmissionClaim {
                            change_set_id: change_set_id.into(),
                            owner_id: attempt_owner,
                        },
                    )?;
                    self.save(
                        change_set_id,
                        state.active_set().expect("opened change set is present"),
                        Some(persisted_revision),
                    )?;
                    return Err(error);
                }
                let mut tickets = Vec::with_capacity(outcomes.len());
                for outcome in outcomes {
                    let ticket_id = outcome.id;
                    let result = match outcome.result {
                        Ok(snapshot) => {
                            dispatch(
                                &mut state,
                                ComposerAction::CompleteSubmission {
                                    change_set_id: change_set_id.into(),
                                    id: ticket_id.clone(),
                                    snapshot,
                                },
                            )?;
                            TicketSubmissionResult {
                                ticket_id,
                                submitted: true,
                                retry_blocked: false,
                                message: None,
                            }
                        }
                        Err(failure) => {
                            let retry_blocked = failure.retry_blocked;
                            let refresh = failure.refresh;
                            if retry_blocked {
                                dispatch(
                                    &mut state,
                                    ComposerAction::BlockTicketRetry {
                                        change_set_id: change_set_id.into(),
                                        id: ticket_id.clone(),
                                    },
                                )?;
                            } else if let Some(refresh) = refresh {
                                let (original, updated) = *refresh;
                                dispatch(
                                    &mut state,
                                    ComposerAction::RefreshAfterFailedSubmission {
                                        change_set_id: change_set_id.into(),
                                        id: ticket_id.clone(),
                                        original,
                                        updated,
                                    },
                                )?;
                            } else {
                                dispatch(
                                    &mut state,
                                    ComposerAction::ResolveCreateAttempt {
                                        change_set_id: change_set_id.into(),
                                        id: ticket_id.clone(),
                                    },
                                )?;
                            }
                            TicketSubmissionResult {
                                ticket_id,
                                submitted: false,
                                retry_blocked,
                                message: Some(failure.message),
                            }
                        }
                    };
                    tickets.push(result);
                }
                dispatch(
                    &mut state,
                    ComposerAction::ReleaseSubmissionClaim {
                        change_set_id: change_set_id.into(),
                        owner_id: attempt_owner,
                    },
                )?;
                let reconciled = state.active_set().expect("opened change set is present");
                let (revision, catalog_revision) =
                    self.save(change_set_id, reconciled, Some(persisted_revision))?;
                Ok(SubmitChangeSetResponse {
                    change_set: Versioned {
                        revision,
                        value: change_set_view(reconciled.clone()),
                    },
                    catalog_revision,
                    outcome: SubmitChangeSetOutcome::Completed { tickets },
                })
            }
        }
    }

    fn apply_operation(
        &self,
        state: &mut ComposerState,
        operation: ChangeSetPatchOperation,
    ) -> Result<Vec<String>, ServiceError> {
        match operation {
            ChangeSetPatchOperation::AddDraftTicket {
                ticket_id,
                draft,
                parent_ticket_id,
            } => {
                if ticket_id.is_empty() || !ticket_id.starts_with("NEW-") {
                    return Err(invalid("draft ticket IDs must start with NEW-"));
                }
                if has_ticket(state, &ticket_id) {
                    return Err(invalid("draft ticket ID already exists"));
                }
                let placement = explicit_placement(state, parent_ticket_id)?;
                dispatch(
                    state,
                    ComposerAction::CreateTicketWithId {
                        id: ticket_id.clone(),
                        title: draft.title,
                        project_key: draft.project_key,
                        kind: draft.kind.into(),
                        placement,
                    },
                )?;
                if !draft.description.is_empty() {
                    update_description(state, &ticket_id, draft.description)?;
                }
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::IncludeJiraTicket {
                jira_key,
                parent_ticket_id,
            } => {
                if jira_key.is_empty() {
                    return Err(invalid("Jira ticket key must not be empty"));
                }
                let ticket = self.fetch_jira(&jira_key)?;
                if has_ticket_identity(state, &ticket.key) {
                    return Err(invalid("Jira ticket is already included"));
                }
                let id = ticket.key.clone();
                dispatch(
                    state,
                    ComposerAction::IncludeTicketAt {
                        ticket,
                        placement: explicit_placement(state, parent_ticket_id)?,
                    },
                )?;
                Ok(vec![id])
            }
            ChangeSetPatchOperation::SyncJiraTicket { ticket_id } => {
                let jira_key = editable_change(state, &ticket_id)?
                    .original
                    .as_ref()
                    .map(|ticket| ticket.key.clone())
                    .ok_or_else(|| invalid("draft tickets cannot sync from Jira"))?;
                let ticket = self.fetch_jira(&jira_key)?;
                dispatch(
                    state,
                    ComposerAction::SetSource {
                        change_set_id: active_id(state),
                        id: ticket_id.clone(),
                        ticket,
                    },
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateTitle { ticket_id, title } => {
                if title.is_empty() {
                    return Err(invalid("ticket title must not be empty"));
                }
                update_title(state, &ticket_id, title)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateDescription {
                ticket_id,
                description,
            } => {
                update_description(state, &ticket_id, description)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::MoveTicket {
                ticket_id,
                parent_ticket_id,
            } => {
                editable_change(state, &ticket_id)?;
                dispatch(
                    state,
                    ComposerAction::ReparentTicket {
                        id: ticket_id.clone(),
                        placement: explicit_placement(state, parent_ticket_id)?,
                    },
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RemoveLocalSubtree { ticket_id } => {
                let removed = state
                    .removal_preview(&ticket_id)
                    .map_err(|message| invalid(message))?
                    .into_iter()
                    .map(|change| change.id.clone())
                    .collect::<Vec<_>>();
                dispatch(state, ComposerAction::RemoveTicket(ticket_id))?;
                Ok(removed)
            }
            ChangeSetPatchOperation::StageJiraDeletion { ticket_id } => {
                let change = editable_change(state, &ticket_id)?;
                if change.kind == ChangeKind::Added {
                    return Err(invalid("draft tickets must be removed locally"));
                }
                dispatch(state, ComposerAction::MarkTicketDeleted(ticket_id.clone()))?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RestoreTicket { ticket_id } => {
                let change = change(state, &ticket_id)?;
                if change.is_submitted() {
                    return Err(ServiceError::SubmittedTicket { ticket_id });
                }
                if change.kind != ChangeKind::Deleted {
                    return Err(invalid("ticket is not staged for deletion"));
                }
                dispatch(state, ComposerAction::RestoreTicket(ticket_id.clone()))?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::ResetTicket { ticket_id } => {
                let change = change(state, &ticket_id)?;
                if change.is_submitted() {
                    return Err(ServiceError::SubmittedTicket { ticket_id });
                }
                if change.kind != ChangeKind::Modified {
                    return Err(invalid("ticket has no local update to reset"));
                }
                dispatch(state, ComposerAction::ResetTicket(ticket_id.clone()))?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::SetCommitSelection { ticket_ids } => {
                if ticket_ids.len()
                    != ticket_ids
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                {
                    return Err(invalid("commit selection contains duplicate ticket IDs"));
                }
                for ticket_id in &ticket_ids {
                    selectable_change(state, ticket_id)?;
                }
                dispatch(
                    state,
                    ComposerAction::SetSelectedTickets(ticket_ids.clone()),
                )?;
                Ok(ticket_ids)
            }
        }
    }

    fn load_change_set(&self, id: &str) -> Result<VersionedChangeSet, ServiceError> {
        self.runtime
            .block_on(self.storage.load_change_set(id))
            .map_err(storage_error)?
            .ok_or_else(|| ServiceError::NotFound {
                resource: "change set".into(),
                id: id.into(),
            })
    }

    fn load_expected_change_set(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<VersionedChangeSet, ServiceError> {
        let change_set = self.load_change_set(id)?;
        if change_set.revision != expected_revision {
            return Err(ServiceError::StaleRevision {
                change_set_id: id.into(),
            });
        }
        if change_set.change_set.closed {
            return Err(ServiceError::ClosedChangeSet {
                change_set_id: id.into(),
            });
        }
        Ok(change_set)
    }

    fn fetch_jira(&self, jira_key: &str) -> Result<Ticket, ServiceError> {
        self.jira
            .fetch_ticket(jira_key)
            .map_err(|message| ServiceError::JiraLookup {
                jira_key: jira_key.into(),
                message,
            })
    }

    fn clear_create_attempts(
        &self,
        change_set_id: &str,
        state: &mut ComposerState,
        create_attempts: &[String],
        attempt_owner: &str,
        expected_revision: i64,
    ) -> Result<(), ServiceError> {
        for ticket_id in create_attempts {
            dispatch(
                state,
                ComposerAction::ResolveCreateAttempt {
                    change_set_id: change_set_id.into(),
                    id: ticket_id.clone(),
                },
            )?;
        }
        dispatch(
            state,
            ComposerAction::ReleaseSubmissionClaim {
                change_set_id: change_set_id.into(),
                owner_id: attempt_owner.into(),
            },
        )?;
        self.save(
            change_set_id,
            state.active_set().expect("opened change set is present"),
            Some(expected_revision),
        )?;
        Ok(())
    }

    fn release_recovered_claim(
        &self,
        change_set_id: &str,
        expected_revision: i64,
        owner_id: String,
    ) -> Result<ChangeSetPatchResponse, ServiceError> {
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let mut state = open_state(versioned.change_set);
        dispatch(
            &mut state,
            ComposerAction::ReleaseSubmissionClaim {
                change_set_id: change_set_id.into(),
                owner_id,
            },
        )?;
        let reconciled = state.active_set().expect("opened change set is present");
        let (revision, catalog_revision) =
            self.save(change_set_id, reconciled, Some(expected_revision))?;
        Ok(ChangeSetPatchResponse {
            change_set: Versioned {
                revision,
                value: change_set_view(reconciled.clone()),
            },
            catalog_revision,
            applied: Vec::new(),
        })
    }

    fn save(
        &self,
        id: &str,
        change_set: &ChangeSet,
        expected_revision: Option<i64>,
    ) -> Result<(i64, i64), ServiceError> {
        match self
            .runtime
            .block_on(
                self.storage
                    .save_change_set_if_revision(change_set, expected_revision),
            )
            .map_err(storage_error)?
        {
            ConditionalSaveChangeSetOutcome::Saved {
                change_set_revision,
                catalog_revision,
            } => Ok((change_set_revision, catalog_revision)),
            ConditionalSaveChangeSetOutcome::Conflict => Err(ServiceError::StaleRevision {
                change_set_id: id.into(),
            }),
        }
    }
}

fn open_state(change_set: ChangeSet) -> ComposerState {
    let id = change_set.id.clone();
    let mut state = ComposerState::from_change_sets(vec![change_set]);
    state
        .dispatch(ComposerAction::OpenChangeSet(id))
        .expect("change set opens");
    state
}

fn active_id(state: &ComposerState) -> String {
    state.active_set().expect("change set opens").id.clone()
}
fn explicit_placement(
    state: &ComposerState,
    parent_ticket_id: Option<String>,
) -> Result<PlacementTarget, ServiceError> {
    let Some(parent_ticket_id) = parent_ticket_id else {
        return Ok(PlacementTarget::Root);
    };
    let parent = change(state, &parent_ticket_id)?;
    if parent.kind == ChangeKind::Deleted {
        return Err(invalid("ticket staged for deletion cannot be a parent"));
    }
    if !parent.is_submitted() {
        editable_change(state, &parent_ticket_id)?;
    }
    Ok(PlacementTarget::ChildOf(parent_ticket_id))
}
fn has_ticket(state: &ComposerState, ticket_id: &str) -> bool {
    state
        .active_set()
        .is_some_and(|set| set.tickets.iter().any(|change| change.id == ticket_id))
}
fn has_ticket_identity(state: &ComposerState, ticket_id: &str) -> bool {
    state.active_set().is_some_and(|set| {
        set.tickets
            .iter()
            .any(|change| change.matches_ticket_identity(ticket_id))
    })
}
fn change<'a>(state: &'a ComposerState, ticket_id: &str) -> Result<&'a TicketChange, ServiceError> {
    state
        .active_set()
        .and_then(|set| set.tickets.iter().find(|change| change.id == ticket_id))
        .ok_or_else(|| ServiceError::NotFound {
            resource: "ticket".into(),
            id: ticket_id.into(),
        })
}
fn editable_change<'a>(
    state: &'a ComposerState,
    ticket_id: &str,
) -> Result<&'a TicketChange, ServiceError> {
    let change = change(state, ticket_id)?;
    if change.is_submitted() {
        return Err(ServiceError::SubmittedTicket {
            ticket_id: ticket_id.into(),
        });
    }
    if change.kind == ChangeKind::Deleted {
        return Err(invalid("ticket staged for deletion is not editable"));
    }
    Ok(change)
}
fn selectable_change<'a>(
    state: &'a ComposerState,
    ticket_id: &str,
) -> Result<&'a TicketChange, ServiceError> {
    let change = change(state, ticket_id)?;
    if change.is_submitted() {
        return Err(ServiceError::SubmittedTicket {
            ticket_id: ticket_id.into(),
        });
    }
    if state
        .active_set()
        .is_some_and(|set| set.submission_attempt.is_some())
    {
        return Err(ServiceError::SubmissionClaimed {
            ticket_id: ticket_id.into(),
        });
    }
    Ok(change)
}
fn dispatch(state: &mut ComposerState, action: ComposerAction) -> Result<(), ServiceError> {
    state.dispatch(action).map_err(placement_error)
}
fn select(state: &mut ComposerState, ticket_id: &str) -> Result<(), ServiceError> {
    editable_change(state, ticket_id)?;
    dispatch(state, ComposerAction::SelectTicket(Some(ticket_id.into())))
}
fn update_title(
    state: &mut ComposerState,
    ticket_id: &str,
    title: String,
) -> Result<(), ServiceError> {
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::UpdateTitle(title))
}
fn update_description(
    state: &mut ComposerState,
    ticket_id: &str,
    description: String,
) -> Result<(), ServiceError> {
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::UpdateDescription(description))
}
fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOperation {
        message: message.into(),
    }
}
fn validate_submit_selection(selected_ticket_ids: &[String]) -> Result<(), ServiceError> {
    if selected_ticket_ids.is_empty() {
        return Err(invalid("submission selection must not be empty"));
    }
    if selected_ticket_ids.iter().any(String::is_empty) {
        return Err(invalid("submission selection contains an empty ticket ID"));
    }
    if selected_ticket_ids.len()
        != selected_ticket_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
    {
        return Err(invalid(
            "submission selection contains duplicate ticket IDs",
        ));
    }
    Ok(())
}
fn validate_submission_outcomes(
    changes: &[TicketChange],
    outcomes: &[crate::jira::TicketSubmitOutcome],
) -> Result<(), ServiceError> {
    let expected = changes
        .iter()
        .map(|change| change.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let actual = outcomes
        .iter()
        .map(|outcome| outcome.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if actual != expected || actual.len() != outcomes.len() {
        return Err(ServiceError::Submission {
            message: "Jira returned incomplete submission results".into(),
        });
    }
    Ok(())
}
fn placement_error(error: PlacementError) -> ServiceError {
    invalid(error.to_string())
}
fn storage_error(error: Box<dyn std::error::Error>) -> ServiceError {
    ServiceError::Storage {
        message: error.to_string(),
    }
}

fn versioned_change_set_view(change_set: VersionedChangeSet) -> Versioned<ChangeSetView> {
    Versioned {
        revision: change_set.revision,
        value: change_set_view(change_set.change_set),
    }
}
fn change_set_view(change_set: ChangeSet) -> ChangeSetView {
    let selected = change_set.selected_ticket_ids.clone();
    let submission_claimed = change_set.submission_attempt.is_some();
    ChangeSetView {
        id: change_set.id,
        name: change_set.name,
        closed: change_set.closed,
        selected_ticket_ids: selected.clone(),
        tickets: change_set
            .tickets
            .into_iter()
            .map(|change| TicketChangeView {
                selected_for_commit: selected.contains(&change.id),
                id: change.id,
                kind: change.kind.into(),
                original: change.original.map(Into::into),
                updated: change.updated.map(Into::into),
                submitted: change.submitted.is_some(),
                retry_blocked: change.retry_blocked,
                create_attempt: change.create_attempt,
                submission_claimed,
            })
            .collect(),
    }
}
impl From<TicketKindView> for TicketKind {
    fn from(value: TicketKindView) -> Self {
        match value {
            TicketKindView::Epic => Self::Epic,
            TicketKindView::Story => Self::Story,
            TicketKindView::Task => Self::Task,
            TicketKindView::Bug => Self::Bug,
            TicketKindView::Subtask => Self::Subtask,
        }
    }
}
impl From<TicketKind> for TicketKindView {
    fn from(value: TicketKind) -> Self {
        match value {
            TicketKind::Epic => Self::Epic,
            TicketKind::Story => Self::Story,
            TicketKind::Task => Self::Task,
            TicketKind::Bug => Self::Bug,
            TicketKind::Subtask => Self::Subtask,
        }
    }
}
impl From<ChangeKind> for ChangeKindView {
    fn from(value: ChangeKind) -> Self {
        match value {
            ChangeKind::Added => Self::Added,
            ChangeKind::Modified => Self::Modified,
            ChangeKind::Deleted => Self::Deleted,
            ChangeKind::Synced => Self::Synced,
        }
    }
}
impl From<Ticket> for TicketView {
    fn from(value: Ticket) -> Self {
        Self {
            key: value.key,
            project_key: value.project_key,
            title: value.title,
            description: value.description,
            description_safe_to_overwrite: value.description_safe_to_overwrite,
            description_overwrite_warning: value.description_overwrite_warning,
            kind: value.kind.into(),
            status: value.status,
            priority: value.priority,
            assignee: value.assignee,
            parent_key: value.parent_key,
            parent_title: value.parent_title,
            parent_kind: value.parent_kind.map(Into::into),
            has_children: value.has_children,
        }
    }
}

#[cfg(test)]
pub(crate) fn test_service(
    storage: Storage,
    runtime: Arc<Runtime>,
    jira: Arc<dyn JiraTicketLookup>,
) -> ComposerService {
    test_service_with_submit(
        storage,
        runtime,
        jira,
        Arc::new(|_: &[TicketChange], _| {
            crate::jira::SubmitBatchOutcome::PreflightError("test submitter".into())
        }),
    )
}

#[cfg(test)]
pub(crate) fn test_service_with_submit(
    storage: Storage,
    runtime: Arc<Runtime>,
    jira: Arc<dyn JiraTicketLookup>,
    jira_submit: Arc<dyn JiraTicketSubmit>,
) -> ComposerService {
    ComposerService::new(storage, runtime, jira, jira_submit)
}
