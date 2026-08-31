use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

use serde_json::json;

use super::{
    AgileBoard, AgileIssuePage, BACKLOG_FIELDS, BACKLOG_JQL, COMPOSER_FIELDS, ISSUE_FIELDS,
    JiraIssue, JiraSprint, MAX_VELOCITY_GOAL_LOOKUPS, SubmitBatchOutcome, ambiguous_create_failure,
    backlog_page_complete, board_backlog, board_backlog_query, board_sprints, board_velocity,
    commit_order, composer_fields, create_available_statuses_from_value, create_issue_fields,
    create_issue_type, create_issue_types_from_value, create_response_failure,
    created_issue_failure, discover_story_points, fetch_composer_issues, is_ticket_number_query,
    issue_fields, issue_key_jql, move_payload, options_from_values, rank_payload,
    same_jira_content, search_composer_issues, search_jql, select_backlog_board,
    should_discover_story_points, sprint_issues, story_points_field_for_load,
    story_points_field_id, story_points_warning, submit_failure, submit_ordered_changes, to_ticket,
    to_ticket_and_work_item, to_work_item, to_work_item_with_subtasks, update_payload,
    velocity_average, velocity_report,
};
use crate::{
    app_settings::AppSettings,
    jira::submit_changes,
    store::{
        composer::{ChangeKind, Ticket, TicketChange, TicketKind},
        work_items::RankPlan,
    },
};

fn ticket(key: &str, kind: TicketKind, parent_key: Option<&str>) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "FIN".into(),
        title: key.into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        kind,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
        parent_key: parent_key.map(str::to_owned),
        parent_title: None,
        parent_kind: None,
        has_children: false,
    }
}

fn jira_settings(base_url: String) -> AppSettings {
    AppSettings {
        jira_base_url: base_url,
        jira_email: "user@example.com".into(),
        jira_api_token: "token".into(),
        jira_default_project: "FIN".into(),
        jira_story_points_field_id: "customfield_10016".into(),
        ..AppSettings::default()
    }
}

fn one_request_server(listener: TcpListener, body: String) -> thread::JoinHandle<(String, bool)> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let size = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        )
        .unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::sleep(Duration::from_millis(50));
        let extra_request = listener.accept().is_ok();
        (
            String::from_utf8_lossy(&request[..size]).into_owned(),
            extra_request,
        )
    })
}

#[test]
fn moving_issues_uses_the_agile_batch_payload() {
    assert_eq!(
        move_payload(&["FIN-1".into(), "FIN-2".into()]),
        json!({ "issues": ["FIN-1", "FIN-2"] })
    );
}

fn added(key: &str, kind: TicketKind, parent_key: Option<&str>) -> TicketChange {
    TicketChange {
        id: key.into(),
        original: None,
        updated: Some(ticket(key, kind, parent_key)),
        kind: ChangeKind::Added,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    }
}

#[test]
fn jira_search_scopes_free_text_but_not_issue_keys() {
    assert_eq!(
        search_jql("", Some("FIN")),
        "project = \"FIN\" AND updated >= -90d ORDER BY updated DESC"
    );
    assert!(search_jql("checkout words", Some("FIN")).starts_with("project = \"FIN\" AND"));
    assert!(search_jql("kan", Some("FIN")).contains("summary ~ \"kan*\""));
    let exact = search_jql("OPS-42", Some("FIN"));
    assert!(exact.contains("key = \"OPS-42\""));
    assert!(!exact.contains("project ="));
    assert!(exact.ends_with("ORDER BY updated DESC"));
    assert_eq!(
        issue_key_jql("FIN-1000"),
        "key = \"FIN-1000\" ORDER BY updated DESC"
    );
}

#[test]
fn composer_fetch_fields_combine_ticket_and_presentation_fields() {
    let fields = composer_fields(Some("customfield_10016"));

    assert!(ISSUE_FIELDS.iter().all(|field| fields.contains(field)));
    assert!(BACKLOG_FIELDS.iter().all(|field| fields.contains(field)));
    assert!(COMPOSER_FIELDS.iter().all(|field| fields.contains(field)));
    assert!(fields.contains(&"customfield_10016"));
    assert!(is_ticket_number_query("42"));
    assert!(!is_ticket_number_query("KAN-42"));
}

