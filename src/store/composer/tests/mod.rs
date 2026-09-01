use serde_json::json;

use super::{
    ChangeKind, ComposerAction, ComposerState, ComposerViewMode, SubmissionSnapshot,
    jira_adf::{
        adf_is_safe_to_overwrite, adf_overwrite_warning, adf_to_markdown, markdown_to_adf,
        validate_markdown,
    },
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
        change_set_id: "CS-2".into(),
        id,
        ticket: refreshed,
    });
    assert_eq!(state.selected_changes().unwrap().title, "Latest from Jira");

    state.dispatch(ComposerAction::UpdateTitle("A safer checkout".into()));

    let change = state.selected_change().unwrap();
    assert_eq!(change.original.as_ref().unwrap().title, "Latest from Jira");
    assert_eq!(change.updated.as_ref().unwrap().title, "A safer checkout");
    assert_eq!(change.kind, ChangeKind::Modified);

    let mut newer_source = state.selected_source().unwrap().clone();
    newer_source.title = "Even newer Jira title".into();
    let id = state.selected_ticket.clone().unwrap();
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-2".into(),
        id,
        ticket: newer_source,
    });
    assert_eq!(
        state
            .selected_change()
            .unwrap()
            .original
            .as_ref()
            .unwrap()
            .title,
        "Even newer Jira title"
    );
    assert_eq!(state.selected_changes().unwrap().title, "A safer checkout");
}

#[test]
fn selected_existing_ticket_key_excludes_local_tickets() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    assert_eq!(
        state.selected_existing_ticket_key().as_deref(),
        Some("FIN-142")
    );

    state.dispatch(ComposerAction::SelectTicket(Some("FIN-157".into())));
    assert_eq!(
        state.selected_existing_ticket_key().as_deref(),
        Some("FIN-157")
    );

    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Local story".into(),
        project_key: "FIN".into(),
        kind: super::TicketKind::Story,
        placement: super::PlacementTarget::Root,
    });
    assert_eq!(state.selected_existing_ticket_key(), None);
}

#[test]
fn replacing_catalog_preserves_valid_selection_and_falls_back_safely() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let selected = state.selected_ticket.clone().unwrap();
    let source = state.selected_changes().unwrap().clone();
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-1".into(),
        id: selected.clone(),
        ticket: source,
    });

    let catalog = state.change_sets.clone();
    state.replace_change_sets(catalog);
    assert_eq!(state.active_change_set.as_deref(), Some("CS-1"));
    assert_eq!(state.selected_ticket.as_deref(), Some(selected.as_str()));
    assert!(state.sources.is_empty());

    let mut changed_catalog = state.change_sets.clone();
    changed_catalog[0]
        .tickets
        .retain(|change| change.id != selected);
    state.replace_change_sets(changed_catalog);
    assert_eq!(state.active_change_set.as_deref(), Some("CS-1"));
    assert_eq!(state.selected_ticket.as_deref(), Some("FIN-157"));

    state.replace_change_sets(Vec::new());
    assert!(state.active_change_set.is_none());
    assert!(state.selected_ticket.is_none());
}

#[test]
fn submission_results_update_the_originating_change_set_after_navigation() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));

    state.dispatch(ComposerAction::CompleteSubmission {
        change_set_id: "CS-2".into(),
        id: "NEW-1".into(),
        snapshot: SubmissionSnapshot {
            original: None,
            updated: Some(super::Ticket {
                key: "FIN-200".into(),
                ..super::demo_jira_tickets()[1].clone()
            }),
        },
    });

    let submitted = state
        .change_sets
        .iter()
        .find(|set| set.id == "CS-2")
        .unwrap()
        .tickets
        .first()
        .unwrap();
    assert!(submitted.is_submitted());
    assert_eq!(submitted.updated.as_ref().unwrap().key, "FIN-200");
}

#[test]
fn post_create_refresh_failure_converts_added_ticket_to_update() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let created = super::Ticket {
        key: "FIN-201".into(),
        ..super::demo_jira_tickets()[1].clone()
    };

    state.dispatch(ComposerAction::RefreshAfterFailedSubmission {
        change_set_id: "CS-2".into(),
        id: "NEW-1".into(),
        original: created.clone(),
        updated: created,
    });

    let change = &state
        .change_sets
        .iter()
        .find(|set| set.id == "CS-2")
        .unwrap()
        .tickets[0];
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(change.original.as_ref().unwrap().key, "FIN-201");
}

#[test]
fn in_flight_change_set_cannot_be_deleted_after_navigation() {
    let mut state = ComposerState::demo();
    assert!(state.begin_submission("CS-1"));
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CloseChangeSet);
    state.dispatch(ComposerAction::DeleteChangeSet("CS-1".into()));

    assert!(state.change_sets.iter().any(|set| set.id == "CS-1"));
    state.end_submission("CS-1");
    state.dispatch(ComposerAction::DeleteChangeSet("CS-1".into()));
    assert!(state.change_sets.iter().all(|set| set.id != "CS-1"));
}

