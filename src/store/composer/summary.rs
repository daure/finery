use std::collections::HashSet;

use super::{ArchiveOutcome, AttachmentChangeKind, ChangeKind, ChangeSet, Ticket, TicketChange};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct ChangeSetSummary {
    pub created: usize,
    pub edited: usize,
    pub deleted: usize,
    pub reference: usize,
    pub diagrams: usize,
    pub uploads: usize,
    pub web_links: usize,
    pub external_links: usize,
    pub submitted: usize,
    pub cancelled: usize,
    pub concluded: usize,
}

impl ChangeSetSummary {
    pub(crate) fn new(set: &ChangeSet) -> Self {
        let member_keys = member_keys(set);
        let mut summary = Self {
            submitted: set.submitted_count(),
            ..Self::default()
        };
        for change in &set.tickets {
            match set.unsubmitted_outcome(change) {
                Some(ArchiveOutcome::Cancelled) => summary.cancelled += 1,
                Some(ArchiveOutcome::Concluded) => summary.concluded += 1,
                None => {}
            }
            match change.kind {
                ChangeKind::Added => summary.created += 1,
                ChangeKind::Modified => summary.edited += 1,
                ChangeKind::Deleted => summary.deleted += 1,
                ChangeKind::Synced => summary.reference += 1,
            }
            let (original, updated) = snapshots(change);
            if let Some(ticket) = updated.or(original) {
                summary.external_links += ticket
                    .issue_links
                    .iter()
                    .filter(|link| !member_keys.contains(link.target_key.as_str()))
                    .count();
            }
            if change.kind != ChangeKind::Deleted
                && let Some(updated) = updated
            {
                summary.add_artifact_changes(original, updated);
            }
        }
        summary
    }

    fn add_artifact_changes(&mut self, original: Option<&Ticket>, updated: &Ticket) {
        let diagrams = original.map_or(&[][..], |ticket| ticket.mermaid_diagrams.as_slice());
        self.diagrams += changed_items(
            diagrams,
            &updated.mermaid_diagrams,
            |diagram| &diagram.id,
            |left, right| {
                left.title == right.title
                    && left.diagram_type == right.diagram_type
                    && left.markup == right.markup
            },
        );
        let attachments = original.map_or(&[][..], |ticket| ticket.attachments.as_slice());
        self.uploads += updated
            .attachments
            .iter()
            .filter(|attachment| {
                attachment.change != AttachmentChangeKind::Deleted
                    && !updated.mermaid_diagrams.iter().any(|diagram| {
                        diagram.published_attachment_id.as_deref() == Some(attachment.id.as_str())
                            || diagram.published_source_attachment_id.as_deref()
                                == Some(attachment.id.as_str())
                    })
                    && (matches!(
                        attachment.change,
                        AttachmentChangeKind::Added | AttachmentChangeKind::Modified
                    ) || !attachments.iter().any(|source| source.id == attachment.id))
            })
            .count();
        let links = original.map_or(&[][..], |ticket| ticket.web_links.as_slice());
        self.web_links += changed_items(
            links,
            &updated.web_links,
            |link| &link.id,
            |left, right| left.title == right.title && left.url == right.url,
        );
    }
}

fn snapshots(change: &TicketChange) -> (Option<&Ticket>, Option<&Ticket>) {
    match &change.submitted {
        Some(snapshot) => (snapshot.original.as_ref(), snapshot.updated.as_ref()),
        None => (change.original.as_ref(), change.updated.as_ref()),
    }
}

fn member_keys(set: &ChangeSet) -> HashSet<&str> {
    set.tickets
        .iter()
        .flat_map(|change| {
            std::iter::once(change.id.as_str()).chain(
                [
                    change.original.as_ref(),
                    change.updated.as_ref(),
                    change.submitted.as_ref().and_then(|s| s.original.as_ref()),
                    change.submitted.as_ref().and_then(|s| s.updated.as_ref()),
                ]
                .into_iter()
                .flatten()
                .map(|ticket| ticket.key.as_str()),
            )
        })
        .collect()
}

fn changed_items<T>(
    original: &[T],
    updated: &[T],
    id: impl Fn(&T) -> &str,
    equal: impl Fn(&T, &T) -> bool,
) -> usize {
    updated
        .iter()
        .filter(|item| {
            original
                .iter()
                .find(|source| id(source) == id(item))
                .is_none_or(|source| !equal(source, item))
        })
        .count()
        + original
            .iter()
            .filter(|source| !updated.iter().any(|item| id(item) == id(source)))
            .count()
}

#[cfg(test)]
#[path = "tests/summary.rs"]
mod tests;