#[test]
fn composer_source_fetch_uses_one_bulk_request_and_maps_both_models() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let settings = jira_settings(format!("http://{}", listener.local_addr().unwrap()));
    let server = one_request_server(
        listener,
        json!({
            "issues": [{
                "key": "FIN-1",
                "fields": {
                    "summary": "Ship search",
                    "project": { "key": "FIN" },
                    "issuetype": { "name": "Story" },
                    "fixVersions": [{ "name": "1.0" }],
                    "customfield_10016": 3
                }
            }]
        })
        .to_string(),
    );

    let issues = fetch_composer_issues(&settings, &["FIN-1".into()]).unwrap();
    let (request, extra_request) = server.join().unwrap();

    let issue = issues.get("FIN-1").unwrap();
    assert_eq!(issue.ticket.title, "Ship search");
    assert_eq!(issue.work_item.fix_versions, ["1.0"]);
    assert_eq!(issue.work_item.story_points, Some(3.0));
    assert!(request.starts_with("POST /rest/api/3/issue/bulkfetch"));
    assert!(request.contains("\"description\""));
    assert!(request.contains("\"fixVersions\""));
    assert!(request.contains("\"customfield_10016\""));
    assert!(!extra_request);
}

#[test]
fn jira_search_uses_one_request_with_rich_presentation_fields() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let settings = jira_settings(format!("http://{}", listener.local_addr().unwrap()));
    let server = one_request_server(
        listener,
        json!({
            "issues": [
                { "key": "FIN-2", "fields": { "summary": "Second", "fixVersions": [{ "name": "2.0" }] } },
                { "key": "FIN-1", "fields": { "summary": "First", "fixVersions": [{ "name": "1.0" }] } }
            ]
        })
        .to_string(),
    );

    let issues = search_composer_issues(&settings, "search").unwrap();
    let (request, extra_request) = server.join().unwrap();

    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.work_item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-2", "FIN-1"]
    );
    assert_eq!(issues[0].work_item.fix_versions, ["2.0"]);
    assert!(request.starts_with("POST /rest/api/3/search/jql"));
    assert!(request.contains("\"description\""));
    assert!(request.contains("\"fixVersions\""));
    assert!(!extra_request);
}

#[test]
fn jira_metadata_options_keep_ids_and_labels() {
    let values = vec![
        json!({ "id": "1", "name": "Story" }),
        json!({ "id": "2", "name": "Bug" }),
    ];

    let options = options_from_values(Some(&values));

    assert_eq!(options[0].id, "1");
    assert_eq!(options[0].label, "Story");
    assert_eq!(options[1].label, "Bug");
}

#[test]
fn jira_project_create_metadata_reads_issue_type_ids() {
    let issue_types = create_issue_types_from_value(&json!({
        "issueTypes": [
            { "id": "10001", "name": "Story" },
            { "id": "10003", "name": "Subtask", "subtask": true }
        ]
    }));

    assert_eq!(issue_types[1].id, "10003");
    assert_eq!(issue_types[1].label, "Subtask");
}

#[test]
fn jira_project_statuses_match_ticket_kind_and_keep_status_ids() {
    let statuses = create_available_statuses_from_value(
        &json!([
            {
                "id": "10001",
                "name": "Story",
                "statuses": [
                    { "id": "10000", "name": "To Do" },
                    { "id": "10001", "name": "In Progress" }
                ]
            },
            {
                "id": "10003",
                "name": "Sub-task",
                "statuses": [{ "id": "10002", "name": "Done" }]
            }
        ]),
        TicketKind::Story,
    );

    assert_eq!(
        statuses,
        vec![
            super::JiraOption {
                id: "10000".into(),
                label: "To Do".into(),
            },
            super::JiraOption {
                id: "10001".into(),
                label: "In Progress".into(),
            },
        ]
    );
}

#[test]
fn jira_issue_maps_ticket_and_presentation_fields_together() {
    let (ticket, work_item) = to_ticket_and_work_item(
        JiraIssue {
            key: "OPS-42".into(),
            fields: json!({
                "summary": "Retry checkout",
                "project": { "key": "OPS" },
                "issuetype": { "name": "Story" },
                "status": { "name": "In Progress" },
                "priority": { "name": "High" },
                "assignee": { "displayName": "Ada", "accountId": "ada-1" },
                "parent": {
                    "key": "OPS-1",
                    "fields": {
                        "summary": "Checkout",
                        "issuetype": { "name": "Epic" }
                    }
                },
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": "Keep basket state." }]
                    }]
                }
            }),
        },
        None,
    );

    assert_eq!(ticket.key, "OPS-42");
    assert_eq!(ticket.project_key, "OPS");
    assert_eq!(ticket.kind, TicketKind::Story);
    assert_eq!(ticket.status, "In Progress");
    assert_eq!(ticket.description, "Keep basket state.");
    assert_eq!(ticket.assignee, "Ada");
    assert_eq!(ticket.assignee_account_id, "ada-1");
    assert_eq!(ticket.parent_key.as_deref(), Some("OPS-1"));
    assert_eq!(ticket.parent_title.as_deref(), Some("Checkout"));
    assert_eq!(ticket.parent_kind, Some(TicketKind::Epic));
    assert_eq!(work_item.epic_name.as_deref(), Some("Checkout"));
    assert!(ISSUE_FIELDS.contains(&"parent"));
    assert!(ISSUE_FIELDS.contains(&"subtasks"));
}

