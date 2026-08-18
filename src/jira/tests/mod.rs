use serde_json::json;

use super::{
    AgileBoard, AgileIssuePage, BACKLOG_JQL, ISSUE_FIELDS, JiraIssue, backlog_page_complete,
    board_backlog_query, commit_order, create_issue_fields, create_issue_type,
    create_issue_types_from_value, issue_fields, options_from_values, same_jira_content,
    search_jql, select_backlog_board, submit_failure, submit_ordered_changes, text_search_jql,
    to_ticket, update_payload,
};
use crate::{
    app_settings::AppSettings,
    jira::submit_changes,
    store::composer::{ChangeKind, Ticket, TicketChange, TicketKind},
};

fn ticket(key: &str, kind: TicketKind, parent_key: Option<&str>) -> Ticket {
    Ticket {
        key: key.into(),
        project_key: "FIN".into(),
        title: key.into(),
        description: String::new(),
        kind,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
        parent_key: parent_key.map(str::to_owned),
        parent_kind: None,
    }
}

fn added(key: &str, kind: TicketKind, parent_key: Option<&str>) -> TicketChange {
    TicketChange {
        id: key.into(),
        original: None,
        updated: Some(ticket(key, kind, parent_key)),
        kind: ChangeKind::Added,
        submitted: None,
        sibling_order: 0,
    }
}

#[test]
fn jira_search_handles_recent_results_text_and_exact_keys() {
    assert_eq!(search_jql(""), "updated >= -90d ORDER BY updated DESC");
    assert!(search_jql("checkout words").contains("summary ~"));
    assert!(search_jql("kan").contains("project = \"KAN\""));
    assert!(text_search_jql("quant").contains("summary ~ \"quant*\""));
    assert!(text_search_jql("cart quant").contains("summary ~ \"cart* quant*\""));
    let exact = search_jql("OPS-42");
    assert!(exact.contains("key = \"OPS-42\""));
    assert!(exact.ends_with("ORDER BY updated DESC"));
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
        "values": [
            { "id": "10001", "name": "Story" },
            { "id": "10003", "name": "Subtask", "subtask": true }
        ]
    }));

    assert_eq!(issue_types[1].id, "10003");
    assert_eq!(issue_types[1].label, "Subtask");
}

#[test]
fn jira_issue_maps_search_fields_and_adf_description() {
    let ticket = to_ticket(JiraIssue {
        key: "OPS-42".into(),
        fields: json!({
            "summary": "Retry checkout",
            "project": { "key": "OPS" },
            "issuetype": { "name": "Story" },
            "status": { "name": "In Progress" },
            "priority": { "name": "High" },
            "assignee": { "displayName": "Ada", "accountId": "ada-1" },
            "parent": { "key": "OPS-1", "fields": { "issuetype": { "name": "Epic" } } },
            "description": {
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": "Keep basket state." }]
                }]
            }
        }),
    });

    assert_eq!(ticket.key, "OPS-42");
    assert_eq!(ticket.project_key, "OPS");
    assert_eq!(ticket.kind, TicketKind::Story);
    assert_eq!(ticket.status, "In Progress");
    assert_eq!(ticket.description, "Keep basket state.");
    assert_eq!(ticket.assignee, "Ada");
    assert_eq!(ticket.assignee_account_id, "ada-1");
    assert_eq!(ticket.parent_key.as_deref(), Some("OPS-1"));
    assert_eq!(ticket.parent_kind, Some(TicketKind::Epic));
    assert!(ISSUE_FIELDS.contains(&"parent"));
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
fn jira_conflict_detection_includes_assignee_account_id() {
    let original = ticket("FIN-2", TicketKind::Task, None);
    let mut remote = original.clone();
    remote.assignee_account_id = "different-account".into();

    assert!(!same_jira_content(&original, &remote));
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
        Err(error) => error,
        Ok(_) => panic!("missing local parent must block commit"),
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
    let query = board_backlog_query(0);

    assert!(
        query
            .iter()
            .any(|(name, value)| *name == "jql" && value == BACKLOG_JQL)
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
