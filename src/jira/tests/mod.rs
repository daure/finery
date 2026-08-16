use serde_json::json;

use super::{JiraIssue, search_jql, text_search_jql, to_ticket};
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