#[test]
fn jira_parent_fields_and_update_payload_follow_cloud_v3_shape() {
    let original = ticket("FIN-2", TicketKind::Task, Some("FIN-1"));
    let desired = ticket("FIN-2", TicketKind::Task, Some("FIN-9"));

    assert_eq!(
        issue_fields(&desired, None, true).pointer("/parent/key"),
        Some(&json!("FIN-9"))
    );
    assert_eq!(
        update_payload(&original, &desired, None)
            .unwrap()
            .pointer("/fields/parent/key"),
        Some(&json!("FIN-9"))
    );
}

#[test]
fn jira_create_subtask_uses_project_issue_type_id() {
    let desired = ticket("NEW-2", TicketKind::Subtask, Some("KAN-49"));
    let issue_type = create_issue_type(
        vec![
            super::JiraOption {
                id: "10001".into(),
                label: "Story".into(),
            },
            super::JiraOption {
                id: "10003".into(),
                label: "Subtask".into(),
            },
        ],
        &desired,
    )
    .unwrap();

    assert_eq!(
        create_issue_fields(&desired, None, &issue_type).pointer("/issuetype/id"),
        Some(&json!("10003"))
    );
    assert_eq!(
        create_issue_fields(&desired, None, &issue_type).pointer("/parent/key"),
        Some(&json!("KAN-49"))
    );
}

#[test]
fn jira_parent_removal_uses_update_operation_and_rejects_root_subtasks() {
    let original = ticket("FIN-2", TicketKind::Task, Some("FIN-1"));
    let root = ticket("FIN-2", TicketKind::Task, None);
    let root_subtask = ticket("FIN-2", TicketKind::Subtask, None);

    assert_eq!(
        update_payload(&original, &root, None)
            .unwrap()
            .pointer("/update/parent/0/set"),
        Some(&json!(null))
    );
    assert!(
        update_payload(&original, &root_subtask, None)
            .unwrap_err()
            .contains("cannot be moved to Root")
    );
}

#[test]
fn jira_update_payload_omits_unchanged_description() {
    let original = ticket("FIN-2", TicketKind::Task, None);
    let mut desired = original.clone();
    desired.title = "Updated title".into();

    assert!(
        update_payload(&original, &desired, None)
            .unwrap()
            .pointer("/fields/description")
            .is_none()
    );

    desired.description = "Updated description".into();
    assert!(
        update_payload(&original, &desired, None)
            .unwrap()
            .pointer("/fields/description")
            .is_some()
    );
}

#[test]
fn created_ticket_recovery_keeps_the_created_key_when_refresh_fails() {
    let created = ticket("FIN-201", TicketKind::Task, None);
    let failure = created_issue_failure("transition failed".into(), created.clone(), None);
    let (original, updated) = *failure.refresh.expect("created ticket must be recoverable");

    assert_eq!(original.key, "FIN-201");
    assert_eq!(updated.key, "FIN-201");
}

#[test]
fn jira_rejects_description_overwrites_that_cannot_round_trip() {
    let unsupported_mark = to_ticket(JiraIssue {
        key: "FIN-2".into(),
        fields: json!({
            "description": {
                "type": "doc", "version": 1, "content": [{
                    "type": "paragraph", "content": [{
                        "type": "text", "text": "Underlined", "marks": [{ "type": "underline" }]
                    }]
                }]
            }
        }),
    });
    let unsupported_content = to_ticket(JiraIssue {
        key: "FIN-3".into(),
        fields: json!({
            "description": {
                "type": "doc", "version": 1, "content": [{ "type": "mediaSingle" }]
            }
        }),
    });
    assert!(!unsupported_mark.description_safe_to_overwrite);
    assert!(!unsupported_content.description_safe_to_overwrite);

    let mut edited = unsupported_mark.clone();
    edited.description.push_str(" changed");
    assert!(
        update_payload(&unsupported_mark, &edited, None)
            .unwrap_err()
            .contains("cannot be edited safely")
    );

    let mut title_only = unsupported_mark.clone();
    title_only.title = "New title".into();
    assert!(
        update_payload(&unsupported_mark, &title_only, None)
            .unwrap()
            .pointer("/fields/description")
            .is_none()
    );
}

