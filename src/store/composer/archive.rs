use super::{ChangeSet, ComposerState, PlacementError, TicketChange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArchiveOutcome {
    Cancelled,
    Concluded,
}

#[cfg(test)]
#[path = "tests/archive.rs"]
mod tests;

impl ArchiveOutcome {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cancelled => "Cancelled",
            Self::Concluded => "Concluded",
        }
    }
}

impl ChangeSet {
    pub(super) fn mark_closed(&mut self) {
        if !self.closed {
            self.closed = true;
            self.closed_at = Some(chrono::Utc::now());
        }
    }

    pub(crate) fn unsubmitted_outcome(&self, change: &TicketChange) -> Option<ArchiveOutcome> {
        if change.is_submitted() {
            None
        } else {
            self.archive_outcome
        }
    }
}

impl ComposerState {
    pub(crate) fn validate_archive(&self, id: &str) -> Result<(), String> {
        let set = self
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .ok_or_else(|| format!("Change set {id} is unavailable"))?;
        if set.closed {
            return Err("This change set is already archived".into());
        }
        if self.change_set_is_submitting(id)
            || set
                .tickets
                .iter()
                .any(|change| change.create_attempt || change.retry_blocked)
        {
            return Err(
                "Resolve the Jira submission attempt before archiving this change set".into(),
            );
        }
        Ok(())
    }

    pub(super) fn archive_change_set(
        &mut self,
        id: &str,
        outcome: ArchiveOutcome,
    ) -> Result<(), PlacementError> {
        self.validate_archive(id)
            .map_err(|_| PlacementError::NotEditable)?;
        let set = self
            .change_set_mut(id)
            .ok_or(PlacementError::UnknownTicket)?;
        set.mark_closed();
        set.archive_outcome = Some(outcome);
        set.selected_ticket_ids.clear();
        self.sources.retain(|(set_id, _), _| set_id != id);
        self.presentations.remove(id);
        Ok(())
    }
}