#[test]
fn submitting_change_set_rejects_field_mutations_without_losing_the_submitted_snapshot() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let before = state.selected_changes().unwrap().clone();

    assert!(state.begin_submission("CS-1"));
    assert!(!state.selected_is_editable());
    assert_eq!(
        state.dispatch(ComposerAction::UpdateTitle("Lost edit".into())),
        Err(super::PlacementError::NotEditable)
    );
    assert_eq!(state.selected_changes(), Some(&before));
}

#[test]
fn source_response_during_submission_does_not_replace_the_submission_snapshot() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let id = state.selected_ticket.clone().unwrap();
    let before = state.selected_change().unwrap().original.clone();
    let mut source = before.clone().unwrap();
    source.title = "Stale Jira source".into();

    assert!(state.begin_submission("CS-1"));
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-1".into(),
        id,
        ticket: source,
    });

    assert_eq!(state.selected_change().unwrap().original, before);
}

#[test]
fn unresolved_create_attempt_blocks_retry_after_restart() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::MarkCreateAttempts {
        change_set_id: "CS-2".into(),
        ids: vec!["NEW-1".into()],
    });
    let mut restored = ComposerState::from_change_sets(state.change_sets.clone());
    restored.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));

    assert!(restored.change_sets[1].tickets[0].create_attempt);
    assert!(
        restored
            .commit_changes(&["NEW-1".into()])
            .unwrap_err()
            .contains("unresolved Jira create attempt")
    );
}

#[test]
fn restart_retains_unverified_submission_attempts() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "New local ticket".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::ClaimSubmission {
        change_set_id: "CS-2".into(),
        ids: vec!["NEW-1".into()],
        owner_id: "other-client".into(),
    });

    let mut restored = ComposerState::from_change_sets(state.change_sets.clone());
    restored.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));

    assert!(restored.active_set().unwrap().submission_attempt.is_some());
    assert_eq!(
        restored.dispatch(ComposerAction::SetSelectedTickets(vec!["NEW-1".into()])),
        Err(super::PlacementError::NotEditable)
    );
    restored.dispatch(ComposerAction::CloseChangeSet);
    restored.dispatch(ComposerAction::DeleteChangeSet("CS-2".into()));
    assert!(restored.change_sets.iter().any(|set| set.id == "CS-2"));
}

#[test]
fn persisted_tickets_without_description_safety_metadata_remain_loadable() {
    let ticket = serde_json::from_value::<super::Ticket>(json!({
        "key": "FIN-1",
        "title": "Legacy ticket",
        "description": "Description",
        "kind": "Task",
        "status": "To Do",
        "priority": "Medium",
        "assignee": "Unassigned"
    }))
    .unwrap();

    assert!(!ticket.description_safe_to_overwrite);
}

#[test]
fn persisted_changes_without_retry_metadata_remain_loadable() {
    let change = serde_json::from_value::<super::TicketChange>(json!({
        "id": "FIN-1",
        "original": null,
        "updated": null,
        "kind": "Synced"
    }))
    .unwrap();

    assert!(!change.retry_blocked);
}

#[test]
fn stale_source_responses_do_not_update_another_change_set_or_retry_blocked_ticket() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-2".into()));
    state.dispatch(ComposerAction::CreateTicket {
        title: "Uncertain create".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::BlockTicketRetry {
        change_set_id: "CS-2".into(),
        id: "NEW-1".into(),
    });
    assert!(
        state
            .commit_changes(&["NEW-1".into()])
            .unwrap_err()
            .contains("may already have been created")
    );

    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let mut stale = super::demo_jira_tickets()[0].clone();
    stale.title = "Stale source".into();
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-2".into(),
        id: "NEW-1".into(),
        ticket: stale,
    });

    assert!(
        state
            .sources
            .get(&("CS-2".into(), "NEW-1".into()))
            .is_none()
    );
}

#[test]
fn submission_result_with_a_missing_target_is_rejected() {
    let mut state = ComposerState::demo();

    assert!(
        state
            .dispatch(ComposerAction::CompleteSubmission {
                change_set_id: "CS-1".into(),
                id: "missing".into(),
                snapshot: SubmissionSnapshot {
                    original: None,
                    updated: None,
                },
            })
            .is_err()
    );
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
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-1".into(),
        id,
        ticket: source,
    });

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
fn description_diff_style_defaults_to_word_and_can_switch_to_side_by_side() {
    let mut state = ComposerState::demo();

    assert!(!state.description_diff_side_by_side);

    state.dispatch(ComposerAction::SetDescriptionDiffSideBySide(true));

    assert!(state.description_diff_side_by_side);
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
    state.sources.insert(("CS-1".into(), id), stale_live_source);
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
            change_set_id: "CS-1".into(),
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
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-1".into(),
        id,
        ticket: source,
    });
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

