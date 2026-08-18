use serde_json::json;

use super::{
    ChangeKind, ComposerAction, ComposerState, ComposerViewMode, SubmissionSnapshot,
    jira_adf::{adf_to_markdown, markdown_to_adf},
};

mod placement;

#[test]
fn included_ticket_uses_live_source_until_first_edit_creates_changes() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    let ticket = super::demo_jira_tickets()[0].clone();
    state.dispatch(ComposerAction::IncludeTicket(ticket));
    let id = state.selected_ticket.clone().unwrap();
    let change = state.selected_change().unwrap();
    assert_eq!(
        change.original.as_ref().unwrap(),
        &super::demo_jira_tickets()[0]
    );
    assert!(change.updated.is_none());

    let mut refreshed = state.selected_changes().unwrap().clone();
    refreshed.title = "Latest from Jira".into();
    state.dispatch(ComposerAction::SetSource {
        id,
        ticket: refreshed,
    });
    assert_eq!(state.selected_changes().unwrap().title, "Latest from Jira");

    state.dispatch(ComposerAction::UpdateTitle("A safer checkout".into()));

    let change = state.selected_change().unwrap();
    assert_eq!(
        change.original.as_ref().unwrap().title,
        "Keep checkout state across retries"
    );
    assert_eq!(change.updated.as_ref().unwrap().title, "A safer checkout");
    assert_eq!(change.kind, ChangeKind::Modified);

    let mut newer_source = state.selected_source().unwrap().clone();
    newer_source.title = "Even newer Jira title".into();
    let id = state.selected_ticket.clone().unwrap();
    state.dispatch(ComposerAction::SetSource {
        id,
        ticket: newer_source,
    });
    assert_eq!(state.selected_changes().unwrap().title, "A safer checkout");
}

#[test]
fn added_tickets_are_selected_and_selection_survives_reopening_change_set() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::IncludeTicket(
        super::demo_jira_tickets()[0].clone(),
    ));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });

    assert_eq!(
        state.active_set().unwrap().selected_ticket_ids,
        vec!["FIN-142", "NEW-1"]
    );

    state.dispatch(ComposerAction::CloseChangeSet);
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));

    assert_eq!(
        state.active_set().unwrap().selected_ticket_ids,
        vec!["FIN-142", "NEW-1"]
    );
}

#[test]
fn source_changes_and_diff_modes_use_their_expected_ticket_values() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let id = state.selected_ticket.clone().unwrap();
    let mut source = state.selected_changes().unwrap().clone();
    source.title = "Latest from Jira".into();
    state.dispatch(ComposerAction::SetSource { id, ticket: source });

    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Source));
    assert_eq!(state.selected_ticket().unwrap().title, "Latest from Jira");
    assert!(!state.selected_is_editable());

    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Changes));
    assert_eq!(state.selected_ticket().unwrap().title, "Latest from Jira");
    assert!(state.selected_is_editable());

    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Diff));
    assert_eq!(state.selected_ticket().unwrap().title, "Latest from Jira");
    assert!(!state.selected_is_editable());
}

#[test]
fn closed_change_sets_use_submission_snapshots_and_forbid_remote_queries() {
    let mut state = ComposerState::demo();
    let change = &mut state.change_sets[0].tickets[0];
    let mut snapshot_source = change.original.clone().unwrap();
    snapshot_source.title = "Source at submit".into();
    let mut snapshot_changes = snapshot_source.clone();
    snapshot_changes.title = "Changes at submit".into();
    change.submitted = Some(SubmissionSnapshot {
        original: Some(snapshot_source.clone()),
        updated: Some(snapshot_changes),
    });
    state.change_sets[0].closed = true;
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let id = state.selected_ticket.clone().unwrap();
    let mut stale_live_source = snapshot_source;
    stale_live_source.title = "Later Jira value".into();
    state.sources.insert(id, stale_live_source);
    state.dispatch(ComposerAction::SetViewMode(ComposerViewMode::Source));

    assert!(!state.remote_queries_allowed());
    assert_eq!(state.selected_ticket().unwrap().title, "Source at submit");
}