#[test]
fn jira_conflict_detection_includes_assignee_account_id() {
    let original = ticket("FIN-2", TicketKind::Task, None);
    let mut remote = original.clone();
    remote.assignee_account_id = "different-account".into();

    assert!(!same_jira_content(&original, &remote));
}

#[test]
fn jira_conflict_detection_includes_description_overwrite_safety() {
    let original = ticket("FIN-2", TicketKind::Task, None);
    let mut remote = original.clone();
    remote.description_safe_to_overwrite = false;

    assert!(!same_jira_content(&original, &remote));
}

#[test]
fn ambiguous_create_failure_blocks_retry() {
    let failure = ambiguous_create_failure("connection reset".into());

    assert!(failure.retry_blocked);
    assert!(failure.message.contains("retry is blocked"));
}

#[test]
fn jira_create_server_errors_block_retry_but_validation_errors_do_not() {
    assert!(
        create_response_failure(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "down".into())
            .retry_blocked
    );
    assert!(
        !create_response_failure(reqwest::StatusCode::BAD_REQUEST, "invalid".into()).retry_blocked
    );
}

#[test]
fn commit_orders_local_parents_before_children_and_blocks_missing_parent_before_io() {
    let changes = vec![
        added("NEW-2", TicketKind::Subtask, Some("NEW-1")),
        added("NEW-1", TicketKind::Story, None),
    ];

    assert_eq!(commit_order(&changes).unwrap(), vec![1, 0]);
    let blocked = match submit_changes(
        &AppSettings::default(),
        &[added("NEW-2", TicketKind::Subtask, Some("NEW-1"))],
    ) {
        SubmitBatchOutcome::PreflightError(error) => error,
        _ => panic!("missing local parent must block commit"),
    };
    assert!(blocked.contains("needs unsent local parent NEW-1 selected"));
}

#[test]
fn failed_parent_skips_descendant_and_created_key_replaces_local_parent_reference() {
    let changes = vec![
        added("NEW-2", TicketKind::Subtask, Some("NEW-1")),
        added("NEW-1", TicketKind::Story, None),
    ];
    let mut calls = Vec::new();
    let outcomes = submit_ordered_changes(&changes, |change| {
        calls.push(change.id.clone());
        if change.id == "NEW-1" {
            Err(submit_failure("parent create failed".into()))
        } else {
            panic!("descendant must not make a remote call")
        }
    })
    .unwrap();
    assert_eq!(calls, ["NEW-1"]);
    assert!(
        outcomes[1]
            .result
            .as_ref()
            .unwrap_err()
            .message
            .contains("skipped")
    );

    let mut resolved_parent = None;
    submit_ordered_changes(&changes, |change| {
        if change.id == "NEW-1" {
            Ok(crate::store::composer::SubmissionSnapshot {
                original: None,
                updated: Some(ticket("FIN-101", TicketKind::Story, None)),
            })
        } else {
            resolved_parent = change.updated.as_ref().unwrap().parent_key.clone();
            Ok(crate::store::composer::SubmissionSnapshot {
                original: None,
                updated: Some(ticket("FIN-102", TicketKind::Subtask, Some("FIN-101"))),
            })
        }
    })
    .unwrap();
    assert_eq!(resolved_parent.as_deref(), Some("FIN-101"));
}

#[test]
fn deleted_descendants_commit_before_parents_even_when_parent_delete_fails() {
    let parent = TicketChange {
        id: "FIN-1".into(),
        original: Some(ticket("FIN-1", TicketKind::Story, None)),
        updated: None,
        kind: ChangeKind::Deleted,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    };
    let child = TicketChange {
        id: "FIN-2".into(),
        original: Some(ticket("FIN-2", TicketKind::Subtask, Some("FIN-1"))),
        updated: None,
        kind: ChangeKind::Deleted,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    };
    let changes = vec![parent, child];
    let mut calls = Vec::new();

    let outcomes = submit_ordered_changes(&changes, |change| {
        calls.push(change.id.clone());
        if change.id == "FIN-2" {
            Ok(crate::store::composer::SubmissionSnapshot {
                original: change.original.clone(),
                updated: None,
            })
        } else {
            Err(submit_failure("parent delete failed".into()))
        }
    })
    .unwrap();

    assert_eq!(calls, ["FIN-2", "FIN-1"]);
    assert!(outcomes[0].result.is_ok());
    assert!(outcomes[1].result.is_err());
}

