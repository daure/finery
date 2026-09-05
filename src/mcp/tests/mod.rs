use std::{io::Write, net::TcpListener, sync::Arc, thread};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rmcp::model::ResourceContents;

use crate::{
    mcp::{
        CHANGE_SET_GUIDANCE, GetChangeSetAttachments, JiraPosition, JiraSection,
        attachment_contents, final_order, issue_sections, placement_rank_plan, run_composer,
        section_order, validate_issue_keys, workspace_capacity_guidance_view, workspace_view,
    },
    service::{
        AppService,
        composer_attachments::{AttachmentRequest, TicketSnapshotView},
        composer_service::{
            ChangeSetCatalogChangeSetView, ChangeSetCatalogView, JiraTicketLookup, Versioned,
            test_service,
        },
    },
    storage::Storage,
    store::{
        composer::{
            AttachmentChangeKind, ChangeKind, ChangeSet, Ticket, TicketAttachment, TicketChange,
            TicketKind,
        },
        work_items::{
            BacklogRunway, BacklogSnapshot, RunwayCapacitySource, RunwayTicket, Sprint,
            VelocityReport, VelocitySprint, WorkItem,
        },
    },
};

#[test]
fn composer_calls_complete_from_async_runtime() {
    let setup_runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = setup_runtime
        .block_on(Storage::connect_for_tests())
        .unwrap();
    let jira: Arc<dyn JiraTicketLookup> =
        Arc::new(|_: &str| -> Result<Ticket, String> { Err("not used".into()) });
    let service = test_service(storage, setup_runtime, jira);
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_composer(service, |service| {
            service.change_set_catalog(true)
        }));

    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn attachment_calls_return_images_text_blobs_and_partial_errors() {
    let remote_image = b"\xff\xd8\xffremote".to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            remote_image.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(&remote_image).unwrap();
    });
    let service = AppService::for_tests();
    {
        let settings = service.settings();
        let mut settings = settings.write().unwrap();
        settings.jira_base_url = format!("http://{address}");
        settings.jira_email = "agent@example.com".into();
        settings.jira_api_token = "token".into();
    }
    let local_image = b"\x89PNG\r\n\x1a\nlocal".to_vec();
    let ticket = Ticket {
        key: "FIN-1".into(),
        project_key: "FIN".into(),
        title: "Images".into(),
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
        attachments: vec![
            TicketAttachment {
                id: "local-1".into(),
                filename: "local.png".into(),
                created: String::new(),
                size: local_image.len() as u64,
                mime_type: Some("image/png".into()),
                content_url: None,
                change: AttachmentChangeKind::Added,
                local_data: Some(local_image.clone()),
            },
            TicketAttachment {
                id: "jira-1".into(),
                filename: "remote.jpg".into(),
                created: String::new(),
                size: 11,
                mime_type: Some("image/jpeg".into()),
                content_url: Some(format!("http://{address}/remote.jpg")),
                change: AttachmentChangeKind::Synced,
                local_data: None,
            },
            TicketAttachment {
                id: "text-1".into(),
                filename: "notes.txt".into(),
                created: String::new(),
                size: 5,
                mime_type: Some("text/plain".into()),
                content_url: None,
                change: AttachmentChangeKind::Added,
                local_data: Some(b"hello".to_vec()),
            },
            TicketAttachment {
                id: "pdf-1".into(),
                filename: "report.pdf".into(),
                created: String::new(),
                size: 8,
                mime_type: Some("application/pdf".into()),
                content_url: None,
                change: AttachmentChangeKind::Added,
                local_data: Some(b"%PDF-1.7".to_vec()),
            },
        ],
        mermaid_diagrams: Vec::new(),
        web_links: Vec::new(),
    };
    service.save_change_set(ChangeSet {
        id: "CS-images".into(),
        name: "Images".into(),
        tickets: vec![TicketChange {
            id: "FIN-1".into(),
            original: Some(ticket),
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
    });
    service.flush().unwrap();

    let content = attachment_contents(
        &service,
        GetChangeSetAttachments {
            change_set_id: "CS-images".into(),
            expected_revision: 1,
            attachments: vec![
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "local-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "text-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "pdf-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "jira-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "local-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "local-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "local-1".into(),
                },
                AttachmentRequest {
                    ticket_id: "FIN-1".into(),
                    snapshot: TicketSnapshotView::Original,
                    attachment_id: "missing".into(),
                },
            ],
        },
    )
    .unwrap();
    server.join().unwrap();

    let images = content
        .iter()
        .filter_map(|content| content.as_image())
        .collect::<Vec<_>>();
    assert_eq!(images.len(), 5);
    assert_eq!(images[0].mime_type, "image/png");
    assert_eq!(images[0].data, BASE64_STANDARD.encode(local_image));
    assert_eq!(images[1].mime_type, "image/jpeg");
    let resources = content
        .iter()
        .filter_map(|content| content.as_resource())
        .collect::<Vec<_>>();
    assert_eq!(resources.len(), 2);
    assert!(matches!(
        &resources[0].resource,
        ResourceContents::TextResourceContents { mime_type, text, .. }
            if mime_type.as_deref() == Some("text/plain") && text == "hello"
    ));
    assert!(matches!(
        &resources[1].resource,
        ResourceContents::BlobResourceContents { mime_type, blob, .. }
            if mime_type.as_deref() == Some("application/pdf")
                && blob == &BASE64_STANDARD.encode(b"%PDF-1.7")
    ));
    assert!(content.iter().any(|content| {
        content
            .as_text()
            .is_some_and(|text| text.text.contains("attachment not found"))
    }));

    let stale = attachment_contents(
        &service,
        GetChangeSetAttachments {
            change_set_id: "CS-images".into(),
            expected_revision: 2,
            attachments: vec![AttachmentRequest {
                ticket_id: "FIN-1".into(),
                snapshot: TicketSnapshotView::Original,
                attachment_id: "local-1".into(),
            }],
        },
    )
    .unwrap_err();
    assert!(stale.starts_with("stale_revision:"));
}

