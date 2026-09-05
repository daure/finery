use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    io::Write,
    net::TcpListener,
    path::PathBuf,
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
        composer_attachments::{
            AttachmentChangeKindView, AttachmentKindView, AttachmentRequest, AttachmentSource,
            TicketSnapshotView,
        },
        composer_service::{
            AssigneeInput, AttachmentSourceInput, ChangeSetPatchOperation, ComposerService,
            DraftTicketInput, ServiceError, SubmitChangeSetOutcome, TicketKindView, TicketView,
            test_service, test_service_with_submit,
        },
    },
    storage::Storage,
    store::composer::{
        AttachmentChangeKind, ChangeKind, ChangeSet, ComposerAction, ComposerState,
        SubmissionAttempt, SubmissionAttemptPhase, SubmissionSnapshot, Ticket, TicketAttachment,
        TicketChange, TicketKind,
    },
};

#[cfg(target_os = "linux")]
#[test]
fn browser_launcher_uses_the_desktop_default_browser() {
    let command = super::browser_command("https://finery.atlassian.net/browse/FIN-42");

    assert_eq!(command.get_program(), OsStr::new("gio"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            OsStr::new("open"),
            OsStr::new("https://finery.atlassian.net/browse/FIN-42")
        ]
    );
}

#[test]
fn clipboard_image_format_detection_covers_supported_formats() {
    assert_eq!(
        super::image_extension(b"\x89PNG\r\n\x1a\nrest"),
        Some("png")
    );
    assert_eq!(super::image_extension(b"\xff\xd8\xffrest"), Some("jpg"));
    assert_eq!(super::image_extension(b"GIF89arest"), Some("gif"));
    assert_eq!(super::image_extension(b"BMrest"), Some("bmp"));
    assert_eq!(super::image_extension(b"RIFF0000WEBPrest"), Some("webp"));
    assert_eq!(super::image_extension(b"\0\0\x01\0rest"), Some("ico"));
    assert_eq!(super::image_extension(b"0000ftypavif0000"), Some("avif"));
    assert_eq!(
        super::image_mime_type(b"\x89PNG\r\n\x1a\nrest"),
        Some("image/png")
    );
    assert_eq!(super::image_extension(b"plain text"), None);
}

#[test]
fn ticket_views_expose_attachment_metadata_without_attachment_content() {
    let mut ticket = ticket("FIN-1");
    ticket.attachments.push(TicketAttachment {
        id: "local-1".into(),
        filename: "design.png".into(),
        created: "2026-09-05T10:00:00Z".into(),
        size: 12,
        mime_type: Some("image/png".into()),
        content_url: Some("https://jira.example/secret".into()),
        change: AttachmentChangeKind::Added,
        local_data: Some(vec![1, 2, 3]),
    });

    let view = TicketView::from(ticket);

    assert_eq!(view.attachments.len(), 1);
    assert_eq!(view.attachments[0].id, "local-1");
    assert_eq!(view.attachments[0].filename, "design.png");
    assert_eq!(view.attachments[0].mime_type.as_deref(), Some("image/png"));
    assert_eq!(view.attachments[0].change, AttachmentChangeKindView::Added);
    assert_eq!(view.attachments[0].kind, AttachmentKindView::Image);
    assert!(view.attachments[0].content_available);
    let json = serde_json::to_value(view).unwrap();
    assert!(json["attachments"][0].get("content_url").is_none());
    assert!(json["attachments"][0].get("local_data").is_none());
}

#[test]
fn change_set_catalog_exposes_attachment_presence_without_metadata() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    let mut change_set = change_set();
    change_set.tickets[0]
        .original
        .as_mut()
        .unwrap()
        .attachments
        .push(TicketAttachment {
            id: "local-1".into(),
            filename: "design.png".into(),
            created: String::new(),
            size: 12,
            mime_type: Some("image/png".into()),
            content_url: None,
            change: AttachmentChangeKind::Added,
            local_data: Some(vec![1, 2, 3]),
        });
    runtime
        .block_on(storage.save_change_set(&change_set))
        .unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let service = test_service(storage, runtime, lookup);

    let catalog = service.change_set_catalog(false).unwrap();
    let ticket = catalog.value.change_sets[0].value.tickets[0]
        .original
        .as_ref()
        .unwrap();
    let json = serde_json::to_value(&catalog).unwrap();

    assert!(catalog.value.change_sets[0].value.has_attachments);
    assert!(ticket.has_attachments);
    assert!(
        json["change_sets"][0]["value"]["tickets"][0]["original"]
            .get("attachments")
            .is_none()
    );
}

