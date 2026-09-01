use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::store::composer::{ChangeKind, ChangeSet, Ticket, TicketChange, TicketKind};

use super::{ConditionalSaveChangeSetOutcome, Storage};

#[test]
fn conditional_saves_increment_revisions_and_reject_stale_versions() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let set = ChangeSet {
            id: "CS-revision".into(),
            name: "First".into(),
            tickets: Vec::new(),
            selected_ticket_ids: Vec::new(),
            closed: false,
            submission_attempt: None,
        };

        assert_eq!(
            storage
                .load_versioned_change_sets()
                .await
                .unwrap()
                .catalog_revision,
            1
        );
        assert_eq!(storage.load_change_set_catalog_revision().await.unwrap(), 1);
        assert_eq!(
            storage
                .save_change_set_if_revision(&set, None)
                .await
                .unwrap(),
            ConditionalSaveChangeSetOutcome::Saved {
                change_set_revision: 1,
                catalog_revision: 2,
            }
        );

        let mut updated = set.clone();
        updated.name = "Second".into();
        assert_eq!(
            storage
                .save_change_set_if_revision(&updated, Some(1))
                .await
                .unwrap(),
            ConditionalSaveChangeSetOutcome::Saved {
                change_set_revision: 2,
                catalog_revision: 3,
            }
        );
        assert_eq!(
            storage
                .save_change_set_if_revision(&set, Some(1))
                .await
                .unwrap(),
            ConditionalSaveChangeSetOutcome::Conflict
        );
        assert_eq!(
            storage
                .save_change_set_if_revision(&updated, None)
                .await
                .unwrap(),
            ConditionalSaveChangeSetOutcome::Conflict
        );

        let missing = ChangeSet {
            id: "CS-missing".into(),
            ..set
        };
        assert_eq!(
            storage
                .save_change_set_if_revision(&missing, Some(1))
                .await
                .unwrap(),
            ConditionalSaveChangeSetOutcome::Conflict
        );

        assert_eq!(
            storage
                .load_change_set("CS-revision")
                .await
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        assert_eq!(storage.load_change_sets().await.unwrap(), vec![updated]);
        assert_eq!(
            storage
                .load_versioned_change_sets()
                .await
                .unwrap()
                .catalog_revision,
            3
        );
        assert_eq!(storage.load_change_set_catalog_revision().await.unwrap(), 3);
    });
}

#[test]
fn concurrent_loads_do_not_combine_change_set_headers_and_tickets() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = std::env::temp_dir().join(format!(
            "finery-storage-snapshot-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::File::create(&path).unwrap();
        let url = format!("sqlite://{}", path.display());
        let writer = Storage::connect(&url).await.unwrap();
        let reader = Storage::connect(&url).await.unwrap();
        let mut set = snapshot_set("A");
        writer.save_change_set(&set).await.unwrap();

        let writer_task = async {
            for name in ["B", "A"].into_iter().cycle().take(100) {
                set = snapshot_set(name);
                writer.save_change_set(&set).await.unwrap();
                tokio::task::yield_now().await;
            }
        };
        let reader_task = async {
            for _ in 0..100 {
                let loaded = reader
                    .load_change_set("CS-snapshot")
                    .await
                    .unwrap()
                    .unwrap();
                let ticket = loaded.change_set.tickets.first().unwrap();
                assert_eq!(
                    loaded.change_set.name,
                    ticket.original.as_ref().unwrap().title
                );
                tokio::task::yield_now().await;
            }
        };
        tokio::join!(writer_task, reader_task);
        drop(reader);
        drop(writer);
        let _ = fs::remove_file(path);
    });
}

fn snapshot_set(name: &str) -> ChangeSet {
    ChangeSet {
        id: "CS-snapshot".into(),
        name: name.into(),
        tickets: vec![TicketChange {
            id: "FIN-1".into(),
            original: Some(Ticket {
                key: "FIN-1".into(),
                project_key: "FIN".into(),
                title: name.into(),
                description: String::new(),
                description_safe_to_overwrite: true,
                description_overwrite_warning: None,
                kind: TicketKind::Task,
                status: "To Do".into(),
                priority: "Medium".into(),
                assignee: "Unassigned".into(),
                assignee_account_id: String::new(),
                parent_key: None,
                parent_title: None,
                parent_kind: None,
                has_children: false,
            }),
            updated: None,
            kind: ChangeKind::Synced,
            submitted: None,
            retry_blocked: false,
            create_attempt: false,
            sibling_order: 0,
        }],
        selected_ticket_ids: Vec::new(),
        closed: false,
        submission_attempt: None,
    }
}
