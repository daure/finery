use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
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
        ChangeKind, ChangeSet, ComposerAction, ComposerState, SubmissionAttempt,
        SubmissionAttemptPhase, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
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
        submission_attempt: None,
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
fn recovery_clears_claimed_attempt_after_process_loss() {
    let mut abandoned = change_set();
    let mut state = ComposerState::from_change_sets(vec![abandoned.clone()]);
    state
        .dispatch(ComposerAction::OpenChangeSet("CS-1".into()))
        .unwrap();
    state
        .dispatch(ComposerAction::ClaimSubmission {
            change_set_id: "CS-1".into(),
            ids: vec!["FIN-1".into()],
            owner_id: "lost-process".into(),
        })
        .unwrap();
    abandoned = state.active_set().unwrap().clone();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&abandoned))
        .unwrap();
    let recovered = test_service(storage, runtime, Arc::new(|key: &str| Ok(ticket(key))))
        .recover_submission_attempt("CS-1", 1, Vec::new())
        .unwrap();
    assert_eq!(recovered.change_set.revision, 2);
    assert!(!recovered.change_set.value.tickets[0].submission_claimed);
}

#[test]
fn recovery_requires_confirmed_jira_keys_for_marked_creates() {
    let mut state = ComposerState::from_change_sets(vec![change_set()]);
    state
        .dispatch(ComposerAction::OpenChangeSet("CS-1".into()))
        .unwrap();
    state
        .dispatch(ComposerAction::CreateTicket {
            title: "Draft".into(),
            project_key: "FIN".into(),
        })
        .unwrap();
    state
        .dispatch(ComposerAction::ClaimSubmission {
            change_set_id: "CS-1".into(),
            ids: vec!["NEW-1".into()],
            owner_id: "lost-process".into(),
        })
        .unwrap();
    state
        .dispatch(ComposerAction::MarkCreateAttempts {
            change_set_id: "CS-1".into(),
            ids: vec!["NEW-1".into()],
        })
        .unwrap();
    state
        .dispatch(ComposerAction::MarkSubmissionCreateAttempts {
            change_set_id: "CS-1".into(),
            owner_id: "lost-process".into(),
        })
        .unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(state.active_set().unwrap()))
        .unwrap();
    let service = test_service(storage, runtime, Arc::new(|key: &str| Ok(ticket(key))));
    let recovered = service
        .recover_submission_attempt("CS-1", 1, Vec::new())
        .unwrap();
    assert!(!recovered.change_set.value.tickets[1].submission_claimed);
}

#[test]
fn recovery_reconciles_started_update_delete_and_create_attempts() {
    let mut set = change_set();
    let mut desired = ticket("FIN-1");
    desired.title = "Updated".into();
    set.tickets[0].kind = ChangeKind::Modified;
    set.tickets[0].updated = Some(desired.clone());
    set.tickets.push(TicketChange {
        id: "FIN-2".into(),
        original: Some(ticket("FIN-2")),
        updated: None,
        kind: ChangeKind::Deleted,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 1,
    });
    set.tickets.push(TicketChange {
        id: "NEW-1".into(),
        original: None,
        updated: Some(ticket("FIN-3")),
        kind: ChangeKind::Added,
        submitted: None,
        retry_blocked: false,
        create_attempt: true,
        sibling_order: 2,
    });
    let mut state = ComposerState::from_change_sets(vec![set]);
    state
        .dispatch(ComposerAction::OpenChangeSet("CS-1".into()))
        .unwrap();
    state
        .dispatch(ComposerAction::ClaimSubmission {
            change_set_id: "CS-1".into(),
            ids: vec!["FIN-1".into(), "FIN-2".into(), "NEW-1".into()],
            owner_id: "lost-process".into(),
        })
        .unwrap();
    state
        .dispatch(ComposerAction::MarkSubmissionJiraStarted {
            change_set_id: "CS-1".into(),
            owner_id: "lost-process".into(),
        })
        .unwrap();
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(state.active_set().unwrap()))
        .unwrap();
    let recovered = test_service(
        storage,
        runtime,
        Arc::new(move |key: &str| match key {
            "FIN-1" => Ok(desired.clone()),
            "FIN-2" => Err("404 not found".into()),
            "FIN-3" => Ok(ticket("FIN-3")),
            _ => Err("unexpected key".into()),
        }),
    )
    .recover_submission_attempt(
        "CS-1",
        1,
        vec![super::composer_service::RecoveredCreate {
            ticket_id: "NEW-1".into(),
            jira_key: "FIN-3".into(),
        }],
    )
    .unwrap();

    assert!(
        recovered
            .change_set
            .value
            .tickets
            .iter()
            .all(|ticket| ticket.submitted)
    );
    assert!(!recovered.change_set.value.tickets[0].submission_claimed);
}

