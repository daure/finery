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
    pub title: String,
    pub description: String,
    pub kind: TicketKind,
    pub status: String,
    pub priority: String,
    pub assignee: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TicketChange {
    pub id: String,
    pub original: Option<Ticket>,
    pub updated: Option<Ticket>,
    pub kind: ChangeKind,
}

impl TicketChange {
    pub(crate) fn visible_ticket(&self, updated: bool) -> Option<&Ticket> {
        if updated {
            self.updated.as_ref().or(self.original.as_ref())
        } else {
            self.original.as_ref().or(self.updated.as_ref())
        }
    }

    pub(crate) fn can_edit(&self, updated: bool) -> bool {
        updated && self.kind != ChangeKind::Deleted
    }

    fn editable_ticket(&mut self) -> Option<&mut Ticket> {
        if self.kind == ChangeKind::Deleted {
            return None;
        }
        if self.updated.is_none() {
            self.updated = self.original.clone();
        }
        if self.kind == ChangeKind::Synced {
            self.kind = ChangeKind::Modified;
        }
        self.updated.as_mut()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChangeSet {
    pub id: String,
    pub name: String,
    pub tickets: Vec<TicketChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerState {
    pub change_sets: Vec<ChangeSet>,
    pub active_change_set: Option<String>,
    pub selected_ticket: Option<String>,
    pub show_updated: bool,
    next_ticket: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    CreateChangeSet { id: String, name: String },
    DeleteChangeSet(String),
    OpenChangeSet(String),
    CloseChangeSet,
    SelectTicket(Option<String>),
    ShowUpdated(bool),
    CreateTicket(String),
    IncludeTicket(Ticket),
    RemoveTicket(String),
    MarkTicketDeleted(String),
    UpdateTitle(String),
    UpdateDescription(String),
    UpdateKind(TicketKind),
    UpdateStatus(String),
    UpdatePriority(String),
    UpdateAssignee(String),
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
            show_updated: true,
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
                    tickets: vec![
                        TicketChange {
                            id: tickets[0].key.clone(),
                            original: Some(tickets[0].clone()),
                            updated: None,
                            kind: ChangeKind::Synced,
                        },
                        TicketChange {
                            id: tickets[1].key.clone(),
                            original: Some(tickets[1].clone()),
                            updated: Some(Ticket {
                                title: "Retry payment authorization safely".into(),
                                ..tickets[1].clone()
                            }),
                            kind: ChangeKind::Modified,
                        },
                        TicketChange {
                            id: tickets[2].key.clone(),
                            original: Some(tickets[2].clone()),
                            updated: None,
                            kind: ChangeKind::Deleted,
                        },
                    ],
                },
                ChangeSet {
                    id: "CS-2".into(),
                    name: "Customer notifications".into(),
                    tickets: Vec::new(),
                },
            ],
            active_change_set: None,
            selected_ticket: None,
            show_updated: true,
            next_ticket: 1,
        }
    }

    pub(crate) fn active_set(&self) -> Option<&ChangeSet> {
        let active = self.active_change_set.as_deref()?;
        self.change_sets.iter().find(|set| set.id == active)
    }

    pub(crate) fn selected_change(&self) -> Option<&TicketChange> {
        let selected = self.selected_ticket.as_deref()?;
        self.active_set()?
            .tickets
            .iter()
            .find(|change| change.id == selected)
    }

    pub(crate) fn selected_ticket(&self) -> Option<&Ticket> {
        self.selected_change()?.visible_ticket(self.show_updated)
    }

    pub(crate) fn selected_is_editable(&self) -> bool {
        self.selected_change()
            .is_some_and(|change| change.can_edit(self.show_updated))
    }

    pub(crate) fn dispatch(&mut self, action: ComposerAction) {
        match action {
            ComposerAction::CreateChangeSet { id, name } => {
                self.change_sets.push(ChangeSet {
                    id,
                    name,
                    tickets: Vec::new(),
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
                self.show_updated = true;
            }
            ComposerAction::CloseChangeSet => self.close_change_set(),
            ComposerAction::SelectTicket(id) => self.selected_ticket = id,
            ComposerAction::ShowUpdated(value) => self.show_updated = value,
            ComposerAction::CreateTicket(title) => self.create_ticket(title),
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
            ComposerAction::UpdateAssignee(value) => {
                self.edit_selected(|ticket| ticket.assignee = value)
            }
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

    fn create_ticket(&mut self, title: String) {
        let id = format!("NEW-{}", self.next_ticket);
        self.next_ticket += 1;
        let ticket = Ticket {
            key: id.clone(),
            title,
            description: String::new(),
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
        };
        if let Some(set) = self.active_set_mut() {
            set.tickets.push(TicketChange {
                id: id.clone(),
                original: None,
                updated: Some(ticket),
                kind: ChangeKind::Added,
            });
            self.selected_ticket = Some(id);
            self.show_updated = true;
        }
    }

    fn include_ticket(&mut self, ticket: Ticket) {
        let id = ticket.key.clone();
        let Some(set) = self.active_set_mut() else {
            return;
        };
        if set.tickets.iter().any(|change| change.id == id) {
            self.selected_ticket = Some(id);
            return;
        }
        set.tickets.push(TicketChange {
            id: id.clone(),
            original: Some(ticket),
            updated: None,
            kind: ChangeKind::Synced,
        });
        self.selected_ticket = Some(id);
    }

    fn remove_ticket(&mut self, id: &str) {
        let Some(set) = self.active_set_mut() else {
            return;
        };
        set.tickets.retain(|change| change.id != id);
        self.selected_ticket = set.tickets.first().map(|change| change.id.clone());
    }

    fn mark_deleted(&mut self, id: &str) {
        let Some(change) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == id))
        else {
            return;
        };
        if change.original.is_some() {
            change.updated = None;
            change.kind = ChangeKind::Deleted;
            self.show_updated = false;
        }
    }

    fn edit_selected(&mut self, edit: impl FnOnce(&mut Ticket)) {
        let Some(selected) = self.selected_ticket.clone() else {
            return;
        };
        let Some(ticket) = self
            .active_set_mut()
            .and_then(|set| set.tickets.iter_mut().find(|change| change.id == selected))
            .and_then(TicketChange::editable_ticket)
        else {
            return;
        };
        edit(ticket);
    }
}

impl ComposerAction {
    pub(crate) fn affects_persistence(&self) -> bool {
        matches!(
            self,
            Self::CreateTicket(_)
                | Self::IncludeTicket(_)
                | Self::RemoveTicket(_)
                | Self::MarkTicketDeleted(_)
                | Self::UpdateTitle(_)
                | Self::UpdateDescription(_)
                | Self::UpdateKind(_)
                | Self::UpdateStatus(_)
                | Self::UpdatePriority(_)
                | Self::UpdateAssignee(_)
        )
    }
}

#[cfg(test)]
pub(crate) fn demo_jira_tickets() -> Vec<Ticket> {
    vec![
        Ticket {
            key: "FIN-142".into(),
            title: "Keep checkout state across retries".into(),
            description: "## Outcome\n\nCustomers can retry checkout without losing their basket.\n\n## Acceptance Criteria\n\n- Basket state survives a failed authorization.\n- A successful retry creates one order.".into(),
            kind: TicketKind::Story,
            status: "In Progress".into(),
            priority: "High".into(),
            assignee: "Mina Patel".into(),
        },
        Ticket {
            key: "FIN-157".into(),
            title: "Retry payment authorization".into(),
            description: "## Description\n\nAdd an idempotent retry path for transient gateway errors.".into(),
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Highest".into(),
            assignee: "Ada Mensah".into(),
        },
        Ticket {
            key: "FIN-131".into(),
            title: "Remove legacy payment callback".into(),
            description: "Legacy callback superseded by the event stream.".into(),
            kind: TicketKind::Task,
            status: "Done".into(),
            priority: "Low".into(),
            assignee: "Lin Chen".into(),
        },
        Ticket {
            key: "FIN-166".into(),
            title: "Display authorization failures clearly".into(),
            description: "Show actionable gateway failures without exposing provider internals.".into(),
            kind: TicketKind::Bug,
            status: "Backlog".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
        },
    ]
}
pub(crate) mod jira_adf;
#[cfg(test)]
mod tests;
