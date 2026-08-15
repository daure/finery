use serde_json::json;

use super::{
    ChangeKind, ComposerAction, ComposerState,
    jira_adf::{adf_to_markdown, markdown_to_adf},
};

#[test]
fn first_edit_preserves_original_and_creates_updated_snapshot() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    let original_title = state.selected_ticket().unwrap().title.clone();

    state.dispatch(ComposerAction::UpdateTitle("A safer checkout".into()));

    let change = state.selected_change().unwrap();
    assert_eq!(change.original.as_ref().unwrap().title, original_title);
    assert_eq!(change.updated.as_ref().unwrap().title, "A safer checkout");
    assert_eq!(change.kind, ChangeKind::Modified);
}

#[test]
fn deleting_ticket_keeps_original_read_only_and_visible() {
    let mut state = ComposerState::demo();
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state.dispatch(ComposerAction::MarkTicketDeleted("FIN-142".into()));

    let change = state.selected_change().unwrap();
    assert_eq!(change.kind, ChangeKind::Deleted);
    assert!(change.original.is_some());
    assert!(change.updated.is_none());
    assert!(!state.selected_is_editable());
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
