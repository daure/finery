use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TicketKind {
    Epic,
    Story,
    Task,
    Bug,
    Subtask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ticket {
    pub key: String,
    #[serde(default)]
    pub project_key: String,
    pub title: String,
    pub description: String,
    pub kind: TicketKind,
    pub status: String,
    pub priority: String,
    pub assignee: String,
    #[serde(default)]
    pub assignee_account_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TicketChange {
    pub id: String,
    pub original: Option<Ticket>,
    pub updated: Option<Ticket>,
    pub kind: ChangeKind,
    #[serde(default)]
    pub submitted: Option<SubmissionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangeSet {
    pub id: String,
    pub name: String,
    pub tickets: Vec<TicketChange>,
    #[serde(default)]
    pub selected_ticket_ids: Vec<String>,
    #[serde(default)]
    pub closed: bool,
}

impl ChangeSet {
    pub(crate) fn submitted_count(&self) -> usize {
        self.tickets
            .iter()
            .filter(|ticket| ticket.is_submitted())
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerState {
    pub change_sets: Vec<ChangeSet>,
    pub active_change_set: Option<String>,
    pub selected_ticket: Option<String>,
    pub view_mode: ComposerViewMode,
    pub sources: HashMap<String, Ticket>,
    next_ticket: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    CreateChangeSet {
        id: String,
        name: String,
    },
    DeleteChangeSet(String),
    OpenChangeSet(String),
    CloseChangeSet,
    SelectTicket(Option<String>),
    SetSelectedTickets(Vec<String>),
    SetViewMode(ComposerViewMode),
    SetSource {
        id: String,
        ticket: Ticket,
    },
    CreateTicket {
        title: String,
        project_key: String,
    },
    IncludeTicket(Ticket),
    RemoveTicket(String),
    MarkTicketDeleted(String),
    UpdateTitle(String),
    UpdateDescription(String),
    UpdateKind(TicketKind),
    UpdateStatus(String),
    UpdatePriority(String),
    UpdateAssignee {
        name: String,
        account_id: String,
    },
    CompleteSubmission {
        id: String,
        snapshot: SubmissionSnapshot,
    },
    RefreshAfterFailedSubmission {
        id: String,
        original: Ticket,
        updated: Ticket,
    },
}

impl ComposerState {
    pub(crate) fn from_change_sets(change_sets: Vec<ChangeSet>) -> Self {
        let next_ticket = change_sets
            .iter()
            .flat_map(|set| &set.tickets)
            .filter_map(|change| change.id.strip_prefix("NEW-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        Self {
            change_sets,
            active_change_set: None,
            selected_ticket: None,
            view_mode: ComposerViewMode::Changes,
            sources: HashMap::new(),
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
                        },
                        TicketChange {
                            id: tickets[2].key.clone(),
                            original: Some(tickets[2].clone()),
                            updated: None,
                            kind: ChangeKind::Deleted,
                            submitted: None,
                        },
                    ],
                    selected_ticket_ids: Vec::new(),
                },
                ChangeSet {
                    id: "CS-2".into(),
                    name: "Customer notifications".into(),
                    tickets: Vec::new(),
                    selected_ticket_ids: Vec::new(),
                    closed: false,
                },
            ],
            active_change_set: None,
            selected_ticket: None,
            view_mode: ComposerViewMode::Changes,
            sources: HashMap::new(),
            next_ticket: 1,
        }
    }

    pub(crate) fn active_set(&self) -> Option<&ChangeSet> {
        let active = self.active_change_set.as_deref()?;
        self.change_sets.iter().find(|set| set.id == active)
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

    pub(crate) fn selected_source(&self) -> Option<&Ticket> {
        self.source_for_change(self.selected_change()?)
    }

    pub(crate) fn source_for_change<'a>(&'a self, change: &'a TicketChange) -> Option<&'a Ticket> {
        let snapshot = || change.submitted.as_ref()?.original.as_ref();
        if !self.remote_queries_allowed() {
            return snapshot();
        }
        self.sources.get(&change.id).or_else(snapshot)
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
            .or_else(|| self.sources.get(&change.id))
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

    pub(crate) fn selected_is_editable(&self) -> bool {
        self.selected_change().is_some_and(|change| {
            self.view_mode == ComposerViewMode::Changes && change.can_edit(true)
        })
    }

    pub(crate) fn changes_ready_for_submit(&self, ids: &[String]) -> bool {
        !ids.is_empty()
            && ids.iter().all(|id| {
                self.active_set()
                    .and_then(|set| set.tickets.iter().find(|change| &change.id == id))
                    .is_some_and(|change| {
                        change.kind == ChangeKind::Added || self.sources.contains_key(id)
                    })
            })
    }

    pub(crate) fn dispatch(&mut self, action: ComposerAction) {
        match action {
            ComposerAction::CreateChangeSet { id, name } => {
                self.change_sets.push(ChangeSet {
                    id,
                    name,
                    tickets: Vec::new(),
                    selected_ticket_ids: Vec::new(),
                    closed: false,
                });
            }
            ComposerAction::DeleteChangeSet(id) => {
                self.change_sets.retain(|set| set.id != id);
                if self.active_change_set.as_deref() == Some(id.as_str()) {
                    self.close_change_set();
                }
            }
            ComposerAction::OpenChangeSet(id) => {
                self.active_change_set = Some(id);
                self.selected_ticket = self
                    .active_set()
                    .and_then(|set| set.tickets.first())
                    .map(|change| change.id.clone());
                self.view_mode = ComposerViewMode::Changes;
            }
            ComposerAction::CloseChangeSet => self.close_change_set(),
            ComposerAction::SelectTicket(id) => self.selected_ticket = id,
            ComposerAction::SetSelectedTickets(ids) => self.set_selected_tickets(ids),
            ComposerAction::SetViewMode(mode) => self.view_mode = mode,
            ComposerAction::SetSource { id, ticket } => {
                self.sources.insert(id, ticket);
            }
            ComposerAction::CreateTicket { title, project_key } => {
                self.create_ticket(title, project_key)
            }
            ComposerAction::IncludeTicket(ticket) => self.include_ticket(ticket),
            ComposerAction::RemoveTicket(id) => self.remove_ticket(&id),
            ComposerAction::MarkTicketDeleted(id) => self.mark_deleted(&id),
            ComposerAction::UpdateTitle(value) => self.edit_selected(|ticket| ticket.title = value),
            ComposerAction::UpdateDescription(value) => {
                self.edit_selected(|ticket| ticket.description = value)
            }
            ComposerAction::UpdateKind(value) => self.edit_selected(|ticket| ticket.kind = value),
            ComposerAction::UpdateStatus(value) => {
                self.edit_selected(|ticket| ticket.status = value)
            }
            ComposerAction::UpdatePriority(value) => {
                self.edit_selected(|ticket| ticket.priority = value)
            }
            ComposerAction::UpdateAssignee { name, account_id } => self.edit_selected(|ticket| {
                ticket.assignee = name;
                ticket.assignee_account_id = account_id;
            }),
            ComposerAction::CompleteSubmission { id, snapshot } => {
                self.complete_submission(&id, snapshot)
            }
            ComposerAction::RefreshAfterFailedSubmission {
                id,
                original,
                updated,
            } => self.refresh_after_failed_submission(&id, original, updated),
        }
    }

    fn active_set_mut(&mut self) -> Option<&mut ChangeSet> {
        let active = self.active_change_set.as_deref()?;
        self.change_sets.iter_mut().find(|set| set.id == active)
    }

    fn close_change_set(&mut self) {
        self.active_change_set = None;
        self.selected_ticket = None;
    }

    fn create_ticket(&mut self, title: String, project_key: String) {
        if self.active_set().is_some_and(|set| set.closed) {
            return;
        }
        let id = format!("NEW-{}", self.next_ticket);
        self.next_ticket += 1;
        let ticket = Ticket {
            key: id.clone(),
            project_key,
            title,
            description: String::new(),
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            assignee_account_id: String::new(),
        };
        if let Some(set) = self.active_set_mut() {
            set.tickets.push(TicketChange {
                id: id.clone(),
                original: None,
                updated: Some(ticket),
                kind: ChangeKind::Added,
                submitted: None,
            });
            set.selected_ticket_ids.push(id.clone());
            self.selected_ticket = Some(id);
            self.view_mode = ComposerViewMode::Changes;
        }
    }

    fn include_ticket(&mut self, ticket: Ticket) {
        let id = ticket.key.clone();
        let Some(set) = self.active_set() else {
            return;
        };
        if set.closed {
            return;
        }
        if set.tickets.iter().any(|change| change.id == id) {
            self.select_ticket_for_submission(&id);
            self.selected_ticket = Some(id);
            return;
        }
        self.sources.insert(id.clone(), ticket);
        let Some(set) = self.active_set_mut() else {
            return;
        };
        set.tickets.push(TicketChange {
            id: id.clone(),
            original: None,
            updated: None,
            kind: ChangeKind::Synced,
            submitted: None,
        });
        set.selected_ticket_ids.push(id.clone());
        self.selected_ticket = Some(id);
    }

    fn remove_ticket(&mut self, id: &str) {
        let Some(set) = self.active_set_mut() else {
            return;
        };
        if set.closed
            || set
                .tickets
                .iter()
                .any(|change| change.id == id && change.is_submitted())
        {
            return;
        }
        set.tickets.retain(|change| change.id != id);
        set.selected_ticket_ids.retain(|selected| selected != id);
        self.selected_ticket = set.tickets.first().map(|change| change.id.clone());
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
            change.original = None;
            change.updated = None;
            change.kind = ChangeKind::Deleted;
            self.view_mode = ComposerViewMode::Changes;
        }
    }

    fn complete_submission(&mut self, id: &str, snapshot: SubmissionSnapshot) {
        let Some(set) = self.active_set_mut() else {
            return;
        };
        let Some(change) = set.tickets.iter_mut().find(|change| change.id == id) else {
            return;
        };
        change.original = snapshot.original.clone();
        change.updated = snapshot.updated.clone();
        change.submitted = Some(snapshot);
        set.selected_ticket_ids.retain(|selected| selected != id);
        set.closed = !set.tickets.is_empty() && set.tickets.iter().all(TicketChange::is_submitted);
        if set.closed {
            self.close_change_set();
        }
    }

    fn refresh_after_failed_submission(&mut self, id: &str, original: Ticket, updated: Ticket) {
        self.sources.insert(id.to_owned(), original);
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        change.original = None;
        change.updated = Some(updated);
        change.kind = ChangeKind::Modified;
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
        let source = self.sources.get(&selected).cloned();
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
            change.original = None;
        }
        if change.kind == ChangeKind::Synced {
            change.kind = ChangeKind::Modified;
        }
        let Some(ticket) = change.updated.as_mut() else {
            return;
        };
        edit(ticket);
    }
}

impl ComposerAction {
    pub(crate) fn affects_persistence(&self) -> bool {
        matches!(
            self,
            Self::CreateTicket { .. }
                | Self::IncludeTicket(_)
                | Self::SetSelectedTickets(_)
                | Self::RemoveTicket(_)
                | Self::MarkTicketDeleted(_)
                | Self::UpdateTitle(_)
                | Self::UpdateDescription(_)
                | Self::UpdateKind(_)
                | Self::UpdateStatus(_)
                | Self::UpdatePriority(_)
                | Self::UpdateAssignee { .. }
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
            kind: TicketKind::Story,
            status: "In Progress".into(),
            priority: "High".into(),
            assignee: "Mina Patel".into(),
            assignee_account_id: "mina".into(),
        },
        Ticket {
            key: "FIN-157".into(),
            project_key: "FIN".into(),
            title: "Retry payment authorization".into(),
            description: "## Description\n\nAdd an idempotent retry path for transient gateway errors.".into(),
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Highest".into(),
            assignee: "Ada Mensah".into(),
            assignee_account_id: "ada".into(),
        },
        Ticket {
            key: "FIN-131".into(),
            project_key: "FIN".into(),
            title: "Remove legacy payment callback".into(),
            description: "Legacy callback superseded by the event stream.".into(),
            kind: TicketKind::Task,
            status: "Done".into(),
            priority: "Low".into(),
            assignee: "Lin Chen".into(),
            assignee_account_id: "lin".into(),
        },
        Ticket {
            key: "FIN-166".into(),
            project_key: "FIN".into(),
            title: "Display authorization failures clearly".into(),
            description: "Show actionable gateway failures without exposing provider internals.".into(),
            kind: TicketKind::Bug,
            status: "Backlog".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            assignee_account_id: String::new(),
        },
    ]
}
pub(crate) mod jira_adf;
#[cfg(test)]
mod tests;
