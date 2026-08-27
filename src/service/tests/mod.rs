use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crate::{
    app_settings::AppSettings,
    jira::{SubmitBatchOutcome, TicketSubmitOutcome},
    service::{
        AppService,
        composer_service::{
            ChangeSetPatchOperation, ComposerService, DraftTicketInput, ServiceError,
            SubmitChangeSetOutcome, TicketKindView, test_service, test_service_with_submit,
        },
    },
    storage::Storage,
    store::composer::{
        ChangeKind, ChangeSet, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
    },
};

fn ticket(key: &str) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "FIN".into(),
        title: "Original".into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        kind: TicketKind::Task,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
        parent_key: None,
        parent_title: None,
        parent_kind: None,
        has_children: false,
    }
}

fn change_set() -> ChangeSet {
    ChangeSet {
        id: "CS-1".into(),
        name: "Plan".into(),
        closed: false,
        selected_ticket_ids: Vec::new(),
        tickets: vec![TicketChange {
            id: "FIN-1".into(),
            original: Some(ticket("FIN-1")),
            updated: None,
            kind: ChangeKind::Synced,
            submitted: None,
            retry_blocked: false,
            create_attempt: false,
            sibling_order: 0,
        }],
    }
}

fn service() -> ComposerService {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&change_set()))
        .unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    test_service(storage, runtime, lookup)
}

#[test]
fn patch_persists_multiple_operations_as_one_revision() {
    let service = service();
    let response = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::AddDraftTicket {
                    ticket_id: "NEW-1".into(),
                    draft: DraftTicketInput {
                        title: "Local task".into(),
                        project_key: "FIN".into(),
                        kind: TicketKindView::Task,
                        description: "Draft detail".into(),
                    },
                    parent_ticket_id: None,
                },
                ChangeSetPatchOperation::UpdateTitle {
                    ticket_id: "FIN-1".into(),
                    title: "Edited".into(),
                },
                ChangeSetPatchOperation::SetCommitSelection {
                    ticket_ids: vec!["FIN-1".into(), "NEW-1".into()],
                },
            ],
        )
        .unwrap();

    assert_eq!(response.change_set.revision, 2);
    assert_eq!(response.applied.len(), 3);
    assert_eq!(response.change_set.value.tickets.len(), 2);
    assert_eq!(
        response.change_set.value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .title,
        "Edited"
    );
    assert_eq!(
        response.change_set.value.selected_ticket_ids,
        vec!["FIN-1", "NEW-1"]
    );
}

#[test]
fn patch_rejects_stale_revision() {
    let service = service();
    service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-1".into(),
                title: "First".into(),
            }],
        )
        .unwrap();

    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-1".into(),
                title: "Second".into(),
            }],
        )
        .unwrap_err();
    assert_eq!(
        error,
        ServiceError::StaleRevision {
            change_set_id: "CS-1".into()
        }
    );
}

#[test]
fn refresh_returns_the_refreshed_change_set() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&change_set()))
        .unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> {
        let mut refreshed = ticket(key);
        refreshed.title = "Refreshed from Jira".into();
        Ok(refreshed)
    });
    let service = test_service(storage, runtime, lookup);

    let response = service.refresh_change_set("CS-1", 1).unwrap();

    assert_eq!(response.change_set.revision, 2);
    assert_eq!(
        response.change_set.value.tickets[0]
            .original
            .as_ref()
            .unwrap()
            .title,
        "Refreshed from Jira"
    );
}

#[test]
fn workspace_refreshes_open_change_sets_and_skips_closed_ones() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    let mut open = change_set();
    open.tickets[0].kind = ChangeKind::Modified;
    open.tickets[0].updated = Some(ticket("FIN-1"));
    runtime.block_on(storage.save_change_set(&open)).unwrap();
    let mut closed = change_set();
    closed.id = "CS-2".into();
    closed.closed = true;
    closed.tickets[0].id = "FIN-2".into();
    closed.tickets[0].original = Some(ticket("FIN-2"));
    runtime.block_on(storage.save_change_set(&closed)).unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let service = test_service(storage, runtime, lookup);
    let mut refreshed = ticket("FIN-1");
    refreshed.title = "Fresh from Jira".into();

    assert_eq!(
        service.open_change_set_jira_ticket_keys().unwrap(),
        ["FIN-1"]
    );
    let catalog = service
        .refresh_open_change_set_baselines(&HashMap::from([("FIN-1".into(), refreshed)]))
        .unwrap();

    assert_eq!(
        catalog.value.change_sets[0].value.tickets[0]
            .original
            .as_ref()
            .unwrap()
            .title,
        "Fresh from Jira"
    );
    assert_eq!(
        catalog.value.change_sets[1].value.tickets[0]
            .original
            .as_ref()
            .unwrap()
            .title,
        "Original"
    );
}

#[test]
fn invalid_later_operation_leaves_earlier_operation_unpersisted() {
    let service = service();
    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::UpdateTitle {
                    ticket_id: "FIN-1".into(),
                    title: "Would persist".into(),
                },
                ChangeSetPatchOperation::UpdateDescription {
                    ticket_id: "missing".into(),
                    description: "Invalid".into(),
                },
            ],
        )
        .unwrap_err();
    assert!(matches!(error, ServiceError::NotFound { .. }));

    let persisted = service.change_set("CS-1").unwrap();
    assert_eq!(persisted.revision, 1);
    assert!(persisted.value.tickets[0].updated.is_none());
}

