use sqlx::Row;

use crate::store::composer::{ChangeKind, ChangeSet, TicketChange};

use super::{Storage, ticket};

#[test]
fn ticket_changes_load_in_persisted_sibling_order() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let mut child = ticket("OPS-1", "First by key");
        child.parent_key = Some("OUT-1".into());
        let set = ChangeSet {
            id: "CS-order".into(),
            name: "Order".into(),
            closed: false,
            selected_ticket_ids: Vec::new(),
            tickets: vec![
                TicketChange {
                    id: "OPS-9".into(),
                    original: Some(ticket("OPS-9", "Last by key")),
                    updated: None,
                    kind: ChangeKind::Synced,
                    submitted: None,
                    sibling_order: 1,
                },
                TicketChange {
                    id: "OPS-1".into(),
                    original: None,
                    updated: Some(child),
                    kind: ChangeKind::Synced,
                    submitted: None,
                    sibling_order: 0,
                },
            ],
        };

        storage.save_change_set(&set).await.unwrap();

        assert_eq!(
            storage.load_change_sets().await.unwrap()[0]
                .tickets
                .iter()
                .map(|change| change.id.as_str())
                .collect::<Vec<_>>(),
            vec!["OPS-1", "OPS-9"]
        );
        assert_eq!(
            storage.load_change_sets().await.unwrap()[0].tickets[0]
                .updated
                .as_ref()
                .unwrap()
                .parent_key
                .as_deref(),
            Some("OUT-1")
        );
    });
}

#[test]
fn column_order_overrides_legacy_json_order_and_round_trips() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let storage = Storage::connect_for_tests().await.unwrap();
        let set = ChangeSet {
            id: "CS-legacy-order".into(),
            name: "Legacy order".into(),
            closed: false,
            selected_ticket_ids: Vec::new(),
            tickets: vec![
                TicketChange {
                    id: "OPS-2".into(),
                    original: Some(ticket("OPS-2", "Second")),
                    updated: None,
                    kind: ChangeKind::Synced,
                    submitted: None,
                    sibling_order: 0,
                },
                TicketChange {
                    id: "OPS-1".into(),
                    original: Some(ticket("OPS-1", "First")),
                    updated: None,
                    kind: ChangeKind::Synced,
                    submitted: None,
                    sibling_order: 0,
                },
            ],
        };
        storage.save_change_set(&set).await.unwrap();
        sqlx::query(
            "UPDATE ticket_changes SET sibling_order = CASE ticket_id WHEN 'OPS-2' THEN 1 ELSE 0 END, payload = json_remove(payload, '$.sibling_order') WHERE change_set_id = ?",
        )
        .bind(&set.id)
        .execute(&storage.pool)
        .await
        .unwrap();

        let loaded = storage.load_change_sets().await.unwrap();
        assert_eq!(
            loaded[0]
                .tickets
                .iter()
                .map(|change| (change.id.as_str(), change.sibling_order))
                .collect::<Vec<_>>(),
            vec![("OPS-1", 0), ("OPS-2", 1)]
        );

        storage.save_change_set(&loaded[0]).await.unwrap();
        let orders = sqlx::query(
            "SELECT sibling_order FROM ticket_changes WHERE change_set_id = ? ORDER BY ticket_id",
        )
        .bind(&set.id)
        .fetch_all(&storage.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<i64, _>("sibling_order").unwrap())
        .collect::<Vec<_>>();
        assert_eq!(orders, vec![0, 1]);
    });
}