#[test]
fn change_set_guidance_includes_canonical_jira_description_tags() {
    assert!(CHANGE_SET_GUIDANCE.contains("{{jira:panel"));
    assert!(CHANGE_SET_GUIDANCE.contains("@mention("));
    assert!(CHANGE_SET_GUIDANCE.contains("@card("));
    assert!(CHANGE_SET_GUIDANCE.contains("@date("));
    assert!(CHANGE_SET_GUIDANCE.contains("{{jira:task-list"));
    assert!(CHANGE_SET_GUIDANCE.contains("accept_unsafe_description_overwrite"));
}

fn work_item(index: usize) -> WorkItem {
    WorkItem {
        key: format!("FIN-{index}"),
        title: format!("Title for FIN-{index}"),
        kind: "Story".into(),
        status: "To Do".into(),
        done: false,
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        labels: Vec::new(),
        fix_versions: Vec::new(),
        epic_name: None,
        story_points: Some(index as f64),
    }
}

#[test]
fn workspace_compacts_backlog_and_change_set_payloads() {
    let open = ChangeSetCatalogChangeSetView {
        id: "CS-open".into(),
        name: "Open".into(),
        closed: false,
        selected_ticket_ids: Vec::new(),
        tickets: Vec::new(),
        has_attachments: true,
    };
    let closed = ChangeSetCatalogChangeSetView {
        id: "CS-closed".into(),
        name: "Closed".into(),
        closed: true,
        selected_ticket_ids: Vec::new(),
        tickets: Vec::new(),
        has_attachments: false,
    };
    let mut sprint_tickets = (0..51).map(work_item).collect::<Vec<_>>();
    sprint_tickets[0].done = true;
    sprint_tickets[0].story_points = None;
    let view = workspace_view(
        BacklogSnapshot {
            board_name: "Finery".into(),
            story_points_configured: true,
            sprints: vec![Sprint {
                id: 1,
                name: "Sprint 1".into(),
                state: "active".into(),
                goal: Some("Ship workspace planning".into()),
                start_date: Some("2026-08-01T09:00:00.000Z".into()),
                end_date: Some("2026-08-14T17:00:00.000Z".into()),
                work_items: sprint_tickets,
                capacity: None,
            }],
            work_items: (51..102).map(work_item).collect(),
            top_level_backlog_keys: Vec::new(),
            warnings: vec!["Story points are unavailable".into()],
            runway: Some(BacklogRunway {
                capacity: 20.5,
                source: RunwayCapacitySource::JiraVelocity,
                estimated_points: 100.0,
                assumed_points: 12.0,
                tickets: (51..102)
                    .map(|index| RunwayTicket {
                        key: format!("FIN-{index}"),
                        virtual_sprint: 1,
                        effective_points: index as f64,
                        assumed: true,
                        assumed_from_average: true,
                    })
                    .collect(),
            }),
            velocity: Some(VelocityReport {
                configured_sprints: 2,
                dynamic_capacity: Some(20.5),
                sprints: vec![
                    VelocitySprint {
                        id: 10,
                        name: "Sprint 0".into(),
                        completed: 22.0,
                        goal: Some("Finish prior work".into()),
                        work_items: None,
                    },
                    VelocitySprint {
                        id: 9,
                        name: "Sprint -1".into(),
                        completed: 20.0,
                        goal: None,
                        work_items: None,
                    },
                    VelocitySprint {
                        id: 8,
                        name: "Sprint -2".into(),
                        completed: 18.0,
                        goal: None,
                        work_items: None,
                    },
                ],
            }),
        },
        Versioned {
            revision: 7,
            value: ChangeSetCatalogView {
                change_sets: vec![
                    Versioned {
                        revision: 2,
                        value: open,
                    },
                    Versioned {
                        revision: 3,
                        value: closed,
                    },
                ],
            },
        },
    )
    .unwrap();

    assert_eq!(view.backlog.sprints[0].tickets.len(), 51);
    assert_eq!(view.backlog.unplanned.tickets.len(), 50);
    assert_eq!(view.backlog.unplanned.total_count, 51);
    assert_eq!(view.backlog.warnings, ["Story points are unavailable"]);
    assert_eq!(
        view.backlog
            .velocity
            .as_ref()
            .unwrap()
            .average_completed_points,
        Some(20.5)
    );
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[0].completed_points,
        22.0
    );
    assert_eq!(
        view.backlog.capacity_guidance.as_ref().unwrap().source,
        "jira_velocity"
    );
    let guidance = view.backlog.capacity_guidance.as_ref().unwrap();
    assert_eq!(guidance.first_unplanned_capacity_band.len(), 50);
    assert_eq!(guidance.first_unplanned_capacity_band[0].key, "FIN-51");
    assert_eq!(
        guidance.first_unplanned_capacity_band[0].effective_points,
        51.0
    );
    assert_eq!(view.backlog.velocity.as_ref().unwrap().sprints.len(), 2);
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[1].name,
        "Sprint -1"
    );
    assert_eq!(
        view.backlog.sprints[0].end_date.as_deref(),
        Some("2026-08-14T17:00:00.000Z")
    );
    assert_eq!(
        view.backlog.sprints[0].goal.as_deref(),
        Some("Ship workspace planning")
    );
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[0]
            .goal
            .as_deref(),
        Some("Finish prior work")
    );
    assert_eq!(view.change_sets.len(), 1);
    assert_eq!(view.change_sets[0].id, "CS-open");
    assert_eq!(view.change_sets[0].revision, 2);
    assert!(view.change_sets[0].has_attachments);

    let json = serde_json::to_value(view).unwrap();
    assert!(json.get("change_set_catalog_revision").is_none());
    assert!(json["change_sets"][0].get("tickets").is_none());
    assert!(json["change_sets"][0].get("closed").is_none());
    assert!(json["backlog"].get("unplanned_tickets").is_none());
    assert!(json["backlog"].get("runway").is_none());
    let sparse_ticket = &json["backlog"]["sprints"][0]["tickets"][0];
    assert_eq!(sparse_ticket["done"], true);
    assert!(sparse_ticket.get("parent").is_none());
    assert!(sparse_ticket.get("has_children").is_none());
    assert!(sparse_ticket.get("story_points").is_none());
}

