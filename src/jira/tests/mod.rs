use serde_json::json;

use super::{
    AgileBoard, AgileIssuePage, BACKLOG_JQL, JiraIssue, backlog_page_complete, board_backlog_query,
    options_from_values, search_jql, select_backlog_board, text_search_jql, to_ticket,
};
use crate::store::composer::TicketKind;

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