#[test]
fn submit_persists_create_marker_before_jira_and_reconciles_once() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&change_set()))
        .unwrap();
    let marker_seen = Arc::new(AtomicBool::new(false));
    let submitted_storage = storage.clone();
    let submitted_runtime = Arc::clone(&runtime);
    let submit_marker_seen = Arc::clone(&marker_seen);
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let submit = Arc::new(move |changes: &[TicketChange]| {
        let change_set = submitted_runtime
            .block_on(submitted_storage.load_change_set("CS-1"))
            .unwrap()
            .unwrap();
        submit_marker_seen.store(
            change_set
                .change_set
                .tickets
                .iter()
                .any(|change| change.id == "NEW-1" && change.create_attempt),
            Ordering::SeqCst,
        );
        SubmitBatchOutcome::Completed(
            changes
                .iter()
                .map(|change| TicketSubmitOutcome {
                    id: change.id.clone(),
                    result: Ok(SubmissionSnapshot {
                        original: None,
                        updated: change.updated.clone(),
                    }),
                })
                .collect(),
        )
    });
    let service = test_service_with_submit(storage, runtime, lookup, submit);
    service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::AddDraftTicket {
                ticket_id: "NEW-1".into(),
                draft: DraftTicketInput {
                    title: "Local task".into(),
                    project_key: "FIN".into(),
                    kind: TicketKindView::Task,
                    description: String::new(),
                },
                parent_ticket_id: None,
            }],
        )
        .unwrap();

    let response = service
        .submit_change_set("CS-1", 2, vec!["NEW-1".into()])
        .unwrap();

    assert!(marker_seen.load(Ordering::SeqCst));
    assert_eq!(response.change_set.revision, 4);
    assert!(matches!(
        response.outcome,
        SubmitChangeSetOutcome::Completed { .. }
    ));
    assert!(response.change_set.value.tickets[1].submitted);
}

#[test]
fn queued_tui_saves_do_not_overwrite_an_external_change_set_update_after_a_conflict() {
    let app = AppService::for_tests();
    let stale = change_set();
    app.save_change_set(stale.clone());
    app.flush().unwrap();

    app.composer_service()
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-1".into(),
                title: "External edit".into(),
            }],
        )
        .unwrap();

    let first_expected = app.queue_composer_save("CS-1").unwrap();
    let second_expected = app.queue_composer_save("CS-1").unwrap();
    assert!(
        super::save_change_set(
            &app.storage,
            &app.runtime,
            &app.composer_sync,
            stale.clone(),
            first_expected,
        )
        .is_err()
    );
    assert!(
        super::save_change_set(
            &app.storage,
            &app.runtime,
            &app.composer_sync,
            stale,
            second_expected,
        )
        .is_ok()
    );
    assert!(!app.composer_writes_pending());
    let persisted = app
        .runtime
        .block_on(app.storage.load_change_set("CS-1"))
        .unwrap()
        .unwrap();
    assert_eq!(persisted.revision, 2);
    assert_eq!(
        persisted.change_set.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .title,
        "External edit"
    );
}

#[test]
fn accepting_an_older_catalog_preserves_newer_tui_revisions() {
    let app = AppService::for_tests();
    let original = change_set();
    app.save_change_set(original.clone());
    app.flush().unwrap();
    let stale_catalog = app
        .runtime
        .block_on(app.storage.load_versioned_change_sets())
        .unwrap();

    let mut updated = original.clone();
    updated.name = "Newer TUI state".into();
    app.save_change_set(updated.clone());
    app.flush().unwrap();

    assert!(!app.accept_composer_catalog(&stale_catalog));
    assert_eq!(app.composer_catalog_revision(), 3);

    app.save_change_set(updated);
    app.flush().unwrap();
    assert_eq!(
        app.runtime
            .block_on(app.storage.load_change_set("CS-1"))
            .unwrap()
            .unwrap()
            .revision,
        3
    );
}

#[test]
fn discovered_story_points_do_not_overwrite_settings_changed_during_backlog_load() {
    let service = AppService::for_tests();
    let loaded_settings = AppSettings::default();
    service.save_settings(AppSettings {
        jira_story_points_field_id: "customfield_manual".into(),
        ..AppSettings::default()
    });

    service.save_discovered_story_points(
        &loaded_settings.jira_story_points_board_id,
        &loaded_settings.jira_story_points_field_id,
        "42".into(),
        "customfield_discovered".into(),
    );

    assert_eq!(
        service
            .settings()
            .read()
            .unwrap()
            .jira_story_points_field_id,
        "customfield_manual"
    );
}

#[test]
fn discovered_story_points_persistence_does_not_overwrite_a_concurrent_manual_edit() {
    let service = Arc::new(AppService::for_tests());
    let (discovery_started, resume_discovery) = service.pause_discovery_settings_persistence();
    let discovery_service = Arc::clone(&service);
    let discovery = thread::spawn(move || {
        discovery_service.save_discovered_story_points("", "", "42".into(), "discovered".into());
    });
    discovery_started.recv().unwrap();

    let (manual_persisted, manual_done) = mpsc::channel();
    let manual_service = Arc::clone(&service);
    let manual = thread::spawn(move || {
        manual_service.save_settings(AppSettings {
            jira_story_points_field_id: "manual".into(),
            ..AppSettings::default()
        });
        manual_persisted.send(()).unwrap();
    });
    assert!(
        manual_done
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "manual edit persisted while discovery still held the settings lock"
    );

    resume_discovery.send(()).unwrap();
    discovery.join().unwrap();
    manual.join().unwrap();
    service.flush().unwrap();

    let stored = service
        .runtime
        .block_on(service.storage.load_settings())
        .unwrap();
    assert_eq!(
        stored.get("jira.story_points_field_id"),
        Some(&"manual".to_string())
    );
}