#[test]
fn recovery_reconciles_a_started_update_only_attempt() {
    let mut set = change_set();
    let mut desired = ticket("FIN-1");
    desired.title = "Updated".into();
    set.tickets[0].kind = ChangeKind::Modified;
    set.tickets[0].updated = Some(desired.clone());
    set.submission_attempt = Some(SubmissionAttempt {
        owner_id: "lost-process".into(),
        ticket_ids: vec!["FIN-1".into()],
        phase: SubmissionAttemptPhase::JiraSubmissionStarted,
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime.block_on(storage.save_change_set(&set)).unwrap();

    let recovered = test_service(
        storage,
        runtime,
        Arc::new(move |_: &str| Ok(desired.clone())),
    )
    .recover_submission_attempt("CS-1", 1, Vec::new())
    .unwrap();

    assert!(recovered.change_set.value.tickets[0].submitted);
    assert!(!recovered.change_set.value.tickets[0].submission_claimed);
}

#[test]
fn recovery_reconciles_a_started_delete_only_attempt() {
    let mut set = change_set();
    set.tickets[0].kind = ChangeKind::Deleted;
    set.tickets[0].updated = None;
    set.submission_attempt = Some(SubmissionAttempt {
        owner_id: "lost-process".into(),
        ticket_ids: vec!["FIN-1".into()],
        phase: SubmissionAttemptPhase::JiraSubmissionStarted,
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime.block_on(storage.save_change_set(&set)).unwrap();

    let recovered = test_service(
        storage,
        runtime,
        Arc::new(|_: &str| Err("404 not found".into())),
    )
    .recover_submission_attempt("CS-1", 1, Vec::new())
    .unwrap();

    assert!(recovered.change_set.value.tickets[0].submitted);
    assert!(!recovered.change_set.value.tickets[0].submission_claimed);
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
fn patch_selects_a_staged_deletion_in_the_same_atomic_request() {
    let service = service();

    let response = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::StageJiraDeletion {
                    ticket_id: "FIN-1".into(),
                },
                ChangeSetPatchOperation::SetCommitSelection {
                    ticket_ids: vec!["FIN-1".into()],
                },
            ],
        )
        .unwrap();

    assert_eq!(response.change_set.value.selected_ticket_ids, ["FIN-1"]);
    assert_eq!(
        response.change_set.value.tickets[0].kind,
        super::composer_service::ChangeKindView::Deleted
    );
}

#[test]
fn patch_rejects_a_submitted_draft_jira_key_alias() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    let mut submitted_draft = TicketChange {
        id: "NEW-1".into(),
        original: None,
        updated: Some(ticket("FIN-9")),
        kind: ChangeKind::Added,
        submitted: Some(SubmissionSnapshot {
            original: None,
            updated: Some(ticket("FIN-9")),
        }),
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    };
    submitted_draft.updated.as_mut().unwrap().key = "FIN-9".into();
    let mut set = change_set();
    set.tickets = vec![submitted_draft];
    runtime.block_on(storage.save_change_set(&set)).unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let service = test_service(storage, runtime, lookup);

    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::IncludeJiraTicket {
                jira_key: "FIN-9".into(),
                parent_ticket_id: None,
            }],
        )
        .unwrap_err();

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
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
    assert_eq!(response.change_set.revision, 6);
    assert!(matches!(
        response.outcome,
        SubmitChangeSetOutcome::Completed { .. }
    ));
    assert!(response.change_set.value.tickets[1].submitted);
}

#[test]
fn submission_claim_blocks_patch_and_refresh_until_jira_reconciliation() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&change_set()))
        .unwrap();
    let (jira_started, jira_started_receiver) = mpsc::channel();
    let (resume_jira, resume_jira_receiver) = mpsc::channel();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let service = test_service_with_submit(
        storage,
        runtime,
        lookup,
        Arc::new({
            let receiver = Mutex::new(resume_jira_receiver);
            move |changes: &[TicketChange]| {
                jira_started.send(()).unwrap();
                receiver.lock().unwrap().recv().unwrap();
                SubmitBatchOutcome::Completed(
                    changes
                        .iter()
                        .map(|change| TicketSubmitOutcome {
                            id: change.id.clone(),
                            result: Ok(SubmissionSnapshot {
                                original: change.original.clone(),
                                updated: change.updated.clone(),
                            }),
                        })
                        .collect(),
                )
            }
        }),
    );
    let submitting = service.clone();
    let submitted =
        thread::spawn(move || submitting.submit_change_set("CS-1", 1, vec!["FIN-1".into()]));
    jira_started_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    assert!(matches!(
        service.apply_change_set_patch(
            "CS-1",
            3,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-1".into(),
                title: "Concurrent edit".into(),
            }],
        ),
        Err(ServiceError::SubmissionClaimed { .. })
    ));
    assert!(matches!(
        service.refresh_change_set("CS-1", 3),
        Err(ServiceError::SubmissionClaimed { .. })
    ));

    resume_jira.send(()).unwrap();
    assert!(submitted.join().unwrap().is_ok());
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
        loaded_settings.jira_story_points_discovery_complete,
        "42".into(),
        "customfield_discovered".into(),
        true,
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
        discovery_service.save_discovered_story_points(
            "",
            "",
            false,
            "42".into(),
            "discovered".into(),
            true,
        );
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

#[test]
fn cloned_services_share_live_settings() {
    let service = AppService::for_tests();
    let http_service = service.clone();
    service.save_settings(AppSettings {
        jira_default_board: "42".into(),
        ..AppSettings::default()
    });

    assert_eq!(
        http_service.settings().read().unwrap().jira_default_board,
        "42"
    );
}