fn ticket(key: &str) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "FIN".into(),
        title: "Original".into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        description_overwrite_warning: None,
        kind: TicketKind::Task,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
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
    }
}

fn attachment_fixture(filename: &str, data: &[u8]) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("finery-composer-attachment-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(filename);
    fs::write(&path, data).unwrap();
    path
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
fn create_change_set_persists_an_empty_open_set() {
    let service = service();

    let created = service.create_change_set("Second plan".into()).unwrap();

    assert_eq!(created.change_set.revision, 1);
    assert_eq!(created.change_set.value.id, "CS-2");
    assert_eq!(created.change_set.value.name, "Second plan");
    assert!(!created.change_set.value.closed);
    assert!(created.change_set.value.tickets.is_empty());
    assert_eq!(created.catalog_revision, 3);
    assert_eq!(
        service
            .create_change_set("Third plan".into())
            .unwrap()
            .change_set
            .value
            .id,
        "CS-3"
    );
}

#[test]
fn change_set_catalog_excludes_closed_sets_unless_requested() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime
        .block_on(storage.save_change_set(&change_set()))
        .unwrap();
    let mut closed = change_set();
    closed.id = "CS-2".into();
    closed.closed = true;
    runtime.block_on(storage.save_change_set(&closed)).unwrap();
    let lookup = Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) });
    let service = test_service(storage, runtime, lookup);

    assert_eq!(
        service
            .change_set_catalog(false)
            .unwrap()
            .value
            .change_sets
            .len(),
        1
    );
    assert_eq!(
        service
            .change_set_catalog(true)
            .unwrap()
            .value
            .change_sets
            .len(),
        2
    );
}

#[test]
fn delete_change_set_removes_only_local_composer_data() {
    let service = service();

    let deleted = service.delete_change_set("CS-1", 1).unwrap();

    assert_eq!(deleted.change_set_id, "CS-1");
    assert_eq!(deleted.catalog_revision, 3);
    assert!(matches!(
        service.change_set("CS-1"),
        Err(ServiceError::NotFound { .. })
    ));
}

