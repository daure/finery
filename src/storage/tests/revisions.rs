use crate::store::composer::ChangeSet;

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