#[test]
fn remote_tickets_allow_refresh_even_when_a_new_ticket_is_selected() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    assert!(!state.has_remote_tickets());

    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    let new_ticket = state.selected_ticket.clone().unwrap();
    assert!(!state.has_remote_tickets());

    state.dispatch(ComposerAction::IncludeTicket(
        super::demo_jira_tickets()[0].clone(),
    ));
    state.dispatch(ComposerAction::SelectTicket(Some(new_ticket)));
    assert!(state.has_remote_tickets());
}

#[test]
fn submitted_tickets_keep_snapshots_and_stay_in_change_set_when_all_are_done() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let tickets = state.active_set().unwrap().tickets.clone();

    for change in tickets {
        let original = change.original.clone();
        let updated = change.updated.clone().or_else(|| change.original.clone());
        state.dispatch(ComposerAction::CompleteSubmission {
            id: change.id,
            snapshot: SubmissionSnapshot { original, updated },
        });
    }

    let set = state
        .change_sets
        .iter()
        .find(|set| set.id == "CS-1")
        .unwrap();
    assert!(set.closed);
    assert_eq!(set.submitted_count(), set.tickets.len());
    assert!(set.tickets.iter().all(|ticket| ticket.submitted.is_some()));
    assert_eq!(state.active_change_set.as_deref(), Some("CS-1"));
}

#[test]
fn deleting_ticket_keeps_live_source_read_only_and_visible() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let id = state.selected_ticket.clone().unwrap();
    let source = state.selected_changes().unwrap().clone();
    state.dispatch(ComposerAction::SetSource { id, ticket: source });
    state.dispatch(ComposerAction::MarkTicketDeleted("FIN-142".into()));

    let change = state.selected_change().unwrap();
    assert_eq!(change.kind, ChangeKind::Deleted);
    assert!(change.original.is_some());
    assert!(change.updated.is_none());
    assert!(state.selected_changes().is_some());
    assert!(!state.selected_is_editable());
}

#[test]
fn restore_and_reset_return_tickets_to_synced_state() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));

    state.dispatch(ComposerAction::MarkTicketDeleted("FIN-142".into()));
    state.dispatch(ComposerAction::RestoreTicket("FIN-142".into()));
    assert_eq!(
        state
            .active_set()
            .unwrap()
            .tickets
            .iter()
            .find(|change| change.id == "FIN-142")
            .unwrap()
            .kind,
        ChangeKind::Synced
    );

    let expected_update = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-157")
        .unwrap()
        .updated
        .clone();
    state.dispatch(ComposerAction::MarkTicketDeleted("FIN-157".into()));
    state.dispatch(ComposerAction::RestoreTicket("FIN-157".into()));
    let change = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-157")
        .unwrap();
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(change.updated, expected_update);

    state.dispatch(ComposerAction::ResetTicket("FIN-157".into()));
    let change = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-157")
        .unwrap();
    assert_eq!(change.kind, ChangeKind::Synced);
    assert!(change.updated.is_none());
}

#[test]
fn removing_ticket_does_not_mark_it_for_jira_deletion() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state.dispatch(ComposerAction::RemoveTicket("FIN-142".into()));

    assert!(
        state
            .active_set()
            .unwrap()
            .tickets
            .iter()
            .all(|ticket| ticket.id != "FIN-142")
    );
}

#[test]
fn jira_description_converts_headings_and_lists_to_markdown() {
    let adf = json!({
        "type": "doc",
        "version": 1,
        "content": [
            { "type": "heading", "attrs": { "level": 2 }, "content": [
                { "type": "text", "text": "Outcome" }
            ]},
            { "type": "paragraph", "content": [
                { "type": "text", "text": "Checkout is safe." }
            ]},
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [{ "type": "paragraph", "content": [
                    { "type": "text", "text": "Retry works." }
                ]}]}
            ]}
        ]
    });

    assert_eq!(
        adf_to_markdown(&adf),
        "## Outcome\n\nCheckout is safe.\n\n- Retry works."
    );
}

#[test]
fn supported_markdown_round_trips_through_jira_adf() {
    let markdown = "## Outcome\n\nCheckout is **safe** and [visible](https://example.com).\n\n- Retry works.\n- One order is created.\n\n```rust\nlet safe = true;\n```";

    assert_eq!(adf_to_markdown(&markdown_to_adf(markdown)), markdown);
}