#[test]
fn deleting_a_reparented_ticket_uses_its_original_parent_for_delete_order() {
    let parent = TicketChange {
        id: "FIN-1".into(),
        original: Some(ticket("FIN-1", TicketKind::Story, None)),
        updated: None,
        kind: ChangeKind::Deleted,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    };
    let child = TicketChange {
        id: "FIN-2".into(),
        original: Some(ticket("FIN-2", TicketKind::Subtask, Some("FIN-1"))),
        updated: Some(ticket("FIN-2", TicketKind::Subtask, Some("NEW-9"))),
        kind: ChangeKind::Deleted,
        submitted: None,
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    };

    assert_eq!(commit_order(&[parent, child]).unwrap(), vec![1, 0]);
}

#[test]
fn backlog_board_falls_back_to_a_project_board_without_scrum_type() {
    let board = select_backlog_board(vec![AgileBoard {
        id: 12,
        name: "KAN".into(),
        kind: "simple".into(),
    }]);

    assert_eq!(board.unwrap().id, 12);
}

#[test]
fn board_backlog_query_excludes_subtasks_and_epics_hidden_by_the_web_backlog_list() {
    let query = board_backlog_query(0, None);

    assert!(
        query
            .iter()
            .any(|(name, value)| *name == "jql" && value == BACKLOG_JQL)
    );
}

#[test]
fn backlog_items_include_subtask_progress_releases_and_epic_names() {
    assert!(BACKLOG_FIELDS.contains(&"parent"));
    assert!(BACKLOG_FIELDS.contains(&"subtasks"));
    assert!(BACKLOG_FIELDS.contains(&"fixVersions"));

    let work_item = to_work_item(
        JiraIssue {
            key: "FIN-1".into(),
            fields: json!({
                "summary": "Parent",
                "status": { "statusCategory": { "key": "done" } },
                "parent": {
                    "key": "FIN-0",
                    "fields": {
                        "summary": "Shopping cart",
                        "issuetype": { "name": "Epic" }
                    }
                },
                "subtasks": [
                    { "key": "FIN-2", "fields": { "status": { "statusCategory": { "key": "done" } } } },
                    { "key": "FIN-3", "fields": { "status": { "statusCategory": { "key": "indeterminate" } } } }
                ],
                "fixVersions": [{ "name": "1.2.0" }]
            }),
        },
        None,
    );

    assert_eq!(work_item.parent_key.as_deref(), Some("FIN-0"));
    assert_eq!(work_item.parent_title.as_deref(), Some("Shopping cart"));
    assert!(work_item.done);
    assert!(work_item.has_children);
    let progress = work_item.subtask_progress.as_ref().unwrap();
    assert_eq!(progress.completed, 1);
    assert_eq!(progress.total, 2);
    assert_eq!(work_item.fix_versions, ["1.2.0"]);
    assert_eq!(work_item.epic_name.as_deref(), Some("Shopping cart"));
}

#[test]
fn board_estimation_field_is_used_only_for_field_estimation() {
    assert_eq!(
        story_points_field_id(&json!({
            "estimation": {
                "type": "field",
                "field": { "fieldId": "customfield_10016" }
            }
        })),
        "customfield_10016"
    );
    assert_eq!(
        story_points_field_id(&json!({
            "estimation": {
                "type": "issueCount",
                "field": { "fieldId": "customfield_10016" }
            }
        })),
        ""
    );
}

#[test]
fn manual_story_point_field_is_not_overwritten_on_the_first_board_load() {
    let manual = AppSettings {
        jira_story_points_field_id: "customfield_10016".into(),
        ..AppSettings::default()
    };
    let discovered = AppSettings {
        jira_story_points_field_id: "customfield_10016".into(),
        jira_story_points_board_id: "1".into(),
        ..AppSettings::default()
    };

    assert!(!should_discover_story_points(&manual, 1));
    assert!(should_discover_story_points(&AppSettings::default(), 1));
    assert!(should_discover_story_points(&discovered, 2));
}