#[test]
fn delete_change_set_rejects_stale_or_submitting_change_sets() {
    let service = service();

    assert!(matches!(
        service.delete_change_set("CS-1", 2),
        Err(ServiceError::StaleRevision { .. })
    ));

    let mut set = change_set();
    set.submission_attempt = Some(SubmissionAttempt {
        owner_id: "attempt-owner".into(),
        ticket_ids: vec!["FIN-1".into()],
        phase: SubmissionAttemptPhase::Claimed,
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime.block_on(storage.save_change_set(&set)).unwrap();
    let submitting = test_service(storage, runtime, Arc::new(|key: &str| Ok(ticket(key))));

    assert!(matches!(
        submitting.delete_change_set("CS-1", 1),
        Err(ServiceError::SubmissionClaimed { .. })
    ));
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
        .recover_submission_attempt("CS-1", 1, Vec::new(), Vec::new())
        .unwrap();
    assert_eq!(recovered.change_set.revision, 2);
    assert!(!recovered.change_set.value.tickets[0].submission_claimed);
}

#[test]
fn recovery_releases_marked_creates_confirmed_absent_from_jira() {
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
    let service = test_service(storage, runtime, Arc::new(|key: &str| Ok(ticket(key))));
    let recovered = service
        .recover_submission_attempt("CS-1", 1, Vec::new(), vec!["NEW-1".into()])
        .unwrap();
    assert!(!recovered.change_set.value.tickets[1].submission_claimed);
    assert!(!recovered.change_set.value.tickets[1].create_attempt);
    assert!(!recovered.change_set.value.tickets[1].submitted);
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
        Vec::new(),
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
    .recover_submission_attempt("CS-1", 1, Vec::new(), Vec::new())
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
    .recover_submission_attempt("CS-1", 1, Vec::new(), Vec::new())
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
                        story_points: None,
                        fix_versions: Vec::new(),
                        labels: Vec::new(),
                        assignee: Some(AssigneeInput {
                            name: "Ada Mensah".into(),
                            account_id: "ada".into(),
                        }),
                    },
                    parent_ticket_id: None,
                },
                ChangeSetPatchOperation::UpdateTitle {
                    ticket_id: "FIN-1".into(),
                    title: "Edited".into(),
                },
                ChangeSetPatchOperation::AddWebLink {
                    ticket_id: "NEW-1".into(),
                    link_id: "local-draft-docs".into(),
                    title: "Draft docs".into(),
                    url: "www.example.com/draft".into(),
                },
                ChangeSetPatchOperation::AddWebLink {
                    ticket_id: "FIN-1".into(),
                    link_id: "local-existing-docs".into(),
                    title: "Old docs".into(),
                    url: "https://example.com/old".into(),
                },
                ChangeSetPatchOperation::UpdateWebLink {
                    ticket_id: "FIN-1".into(),
                    link_id: "local-existing-docs".into(),
                    title: "Current docs".into(),
                    url: "https://example.com/current".into(),
                },
                ChangeSetPatchOperation::SetCommitSelection {
                    ticket_ids: vec!["FIN-1".into(), "NEW-1".into()],
                },
            ],
        )
        .unwrap();

    assert_eq!(response.change_set.revision, 2);
    assert_eq!(response.applied.len(), 6);
    assert_eq!(response.change_set.value.tickets.len(), 2);
    assert_eq!(
        response.change_set.value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .title,
        "Edited"
    );
    let existing_link = &response.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .web_links[0];
    assert_eq!(existing_link.title, "Current docs");
    assert_eq!(existing_link.url, "https://example.com/current");
    let draft_link = &response.change_set.value.tickets[1]
        .updated
        .as_ref()
        .unwrap()
        .web_links[0];
    assert_eq!(draft_link.title, "Draft docs");
    assert_eq!(draft_link.url, "https://www.example.com/draft");
    assert_eq!(
        response.change_set.value.selected_ticket_ids,
        vec!["FIN-1", "NEW-1"]
    );
    assert_eq!(
        response.change_set.value.tickets[1]
            .updated
            .as_ref()
            .unwrap()
            .assignee_account_id,
        "ada"
    );
}