#[test]
fn workspace_caps_velocity_projection_at_ten_sprints() {
    let view = workspace_view(
        BacklogSnapshot {
            board_name: "Finery".into(),
            story_points_configured: true,
            sprints: Vec::new(),
            work_items: Vec::new(),
            top_level_backlog_keys: Vec::new(),
            warnings: Vec::new(),
            runway: None,
            velocity: Some(VelocityReport {
                configured_sprints: 11,
                dynamic_capacity: Some(20.5),
                sprints: (0..11)
                    .map(|index| VelocitySprint {
                        id: index,
                        name: format!("Sprint {index}"),
                        completed: index as f64,
                        goal: None,
                        work_items: None,
                    })
                    .collect(),
            }),
        },
        Versioned {
            revision: 1,
            value: ChangeSetCatalogView {
                change_sets: Vec::new(),
            },
        },
    )
    .unwrap();

    let serialized_view = serde_json::to_value(view).unwrap();
    let velocity_sprints = serialized_view["backlog"]["velocity"]["sprints"]
        .as_array()
        .unwrap();
    assert_eq!(velocity_sprints.len(), 10);
    assert_eq!(velocity_sprints.last().unwrap()["name"], "Sprint 9");
}

#[test]
fn capacity_guidance_uses_key_references_and_points_sources() {
    let mut fixed = work_item(2);
    fixed.story_points = None;
    let mut bug = work_item(3);
    bug.kind = "Bug".into();
    bug.story_points = None;
    let mut average = work_item(4);
    average.story_points = None;
    let guidance = workspace_capacity_guidance_view(
        BacklogRunway {
            capacity: 20.0,
            source: RunwayCapacitySource::Fixed,
            estimated_points: 3.0,
            assumed_points: 0.0,
            tickets: vec![
                RunwayTicket {
                    key: "FIN-1".into(),
                    virtual_sprint: 1,
                    effective_points: 1.0,
                    assumed: false,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-2".into(),
                    virtual_sprint: 1,
                    effective_points: 2.0,
                    assumed: true,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-3".into(),
                    virtual_sprint: 1,
                    effective_points: 0.0,
                    assumed: false,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-4".into(),
                    virtual_sprint: 1,
                    effective_points: 2.0,
                    assumed: true,
                    assumed_from_average: true,
                },
            ],
        },
        &[work_item(1), fixed, bug, average],
    )
    .unwrap();

    assert_eq!(
        guidance
            .first_unplanned_capacity_band
            .iter()
            .map(|candidate| (candidate.key.as_str(), candidate.effective_points))
            .collect::<Vec<_>>(),
        [
            ("FIN-1", 1.0),
            ("FIN-2", 2.0),
            ("FIN-3", 0.0),
            ("FIN-4", 2.0)
        ]
    );
    let json = serde_json::to_value(guidance).unwrap();
    let band = json["first_unplanned_capacity_band"].as_array().unwrap();
    assert_eq!(band[0].as_object().unwrap().len(), 3);
    assert!(band[0].get("title").is_none());
    assert_eq!(
        band.iter()
            .map(|candidate| candidate["points_source"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "story_points",
            "fixed_assumption",
            "unestimated_bug",
            "average_assumption",
        ]
    );
}

#[test]
fn placement_order_keeps_multiple_moved_tickets_in_requested_order() {
    let order = final_order(
        &[
            "FIN-1".into(),
            "FIN-2".into(),
            "FIN-3".into(),
            "FIN-4".into(),
        ],
        &["FIN-4".into(), "FIN-2".into()],
        &JiraPosition::After {
            issue_key: "FIN-1".into(),
        },
    )
    .unwrap();

    assert_eq!(order, ["FIN-1", "FIN-4", "FIN-2", "FIN-3"]);
}

#[test]
fn placement_plan_can_order_an_entire_destination() {
    let plan = placement_rank_plan(
        &["FIN-3".into(), "FIN-2".into(), "FIN-1".into()],
        &["FIN-3".into(), "FIN-2".into(), "FIN-1".into()],
    )
    .unwrap()
    .unwrap();

    assert_eq!(plan.issues, ["FIN-2", "FIN-1"]);
    assert_eq!(plan.rank_after_issue.as_deref(), Some("FIN-3"));
    assert_eq!(plan.rank_before_issue, None);
}

#[test]
fn backlog_bottom_ranks_after_hidden_epics() {
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![work_item(1), work_item(2)],
        top_level_backlog_keys: vec!["FIN-1".into(), "FIN-2".into(), "FIN-EPIC".into()],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };
    let order = section_order(&snapshot, JiraSection::Backlog).unwrap();
    let final_order = final_order(&order, &["FIN-2".into()], &JiraPosition::Bottom).unwrap();
    let plan = placement_rank_plan(&["FIN-2".into()], &final_order)
        .unwrap()
        .unwrap();

    assert_eq!(plan.rank_after_issue.as_deref(), Some("FIN-EPIC"));
}

#[test]
fn embedded_backlog_subtasks_are_rejected_before_ranking_or_swapping() {
    let mut subtask = work_item(2);
    subtask.kind = "Sub-task".into();
    subtask.parent_key = Some("FIN-1".into());
    let snapshot = BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: vec![work_item(1), subtask],
        top_level_backlog_keys: vec!["FIN-1".into()],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };

    let error = validate_issue_keys(&["FIN-2".into()], &issue_sections(&snapshot)).unwrap_err();

    assert_eq!(
        error,
        "Issue 'FIN-2' is not in this board's active, future, or backlog sections"
    );
}
