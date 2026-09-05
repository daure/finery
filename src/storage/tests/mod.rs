use crate::store::composer::{
    ChangeKind, ChangeSet, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
};

use super::{ConditionalSaveChangeSetOutcome, Storage};

mod revisions;
mod ticket_order;

fn ticket(key: &str, title: &str) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "OPS".into(),
        title: title.into(),
        description: "Description".into(),
        description_safe_to_overwrite: true,
        description_overwrite_warning: None,
        kind: TicketKind::Story,
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
        assignee_account_id: "ada".into(),
        story_points: None,
        fix_versions: Vec::new(),
        labels: Vec::new(),
        parent_key: None,
        parent_title: None,
        parent_kind: None,
        has_children: false,
        attachments: Vec::new(),
        mermaid_diagrams: Vec::new(),
        web_links: Vec::new(),
        issue_links: Vec::new(),
    }
}

#[test]
fn change_sets_ticket_snapshots_and_settings_survive_round_trip() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let set = ChangeSet {
            id: "CS-1".into(),
            name: "Checkout".into(),
            closed: true,
            selected_ticket_ids: vec!["OPS-1".into()],
            submission_attempt: None,
            tickets: vec![TicketChange {
                id: "OPS-1".into(),
                original: Some(ticket("OPS-1", "Original")),
                updated: Some(ticket("OPS-1", "Updated")),
                kind: ChangeKind::Modified,
                submitted: Some(SubmissionSnapshot {
                    original: Some(ticket("OPS-1", "Original")),
                    updated: Some(ticket("OPS-1", "Updated")),
                }),
                retry_blocked: false,
                create_attempt: true,
                sibling_order: 0,
            }],
        };

        storage.save_change_set(&set).await.unwrap();
        storage
            .set_settings(&[("reader.wpm", "450".into())])
            .await
            .unwrap();
        storage
            .set_settings(&[("reader.wpm", "500".into())])
            .await
            .unwrap();
        storage
            .set_settings(&[
                ("jira.story_points_board_id", "42".into()),
                ("jira.story_points_field_id", "customfield_10016".into()),
            ])
            .await
            .unwrap();

        assert_eq!(storage.load_change_sets().await.unwrap(), vec![set]);
        assert_eq!(
            storage.load_settings().await.unwrap().get("reader.wpm"),
            Some(&"500".to_string())
        );
        assert_eq!(
            storage
                .load_settings()
                .await
                .unwrap()
                .get("jira.story_points_field_id"),
            Some(&"customfield_10016".to_string())
        );
    });
}

#[test]
fn deleting_change_set_cascades_ticket_changes() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let set = ChangeSet {
            id: "CS-2".into(),
            name: "Disposable".into(),
            closed: false,
            selected_ticket_ids: vec!["NEW-1".into()],
            submission_attempt: None,
            tickets: vec![TicketChange {
                id: "NEW-1".into(),
                original: None,
                updated: Some(ticket("NEW-1", "Local")),
                kind: ChangeKind::Added,
                submitted: None,
                retry_blocked: false,
                create_attempt: false,
                sibling_order: 0,
            }],
        };
        storage.save_change_set(&set).await.unwrap();

        storage.delete_change_set(&set.id).await.unwrap();

        assert!(storage.load_change_sets().await.unwrap().is_empty());
    });
}

#[test]
fn recent_ticket_stack_keeps_unique_keys_in_open_order() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();

        storage.record_recent_ticket("OPS-1", 1, 2).await.unwrap();
        storage.record_recent_ticket("OPS-2", 2, 2).await.unwrap();
        storage.record_recent_ticket("OPS-1", 3, 2).await.unwrap();
        storage.record_recent_ticket("OPS-3", 4, 2).await.unwrap();

        assert_eq!(
            storage.load_recent_ticket_keys(15).await.unwrap(),
            vec!["OPS-3", "OPS-1"]
        );
    });
}