#[test]
fn patch_adds_and_removes_a_file_attachment_in_persistent_storage() {
    let service = service();
    let file = b"%PDF-1.7 attachment".to_vec();
    let path = attachment_fixture("report.pdf", &file);

    let added = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::AddAttachment {
                ticket_id: "FIN-1".into(),
                filename: None,
                mime_type: Some("application/pdf".into()),
                source: AttachmentSourceInput::FilePath {
                    path: path.to_string_lossy().into_owned(),
                },
            }],
        )
        .unwrap();

    assert_eq!(added.change_set.revision, 2);
    let attachment = &added.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .attachments[0];
    assert!(attachment.id.starts_with("local-"));
    assert_eq!(attachment.filename, "report.pdf");
    assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(attachment.kind, AttachmentKindView::Other);
    assert_eq!(attachment.change, AttachmentChangeKindView::Added);
    let attachment_id = attachment.id.clone();
    let persisted = service
        .attachment_sources(
            "CS-1",
            2,
            &[AttachmentRequest {
                ticket_id: "FIN-1".into(),
                snapshot: TicketSnapshotView::Updated,
                attachment_id: attachment_id.clone(),
            }],
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(persisted.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(persisted.source, AttachmentSource::Local(file));

    let removed = service
        .apply_change_set_patch(
            "CS-1",
            2,
            vec![ChangeSetPatchOperation::RemoveAttachment {
                ticket_id: "FIN-1".into(),
                attachment_id,
            }],
        )
        .unwrap();

    assert_eq!(removed.change_set.revision, 3);
    assert!(
        removed.change_set.value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .attachments
            .is_empty()
    );
    assert!(
        service.change_set("CS-1").unwrap().value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .attachments
            .is_empty()
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn patch_adds_updates_and_removes_a_local_mermaid_diagram() {
    let service = service();
    let added = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::AddMermaidDiagram {
                ticket_id: "FIN-1".into(),
                title: "Order lifecycle".into(),
                diagram_type: "State".into(),
                markup: "stateDiagram-v2\n  [*] --> Draft\n  Draft --> Done".into(),
            }],
        )
        .unwrap();

    let diagram = &added.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .mermaid_diagrams[0];
    assert!(diagram.id.starts_with("local-diagram-"));
    assert_eq!(diagram.title, "Order lifecycle");
    assert_eq!(diagram.diagram_type, "State");
    assert!(diagram.rendered);
    assert_eq!(diagram.rendered_theme, tuicore::theme().name().id());
    let diagram_id = diagram.id.clone();

    let renamed = service
        .apply_change_set_patch(
            "CS-1",
            2,
            vec![ChangeSetPatchOperation::UpdateMermaidDiagramTitle {
                ticket_id: "FIN-1".into(),
                diagram_id: diagram_id.clone(),
                title: "Order states".into(),
            }],
        )
        .unwrap();
    assert_eq!(
        renamed.change_set.value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .mermaid_diagrams[0]
            .title,
        "Order states"
    );

    let updated = service
        .apply_change_set_patch(
            "CS-1",
            3,
            vec![ChangeSetPatchOperation::UpdateMermaidDiagramMarkup {
                ticket_id: "FIN-1".into(),
                diagram_id: diagram_id.clone(),
                markup: "stateDiagram-v2\n  [*] --> Open\n  Open --> Closed".into(),
            }],
        )
        .unwrap();
    let diagram = &updated.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .mermaid_diagrams[0];
    assert_eq!(
        diagram.markup,
        "stateDiagram-v2\n  [*] --> Open\n  Open --> Closed"
    );
    assert!(diagram.rendered);
    assert_eq!(diagram.rendered_theme, tuicore::theme().name().id());

    let removed = service
        .apply_change_set_patch(
            "CS-1",
            4,
            vec![ChangeSetPatchOperation::RemoveMermaidDiagram {
                ticket_id: "FIN-1".into(),
                diagram_id,
            }],
        )
        .unwrap();
    assert!(
        removed.change_set.value.tickets[0]
            .updated
            .as_ref()
            .unwrap()
            .mermaid_diagrams
            .is_empty()
    );
}

#[test]
fn patch_rejects_invalid_mermaid_without_persisting_other_operations() {
    let service = service();

    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::UpdateTitle {
                    ticket_id: "FIN-1".into(),
                    title: "Changed title".into(),
                },
                ChangeSetPatchOperation::AddMermaidDiagram {
                    ticket_id: "FIN-1".into(),
                    title: "Broken diagram".into(),
                    diagram_type: "Flow".into(),
                    markup: "not Mermaid".into(),
                },
            ],
        )
        .unwrap_err();

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
    let persisted = service.change_set("CS-1").unwrap();
    assert_eq!(persisted.revision, 1);
    assert_eq!(
        persisted.value.tickets[0].original.as_ref().unwrap().title,
        "Original"
    );
}

#[test]
fn patch_downloads_a_url_attachment_into_persistent_storage() {
    let file = b"%PDF-1.7 downloaded".to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            file.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&file).unwrap();
    });
    let service = service();

    let added = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::AddAttachment {
                ticket_id: "FIN-1".into(),
                filename: None,
                mime_type: Some("application/pdf".into()),
                source: AttachmentSourceInput::Url {
                    url: format!("http://{address}/download.pdf"),
                },
            }],
        )
        .unwrap();
    server.join().unwrap();

    let attachment = &added.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .attachments[0];
    assert_eq!(attachment.filename, "download.pdf");
    assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
}

#[test]
fn patch_removal_stages_a_synced_attachment_for_jira_deletion() {
    let mut set = change_set();
    set.tickets[0]
        .original
        .as_mut()
        .unwrap()
        .attachments
        .push(TicketAttachment {
            id: "jira-1".into(),
            filename: "old.png".into(),
            created: String::new(),
            size: 12,
            mime_type: Some("image/png".into()),
            content_url: Some("https://jira.example/old.png".into()),
            change: AttachmentChangeKind::Synced,
            local_data: None,
        });
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = runtime.block_on(Storage::connect_for_tests()).unwrap();
    runtime.block_on(storage.save_change_set(&set)).unwrap();
    let service = test_service(
        storage,
        runtime,
        Arc::new(|key: &str| -> Result<Ticket, String> { Ok(ticket(key)) }),
    );

    let removed = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::RemoveAttachment {
                ticket_id: "FIN-1".into(),
                attachment_id: "jira-1".into(),
            }],
        )
        .unwrap();

    let attachment = &removed.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap()
        .attachments[0];
    assert_eq!(attachment.change, AttachmentChangeKindView::Deleted);
}

