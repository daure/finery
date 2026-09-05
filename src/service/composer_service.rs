use std::{
    collections::HashMap,
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use reqwest::Url;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tuicore::{MermaidRasterOptions, MermaidRenderer, theme};

use crate::{
    service::composer_attachments::{
        ATTACHMENT_BYTES_LIMIT, AttachmentRequest, AttachmentView, ResolvedAttachment,
        image_mime_type_for_filename, resolve_attachments,
    },
    storage::{
        ConditionalDeleteChangeSetOutcome, ConditionalSaveChangeSetOutcome, Storage,
        VersionedChangeSet,
    },
    store::composer::{
        ChangeKind, ChangeSet, ComposerAction, ComposerState, PlacementError, PlacementTarget,
        SubmissionAttemptPhase, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
        TicketWebLink, rebase_ticket, submission_attempt_owner,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Versioned<T> {
    pub revision: i64,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetCatalogView {
    pub change_sets: Vec<Versioned<ChangeSetCatalogChangeSetView>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSetCatalogChangeSetView {
    pub id: String,
    pub name: String,
    pub closed: bool,
    pub selected_ticket_ids: Vec<String>,
    pub tickets: Vec<CatalogTicketChangeView>,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogTicketChangeView {
    pub id: String,
    pub kind: ChangeKindView,
    pub original: Option<CatalogTicketView>,
    pub updated: Option<CatalogTicketView>,
    pub submitted: bool,
    pub selected_for_commit: bool,
    pub retry_blocked: bool,
    pub create_attempt: bool,
    pub submission_claimed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogTicketView {
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
    pub assignee_account_id: String,
    pub story_points: Option<f64>,
    pub fix_versions: Vec<String>,
    pub labels: Vec<String>,
    pub parent_key: Option<String>,
    pub parent_title: Option<String>,
    pub parent_kind: Option<TicketKindView>,
    pub has_children: bool,
    pub has_attachments: bool,
    pub has_web_links: bool,
    pub has_issue_links: bool,
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
    pub assignee_account_id: String,
    pub story_points: Option<f64>,
    pub fix_versions: Vec<String>,
    pub labels: Vec<String>,
    pub parent_key: Option<String>,
    pub parent_title: Option<String>,
    pub parent_kind: Option<TicketKindView>,
    pub has_children: bool,
    pub attachments: Vec<AttachmentView>,
    pub mermaid_diagrams: Vec<MermaidDiagramView>,
    pub web_links: Vec<WebLinkView>,
    pub issue_links: Vec<IssueLinkView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MermaidDiagramView {
    pub id: String,
    pub title: String,
    pub diagram_type: String,
    pub markup: String,
    pub rendered: bool,
    pub rendered_theme: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebLinkView {
    pub id: String,
    pub global_id: Option<String>,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IssueLinkView {
    pub id: String,
    pub relationship: String,
    pub target_key: String,
    pub target_title: String,
    pub outward: bool,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DraftTicketInput {
    pub title: String,
    pub project_key: String,
    pub kind: TicketKindView,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub story_points: Option<f64>,
    #[serde(default)]
    pub fix_versions: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<AssigneeInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssigneeInput {
    pub name: String,
    pub account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentSourceInput {
    FilePath { path: String },
    Url { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    UpdateStoryPoints {
        ticket_id: String,
        story_points: Option<f64>,
    },
    UpdateFixVersions {
        ticket_id: String,
        fix_versions: Vec<String>,
    },
    UpdateLabels {
        ticket_id: String,
        labels: Vec<String>,
    },
    UpdateAssignee {
        ticket_id: String,
        assignee: Option<AssigneeInput>,
    },
    AddWebLink {
        ticket_id: String,
        link_id: String,
        title: String,
        url: String,
    },
    UpdateWebLink {
        ticket_id: String,
        link_id: String,
        title: String,
        url: String,
    },
    RemoveWebLink {
        ticket_id: String,
        link_id: String,
    },
    AddIssueLink {
        ticket_id: String,
        relationship: String,
        target_key: String,
        target_title: String,
        outward: bool,
    },
    RemoveIssueLink {
        ticket_id: String,
        link_id: String,
    },
    AddAttachment {
        ticket_id: String,
        filename: Option<String>,
        mime_type: Option<String>,
        source: AttachmentSourceInput,
    },
    RemoveAttachment {
        ticket_id: String,
        attachment_id: String,
    },
    AddMermaidDiagram {
        ticket_id: String,
        title: String,
        diagram_type: String,
        markup: String,
    },
    UpdateMermaidDiagramTitle {
        ticket_id: String,
        diagram_id: String,
        title: String,
    },
    UpdateMermaidDiagramMarkup {
        ticket_id: String,
        diagram_id: String,
        markup: String,
    },
    RemoveMermaidDiagram {
        ticket_id: String,
        diagram_id: String,
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

    pub fn change_set_catalog(
        &self,
        include_closed: bool,
    ) -> Result<Versioned<ChangeSetCatalogView>, ServiceError> {
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
                    .filter(|change_set| include_closed || !change_set.change_set.closed)
                    .map(versioned_change_set_catalog_view)
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

    pub(crate) fn attachment_sources(
        &self,
        change_set_id: &str,
        expected_revision: i64,
        requests: &[AttachmentRequest],
    ) -> Result<Vec<Result<ResolvedAttachment, String>>, ServiceError> {
        let versioned = self.load_change_set(change_set_id)?;
        if versioned.revision != expected_revision {
            return Err(ServiceError::StaleRevision {
                change_set_id: change_set_id.into(),
            });
        }
        Ok(resolve_attachments(&versioned.change_set, requests))
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

    #[cfg(test)]
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

    #[cfg(test)]
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
        self.change_set_catalog(true)
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
        validate_patch_descriptions(&state)?;
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
        abandoned_creates: Vec<String>,
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
        let confirmed_create_ids = recovered_creates
            .iter()
            .map(|recovered| recovered.ticket_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let abandoned_create_ids = abandoned_creates
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        if confirmed_create_ids.len() != recovered_creates.len()
            || abandoned_create_ids.len() != abandoned_creates.len()
            || confirmed_create_ids
                .intersection(&abandoned_create_ids)
                .next()
                .is_some()
            || confirmed_create_ids.len() + abandoned_create_ids.len() != creates.len()
            || recovered_creates.iter().any(|recovered| {
                !creates.contains(&recovered.ticket_id) || recovered.jira_key.trim().is_empty()
            })
            || abandoned_creates
                .iter()
                .any(|ticket_id| !creates.contains(ticket_id))
        {
            return Err(invalid(
                "confirm every claimed draft create with its Jira key or as absent before recovery",
            ));
        }
        let recovered_creates = recovered_creates
            .into_iter()
            .map(|recovered| (recovered.ticket_id, recovered.jira_key))
            .collect::<HashMap<_, _>>();
        for change in claimed {
            if change.kind == ChangeKind::Added && abandoned_create_ids.contains(change.id.as_str())
            {
                dispatch(
                    &mut state,
                    ComposerAction::ResolveCreateAttempt {
                        change_set_id: change_set_id.into(),
                        id: change.id,
                    },
                )?;
                continue;
            }
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
        update_title: Option<String>,
        allow_unsafe_description_overwrite: bool,
    ) -> Result<SubmitChangeSetResponse, ServiceError> {
        validate_submit_selection(&selected_ticket_ids)?;
        if update_title
            .as_ref()
            .is_some_and(|title| title.trim().is_empty())
        {
            return Err(invalid("change set name must not be empty"));
        }
        let versioned = self.load_expected_change_set(change_set_id, expected_revision)?;
        let mut state = open_state(versioned.change_set);
        if let Some(title) = update_title {
            dispatch(&mut state, ComposerAction::RenameChangeSet(title))?;
        }
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
                let catalog = self.change_set_catalog(true)?.revision;
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
                let catalog = self.change_set_catalog(true)?.revision;
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
                update_story_points(state, &ticket_id, draft.story_points)?;
                update_fix_versions(state, &ticket_id, draft.fix_versions)?;
                update_labels(state, &ticket_id, draft.labels)?;
                if let Some(assignee) = draft.assignee {
                    update_assignee(state, &ticket_id, Some(assignee))?;
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
            ChangeSetPatchOperation::UpdateStoryPoints {
                ticket_id,
                story_points,
            } => {
                update_story_points(state, &ticket_id, story_points)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateFixVersions {
                ticket_id,
                fix_versions,
            } => {
                update_fix_versions(state, &ticket_id, fix_versions)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateLabels { ticket_id, labels } => {
                update_labels(state, &ticket_id, labels)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateAssignee {
                ticket_id,
                assignee,
            } => {
                update_assignee(state, &ticket_id, assignee)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::AddWebLink {
                ticket_id,
                link_id,
                title,
                url,
            } => {
                add_web_link(state, &ticket_id, link_id, title, url)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateWebLink {
                ticket_id,
                link_id,
                title,
                url,
            } => {
                update_web_link(state, &ticket_id, link_id, title, url)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RemoveWebLink { ticket_id, link_id } => {
                remove_web_link(state, &ticket_id, link_id)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::AddIssueLink {
                ticket_id,
                relationship,
                target_key,
                target_title,
                outward,
            } => {
                add_issue_link(
                    state,
                    &ticket_id,
                    relationship,
                    target_key,
                    target_title,
                    outward,
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RemoveIssueLink { ticket_id, link_id } => {
                remove_issue_link(state, &ticket_id, link_id)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::AddAttachment {
                ticket_id,
                filename,
                mime_type,
                source,
            } => {
                let (source_filename, data) = read_attachment(&source)?;
                let (filename, mime_type, data) =
                    validate_attachment(filename.unwrap_or(source_filename), mime_type, data)?;
                select(state, &ticket_id)?;
                dispatch(
                    state,
                    ComposerAction::AddAttachment {
                        filename,
                        mime_type,
                        data,
                    },
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RemoveAttachment {
                ticket_id,
                attachment_id,
            } => {
                remove_attachment(state, &ticket_id, &attachment_id)?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::AddMermaidDiagram {
                ticket_id,
                title,
                diagram_type,
                markup,
            } => {
                let (rendered_png, rendered_theme) =
                    render_mermaid_diagram(&title, &diagram_type, &markup)?;
                select(state, &ticket_id)?;
                dispatch(
                    state,
                    ComposerAction::AddMermaidDiagram {
                        title,
                        diagram_type,
                        markup,
                        rendered_png,
                        rendered_theme,
                    },
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateMermaidDiagramTitle {
                ticket_id,
                diagram_id,
                title,
            } => {
                let index = mermaid_diagram_index(state, &ticket_id, &diagram_id)?;
                if title.trim().is_empty() {
                    return Err(invalid("diagram title must not be empty"));
                }
                select_mermaid_diagram(state, &ticket_id, index)?;
                dispatch(state, ComposerAction::RenameSelectedMermaidDiagram(title))?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::UpdateMermaidDiagramMarkup {
                ticket_id,
                diagram_id,
                markup,
            } => {
                let index = mermaid_diagram_index(state, &ticket_id, &diagram_id)?;
                let diagram_type = editable_ticket(state, &ticket_id)?
                    .mermaid_diagrams
                    .get(index)
                    .map(|diagram| diagram.diagram_type.as_str())
                    .ok_or_else(|| ServiceError::NotFound {
                        resource: "mermaid diagram".into(),
                        id: diagram_id.clone(),
                    })?;
                let (rendered_png, rendered_theme) =
                    render_mermaid_diagram("diagram", diagram_type, &markup)?;
                select_mermaid_diagram(state, &ticket_id, index)?;
                dispatch(
                    state,
                    ComposerAction::UpdateSelectedMermaidDiagramMarkup {
                        markup,
                        rendered_png,
                        rendered_theme,
                    },
                )?;
                Ok(vec![ticket_id])
            }
            ChangeSetPatchOperation::RemoveMermaidDiagram {
                ticket_id,
                diagram_id,
            } => {
                let index = mermaid_diagram_index(state, &ticket_id, &diagram_id)?;
                select_mermaid_diagram(state, &ticket_id, index)?;
                dispatch(state, ComposerAction::RemoveSelectedMermaidDiagram)?;
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
fn update_story_points(
    state: &mut ComposerState,
    ticket_id: &str,
    story_points: Option<f64>,
) -> Result<(), ServiceError> {
    if story_points.is_some_and(|points| !points.is_finite() || points < 0.0) {
        return Err(invalid("story points must be a finite non-negative number"));
    }
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::UpdateStoryPoints(story_points))
}
fn update_fix_versions(
    state: &mut ComposerState,
    ticket_id: &str,
    fix_versions: Vec<String>,
) -> Result<(), ServiceError> {
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::UpdateFixVersions(clean_values(fix_versions)),
    )
}
fn update_labels(
    state: &mut ComposerState,
    ticket_id: &str,
    labels: Vec<String>,
) -> Result<(), ServiceError> {
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::UpdateLabels(clean_values(labels)))
}
fn update_assignee(
    state: &mut ComposerState,
    ticket_id: &str,
    assignee: Option<AssigneeInput>,
) -> Result<(), ServiceError> {
    let (name, account_id) = match assignee {
        Some(assignee) => {
            let name = assignee.name.trim();
            let account_id = assignee.account_id.trim();
            if name.is_empty() || account_id.is_empty() {
                return Err(invalid("assignee name and account ID must not be empty"));
            }
            (name.into(), account_id.into())
        }
        None => ("Unassigned".into(), String::new()),
    };
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::UpdateAssignee { name, account_id })
}

fn add_web_link(
    state: &mut ComposerState,
    ticket_id: &str,
    link_id: String,
    title: String,
    url: String,
) -> Result<(), ServiceError> {
    if !link_id.starts_with("local-") || link_id.trim() == "local-" {
        return Err(invalid("new web-link IDs must start with local-"));
    }
    let (title, url) = validate_web_link(title, url)?;
    let change = editable_change(state, ticket_id)?;
    if change
        .updated
        .as_ref()
        .or(change.original.as_ref())
        .is_some_and(|ticket| ticket.web_links.iter().any(|link| link.id == link_id))
    {
        return Err(ServiceError::AlreadyExists {
            resource: "web link".into(),
            id: link_id,
        });
    }
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::AddWebLink {
            id: link_id,
            title,
            url,
        },
    )
}

fn update_web_link(
    state: &mut ComposerState,
    ticket_id: &str,
    link_id: String,
    title: String,
    url: String,
) -> Result<(), ServiceError> {
    let (title, url) = validate_web_link(title, url)?;
    ensure_web_link(state, ticket_id, &link_id)?;
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::UpdateWebLink {
            id: link_id,
            title,
            url,
        },
    )
}

fn remove_web_link(
    state: &mut ComposerState,
    ticket_id: &str,
    link_id: String,
) -> Result<(), ServiceError> {
    ensure_web_link(state, ticket_id, &link_id)?;
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::RemoveWebLink(link_id))
}

fn ensure_web_link(
    state: &ComposerState,
    ticket_id: &str,
    link_id: &str,
) -> Result<(), ServiceError> {
    let change = editable_change(state, ticket_id)?;
    let ticket = change.updated.as_ref().or(change.original.as_ref());
    if ticket.is_some_and(|ticket| ticket.web_links.iter().any(|link| link.id == link_id)) {
        Ok(())
    } else {
        Err(ServiceError::NotFound {
            resource: "web link".into(),
            id: link_id.into(),
        })
    }
}

fn add_issue_link(
    state: &mut ComposerState,
    ticket_id: &str,
    relationship: String,
    target_key: String,
    target_title: String,
    outward: bool,
) -> Result<(), ServiceError> {
    if relationship.trim().is_empty() || target_key.trim().is_empty() {
        return Err(invalid("issue links need a relationship and target ticket"));
    }
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::AddIssueLink {
            relationship: relationship.trim().into(),
            target_key: target_key.trim().into(),
            target_title: target_title.trim().into(),
            outward,
        },
    )
}

fn remove_issue_link(
    state: &mut ComposerState,
    ticket_id: &str,
    link_id: String,
) -> Result<(), ServiceError> {
    let change = editable_change(state, ticket_id)?;
    let ticket = change.updated.as_ref().or(change.original.as_ref());
    if !ticket.is_some_and(|ticket| ticket.issue_links.iter().any(|link| link.id == link_id)) {
        return Err(ServiceError::NotFound {
            resource: "issue link".into(),
            id: link_id,
        });
    }
    select(state, ticket_id)?;
    dispatch(state, ComposerAction::RemoveIssueLink(link_id))
}

pub(crate) fn validate_web_link(
    title: String,
    url: String,
) -> Result<(String, String), ServiceError> {
    let title = title.trim().to_owned();
    let url = url.trim();
    if title.is_empty() {
        return Err(invalid("web-link title must not be empty"));
    }
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("https://{url}")
    };
    let parsed = Url::parse(&url).map_err(|_| invalid("web-link URL is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid("web-link URL must use http or https"));
    }
    let valid_host = parsed.host_str().is_some_and(|host| {
        host.contains('.')
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
                    && label
                        .chars()
                        .next()
                        .is_some_and(|character| character.is_ascii_alphanumeric())
                    && label
                        .chars()
                        .last()
                        .is_some_and(|character| character.is_ascii_alphanumeric())
            })
    });
    if !valid_host {
        return Err(invalid(
            "web-link URL must contain a valid domain with at least one dot",
        ));
    }
    Ok((title, url))
}
fn read_attachment(source: &AttachmentSourceInput) -> Result<(String, Vec<u8>), ServiceError> {
    match source {
        AttachmentSourceInput::FilePath { path } => read_attachment_file(path),
        AttachmentSourceInput::Url { url } => download_attachment(url),
    }
}

fn read_attachment_file(path: &str) -> Result<(String, Vec<u8>), ServiceError> {
    let path = PathBuf::from(path);
    let metadata = fs::metadata(&path)
        .map_err(|error| invalid(format!("could not inspect attachment file: {error}")))?;
    if !metadata.is_file() {
        return Err(invalid("attachment file must be a regular file"));
    }
    if metadata.len() > ATTACHMENT_BYTES_LIMIT as u64 {
        return Err(invalid(format!(
            "attachment must not exceed {ATTACHMENT_BYTES_LIMIT} bytes"
        )));
    }
    let filename = filename_from_path(&path)?;
    let data = fs::read(path)
        .map_err(|error| invalid(format!("could not read attachment file: {error}")))?;
    Ok((filename, data))
}

fn download_attachment(url: &str) -> Result<(String, Vec<u8>), ServiceError> {
    let url = Url::parse(url).map_err(|_| invalid("attachment URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid("attachment URL must use http or https"));
    }
    let filename = filename_from_url(&url);
    let response = reqwest::blocking::Client::new()
        .get(url)
        .send()
        .map_err(|error| invalid(format!("could not download attachment: {error}")))?
        .error_for_status()
        .map_err(|error| invalid(format!("could not download attachment: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > ATTACHMENT_BYTES_LIMIT as u64)
    {
        return Err(invalid(format!(
            "attachment must not exceed {ATTACHMENT_BYTES_LIMIT} bytes"
        )));
    }
    let mut data = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(ATTACHMENT_BYTES_LIMIT),
    );
    response
        .take(ATTACHMENT_BYTES_LIMIT.saturating_add(1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| invalid(format!("could not download attachment: {error}")))?;
    Ok((filename, data))
}

fn filename_from_path(path: &Path) -> Result<String, ServiceError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid("attachment file path must include a UTF-8 filename"))
}

fn filename_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or("attachment")
        .to_owned()
}

fn validate_attachment(
    filename: String,
    mime_type: Option<String>,
    data: Vec<u8>,
) -> Result<(String, Option<String>, Vec<u8>), ServiceError> {
    let filename = filename.trim();
    if filename.is_empty()
        || filename.len() > 255
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(invalid(
            "attachment filename must be 1-255 characters without paths or control characters",
        ));
    }
    if data.is_empty() {
        return Err(invalid("attachment content must not be empty"));
    }
    if data.len() > ATTACHMENT_BYTES_LIMIT {
        return Err(invalid(format!(
            "attachment must not exceed {ATTACHMENT_BYTES_LIMIT} bytes"
        )));
    }
    let declared_mime_type = mime_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !declared_mime_type.is_empty()
        && (declared_mime_type.len() > 255 || !valid_mime_type(&declared_mime_type))
    {
        return Err(invalid("attachment MIME type is invalid"));
    }
    let declared_mime_type = if declared_mime_type == "image/x-icon" {
        "image/vnd.microsoft.icon"
    } else {
        declared_mime_type.as_str()
    };
    let detected_image_mime_type = super::image_mime_type(&data);
    let filename_image_mime_type = image_mime_type_for_filename(filename);
    if detected_image_mime_type.is_some()
        || declared_mime_type.starts_with("image/")
        || filename_image_mime_type.is_some()
    {
        let detected_mime_type = detected_image_mime_type
            .ok_or_else(|| invalid("attachment content is not a supported image"))?;
        if !declared_mime_type.is_empty() && declared_mime_type != detected_mime_type {
            return Err(invalid(format!(
                "attachment MIME type does not match its content; detected {detected_mime_type}"
            )));
        }
        if filename_image_mime_type != Some(detected_mime_type) {
            return Err(invalid(format!(
                "attachment filename extension does not match {detected_mime_type} content"
            )));
        }
    }
    let mime_type = if declared_mime_type.is_empty() {
        detected_image_mime_type.map(str::to_owned)
    } else {
        Some(declared_mime_type.into())
    };
    Ok((filename.into(), mime_type, data))
}
fn valid_mime_type(mime_type: &str) -> bool {
    let Some((top_level, subtype)) = mime_type.split_once('/') else {
        return false;
    };
    !top_level.is_empty()
        && !subtype.is_empty()
        && [top_level, subtype].into_iter().all(|part| {
            part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
        })
}
fn remove_attachment(
    state: &mut ComposerState,
    ticket_id: &str,
    attachment_id: &str,
) -> Result<(), ServiceError> {
    let change = editable_change(state, ticket_id)?;
    let ticket = change
        .updated
        .as_ref()
        .or(change.original.as_ref())
        .ok_or_else(|| invalid("ticket has no attachment snapshot"))?;
    let (index, attachment) = ticket
        .attachments
        .iter()
        .enumerate()
        .find(|(_, attachment)| attachment.id == attachment_id)
        .ok_or_else(|| ServiceError::NotFound {
            resource: "attachment".into(),
            id: attachment_id.into(),
        })?;
    if attachment.change == crate::store::composer::AttachmentChangeKind::Deleted {
        return Err(invalid("attachment is already staged for deletion"));
    }
    let locally_added = attachment.change == crate::store::composer::AttachmentChangeKind::Added;
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::SelectTicket(Some(format!("{ticket_id}:attachment:{index}"))),
    )?;
    dispatch(
        state,
        if locally_added {
            ComposerAction::RemoveSelectedAttachment
        } else {
            ComposerAction::DeleteSelectedAttachment
        },
    )
}

fn editable_ticket<'a>(
    state: &'a ComposerState,
    ticket_id: &str,
) -> Result<&'a Ticket, ServiceError> {
    let change = editable_change(state, ticket_id)?;
    change
        .updated
        .as_ref()
        .or(change.original.as_ref())
        .ok_or_else(|| invalid("ticket has no editable snapshot"))
}

fn mermaid_diagram_index(
    state: &ComposerState,
    ticket_id: &str,
    diagram_id: &str,
) -> Result<usize, ServiceError> {
    editable_ticket(state, ticket_id)?
        .mermaid_diagrams
        .iter()
        .position(|diagram| diagram.id == diagram_id)
        .ok_or_else(|| ServiceError::NotFound {
            resource: "mermaid diagram".into(),
            id: diagram_id.into(),
        })
}

fn select_mermaid_diagram(
    state: &mut ComposerState,
    ticket_id: &str,
    index: usize,
) -> Result<(), ServiceError> {
    select(state, ticket_id)?;
    dispatch(
        state,
        ComposerAction::SelectTicket(Some(format!("{ticket_id}:diagram:{index}"))),
    )
}

fn render_mermaid_diagram(
    title: &str,
    diagram_type: &str,
    markup: &str,
) -> Result<(Vec<u8>, String), ServiceError> {
    if title.trim().is_empty() {
        return Err(invalid("diagram title must not be empty"));
    }
    if diagram_type.trim().is_empty() {
        return Err(invalid("diagram type must not be empty"));
    }
    if markup.trim().is_empty() {
        return Err(invalid("diagram markup must not be empty"));
    }
    let active_theme = theme();
    MermaidRenderer::new()
        .render_png_with_theme(markup, &MermaidRasterOptions::default(), &active_theme)
        .map(|png| (png, active_theme.name().id().to_owned()))
        .map_err(|error| invalid(format!("invalid Mermaid diagram: {error}")))
}
fn clean_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}
fn validate_patch_descriptions(state: &ComposerState) -> Result<(), ServiceError> {
    let Some(change_set) = state.active_set() else {
        return Ok(());
    };
    let errors = change_set
        .tickets
        .iter()
        .filter_map(|change| {
            let desired = change.updated.as_ref()?;
            let changed = change
                .original
                .as_ref()
                .is_none_or(|original| original.description != desired.description);
            changed
                .then(|| {
                    crate::store::composer::jira_adf::validate_markdown(&desired.description)
                        .err()
                        .map(|error| format!("{}: {error}", change.id))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(invalid(errors.join("\n")))
    }
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
fn versioned_change_set_catalog_view(
    change_set: VersionedChangeSet,
) -> Versioned<ChangeSetCatalogChangeSetView> {
    Versioned {
        revision: change_set.revision,
        value: change_set_catalog_view(change_set.change_set),
    }
}
fn change_set_catalog_view(change_set: ChangeSet) -> ChangeSetCatalogChangeSetView {
    let selected = change_set.selected_ticket_ids.clone();
    let submission_claimed = change_set.submission_attempt.is_some();
    let tickets = change_set
        .tickets
        .into_iter()
        .map(|change| CatalogTicketChangeView {
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
        .collect::<Vec<_>>();
    let has_attachments = tickets.iter().any(|change| {
        change
            .original
            .as_ref()
            .is_some_and(|ticket| ticket.has_attachments)
            || change
                .updated
                .as_ref()
                .is_some_and(|ticket| ticket.has_attachments)
    });
    ChangeSetCatalogChangeSetView {
        id: change_set.id,
        name: change_set.name,
        closed: change_set.closed,
        selected_ticket_ids: selected,
        tickets,
        has_attachments,
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
            assignee_account_id: value.assignee_account_id,
            story_points: value.story_points,
            fix_versions: value.fix_versions,
            labels: value.labels,
            parent_key: value.parent_key,
            parent_title: value.parent_title,
            parent_kind: value.parent_kind.map(Into::into),
            has_children: value.has_children,
            attachments: value.attachments.into_iter().map(Into::into).collect(),
            mermaid_diagrams: value
                .mermaid_diagrams
                .into_iter()
                .map(|diagram| MermaidDiagramView {
                    id: diagram.id,
                    title: diagram.title,
                    diagram_type: diagram.diagram_type,
                    markup: diagram.markup,
                    rendered: !diagram.rendered_png.is_empty(),
                    rendered_theme: diagram.rendered_theme,
                })
                .collect(),
            web_links: value.web_links.into_iter().map(Into::into).collect(),
            issue_links: value.issue_links.into_iter().map(Into::into).collect(),
        }
    }
}
impl From<Ticket> for CatalogTicketView {
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
            assignee_account_id: value.assignee_account_id,
            story_points: value.story_points,
            fix_versions: value.fix_versions,
            labels: value.labels,
            parent_key: value.parent_key,
            parent_title: value.parent_title,
            parent_kind: value.parent_kind.map(Into::into),
            has_children: value.has_children,
            has_attachments: !value.attachments.is_empty(),
            has_web_links: !value.web_links.is_empty(),
            has_issue_links: !value.issue_links.is_empty(),
        }
    }
}

impl From<TicketWebLink> for WebLinkView {
    fn from(value: TicketWebLink) -> Self {
        Self {
            id: value.id,
            global_id: value.global_id,
            title: value.title,
            url: value.url,
        }
    }
}

impl From<crate::store::composer::TicketIssueLink> for IssueLinkView {
    fn from(value: crate::store::composer::TicketIssueLink) -> Self {
        Self {
            id: value.id,
            relationship: value.relationship,
            target_key: value.target_key,
            target_title: value.target_title,
            outward: value.outward,
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