#[test]
fn failed_story_points_discovery_warns_and_retries_without_a_persisted_field() {
    let (discovered, warning) = discover_story_points(
        &AppSettings::default(),
        1,
        Err("Jira returned 403: forbidden".into()),
    );
    let interrupted = AppSettings {
        jira_story_points_board_id: "1".into(),
        ..AppSettings::default()
    };

    assert!(discovered.is_none());
    assert!(
        warning
            .unwrap()
            .contains("retry on the next backlog refresh")
    );
    assert!(should_discover_story_points(&interrupted, 1));
}

#[test]
fn legacy_discovered_story_points_field_loads_when_rediscovery_fails() {
    let legacy = AppSettings {
        jira_story_points_field_id: "customfield_10016".into(),
        jira_story_points_board_id: "1".into(),
        ..AppSettings::default()
    };
    let (discovered, warning) =
        discover_story_points(&legacy, 1, Err("Jira returned 403: forbidden".into()));

    assert!(should_discover_story_points(&legacy, 1));
    assert!(discovered.is_none());
    assert!(warning.is_some());
    assert_eq!(
        story_points_field_for_load(&legacy, discovered.as_ref()),
        Some("customfield_10016")
    );
}

#[test]
fn empty_story_point_field_retries_until_absence_is_confirmed() {
    let incomplete = AppSettings {
        jira_story_points_board_id: "1".into(),
        ..AppSettings::default()
    };
    let complete = AppSettings {
        jira_story_points_board_id: "1".into(),
        jira_story_points_discovery_complete: true,
        ..AppSettings::default()
    };

    assert!(should_discover_story_points(&incomplete, 1));
    assert!(!should_discover_story_points(&complete, 1));
}

#[test]
fn backlog_work_items_parse_numeric_story_points() {
    let work_item = to_work_item(
        JiraIssue {
            key: "FIN-1".into(),
            fields: json!({ "customfield_10016": 3.5 }),
        },
        Some("customfield_10016"),
    );

    assert_eq!(work_item.story_points, Some(3.5));
}

#[test]
fn sprint_work_items_include_embedded_subtasks_when_jira_omits_them() {
    let (parent, subtasks) = to_work_item_with_subtasks(
        JiraIssue {
            key: "FIN-1".into(),
            fields: json!({
                "summary": "Parent work",
                "subtasks": [{
                    "key": "FIN-2",
                    "fields": {
                        "summary": "Child work",
                        "issuetype": { "name": "Sub-task" },
                        "status": { "name": "To Do" }
                    }
                }]
            }),
        },
        None,
    );

    assert_eq!(parent.key, "FIN-1");
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].key, "FIN-2");
    assert_eq!(subtasks[0].parent_key.as_deref(), Some("FIN-1"));
    assert_eq!(subtasks[0].parent_title.as_deref(), Some("Parent work"));
}

#[test]
fn backlog_warns_when_loaded_tickets_lack_story_points() {
    let snapshot = crate::store::work_items::BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: true,
        sprints: Vec::new(),
        work_items: vec![to_work_item(
            JiraIssue {
                key: "FIN-1".into(),
                fields: json!({ "customfield_10016": null }),
            },
            Some("customfield_10016"),
        )],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    };

    assert!(
        story_points_warning(&snapshot)
            .unwrap()
            .contains("No loaded backlog tickets have story-point values")
    );
}

#[test]
fn velocity_chart_average_uses_completed_estimates() {
    let average = velocity_average(json!({
        "velocityStatEntries": {
            "101": { "completed": { "value": 16.0 } },
            "102": { "completed": { "value": 24.0 } }
        }
    }))
    .unwrap();

    assert_eq!(average, 20.0);
}

#[test]
fn velocity_report_uses_the_most_recent_configured_sprints_and_their_names() {
    let report = velocity_report(
        json!({
            "sprints": [
                { "id": 101, "name": "Sprint 1" },
                { "id": 102, "name": "Sprint 2" },
                { "id": 103, "name": "Sprint 3" }
            ],
            "velocityStatEntries": {
                "101": { "completed": { "value": 16.0 } },
                "102": { "completed": { "value": 24.0 } },
                "103": { "completed": { "value": 20.0 } }
            }
        }),
        2,
    )
    .unwrap();

    assert_eq!(report.configured_sprints, 2);
    assert_eq!(report.dynamic_capacity, Some(22.0));
    assert_eq!(
        report
            .sprints
            .iter()
            .map(|sprint| sprint.name.as_str())
            .collect::<Vec<_>>(),
        ["Sprint 3", "Sprint 2", "Sprint 1"]
    );
}

