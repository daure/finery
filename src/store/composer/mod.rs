use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::store::work_items::WorkItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TicketKind {
    Epic,
    Story,
    Task,
    Bug,
    Subtask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ticket {
    pub key: String,
    #[serde(default)]
    pub project_key: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub description_safe_to_overwrite: bool,
    #[serde(default)]
    pub description_overwrite_warning: Option<String>,
    pub kind: TicketKind,
    pub status: String,
    pub priority: String,
    pub assignee: String,
    #[serde(default)]
    pub assignee_account_id: String,
    #[serde(default)]
    pub story_points: Option<f64>,
    #[serde(default)]
    pub fix_versions: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub parent_key: Option<String>,
    #[serde(default)]
    pub parent_title: Option<String>,
    #[serde(default)]
    pub parent_kind: Option<TicketKind>,
    #[serde(default)]
    pub has_children: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TicketPresentation {
    pub work_item: WorkItem,
    pub story_points_configured: bool,
    pub assumed_story_points: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Synced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ComposerViewMode {
    Source,
    Changes,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TicketChange {
    pub id: String,
    pub original: Option<Ticket>,
    pub updated: Option<Ticket>,
    pub kind: ChangeKind,
    #[serde(default)]
    pub submitted: Option<SubmissionSnapshot>,
    #[serde(default)]
    pub retry_blocked: bool,
    #[serde(default)]
    pub create_attempt: bool,
    #[serde(default)]
    pub sibling_order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubmissionAttempt {
    pub owner_id: String,
    pub ticket_ids: Vec<String>,
    pub phase: SubmissionAttemptPhase,
}

pub(crate) fn submission_attempt_owner() -> String {
    static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SubmissionAttemptPhase {
    Claimed,
    CreateAttemptsMarked,
    JiraSubmissionStarted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlacementTarget {
    Root,
    ChildOf(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlacementError {
    UnknownTicket,
    InvalidParentKind,
    UnknownParentKind,
    Cycle,
    ClosedChangeSet,
    NotEditable,
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownTicket => "Ticket is unavailable",
            Self::InvalidParentKind => "That ticket type cannot use this parent",
            Self::UnknownParentKind => "Parent ticket type is unavailable",
            Self::Cycle => "A ticket cannot be its own ancestor",
            Self::ClosedChangeSet => "Closed change sets cannot be changed",
            Self::NotEditable => "This ticket cannot be moved",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SubmissionSnapshot {
    pub original: Option<Ticket>,
    pub updated: Option<Ticket>,
}

impl TicketChange {
    pub(crate) fn can_edit(&self, updated: bool) -> bool {
        updated && self.kind != ChangeKind::Deleted && self.submitted.is_none()
    }

    pub(crate) fn is_submitted(&self) -> bool {
        self.submitted.is_some()
    }

    pub(crate) fn matches_ticket_identity(&self, key: &str) -> bool {
        self.id == key
            || self
                .original
                .as_ref()
                .is_some_and(|ticket| ticket.key == key)
            || self
                .updated
                .as_ref()
                .is_some_and(|ticket| ticket.key == key)
            || self.submitted.as_ref().is_some_and(|snapshot| {
                snapshot
                    .original
                    .as_ref()
                    .is_some_and(|ticket| ticket.key == key)
                    || snapshot
                        .updated
                        .as_ref()
                        .is_some_and(|ticket| ticket.key == key)
            })
    }
}

impl ChangeSet {
    pub(crate) fn submission_attempt_ticket_id(&self) -> Option<&str> {
        self.submission_attempt
            .as_ref()
            .and_then(|attempt| attempt.ticket_ids.first().map(String::as_str))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChangeSet {
    pub id: String,
    pub name: String,
    pub tickets: Vec<TicketChange>,
    #[serde(default)]
    pub selected_ticket_ids: Vec<String>,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub submission_attempt: Option<SubmissionAttempt>,
}

impl ChangeSet {
    pub(crate) fn submitted_count(&self) -> usize {
        self.tickets
            .iter()
            .filter(|ticket| ticket.is_submitted())
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ComposerState {
    pub change_sets: Vec<ChangeSet>,
    pub active_change_set: Option<String>,
    pub selected_ticket: Option<String>,
    pub view_mode: ComposerViewMode,
    pub description_diff_side_by_side: bool,
    pub sources: HashMap<(String, String), Ticket>,
    presentations: HashMap<String, HashMap<String, TicketPresentation>>,
    submitting_change_sets: HashSet<String>,
    next_ticket: usize,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum ComposerAction {
    CreateChangeSet {
        id: String,
        name: String,
    },
    DeleteChangeSet(String),
    OpenChangeSet(String),
    CloseChangeSet,
    RenameChangeSet(String),
    SelectTicket(Option<String>),
    SetSelectedTickets(Vec<String>),
    SetViewMode(ComposerViewMode),
    SetDescriptionDiffSideBySide(bool),
    SetSource {
        change_set_id: String,
        id: String,
        ticket: Ticket,
    },
    SetPresentation {
        change_set_id: String,
        id: String,
        presentation: TicketPresentation,
    },
    CreateTicket {
        title: String,
        project_key: String,
    },
    CreateTicketAt {
        title: String,
        project_key: String,
        kind: TicketKind,
        placement: PlacementTarget,
    },
    CreateTicketWithId {
        id: String,
        title: String,
        project_key: String,
        kind: TicketKind,
        placement: PlacementTarget,
    },
    IncludeTicket(Ticket),
    IncludeTicketAt {
        ticket: Ticket,
        placement: PlacementTarget,
    },
    ReparentTicket {
        id: String,
        placement: PlacementTarget,
    },
    RemoveTicket(String),
    MarkTicketDeleted(String),
    RestoreTicket(String),
    ResetTicket(String),
    UpdateTitle(String),
    UpdateDescription(String),
    UpdateKind(TicketKind),
    UpdateStatus(String),
    UpdatePriority(String),
    UpdateStoryPoints(Option<f64>),
    UpdateFixVersions(Vec<String>),
    UpdateLabels(Vec<String>),
    UpdateAssignee {
        name: String,
        account_id: String,
    },
    CompleteSubmission {
        change_set_id: String,
        id: String,
        snapshot: SubmissionSnapshot,
    },
    RefreshAfterFailedSubmission {
        change_set_id: String,
        id: String,
        original: Ticket,
        updated: Ticket,
    },
    BlockTicketRetry {
        change_set_id: String,
        id: String,
    },
    MarkCreateAttempts {
        change_set_id: String,
        ids: Vec<String>,
    },
    ResolveCreateAttempt {
        change_set_id: String,
        id: String,
    },
    ClaimSubmission {
        change_set_id: String,
        ids: Vec<String>,
        owner_id: String,
    },
    MarkSubmissionCreateAttempts {
        change_set_id: String,
        owner_id: String,
    },
    MarkSubmissionJiraStarted {
        change_set_id: String,
        owner_id: String,
    },
    ReleaseSubmissionClaim {
        change_set_id: String,
        owner_id: String,
    },
}

impl ComposerState {
    pub(crate) fn from_change_sets(change_sets: Vec<ChangeSet>) -> Self {
        let next_ticket = next_ticket_id(&change_sets);
        Self {
            change_sets,
            active_change_set: None,
            selected_ticket: None,
            view_mode: ComposerViewMode::Changes,
            description_diff_side_by_side: false,
            sources: HashMap::new(),
            presentations: HashMap::new(),
            submitting_change_sets: HashSet::new(),
            next_ticket,
        }
    }

    #[cfg(test)]
    pub(crate) fn demo() -> Self {
        let tickets = demo_jira_tickets();
        Self {
            change_sets: vec![
                ChangeSet {
                    id: "CS-1".into(),
                    name: "Checkout reliability".into(),
                    closed: false,
                    tickets: vec![
                        TicketChange {
                            id: tickets[0].key.clone(),
                            original: Some(tickets[0].clone()),
                            updated: None,
                            kind: ChangeKind::Synced,
                            submitted: None,
                            retry_blocked: false,
                            create_attempt: false,
                            sibling_order: 0,
                        },
                        TicketChange {
                            id: tickets[1].key.clone(),
                            original: Some(tickets[1].clone()),
                            updated: Some(Ticket {
                                title: "Retry payment authorization safely".into(),
                                ..tickets[1].clone()
                            }),
                            kind: ChangeKind::Modified,
                            submitted: None,
                            retry_blocked: false,
                            create_attempt: false,
                            sibling_order: 1,
                        },
                        TicketChange {
                            id: tickets[2].key.clone(),
                            original: Some(tickets[2].clone()),
                            updated: None,
                            kind: ChangeKind::Deleted,
                            submitted: None,
                            retry_blocked: false,
                            create_attempt: false,
                            sibling_order: 2,
                        },
                    ],
                    selected_ticket_ids: Vec::new(),
                    submission_attempt: None,
                },
                ChangeSet {
                    id: "CS-2".into(),
                    name: "Customer notifications".into(),
                    tickets: Vec::new(),
                    selected_ticket_ids: Vec::new(),
                    closed: false,
                    submission_attempt: None,
                },
            ],
            active_change_set: None,
            selected_ticket: None,
            view_mode: ComposerViewMode::Changes,
            description_diff_side_by_side: false,
            sources: HashMap::new(),
            presentations: HashMap::new(),
            submitting_change_sets: HashSet::new(),
            next_ticket: 1,
        }
    }

    pub(crate) fn active_set(&self) -> Option<&ChangeSet> {
        let active = self.active_change_set.as_deref()?;
        self.change_sets.iter().find(|set| set.id == active)
    }

    pub(crate) fn replace_change_sets(&mut self, change_sets: Vec<ChangeSet>) {
        let active_change_set = self.active_change_set.clone();
        let selected_ticket = self.selected_ticket.clone();
        self.change_sets = change_sets;
        self.sources.clear();
        self.presentations.clear();
        self.submitting_change_sets
            .retain(|id| self.change_sets.iter().any(|set| &set.id == id));
        self.next_ticket = next_ticket_id(&self.change_sets);

        if let Some(active) = active_change_set
            && self.change_sets.iter().any(|set| set.id == active)
        {
            self.active_change_set = Some(active);
            self.selected_ticket = selected_ticket.filter(|selected| {
                self.active_set()
                    .is_some_and(|set| set.tickets.iter().any(|change| change.id == *selected))
            });
            if self.selected_ticket.is_none() {
                self.selected_ticket = self
                    .ordered_changes()
                    .into_iter()
                    .find(|change| self.ticket_for_change(change).is_some())
                    .map(|change| change.id.clone());
            }
        } else {
            self.close_change_set();
        }
    }

    pub(crate) fn begin_submission(&mut self, change_set_id: &str) -> bool {
        if self
            .change_sets
            .iter()
            .any(|set| set.id == change_set_id && set.submission_attempt.is_none())
        {
            self.submitting_change_sets.insert(change_set_id.into());
            true
        } else {
            false
        }
    }

    pub(crate) fn end_submission(&mut self, change_set_id: &str) {
        self.submitting_change_sets.remove(change_set_id);
    }

    pub(crate) fn change_set_is_submitting(&self, change_set_id: &str) -> bool {
        self.submitting_change_sets.contains(change_set_id)
            || self
                .change_sets
                .iter()
                .find(|set| set.id == change_set_id)
                .is_some_and(|set| set.submission_attempt.is_some())
    }

    fn active_change_set_is_submitting(&self) -> bool {
        self.active_change_set
            .as_deref()
            .is_some_and(|id| self.change_set_is_submitting(id))
    }

    pub(crate) fn remote_queries_allowed(&self) -> bool {
        self.active_set().is_some_and(|set| !set.closed)
    }

    pub(crate) fn has_remote_tickets(&self) -> bool {
        self.active_set().is_some_and(|set| {
            set.tickets
                .iter()
                .any(|change| !change.id.starts_with("NEW-"))
        })
    }

    pub(crate) fn selected_change(&self) -> Option<&TicketChange> {
        let selected = self.selected_ticket.as_deref()?;
        self.active_set()?
            .tickets
            .iter()
            .find(|change| change.id == selected)
    }

    pub(crate) fn selected_ticket(&self) -> Option<&Ticket> {
        self.ticket_for_change(self.selected_change()?)
    }

    pub(crate) fn selected_existing_ticket_key(&self) -> Option<String> {
        let change = self.selected_change()?;
        let ticket = self.changes_for_change(change)?;
        (!ticket.key.starts_with("NEW-")).then(|| ticket.key.clone())
    }

    pub(crate) fn selected_source(&self) -> Option<&Ticket> {
        self.source_for_change(self.selected_change()?)
    }

    pub(crate) fn source_for_change<'a>(&'a self, change: &'a TicketChange) -> Option<&'a Ticket> {
        let snapshot = || change.submitted.as_ref()?.original.as_ref();
        if !self.remote_queries_allowed() {
            return snapshot().or(change.original.as_ref());
        }
        self.active_source_for_change(change)
            .or(change.original.as_ref())
            .or_else(snapshot)
    }

    pub(crate) fn selected_changes(&self) -> Option<&Ticket> {
        self.changes_for_change(self.selected_change()?)
    }

    pub(crate) fn changes_for_change<'a>(&'a self, change: &'a TicketChange) -> Option<&'a Ticket> {
        change
            .submitted
            .as_ref()
            .and_then(|snapshot| snapshot.updated.as_ref().or(snapshot.original.as_ref()))
            .or(change.updated.as_ref())
            .or_else(|| self.active_source_for_change(change))
            .or(change.original.as_ref())
    }

    pub(crate) fn ticket_for_change<'a>(&'a self, change: &'a TicketChange) -> Option<&'a Ticket> {
        match self.view_mode {
            ComposerViewMode::Source => self
                .source_for_change(change)
                .or_else(|| self.changes_for_change(change)),
            ComposerViewMode::Changes | ComposerViewMode::Diff => self.changes_for_change(change),
        }
    }

    pub(crate) fn presentation_for_change(
        &self,
        change: &TicketChange,
    ) -> Option<&TicketPresentation> {
        self.active_change_set
            .as_ref()
            .and_then(|change_set_id| self.presentations.get(change_set_id))
            .and_then(|presentations| presentations.get(&change.id))
    }

    pub(crate) fn selected_is_editable(&self) -> bool {
        self.selected_change().is_some_and(|change| {
            self.view_mode == ComposerViewMode::Changes
                && !self.active_change_set_is_submitting()
                && change.can_edit(true)
        })
    }

    pub(crate) fn selected_parent_id(&self) -> Option<String> {
        let ticket = self.selected_ticket()?;
        let parent = ticket.parent_key.as_deref()?;
        self.active_set()
            .and_then(|set| {
                set.tickets.iter().find(|change| {
                    change.id == parent
                        || self
                            .changes_for_change(change)
                            .is_some_and(|candidate| candidate.key == parent)
                })
            })
            .map(|change| change.id.clone())
            .or_else(|| Some(parent.into()))
    }

    #[allow(dead_code)]
    pub(crate) fn legal_child_kinds(&self, parent_id: Option<&str>) -> Vec<TicketKind> {
        let parent = parent_id.map(|id| self.parent_kind_for(id));
        parent
            .map(|kind| kind.map_or_else(Vec::new, |kind| allowed_child_kinds(Some(kind)).to_vec()))
            .unwrap_or_else(|| allowed_child_kinds(None).to_vec())
    }

    #[allow(dead_code)]
    pub(crate) fn parent_candidates(&self, child_kind: TicketKind) -> Vec<&TicketChange> {
        self.ordered_changes()
            .into_iter()
            .filter(|change| {
                self.changes_for_change(change).is_some_and(|ticket| {
                    allowed_child_kinds(Some(ticket.kind)).contains(&child_kind)
                })
            })
            .collect()
    }

    pub(crate) fn legal_kinds_for_selected(&self) -> Vec<TicketKind> {
        let Some(ticket) = self.selected_ticket() else {
            return Vec::new();
        };
        let placement = ticket
            .parent_key
            .as_ref()
            .map(|parent| PlacementTarget::ChildOf(parent.clone()))
            .unwrap_or(PlacementTarget::Root);
        [
            TicketKind::Epic,
            TicketKind::Story,
            TicketKind::Task,
            TicketKind::Bug,
            TicketKind::Subtask,
        ]
        .into_iter()
        .filter(|kind| {
            self.validate_tree_with(
                self.selected_ticket.as_deref().unwrap_or_default(),
                *kind,
                &placement,
                ticket.parent_kind,
            )
            .is_ok()
        })
        .collect()
    }

    #[allow(dead_code)]
    pub(crate) fn ordered_changes(&self) -> Vec<&TicketChange> {
        let Some(set) = self.active_set() else {
            return Vec::new();
        };
        let aliases = set
            .tickets
            .iter()
            .flat_map(|change| {
                std::iter::once((change.id.as_str(), change.id.as_str())).chain(
                    self.changes_for_change(change)
                        .map(|ticket| (ticket.key.as_str(), change.id.as_str())),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut children = HashMap::<Option<&str>, Vec<&TicketChange>>::new();
        for change in &set.tickets {
            let parent = self
                .changes_for_change(change)
                .and_then(|ticket| ticket.parent_key.as_deref())
                .and_then(|parent| aliases.get(parent).copied());
            children.entry(parent).or_default().push(change);
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|left, right| {
                left.sibling_order
                    .cmp(&right.sibling_order)
                    .then_with(|| left.id.cmp(&right.id))
            });
        }

        let mut ordered = Vec::with_capacity(set.tickets.len());
        let mut visited = std::collections::HashSet::new();
        collect_ordered_changes(None, &children, &mut visited, &mut ordered);
        for change in &set.tickets {
            if visited.insert(change.id.as_str()) {
                collect_ordered_changes(
                    Some(change.id.as_str()),
                    &children,
                    &mut visited,
                    &mut ordered,
                );
            }
        }
        ordered
    }

    pub(crate) fn validate_placement(
        &self,
        id: &str,
        placement: &PlacementTarget,
    ) -> Result<(), PlacementError> {
        let ticket = self
            .active_set()
            .and_then(|set| set.tickets.iter().find(|change| change.id == id))
            .and_then(|change| self.changes_for_change(change))
            .ok_or(PlacementError::UnknownTicket)?;
        self.validate_tree_with(
            id,
            ticket.kind,
            placement,
            placement_parent_kind(ticket, placement),
        )
    }

    pub(crate) fn changes_ready_for_submit(&self, ids: &[String]) -> bool {
        self.commit_changes(ids).is_ok()
    }

    pub(crate) fn removal_preview(&self, id: &str) -> Result<Vec<&TicketChange>, String> {
        let set = self
            .active_set()
            .ok_or_else(|| "Open a change set before removing tickets".to_string())?;
        if set.closed {
            return Err("Closed change sets cannot be changed".into());
        }
        let removed = self.subtree_ids(id);
        if removed.is_empty() {
            return Err(format!("Ticket {id} is unavailable"));
        }
        let changes = set
            .tickets
            .iter()
            .filter(|change| removed.contains(&change.id))
            .collect::<Vec<_>>();
        if let Some(submitted) = changes.iter().find(|change| change.is_submitted()) {
            return Err(format!(
                "Cannot remove this subtree because {} was already submitted",
                submitted.id
            ));
        }
        Ok(changes)
    }

    pub(crate) fn commit_changes(&self, ids: &[String]) -> Result<Vec<TicketChange>, String> {
        if ids.is_empty() {
            return Err("Select at least one ticket to commit".into());
        }
        let set = self
            .active_set()
            .ok_or_else(|| "Open a change set before committing".to_string())?;
        let mut selected = ids.iter().collect::<std::collections::HashSet<_>>();
        let mut pending = ids.to_vec();
        while let Some(id) = pending.pop() {
            let change = set
                .tickets
                .iter()
                .find(|change| change.id == id)
                .ok_or_else(|| format!("Selected ticket {id} is unavailable"))?;
            if change.is_submitted() {
                continue;
            }
            let parent = change_parent(change);
            let Some(parent) = parent.as_deref() else {
                continue;
            };
            if !parent.starts_with("NEW-") {
                continue;
            }
            let parent_change = set
                .tickets
                .iter()
                .find(|change| change.id == parent)
                .ok_or_else(|| {
                    format!(
                        "Commit blocked: {} needs unsent local parent {parent} selected",
                        change.id
                    )
                })?;
            if parent_change.is_submitted() {
                return Err(format!(
                    "Commit blocked: {} still references submitted local parent {parent}",
                    change.id
                ));
            }
            if selected.insert(&parent_change.id) {
                pending.push(parent_change.id.clone());
            }
        }
        set.tickets
            .iter()
            .filter(|change| selected.contains(&change.id) && !change.is_submitted())
            .cloned()
            .map(|mut change| {
                if change.retry_blocked {
                    return Err(format!(
                        "Commit blocked: {} may already have been created in Jira; refresh or remove it before retrying",
                        change.id
                    ));
                }
                if change.create_attempt {
                    return Err(format!(
                        "Commit blocked: {} has an unresolved Jira create attempt; refresh or remove it before retrying",
                        change.id
                    ));
                }
                if change.kind != ChangeKind::Added && change.original.is_none() {
                    change.original =
                        Some(self.active_source_for_change(&change).cloned().ok_or_else(|| {
                            format!("Jira source for {} must load before committing", change.id)
                        })?);
                }
                Ok(change)
            })
            .collect()
    }

    pub(crate) fn dispatch(&mut self, action: ComposerAction) -> Result<(), PlacementError> {
        if self.active_change_set_is_submitting() && action.mutates_active_change_set() {
            return Err(PlacementError::NotEditable);
        }
        match action {
            ComposerAction::CreateChangeSet { id, name } => {
                self.change_sets.push(ChangeSet {
                    id,
                    name,
                    tickets: Vec::new(),
                    selected_ticket_ids: Vec::new(),
                    closed: false,
                    submission_attempt: None,
                });
            }
            ComposerAction::DeleteChangeSet(id) => {
                if self.change_set_is_submitting(&id) {
                    return Ok(());
                }
                self.change_sets.retain(|set| set.id != id);
                if self.active_change_set.as_deref() == Some(id.as_str()) {
                    self.close_change_set();
                }
            }
            ComposerAction::OpenChangeSet(id) => {
                self.active_change_set = Some(id);
                self.view_mode = ComposerViewMode::Changes;
                self.selected_ticket = self
                    .ordered_changes()
                    .first()
                    .map(|change| change.id.clone());
            }
            ComposerAction::CloseChangeSet => self.close_change_set(),
            ComposerAction::RenameChangeSet(name) => {
                if let Some(change_set) = self.active_set_mut() {
                    change_set.name = name;
                }
            }
            ComposerAction::SelectTicket(id) => self.selected_ticket = id,
            ComposerAction::SetSelectedTickets(ids) => self.set_selected_tickets(ids),
            ComposerAction::SetViewMode(mode) => self.view_mode = mode,
            ComposerAction::SetDescriptionDiffSideBySide(value) => {
                self.description_diff_side_by_side = value
            }
            ComposerAction::SetSource {
                change_set_id,
                id,
                ticket,
            } => self.set_source(&change_set_id, id, ticket),
            ComposerAction::SetPresentation {
                change_set_id,
                id,
                presentation,
            } => self.set_presentation(&change_set_id, &id, presentation),
            ComposerAction::CreateTicket { title, project_key } => {
                self.create_ticket(title, project_key, TicketKind::Task, PlacementTarget::Root)?
            }
            ComposerAction::CreateTicketAt {
                title,
                project_key,
                kind,
                placement,
            } => self.create_ticket(title, project_key, kind, placement)?,
            ComposerAction::CreateTicketWithId {
                id,
                title,
                project_key,
                kind,
                placement,
            } => self.create_ticket_with_id(id, title, project_key, kind, placement)?,
            ComposerAction::IncludeTicket(ticket) => {
                self.include_ticket(ticket, PlacementTarget::Root, false)?
            }
            ComposerAction::IncludeTicketAt { ticket, placement } => {
                self.include_ticket(ticket, placement, true)?
            }
            ComposerAction::ReparentTicket { id, placement } => {
                self.reparent_ticket(&id, placement)?;
            }
            ComposerAction::RemoveTicket(id) => self.remove_ticket(&id),
            ComposerAction::MarkTicketDeleted(id) => self.mark_deleted(&id),
            ComposerAction::RestoreTicket(id) => self.restore_ticket(&id),
            ComposerAction::ResetTicket(id) => self.reset_ticket(&id),
            ComposerAction::UpdateTitle(value) => self.edit_selected(|ticket| ticket.title = value),
            ComposerAction::UpdateDescription(value) => {
                self.edit_selected(|ticket| ticket.description = value)
            }
            ComposerAction::UpdateKind(value) => self.update_selected_kind(value)?,
            ComposerAction::UpdateStatus(value) => {
                self.edit_selected(|ticket| ticket.status = value)
            }
            ComposerAction::UpdatePriority(value) => {
                self.edit_selected(|ticket| ticket.priority = value)
            }
            ComposerAction::UpdateStoryPoints(value) => {
                self.edit_selected(|ticket| ticket.story_points = value)
            }
            ComposerAction::UpdateFixVersions(value) => {
                self.edit_selected(|ticket| ticket.fix_versions = value)
            }
            ComposerAction::UpdateLabels(value) => {
                self.edit_selected(|ticket| ticket.labels = value)
            }
            ComposerAction::UpdateAssignee { name, account_id } => self.edit_selected(|ticket| {
                ticket.assignee = name;
                ticket.assignee_account_id = account_id;
            }),
            ComposerAction::CompleteSubmission {
                change_set_id,
                id,
                snapshot,
            } => self.complete_submission(&change_set_id, &id, snapshot)?,
            ComposerAction::RefreshAfterFailedSubmission {
                change_set_id,
                id,
                original,
                updated,
            } => self.refresh_after_failed_submission(&change_set_id, &id, original, updated)?,
            ComposerAction::BlockTicketRetry { change_set_id, id } => {
                self.block_ticket_retry(&change_set_id, &id)?
            }
            ComposerAction::MarkCreateAttempts { change_set_id, ids } => {
                self.mark_create_attempts(&change_set_id, &ids)?
            }
            ComposerAction::ResolveCreateAttempt { change_set_id, id } => {
                self.resolve_create_attempt(&change_set_id, &id)?
            }
            ComposerAction::ClaimSubmission {
                change_set_id,
                ids,
                owner_id,
            } => self.claim_submission(&change_set_id, &ids, owner_id)?,
            ComposerAction::MarkSubmissionCreateAttempts {
                change_set_id,
                owner_id,
            } => self.mark_submission_create_attempts(&change_set_id, &owner_id)?,
            ComposerAction::MarkSubmissionJiraStarted {
                change_set_id,
                owner_id,
            } => self.mark_submission_jira_started(&change_set_id, &owner_id)?,
            ComposerAction::ReleaseSubmissionClaim {
                change_set_id,
                owner_id,
            } => self.release_submission_claim(&change_set_id, &owner_id)?,
        }
        Ok(())
    }

    fn active_set_mut(&mut self) -> Option<&mut ChangeSet> {
        let active = self.active_change_set.as_deref()?;
        self.change_sets.iter_mut().find(|set| set.id == active)
    }

    fn change_set_mut(&mut self, id: &str) -> Option<&mut ChangeSet> {
        self.change_sets.iter_mut().find(|set| set.id == id)
    }

    fn set_source(&mut self, change_set_id: &str, id: String, ticket: Ticket) {
        if self.active_change_set.as_deref() != Some(change_set_id)
            || !self.remote_queries_allowed()
            || self.change_set_is_submitting(change_set_id)
        {
            return;
        }
        let submission_in_progress = self
            .change_sets
            .iter()
            .find(|set| set.id == change_set_id)
            .is_some_and(|set| set.submission_attempt.is_some());
        let Some(change) = self
            .change_set_mut(change_set_id)
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        if !change.is_submitted() && !submission_in_progress && change.original.is_some() {
            change.updated = change.updated.as_ref().map(|updated| {
                rebase_ticket(
                    change.original.as_ref().expect("original exists"),
                    updated,
                    &ticket,
                )
            });
            change.original = Some(ticket.clone());
        }
        self.sources.insert((change_set_id.to_owned(), id), ticket);
    }

    fn set_presentation(
        &mut self,
        change_set_id: &str,
        id: &str,
        presentation: TicketPresentation,
    ) {
        if self.active_change_set.as_deref() != Some(change_set_id)
            || !self.remote_queries_allowed()
        {
            return;
        }
        let change_id = self
            .active_set()
            .and_then(|set| {
                set.tickets
                    .iter()
                    .find(|change| change.matches_ticket_identity(id))
            })
            .map(|change| change.id.clone());
        if let Some(change_id) = change_id {
            self.presentations
                .entry(change_set_id.into())
                .or_default()
                .insert(change_id, presentation);
        }
    }

    fn close_change_set(&mut self) {
        self.active_change_set = None;
        self.selected_ticket = None;
    }

    fn create_ticket(
        &mut self,
        title: String,
        project_key: String,
        kind: TicketKind,
        placement: PlacementTarget,
    ) -> Result<(), PlacementError> {
        let id = format!("NEW-{}", self.next_ticket);
        self.create_ticket_with_id(id, title, project_key, kind, placement)?;
        self.next_ticket += 1;
        Ok(())
    }

    fn create_ticket_with_id(
        &mut self,
        id: String,
        title: String,
        project_key: String,
        kind: TicketKind,
        placement: PlacementTarget,
    ) -> Result<(), PlacementError> {
        if self.active_set().is_none() {
            return Err(PlacementError::UnknownTicket);
        }
        if self.active_set().is_some_and(|set| set.closed) {
            return Err(PlacementError::ClosedChangeSet);
        }
        if self
            .active_set()
            .is_some_and(|set| set.tickets.iter().any(|change| change.id == id))
        {
            return Err(PlacementError::UnknownTicket);
        }
        let mut ticket = Ticket {
            key: id.clone(),
            project_key,
            title,
            description: String::new(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            assignee_account_id: String::new(),
            story_points: None,
            fix_versions: Vec::new(),
            labels: Vec::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        };
        self.validate_new_placement(ticket.kind, &placement, None)?;
        ticket.parent_key = self.resolved_parent_key(&placement);
        ticket.parent_title = ticket
            .parent_key
            .as_deref()
            .and_then(|parent| self.parent_title_for(parent));
        ticket.parent_kind = ticket
            .parent_key
            .as_deref()
            .and_then(|parent| self.parent_kind_for(parent));
        let sibling_order = self.next_sibling_order(ticket.parent_key.as_deref());
        if let Some(set) = self.active_set_mut() {
            set.tickets.push(TicketChange {
                id: id.clone(),
                original: None,
                updated: Some(ticket),
                kind: ChangeKind::Added,
                submitted: None,
                retry_blocked: false,
                create_attempt: false,
                sibling_order,
            });
            set.selected_ticket_ids.push(id.clone());
            self.selected_ticket = Some(id);
            self.view_mode = ComposerViewMode::Changes;
        }
        Ok(())
    }

    fn include_ticket(
        &mut self,
        mut ticket: Ticket,
        placement: PlacementTarget,
        reparent_existing: bool,
    ) -> Result<(), PlacementError> {
        let id = ticket.key.clone();
        if self.active_set().is_none_or(|set| set.closed) {
            return Err(if self.active_set().is_some() {
                PlacementError::ClosedChangeSet
            } else {
                PlacementError::UnknownTicket
            });
        }
        let existing_id = self.active_set().and_then(|set| {
            set.tickets
                .iter()
                .find(|change| change.matches_ticket_identity(&id))
                .map(|change| change.id.clone())
        });
        if let Some(existing_id) = existing_id {
            if reparent_existing {
                self.reparent_ticket(&existing_id, placement)?;
            }
            self.select_ticket_for_submission(&existing_id);
            self.selected_ticket = Some(existing_id);
            return Ok(());
        }
        let source = ticket.clone();
        let stages_parent = matches!(&placement, PlacementTarget::ChildOf(_));
        let parent_key = match &placement {
            PlacementTarget::Root => source.parent_key.clone(),
            PlacementTarget::ChildOf(_) => self.resolved_parent_key(&placement),
        };
        let effective_placement = parent_key
            .as_ref()
            .map(|parent| PlacementTarget::ChildOf(parent.clone()))
            .unwrap_or(PlacementTarget::Root);
        self.validate_new_placement(
            ticket.kind,
            &effective_placement,
            placement_parent_kind(&ticket, &effective_placement),
        )?;
        let parent_kind = parent_key
            .as_deref()
            .and_then(|parent| self.parent_kind_for(parent))
            .or_else(|| {
                (source.parent_key == parent_key)
                    .then_some(source.parent_kind)
                    .flatten()
            });
        ticket.parent_key = parent_key.clone();
        ticket.parent_title = parent_key
            .as_deref()
            .and_then(|parent| self.parent_title_for(parent))
            .or_else(|| {
                (source.parent_key == parent_key)
                    .then_some(source.parent_title.clone())
                    .flatten()
            });
        ticket.parent_kind = parent_kind;
        let sibling_order = self.next_sibling_order(parent_key.as_deref());
        let Some(set) = self.active_set_mut() else {
            return Err(PlacementError::UnknownTicket);
        };
        set.tickets.push(TicketChange {
            id: id.clone(),
            original: Some(source.clone()),
            updated: (stages_parent && source.parent_key != parent_key).then_some(ticket),
            kind: if stages_parent && source.parent_key != parent_key {
                ChangeKind::Modified
            } else {
                ChangeKind::Synced
            },
            submitted: None,
            retry_blocked: false,
            create_attempt: false,
            sibling_order,
        });
        set.selected_ticket_ids.push(id.clone());
        self.selected_ticket = Some(id);
        Ok(())
    }

    fn remove_ticket(&mut self, id: &str) {
        let removed = {
            let Ok(preview) = self.removal_preview(id) else {
                return;
            };
            preview
                .into_iter()
                .map(|change| change.id.clone())
                .collect::<std::collections::HashSet<_>>()
        };
        let selected_removed = self
            .selected_ticket
            .as_ref()
            .is_some_and(|selected| removed.contains(selected));
        let Some(set) = self.active_set_mut() else {
            return;
        };
        set.tickets.retain(|change| !removed.contains(&change.id));
        set.selected_ticket_ids
            .retain(|selected| !removed.contains(selected));
        if selected_removed {
            self.selected_ticket = set.tickets.first().map(|change| change.id.clone());
        }
    }

    fn mark_deleted(&mut self, id: &str) {
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        if change.is_submitted() {
            return;
        }
        if change.kind != ChangeKind::Added {
            change.kind = ChangeKind::Deleted;
            self.view_mode = ComposerViewMode::Changes;
        }
    }

    fn restore_ticket(&mut self, id: &str) {
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        if change.kind == ChangeKind::Deleted && !change.is_submitted() {
            change.kind = if change.updated.is_some() {
                ChangeKind::Modified
            } else {
                ChangeKind::Synced
            };
            self.view_mode = ComposerViewMode::Changes;
        }
    }

    fn reset_ticket(&mut self, id: &str) {
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        if change.kind == ChangeKind::Modified && !change.is_submitted() {
            change.updated = None;
            change.kind = ChangeKind::Synced;
            self.view_mode = ComposerViewMode::Changes;
        }
    }

    fn complete_submission(
        &mut self,
        change_set_id: &str,
        id: &str,
        snapshot: SubmissionSnapshot,
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        let change = set
            .tickets
            .iter_mut()
            .find(|change| change.id == id)
            .ok_or(PlacementError::UnknownTicket)?;
        change.original = snapshot.original.clone();
        change.updated = snapshot.updated.clone();
        change.submitted = Some(snapshot);
        change.retry_blocked = false;
        change.create_attempt = false;
        let created_key = change
            .updated
            .as_ref()
            .or(change.original.as_ref())
            .map(|ticket| ticket.key.clone());
        if id.starts_with("NEW-")
            && let Some(created_key) = created_key
        {
            for dependent in &mut set.tickets {
                if !dependent.is_submitted()
                    && let Some(ticket) = dependent.updated.as_mut()
                    && ticket.parent_key.as_deref() == Some(id)
                {
                    ticket.parent_key = Some(created_key.clone());
                }
            }
        }
        set.selected_ticket_ids.retain(|selected| selected != id);
        set.closed = !set.tickets.is_empty() && set.tickets.iter().all(TicketChange::is_submitted);
        Ok(())
    }

    fn refresh_after_failed_submission(
        &mut self,
        change_set_id: &str,
        id: &str,
        original: Ticket,
        updated: Ticket,
    ) -> Result<(), PlacementError> {
        let (sources, change_sets) = (&mut self.sources, &mut self.change_sets);
        let change = change_sets
            .iter_mut()
            .find(|set| set.id == change_set_id)
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
            .ok_or(PlacementError::UnknownTicket)?;
        sources.insert((change_set_id.to_owned(), id.to_owned()), original.clone());
        change.updated = change
            .original
            .as_ref()
            .map(|prior_original| rebase_ticket(prior_original, &updated, &original));
        change.original = Some(original);
        change.kind = ChangeKind::Modified;
        change.retry_blocked = false;
        change.create_attempt = false;
        Ok(())
    }

    fn block_ticket_retry(&mut self, change_set_id: &str, id: &str) -> Result<(), PlacementError> {
        let change = self
            .change_set_mut(change_set_id)
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
            .ok_or(PlacementError::UnknownTicket)?;
        change.retry_blocked = true;
        Ok(())
    }

    fn mark_create_attempts(
        &mut self,
        change_set_id: &str,
        ids: &[String],
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        for change in &mut set.tickets {
            if ids.contains(&change.id) && change.kind == ChangeKind::Added {
                change.create_attempt = true;
            }
        }
        Ok(())
    }

    fn resolve_create_attempt(
        &mut self,
        change_set_id: &str,
        id: &str,
    ) -> Result<(), PlacementError> {
        let change = self
            .change_set_mut(change_set_id)
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
            .ok_or(PlacementError::UnknownTicket)?;
        change.create_attempt = false;
        Ok(())
    }

    fn claim_submission(
        &mut self,
        change_set_id: &str,
        ids: &[String],
        owner_id: String,
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        if set.submission_attempt.is_none() {
            set.submission_attempt = Some(SubmissionAttempt {
                owner_id,
                ticket_ids: ids.to_vec(),
                phase: SubmissionAttemptPhase::Claimed,
            });
        }
        Ok(())
    }

    fn mark_submission_create_attempts(
        &mut self,
        change_set_id: &str,
        owner_id: &str,
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        if let Some(attempt) = set.submission_attempt.as_mut()
            && attempt.owner_id == owner_id
        {
            attempt.phase = SubmissionAttemptPhase::CreateAttemptsMarked;
        }
        Ok(())
    }

    fn mark_submission_jira_started(
        &mut self,
        change_set_id: &str,
        owner_id: &str,
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        if let Some(attempt) = set.submission_attempt.as_mut()
            && attempt.owner_id == owner_id
        {
            attempt.phase = SubmissionAttemptPhase::JiraSubmissionStarted;
        }
        Ok(())
    }

    fn release_submission_claim(
        &mut self,
        change_set_id: &str,
        owner_id: &str,
    ) -> Result<(), PlacementError> {
        let set = self
            .change_set_mut(change_set_id)
            .ok_or(PlacementError::UnknownTicket)?;
        if set
            .submission_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.owner_id == owner_id)
        {
            set.submission_attempt = None;
        }
        Ok(())
    }

    fn set_selected_tickets(&mut self, ids: Vec<String>) {
        let Some(set) = self.active_set_mut() else {
            return;
        };
        set.selected_ticket_ids = ids.into_iter().fold(Vec::new(), |mut selected, id| {
            if !selected.contains(&id)
                && set
                    .tickets
                    .iter()
                    .any(|change| change.id == id && !change.is_submitted())
            {
                selected.push(id);
            }
            selected
        });
    }

    fn select_ticket_for_submission(&mut self, id: &str) {
        if let Some(set) = self.active_set_mut()
            && set
                .tickets
                .iter()
                .any(|change| change.id == id && !change.is_submitted())
            && !set
                .selected_ticket_ids
                .iter()
                .any(|selected| selected == id)
        {
            set.selected_ticket_ids.push(id.to_owned());
        }
    }

    fn edit_selected(&mut self, edit: impl FnOnce(&mut Ticket)) {
        let Some(selected) = self.selected_ticket.clone() else {
            return;
        };
        let source = self.active_source(&selected).cloned();
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == selected))
        else {
            return;
        };
        if change.kind == ChangeKind::Deleted || change.is_submitted() {
            return;
        }
        if change.updated.is_none() {
            change.updated = source.or_else(|| change.original.clone());
        }
        if change.kind == ChangeKind::Synced {
            change.kind = ChangeKind::Modified;
        }
        let Some(ticket) = change.updated.as_mut() else {
            return;
        };
        edit(ticket);
    }

    fn update_selected_kind(&mut self, kind: TicketKind) -> Result<(), PlacementError> {
        let Some(selected) = self.selected_ticket.clone() else {
            return Err(PlacementError::UnknownTicket);
        };
        let Some(ticket) = self
            .active_set()
            .and_then(|set| set.tickets.iter().find(|change| change.id == selected))
            .and_then(|change| self.changes_for_change(change))
        else {
            return Err(PlacementError::UnknownTicket);
        };
        let placement = ticket
            .parent_key
            .as_ref()
            .map(|parent| PlacementTarget::ChildOf(parent.clone()))
            .unwrap_or(PlacementTarget::Root);
        self.validate_tree_with(
            &selected,
            kind,
            &placement,
            placement_parent_kind(ticket, &placement),
        )?;
        self.edit_selected(|ticket| ticket.kind = kind);
        Ok(())
    }

    fn validate_new_placement(
        &self,
        kind: TicketKind,
        placement: &PlacementTarget,
        parent_kind: Option<TicketKind>,
    ) -> Result<(), PlacementError> {
        let parent_kind = parent_kind.or_else(|| match placement {
            PlacementTarget::Root => None,
            PlacementTarget::ChildOf(parent) => self.parent_kind_for(parent),
        });
        self.validate_tree_with("", kind, placement, parent_kind)
    }

    fn next_sibling_order(&self, parent: Option<&str>) -> usize {
        self.active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter(|change| {
                self.changes_for_change(change)
                    .and_then(|ticket| ticket.parent_key.as_deref())
                    == parent
            })
            .map(|change| change.sibling_order)
            .max()
            .map_or(0, |order| order.saturating_add(1))
    }

    fn validate_tree_with(
        &self,
        id: &str,
        kind: TicketKind,
        placement: &PlacementTarget,
        parent_kind: Option<TicketKind>,
    ) -> Result<(), PlacementError> {
        let set = self.active_set().ok_or(PlacementError::UnknownTicket)?;
        if set.closed {
            return Err(PlacementError::ClosedChangeSet);
        }
        let mut nodes = HashMap::<String, (TicketKind, Option<String>, Option<TicketKind>)>::new();
        let mut aliases = HashMap::<String, String>::new();
        for change in &set.tickets {
            let Some(ticket) = self.changes_for_change(change) else {
                continue;
            };
            aliases.insert(change.id.clone(), change.id.clone());
            aliases.insert(ticket.key.clone(), change.id.clone());
            nodes.insert(
                change.id.clone(),
                (ticket.kind, ticket.parent_key.clone(), ticket.parent_kind),
            );
        }
        if !id.is_empty() && !nodes.contains_key(id) {
            return Err(PlacementError::UnknownTicket);
        }
        let parent = match placement {
            PlacementTarget::Root => None,
            PlacementTarget::ChildOf(parent) => Some(parent.clone()),
        };
        if parent
            .as_deref()
            .and_then(|parent| aliases.get(parent))
            .is_some_and(|parent| parent == id)
        {
            return Err(PlacementError::Cycle);
        }
        nodes.insert(id.into(), (kind, parent, parent_kind));
        aliases.insert(id.into(), id.into());

        for child_id in nodes.keys() {
            let mut visited = std::collections::HashSet::new();
            let mut current = Some(child_id.as_str());
            while let Some(node) = current {
                if !visited.insert(node) {
                    return Err(PlacementError::Cycle);
                }
                current = nodes
                    .get(node)
                    .and_then(|(_, parent, _)| parent.as_deref())
                    .and_then(|parent| aliases.get(parent).map(String::as_str));
            }
        }
        for (child_kind, parent, external_parent_kind) in nodes.values() {
            if parent.is_none() && !allowed_child_kinds(None).contains(child_kind) {
                return Err(PlacementError::InvalidParentKind);
            }
            let Some(parent) = parent else {
                continue;
            };
            let internal_parent = aliases.get(parent).unwrap_or(parent);
            let parent_kind = nodes
                .get(internal_parent)
                .map(|(kind, _, _)| *kind)
                .or(*external_parent_kind)
                .ok_or(PlacementError::UnknownParentKind)?;
            if !allowed_child_kinds(Some(parent_kind)).contains(child_kind) {
                return Err(PlacementError::InvalidParentKind);
            }
        }
        Ok(())
    }

    fn reparent_ticket(
        &mut self,
        id: &str,
        placement: PlacementTarget,
    ) -> Result<(), PlacementError> {
        self.validate_placement(id, &placement)?;
        let source = self.active_source(id).cloned();
        let parent_key = self.resolved_parent_key(&placement);
        let parent_kind = parent_key
            .as_deref()
            .and_then(|parent| self.parent_kind_for(parent));
        let parent_title = parent_key
            .as_deref()
            .and_then(|parent| self.parent_title_for(parent));
        let sibling_order = self.next_sibling_order(parent_key.as_deref());
        let change = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
            .ok_or(PlacementError::UnknownTicket)?;
        if change.is_submitted() || change.kind == ChangeKind::Deleted {
            return Err(PlacementError::NotEditable);
        }
        if change.updated.is_none() {
            change.updated = source.or_else(|| change.original.clone());
        }
        let ticket = change
            .updated
            .as_mut()
            .ok_or(PlacementError::UnknownTicket)?;
        ticket.parent_key = parent_key;
        ticket.parent_title = parent_title;
        ticket.parent_kind = parent_kind;
        change.sibling_order = sibling_order;
        if change.kind == ChangeKind::Synced {
            change.kind = ChangeKind::Modified;
        }
        Ok(())
    }

    fn parent_kind_for(&self, parent: &str) -> Option<TicketKind> {
        if let Some(change) = self.active_set().and_then(|set| {
            set.tickets.iter().find(|change| {
                change.id == parent
                    || self
                        .changes_for_change(change)
                        .is_some_and(|ticket| ticket.key == parent)
            })
        }) {
            return self.changes_for_change(change).map(|ticket| ticket.kind);
        }
        let kinds = self
            .active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter_map(|change| self.changes_for_change(change))
            .filter(|ticket| ticket.parent_key.as_deref() == Some(parent))
            .filter_map(|ticket| ticket.parent_kind)
            .collect::<std::collections::HashSet<_>>();
        (kinds.len() == 1).then(|| *kinds.iter().next().unwrap())
    }

    fn parent_title_for(&self, parent: &str) -> Option<String> {
        self.active_set()
            .and_then(|set| {
                set.tickets.iter().find(|change| {
                    change.id == parent
                        || self
                            .changes_for_change(change)
                            .is_some_and(|ticket| ticket.key == parent)
                })
            })
            .and_then(|change| self.changes_for_change(change))
            .map(|ticket| ticket.title.clone())
    }

    fn resolved_parent_key(&self, placement: &PlacementTarget) -> Option<String> {
        let PlacementTarget::ChildOf(parent) = placement else {
            return None;
        };
        self.active_set()
            .and_then(|set| {
                set.tickets.iter().find(|change| {
                    change.id == *parent
                        || self
                            .changes_for_change(change)
                            .is_some_and(|ticket| ticket.key == *parent)
                })
            })
            .and_then(|change| self.changes_for_change(change))
            .map(|ticket| ticket.key.clone())
            .or_else(|| Some(parent.clone()))
    }

    fn active_source(&self, id: &str) -> Option<&Ticket> {
        self.active_change_set
            .as_ref()
            .and_then(|change_set_id| self.sources.get(&(change_set_id.clone(), id.to_owned())))
    }

    fn active_source_for_change(&self, change: &TicketChange) -> Option<&Ticket> {
        self.active_source(&change.id)
    }

    fn subtree_ids(&self, root: &str) -> std::collections::HashSet<String> {
        let Some(set) = self.active_set() else {
            return std::collections::HashSet::new();
        };
        let aliases = set
            .tickets
            .iter()
            .flat_map(|change| {
                std::iter::once((change.id.clone(), change.id.clone())).chain(
                    self.changes_for_change(change)
                        .map(|ticket| (ticket.key.clone(), change.id.clone())),
                )
            })
            .collect::<HashMap<_, _>>();
        let Some(root) = aliases.get(root).cloned() else {
            return std::collections::HashSet::new();
        };
        let parents = set
            .tickets
            .iter()
            .filter_map(|change| {
                self.changes_for_change(change).map(|ticket| {
                    (
                        change.id.clone(),
                        ticket
                            .parent_key
                            .as_ref()
                            .and_then(|parent| aliases.get(parent))
                            .cloned(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut removed = std::collections::HashSet::from([root]);
        let mut changed = true;
        while changed {
            changed = false;
            for (id, parent) in &parents {
                if parent
                    .as_ref()
                    .is_some_and(|parent| removed.contains(parent))
                {
                    changed |= removed.insert(id.clone());
                }
            }
        }
        removed
    }
}

pub(crate) fn change_parent(change: &TicketChange) -> Option<String> {
    let ticket = if change.kind == ChangeKind::Deleted {
        change.original.as_ref()
    } else {
        change.updated.as_ref().or(change.original.as_ref())
    };
    ticket.and_then(|ticket| ticket.parent_key.clone())
}

pub(crate) fn rebase_ticket(original: &Ticket, updated: &Ticket, refreshed: &Ticket) -> Ticket {
    let mut rebased = refreshed.clone();
    if original.project_key != updated.project_key {
        rebased.project_key = updated.project_key.clone();
    }
    if original.title != updated.title {
        rebased.title = updated.title.clone();
    }
    if original.description != updated.description {
        rebased.description = updated.description.clone();
    }
    if original.kind != updated.kind {
        rebased.kind = updated.kind;
    }
    if original.status != updated.status {
        rebased.status = updated.status.clone();
    }
    if original.priority != updated.priority {
        rebased.priority = updated.priority.clone();
    }
    if original.assignee != updated.assignee {
        rebased.assignee = updated.assignee.clone();
    }
    if original.assignee_account_id != updated.assignee_account_id {
        rebased.assignee_account_id = updated.assignee_account_id.clone();
    }
    if original.story_points != updated.story_points {
        rebased.story_points = updated.story_points;
    }
    if original.fix_versions != updated.fix_versions {
        rebased.fix_versions = updated.fix_versions.clone();
    }
    if original.labels != updated.labels {
        rebased.labels = updated.labels.clone();
    }
    if original.parent_key != updated.parent_key {
        rebased.parent_key = updated.parent_key.clone();
        rebased.parent_title = updated.parent_title.clone();
        rebased.parent_kind = updated.parent_kind;
    }
    rebased
}

fn next_ticket_id(change_sets: &[ChangeSet]) -> usize {
    change_sets
        .iter()
        .flat_map(|set| &set.tickets)
        .filter_map(|change| change.id.strip_prefix("NEW-")?.parse::<usize>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn allowed_child_kinds(parent: Option<TicketKind>) -> &'static [TicketKind] {
    match parent {
        None => &[
            TicketKind::Epic,
            TicketKind::Story,
            TicketKind::Task,
            TicketKind::Bug,
        ],
        Some(TicketKind::Epic) => &[TicketKind::Story, TicketKind::Task, TicketKind::Bug],
        Some(TicketKind::Story | TicketKind::Task | TicketKind::Bug) => &[TicketKind::Subtask],
        Some(TicketKind::Subtask) => &[],
    }
}

fn placement_parent_kind(ticket: &Ticket, placement: &PlacementTarget) -> Option<TicketKind> {
    match placement {
        PlacementTarget::Root => None,
        PlacementTarget::ChildOf(parent) if ticket.parent_key.as_deref() == Some(parent) => {
            ticket.parent_kind
        }
        PlacementTarget::ChildOf(_) => None,
    }
}

#[allow(dead_code)]
fn collect_ordered_changes<'a>(
    parent: Option<&'a str>,
    children: &HashMap<Option<&'a str>, Vec<&'a TicketChange>>,
    visited: &mut std::collections::HashSet<&'a str>,
    ordered: &mut Vec<&'a TicketChange>,
) {
    let Some(siblings) = children.get(&parent) else {
        return;
    };
    for change in siblings {
        if visited.insert(change.id.as_str()) {
            ordered.push(change);
            collect_ordered_changes(Some(change.id.as_str()), children, visited, ordered);
        }
    }
}

impl ComposerAction {
    fn mutates_active_change_set(&self) -> bool {
        matches!(
            self,
            Self::RenameChangeSet(_)
                | Self::CreateTicket { .. }
                | Self::CreateTicketAt { .. }
                | Self::CreateTicketWithId { .. }
                | Self::IncludeTicket(_)
                | Self::IncludeTicketAt { .. }
                | Self::ReparentTicket { .. }
                | Self::RemoveTicket(_)
                | Self::MarkTicketDeleted(_)
                | Self::RestoreTicket(_)
                | Self::ResetTicket(_)
                | Self::UpdateTitle(_)
                | Self::UpdateDescription(_)
                | Self::UpdateKind(_)
                | Self::UpdateStatus(_)
                | Self::UpdatePriority(_)
                | Self::UpdateStoryPoints(_)
                | Self::UpdateFixVersions(_)
                | Self::UpdateLabels(_)
                | Self::UpdateAssignee { .. }
                | Self::SetSelectedTickets(_)
        )
    }

    pub(crate) fn affects_persistence(&self) -> bool {
        matches!(
            self,
            Self::RenameChangeSet(_)
                | Self::CreateTicket { .. }
                | Self::CreateTicketAt { .. }
                | Self::CreateTicketWithId { .. }
                | Self::IncludeTicket(_)
                | Self::IncludeTicketAt { .. }
                | Self::ReparentTicket { .. }
                | Self::SetSelectedTickets(_)
                | Self::RemoveTicket(_)
                | Self::MarkTicketDeleted(_)
                | Self::RestoreTicket(_)
                | Self::ResetTicket(_)
                | Self::UpdateTitle(_)
                | Self::UpdateDescription(_)
                | Self::UpdateKind(_)
                | Self::UpdateStatus(_)
                | Self::UpdatePriority(_)
                | Self::UpdateStoryPoints(_)
                | Self::UpdateFixVersions(_)
                | Self::UpdateLabels(_)
                | Self::UpdateAssignee { .. }
                | Self::BlockTicketRetry { .. }
                | Self::MarkCreateAttempts { .. }
                | Self::ResolveCreateAttempt { .. }
                | Self::ClaimSubmission { .. }
                | Self::ReleaseSubmissionClaim { .. }
        )
    }
}

#[cfg(test)]
pub(crate) fn demo_jira_tickets() -> Vec<Ticket> {
    vec![
        Ticket {
            key: "FIN-142".into(),
            project_key: "FIN".into(),
            title: "Keep checkout state across retries".into(),
            description: "## Outcome\n\nCustomers can retry checkout without losing their basket.\n\n## Acceptance Criteria\n\n- Basket state survives a failed authorization.\n- A successful retry creates one order.".into(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind: TicketKind::Story,
            status: "In Progress".into(),
            priority: "High".into(),
            assignee: "Mina Patel".into(),
            assignee_account_id: "mina".into(),
            story_points: Some(3.0),
            fix_versions: vec!["1.0".into()],
            labels: vec!["checkout".into()],
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        },
        Ticket {
            key: "FIN-157".into(),
            project_key: "FIN".into(),
            title: "Retry payment authorization".into(),
            description: "## Description\n\nAdd an idempotent retry path for transient gateway errors.".into(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Highest".into(),
            assignee: "Ada Mensah".into(),
            assignee_account_id: "ada".into(),
            story_points: Some(2.0),
            fix_versions: Vec::new(),
            labels: Vec::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        },
        Ticket {
            key: "FIN-131".into(),
            project_key: "FIN".into(),
            title: "Remove legacy payment callback".into(),
            description: "Legacy callback superseded by the event stream.".into(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind: TicketKind::Task,
            status: "Done".into(),
            priority: "Low".into(),
            assignee: "Lin Chen".into(),
            assignee_account_id: "lin".into(),
            story_points: None,
            fix_versions: Vec::new(),
            labels: Vec::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        },
        Ticket {
            key: "FIN-166".into(),
            project_key: "FIN".into(),
            title: "Display authorization failures clearly".into(),
            description: "Show actionable gateway failures without exposing provider internals.".into(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind: TicketKind::Bug,
            status: "Backlog".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            assignee_account_id: String::new(),
            story_points: None,
            fix_versions: Vec::new(),
            labels: Vec::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
        },
    ]
}
pub(crate) mod jira_adf;
#[cfg(test)]
mod tests;
