use crate::store::composer::{ChangeKind, ChangeSet, Ticket, TicketChange, TicketKind};

use super::Storage;

fn ticket(key: &str, title: &str) -> Ticket {
    Ticket {
        key: key.into(),
        title: title.into(),
        description: "Description".into(),
        kind: TicketKind::Story,
        status: "To Do".into(),
        priority: "High".into(),
        assignee: "Ada".into(),
    }
}

#[test]
fn change_sets_ticket_snapshots_and_settings_survive_round_trip() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let set = ChangeSet {
            id: "CS-1".into(),
            name: "Checkout".into(),
            tickets: vec![TicketChange {
                id: "OPS-1".into(),
                original: Some(ticket("OPS-1", "Original")),
                updated: Some(ticket("OPS-1", "Updated")),
                kind: ChangeKind::Modified,
            }],
        };

        storage.save_change_set(&set).await.unwrap();
        storage.set_setting("reader.wpm", "450").await.unwrap();
        storage.set_setting("reader.wpm", "500").await.unwrap();

        assert_eq!(storage.load_change_sets().await.unwrap(), vec![set]);
        assert_eq!(
            storage.load_settings().await.unwrap().get("reader.wpm"),
            Some(&"500".to_string())
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
            tickets: vec![TicketChange {
                id: "NEW-1".into(),
                original: None,
                updated: Some(ticket("NEW-1", "Local")),
                kind: ChangeKind::Added,
            }],
        };
        storage.save_change_set(&set).await.unwrap();

        storage.delete_change_set(&set.id).await.unwrap();

        assert!(storage.load_change_sets().await.unwrap().is_empty());
    });
}