#[test]
fn velocity_history_is_newest_first_without_sprint_metadata() {
    let report = velocity_report(
        json!({
            "velocityStatEntries": {
                "101": { "completed": { "value": 16.0 } },
                "103": { "completed": { "value": 20.0 } },
                "102": { "completed": { "value": 24.0 } }
            }
        }),
        2,
    )
    .unwrap();

    assert_eq!(
        report
            .sprints
            .iter()
            .map(|sprint| sprint.id)
            .collect::<Vec<_>>(),
        [103, 102, 101]
    );
    assert_eq!(report.dynamic_capacity, Some(22.0));
}

#[test]
fn sprint_goal_normalization_trims_text_and_discards_blank_values() {
    let goal = |value| {
        serde_json::from_value::<JiraSprint>(json!({
            "id": 1,
            "name": "Sprint 1",
            "state": "future",
            "goal": value,
        }))
        .unwrap()
        .goal
    };
    let missing = serde_json::from_value::<JiraSprint>(json!({
        "id": 1,
        "name": "Sprint 1",
        "state": "future",
    }))
    .unwrap();

    assert_eq!(
        goal(json!("  Ship planning  ")),
        Some("Ship planning".into())
    );
    assert_eq!(goal(json!("")), None);
    assert_eq!(goal(json!(" \n\t ")), None);
    assert_eq!(missing.goal, None);
}

#[test]
fn velocity_loading_enriches_only_configured_sprints_with_best_effort_goals() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let responses = [
            (
                "200 OK",
                json!({
                    "sprints": [
                        { "id": 103, "name": "Sprint 3" },
                        { "id": 102, "name": "Sprint 2" },
                        { "id": 101, "name": "Sprint 1" }
                    ],
                    "velocityStatEntries": {
                        "101": { "completed": { "value": 16.0 } },
                        "102": { "completed": { "value": 24.0 } },
                        "103": { "completed": { "value": 20.0 } }
                    }
                })
                .to_string(),
            ),
            (
                "200 OK",
                json!({
                    "id": 103,
                    "name": "Sprint 3",
                    "state": "closed",
                    "goal": "Ship velocity enrichment"
                })
                .to_string(),
            ),
            ("500 Internal Server Error", "goal lookup failed".into()),
        ];
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            requests.push(String::from_utf8_lossy(&request[..size]).into_owned());
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .unwrap();
        }
        requests
    });

    let report = board_velocity(
        &reqwest::blocking::Client::new(),
        &base_url,
        "user@example.com",
        "token",
        42,
        2,
    )
    .unwrap();
    let requests = server.join().unwrap();

    assert_eq!(
        report.sprints[0].goal.as_deref(),
        Some("Ship velocity enrichment")
    );
    assert_eq!(report.sprints[1].goal, None);
    assert_eq!(report.sprints[2].goal, None);
    assert!(requests[0].contains("/rest/greenhopper/1.0/rapid/charts/velocity.json"));
    assert!(requests[1].contains("/rest/agile/1.0/sprint/103"));
    assert!(requests[2].contains("/rest/agile/1.0/sprint/102"));
}

#[test]
fn velocity_goal_loading_is_bounded_when_configured_history_is_huge() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let chart = json!({
            "sprints": (1..=MAX_VELOCITY_GOAL_LOOKUPS + 1)
                .map(|id| json!({ "id": id, "name": format!("Sprint {id}") }))
                .collect::<Vec<_>>(),
            "velocityStatEntries": (1..=MAX_VELOCITY_GOAL_LOOKUPS + 1)
                .map(|id| json!({ "id": id, "completed": { "value": id as f64 } }))
                .collect::<Vec<_>>(),
        })
        .to_string();
        let mut requests = Vec::new();
        for index in 0..=MAX_VELOCITY_GOAL_LOOKUPS {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            requests.push(String::from_utf8_lossy(&request[..size]).into_owned());
            let body = if index == 0 {
                chart.clone()
            } else {
                json!({ "id": index, "name": "Sprint", "state": "closed" }).to_string()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .unwrap();
        }
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0; 4096];
                    let size = stream.read(&mut request).unwrap();
                    requests.push(String::from_utf8_lossy(&request[..size]).into_owned());
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("server accept failed: {error}"),
            }
        }
        requests
    });

    let report = board_velocity(
        &reqwest::blocking::Client::new(),
        &base_url,
        "user@example.com",
        "token",
        42,
        usize::MAX,
    )
    .unwrap();
    let requests = server.join().unwrap();

    assert_eq!(report.sprints.len(), MAX_VELOCITY_GOAL_LOOKUPS + 1);
    assert_eq!(requests.len(), MAX_VELOCITY_GOAL_LOOKUPS + 1);
    assert!(requests[1].contains("/rest/agile/1.0/sprint/11"));
    assert!(
        requests
            .last()
            .unwrap()
            .contains("/rest/agile/1.0/sprint/2")
    );
}

