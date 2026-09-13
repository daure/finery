use super::*;
use crate::store::composer::{SubmissionSnapshot, TicketIssueLink};
use serde_json::json;

fn ticket(key: &str) -> Ticket {
    serde_json::from_value(json!({
        "key": key, "title": "Checkout", "description": "", "kind": "Story",
        "status": "To do", "priority": "Medium", "assignee": ""
    }))
    .unwrap()
}

fn change(
    id: &str,
    kind: ChangeKind,
    original: Option<Ticket>,
    updated: Option<Ticket>,
) -> TicketChange {
    TicketChange {
        id: id.into(),
        original,
        updated,
        kind,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    }
}

fn set(tickets: Vec<TicketChange>) -> ChangeSet {
    ChangeSet {
        id: "CS-1".into(),
        name: "Improve checkout".into(),
        tickets,
        selected_ticket_ids: Vec::new(),
        closed: false,
        submission_attempt: None,
    }
}

#[test]
fn external_links_exclude_every_local_and_submitted_ticket_identity() {
    let mut reference = ticket("FIN-1");
    reference.issue_links = ["NEW-2", "FIN-2", "FIN-3", "FIN-9", "FIN-9"]
        .into_iter()
        .enumerate()
        .map(|(index, target)| TicketIssueLink {
            id: index.to_string(),
            relationship: "Blocks".into(),
            target_key: target.into(),
            target_title: "Target".into(),
            outward: true,
        })
        .collect();
    let mut created = change("NEW-2", ChangeKind::Added, None, Some(ticket("NEW-2")));
    created.submitted = Some(SubmissionSnapshot {
        original: None,
        updated: Some(ticket("FIN-2")),
        warnings: Vec::new(),
    });
    let summary = ChangeSetSummary::new(&set(vec![
        change("FIN-1", ChangeKind::Synced, Some(reference), None),
        created,
        change("FIN-3", ChangeKind::Deleted, Some(ticket("FIN-3")), None),
        change(
            "FIN-4",
            ChangeKind::Modified,
            Some(ticket("FIN-4")),
            Some(ticket("FIN-4")),
        ),
    ]));
    assert_eq!(
        summary,
        ChangeSetSummary {
            created: 1,
            edited: 1,
            deleted: 1,
            reference: 1,
            external_links: 2,
            submitted: 1,
            ..Default::default()
        }
    );
}

#[test]
fn artifacts_count_changes_and_uploads_without_counting_generated_diagram_files() {
    let mut original = ticket("FIN-1");
    original.web_links = serde_json::from_value(json!([
        {"id":"same", "title":"Docs", "url":"https://example.com/docs"},
        {"id":"edit", "title":"Old", "url":"https://example.com/old"},
        {"id":"remove", "title":"Obsolete", "url":"https://example.com/obsolete"}
    ]))
    .unwrap();
    original.attachments = serde_json::from_value(json!([
        {"id":"same", "filename":"existing.txt", "created":"", "size":10},
        {"id":"delete", "filename":"obsolete.txt", "created":"", "size":10}
    ]))
    .unwrap();
    original.mermaid_diagrams = serde_json::from_value(json!([
        {"id":"same", "title":"Existing", "diagram_type":"flowchart", "markup":"source"}
    ]))
    .unwrap();
    let mut updated = original.clone();
    updated.web_links[1].title = "Updated".into();
    updated.web_links.pop();
    updated.web_links.push(
        serde_json::from_value(json!(
            {"id":"new", "title":"New", "url":"https://example.com/new"}
        ))
        .unwrap(),
    );
    updated.attachments[1].change = AttachmentChangeKind::Deleted;
    updated.attachments.extend(
        serde_json::from_value::<Vec<_>>(json!([
            {"id":"upload", "filename":"new.txt", "created":"", "size":10, "change":"Added"},
            {"id":"generated", "filename":"diagram.png", "created":"", "size":10}
        ]))
        .unwrap(),
    );
    updated.mermaid_diagrams[0].rendered_theme = "another-theme".into();
    updated.mermaid_diagrams.push(
        serde_json::from_value(json!(
            {"id":"new", "title":"New", "diagram_type":"flowchart", "markup":"source",
             "published_attachment_id":"generated"}
        ))
        .unwrap(),
    );
    let pending = change(
        "FIN-1",
        ChangeKind::Modified,
        Some(original.clone()),
        Some(updated.clone()),
    );
    let summary = ChangeSetSummary::new(&set(vec![pending.clone()]));
    assert_eq!(
        (summary.diagrams, summary.uploads, summary.web_links),
        (1, 1, 3)
    );

    updated.attachments[2].change = AttachmentChangeKind::Synced;
    let mut submitted = pending;
    submitted.submitted = Some(SubmissionSnapshot {
        original: Some(original),
        updated: Some(updated),
        warnings: Vec::new(),
    });
    let summary = ChangeSetSummary::new(&set(vec![submitted]));
    assert_eq!(
        (
            summary.diagrams,
            summary.uploads,
            summary.web_links,
            summary.submitted
        ),
        (1, 1, 3, 1)
    );
}

#[test]
fn reference_and_deleted_ticket_artifacts_are_context() {
    let mut source = ticket("FIN-1");
    source.attachments = serde_json::from_value(json!([
        {"id":"file", "filename":"existing.txt", "created":"", "size":10}
    ]))
    .unwrap();
    source.web_links = serde_json::from_value(json!([
        {"id":"link", "title":"Docs", "url":"https://example.com/docs"}
    ]))
    .unwrap();
    let summary = ChangeSetSummary::new(&set(vec![
        change("FIN-1", ChangeKind::Synced, Some(source.clone()), None),
        change("FIN-2", ChangeKind::Deleted, Some(source), None),
    ]));
    assert_eq!(
        summary,
        ChangeSetSummary {
            reference: 1,
            deleted: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        ChangeSetSummary::new(&set(Vec::new())),
        ChangeSetSummary::default()
    );
}