#[test]
fn patch_rejects_an_image_mime_mismatch_without_persisting() {
    let service = service();
    let path = attachment_fixture("design.png", b"\xff\xd8\xffjpeg");

    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::AddAttachment {
                ticket_id: "FIN-1".into(),
                filename: None,
                mime_type: Some("image/png".into()),
                source: AttachmentSourceInput::FilePath {
                    path: path.to_string_lossy().into_owned(),
                },
            }],
        )
        .unwrap_err();

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
    assert_eq!(service.change_set("CS-1").unwrap().revision, 1);
    fs::remove_file(path).unwrap();
}

#[test]
fn patch_rejects_invalid_descriptions_without_persisting_other_operations() {
    let service = service();

    let error = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::UpdateDescription {
                    ticket_id: "FIN-1".into(),
                    description: "@mention(\"@Ada\", \"account-1\"".into(),
                },
                ChangeSetPatchOperation::StageJiraDeletion {
                    ticket_id: "FIN-1".into(),
                },
                ChangeSetPatchOperation::SetCommitSelection {
                    ticket_ids: vec!["FIN-1".into()],
                },
            ],
        )
        .unwrap_err();

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
    assert_eq!(service.change_set("CS-1").unwrap().revision, 1);
    let persisted = service.change_set("CS-1").unwrap().value;
    assert!(persisted.selected_ticket_ids.is_empty());
    assert_eq!(
        persisted.tickets[0].kind,
        super::composer_service::ChangeKindView::Synced
    );
    assert!(persisted.tickets[0].updated.is_none());
}

#[test]
fn patch_updates_ticket_metadata_and_assignee() {
    let service = service();

    let response = service
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![
                ChangeSetPatchOperation::UpdateStoryPoints {
                    ticket_id: "FIN-1".into(),
                    story_points: Some(3.0),
                },
                ChangeSetPatchOperation::UpdateFixVersions {
                    ticket_id: "FIN-1".into(),
                    fix_versions: vec!["1.2.0".into()],
                },
                ChangeSetPatchOperation::UpdateLabels {
                    ticket_id: "FIN-1".into(),
                    labels: vec!["frontend".into(), "release".into()],
                },
                ChangeSetPatchOperation::UpdateAssignee {
                    ticket_id: "FIN-1".into(),
                    assignee: Some(AssigneeInput {
                        name: "Ada Mensah".into(),
                        account_id: "ada".into(),
                    }),
                },
            ],
        )
        .unwrap();

    let ticket = response.change_set.value.tickets[0]
        .updated
        .as_ref()
        .unwrap();
    assert_eq!(ticket.story_points, Some(3.0));
    assert_eq!(ticket.fix_versions, ["1.2.0"]);
    assert_eq!(ticket.labels, ["frontend", "release"]);
    assert_eq!(ticket.assignee, "Ada Mensah");
    assert_eq!(ticket.assignee_account_id, "ada");
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
    let submit = Arc::new(move |changes: &[TicketChange], _| {
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
                    story_points: None,
                    fix_versions: Vec::new(),
                    labels: Vec::new(),
                    assignee: None,
                },
                parent_ticket_id: None,
            }],
        )
        .unwrap();

    let response = service
        .submit_change_set(
            "CS-1",
            2,
            vec!["NEW-1".into()],
            Some("Submitted plan".into()),
            false,
        )
        .unwrap();

    assert!(marker_seen.load(Ordering::SeqCst));
    assert_eq!(response.change_set.revision, 6);
    assert!(matches!(
        response.outcome,
        SubmitChangeSetOutcome::Completed { .. }
    ));
    assert_eq!(response.change_set.value.name, "Submitted plan");
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
            move |changes: &[TicketChange], _| {
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
    let submitted = thread::spawn(move || {
        submitting.submit_change_set("CS-1", 1, vec!["FIN-1".into()], None, false)
    });
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