#[test]
fn active_and_future_sprint_loading_parses_goals() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..size]).into_owned();
        let body = json!({
            "values": [{
                "id": 42,
                "name": "Sprint 42",
                "state": "future",
                "goal": "Ship sprint planning"
            }],
            "isLast": true,
            "startAt": 0,
            "maxResults": 50
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        )
        .unwrap();
        request
    });

    let sprints = board_sprints(
        &reqwest::blocking::Client::new(),
        &base_url,
        "user@example.com",
        "token",
        7,
    )
    .unwrap();
    let request = server.join().unwrap();

    assert_eq!(sprints[0].goal.as_deref(), Some("Ship sprint planning"));
    assert!(request.contains("/rest/agile/1.0/board/7/sprint"));
}

#[test]
fn jira_rank_payload_uses_the_before_or_after_anchor() {
    let before = rank_payload(&RankPlan {
        issues: vec!["FIN-2".into(), "FIN-3".into()],
        rank_before_issue: Some("FIN-4".into()),
        rank_after_issue: None,
    });
    assert_eq!(
        before,
        json!({ "issues": ["FIN-2", "FIN-3"], "rankBeforeIssue": "FIN-4" })
    );

    let after = rank_payload(&RankPlan {
        issues: vec!["FIN-2".into()],
        rank_before_issue: None,
        rank_after_issue: Some("FIN-1".into()),
    });
    assert_eq!(
        after,
        json!({ "issues": ["FIN-2"], "rankAfterIssue": "FIN-1" })
    );
}

#[test]
fn backlog_pagination_continues_when_jira_omits_total() {
    let page = AgileIssuePage {
        issues: vec![JiraIssue {
            key: "FIN-1".into(),
            fields: json!({}),
        }],
        is_last: false,
        start_at: 0,
        max_results: 100,
        total: 0,
        next_page_token: None,
    };

    assert!(!backlog_page_complete(&page, page.issues.len()));
}

#[test]
fn sprint_loading_uses_agile_offset_pagination_across_pages() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for (index, issue) in ["FIN-1", "FIN-2"].into_iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            requests.push(String::from_utf8_lossy(&request[..size]).into_owned());
            let body = json!({
                "issues": [{ "key": issue, "fields": { "summary": issue } }],
                "isLast": false,
                "startAt": index,
                "maxResults": 1,
                "total": 2
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
        requests
    });

    let items = sprint_issues(
        &reqwest::blocking::Client::new(),
        &base_url,
        "user@example.com",
        "token",
        42,
        None,
    )
    .unwrap();
    let requests = server.join().unwrap();

    assert_eq!(
        items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-1", "FIN-2"]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.contains("/rest/agile/1.0/sprint/42/issue"))
    );
    assert!(requests[0].contains("startAt=0"));
    assert!(requests[1].contains("startAt=1"));
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("nextPageToken"))
    );
}

#[test]
fn backlog_loading_includes_embedded_subtasks() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let size = stream.read(&mut request).unwrap();
        let body = json!({
            "issues": [{
                "key": "FIN-1",
                "fields": {
                    "summary": "Parent",
                    "subtasks": [{
                        "key": "FIN-2",
                        "fields": { "summary": "Child", "issuetype": { "name": "Sub-task" } }
                    }]
                }
            }],
            "isLast": true,
            "startAt": 0,
            "maxResults": 100,
            "total": 1
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        String::from_utf8_lossy(&request[..size]).into_owned()
    });

    let items = board_backlog(
        &reqwest::blocking::Client::new(),
        &base_url,
        "user@example.com",
        "token",
        42,
        None,
    )
    .unwrap();
    let request = server.join().unwrap();

    assert_eq!(
        items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["FIN-1", "FIN-2"]
    );
    assert_eq!(items[1].parent_key.as_deref(), Some("FIN-1"));
    assert!(request.contains("/rest/agile/1.0/board/42/backlog"));
}