#[test]
fn adf_round_trips_literal_markdown_punctuation_and_adjacent_text_nodes() {
    let adf = json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [
                { "type": "text", "text": "literal * _ ` [ ] ~~ and \\" },
                { "type": "text", "text": "plain" },
                { "type": "text", "text": "bold", "marks": [{ "type": "strong" }] },
                { "type": "text", "text": "link", "marks": [{ "type": "link", "attrs": { "href": "https://example.com" } }] }
            ]
        }]
    });

    assert!(adf_is_safe_to_overwrite(&adf));
}

#[test]
fn jira_adf_round_trips_common_complex_descriptions() {
    let cases = [
        "- Seller cannot revise their final bid.\n  - Buyer cannot counter the seller's final bid.",
        "- Parent\n  3. Ordered child\n     - Nested bullet\n- Next parent",
        "7. Starts at seven\n   - Nested bullet\n8. Continues at eight",
        "- **Strong**, *emphasis*, ~~strike~~, `code`, and [a link](https://example.com).\n  - Child item",
        "## Decision\n\n> Quoted rationale\n\n```rust\nlet final_offer = true;\n```\n\n---\n\n- Confirmed",
    ];

    for markdown in cases {
        let adf = markdown_to_adf(markdown);
        assert!(
            adf_is_safe_to_overwrite(&adf),
            "did not round-trip: {markdown}"
        );
    }
}

#[test]
fn jira_adf_explains_unsupported_formatting() {
    let underlined = json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [{
                "type": "text", "text": "Underlined", "marks": [{ "type": "underline" }]
            }]
        }]
    });

    assert_eq!(
        adf_overwrite_warning(&underlined).as_deref(),
        Some("underlined text")
    );
}

#[test]
fn jira_adf_rejects_malformed_lossless_tags() {
    assert!(validate_markdown(
        "{{jira:panel {\"panelType\":\"info\"}}}\n{{jira:mention {\"id\":\"account-1\",\"text\":\"@Ada\"} /}}\n{{/jira:panel}}"
    )
    .is_ok());
    assert!(validate_markdown("{{jira:mention {\"id\":\"account-1\"}}}").is_err());
    assert!(validate_markdown("{{jira:mention {\"id\":\"account-1\"} /}}").is_err());
    assert!(validate_markdown("{{jira:inline-card {\"url\":\"\"} /}}").is_err());
    assert!(validate_markdown("{{jira:panel {}}}\nBody\n{{/jira:panel}}").is_err());
    assert!(validate_markdown("{{jira:panel {\"panelType\":\"info\"}}}\nMissing close").is_err());
    assert!(validate_markdown("{{/jira:panel}}").is_err());
    assert!(validate_markdown("{{/jira:panel}").is_err());
}

#[test]
fn jira_adf_round_trips_escaped_literal_tag_openings() {
    let adf = json!({ "type": "doc", "version": 1, "content": [{
        "type": "paragraph", "content": [{
            "type": "text", "text": "Show {{literal braces without a Jira tag."
        }]
    }]});

    let markdown = adf_to_markdown(&adf);
    assert_eq!(markdown, "Show \\{\\{literal braces without a Jira tag.");
    assert!(validate_markdown(&markdown).is_ok());
    assert!(adf_is_safe_to_overwrite(&adf));
}

#[test]
fn jira_adf_keeps_lossy_features_behind_the_overwrite_guard() {
    let adf = json!({ "type": "doc", "version": 1, "content": [{ "type": "mediaSingle" }]});

    assert!(!adf_is_safe_to_overwrite(&adf));
    assert_eq!(adf_overwrite_warning(&adf).as_deref(), Some("Jira media"));
}

#[test]
fn jira_adf_round_trips_tables_panels_mentions_and_smart_links() {
    let adf = json!({
        "type": "doc", "version": 1, "content": [
            { "type": "panel", "attrs": { "panelType": "info" }, "content": [{
                "type": "paragraph", "content": [
                    { "type": "text", "text": "Ask " },
                    { "type": "mention", "attrs": { "id": "account-1", "text": "@Ada" }},
                    { "type": "text", "text": " to review " },
                    { "type": "inlineCard", "attrs": { "url": "https://example.com/design" }}
                ]
            }]},
            { "type": "table", "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "attrs": {}, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Rule" }]}]},
                    { "type": "tableHeader", "attrs": {}, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Owner" }]}]}
                ]},
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "attrs": {}, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Seller | buyer" }]}]},
                    { "type": "tableCell", "attrs": {}, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Ada", "marks": [{ "type": "strong" }]}]}]}
                ]}
            ]}
        ]
    });

    assert!(adf_is_safe_to_overwrite(&adf));
    assert_eq!(
        adf_to_markdown(&adf),
        "{{jira:panel {\"panelType\":\"info\"}}}\nAsk {{jira:mention {\"id\":\"account-1\",\"text\":\"@Ada\"} /}} to review {{jira:inline-card {\"url\":\"https://example.com/design\"} /}}\n{{/jira:panel}}\n\n| Rule | Owner |\n| --- | --- |\n| Seller \\| buyer | **Ada** |"
    );
}
