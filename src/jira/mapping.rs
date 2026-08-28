use serde_json::Value;

use crate::store::{
    composer::{
        Ticket, TicketKind,
        jira_adf::{adf_is_safe_to_overwrite, adf_to_markdown},
    },
    work_items::{BacklogSnapshot, WorkItem},
};

use super::JiraIssue;

pub(super) fn to_ticket(issue: JiraIssue) -> Ticket {
    let field = |name: &str| issue.fields.get(name).unwrap_or(&Value::Null);
    let (parent_key, parent_title) = parent_metadata(field("parent"));
    Ticket {
        key: issue.key.clone(),
        project_key: field("project")
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| issue.key.split_once('-').map_or("", |(key, _)| key))
            .into(),
        title: field("summary").as_str().unwrap_or(&issue.key).into(),
        description: adf_to_markdown(field("description")),
        description_safe_to_overwrite: adf_is_safe_to_overwrite(field("description")),
        kind: field("issuetype")
            .get("name")
            .and_then(Value::as_str)
            .map(ticket_kind)
            .unwrap_or(TicketKind::Task),
        status: named_field(field("status")).unwrap_or_default(),
        priority: named_field(field("priority")).unwrap_or_default(),
        assignee: field("assignee")
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("Unassigned")
            .into(),
        assignee_account_id: field("assignee")
            .get("accountId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        parent_key,
        parent_title,
        parent_kind: field("parent")
            .pointer("/fields/issuetype/name")
            .and_then(Value::as_str)
            .map(ticket_kind),
        has_children: field("subtasks")
            .as_array()
            .is_some_and(|subtasks| !subtasks.is_empty()),
    }
}

pub(super) fn to_work_item(issue: JiraIssue, story_points_field_id: Option<&str>) -> WorkItem {
    let field = |name: &str| issue.fields.get(name).unwrap_or(&Value::Null);
    let (parent_key, parent_title) = parent_metadata(field("parent"));
    WorkItem {
        key: issue.key.clone(),
        title: field("summary").as_str().unwrap_or(&issue.key).into(),
        kind: named_field(field("issuetype")).unwrap_or_else(|| "Issue".into()),
        status: named_field(field("status")).unwrap_or_default(),
        priority: named_field(field("priority")).unwrap_or_default(),
        assignee: field("assignee")
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("Unassigned")
            .into(),
        parent_key,
        parent_title,
        has_children: field("subtasks")
            .as_array()
            .is_some_and(|subtasks| !subtasks.is_empty()),
        story_points: story_points_field_id.and_then(|field_id| {
            field(field_id)
                .as_f64()
                .or_else(|| field(field_id).as_str()?.parse().ok())
        }),
    }
}

pub(super) fn story_points_warning(snapshot: &BacklogSnapshot) -> Option<String> {
    let tickets = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .collect::<Vec<_>>();
    (!tickets.is_empty() && tickets.iter().all(|ticket| ticket.story_points.is_none())).then(|| "No loaded backlog tickets have story-point values. Check the Jira story-points custom-field ID in Settings.".into())
}

pub(super) fn parent_metadata(parent: &Value) -> (Option<String>, Option<String>) {
    let parent_key = parent.get("key").and_then(Value::as_str).map(str::to_owned);
    let parent_title = parent_key.as_ref().map(|key| {
        parent
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .into()
    });
    (parent_key, parent_title)
}

pub(super) fn named_field(value: &Value) -> Option<String> {
    value.get("name").and_then(Value::as_str).map(str::to_owned)
}

pub(super) fn ticket_kind(name: &str) -> TicketKind {
    match name.to_ascii_lowercase().as_str() {
        "epic" => TicketKind::Epic,
        "story" => TicketKind::Story,
        "bug" => TicketKind::Bug,
        "subtask" | "sub-task" => TicketKind::Subtask,
        _ => TicketKind::Task,
    }
}

pub(super) fn ticket_kind_name(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Epic => "Epic",
        TicketKind::Story => "Story",
        TicketKind::Task => "Task",
        TicketKind::Bug => "Bug",
        TicketKind::Subtask => "Sub-task",
    }
}

pub(super) fn search_jql(query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        return "updated >= -90d ORDER BY updated DESC".into();
    }
    let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
    let text = wildcard_text(query);
    if looks_like_key(query) {
        format!(
            "(key = \"{escaped}\" OR summary ~ \"{text}\" OR text ~ \"{text}\") ORDER BY updated DESC"
        )
    } else if looks_like_project_key(query) {
        format!(
            "(project = \"{}\" OR summary ~ \"{text}\" OR text ~ \"{text}\") ORDER BY updated DESC",
            escaped.to_ascii_uppercase()
        )
    } else {
        text_search_jql(query)
    }
}

pub(super) fn text_search_jql(query: &str) -> String {
    let text = wildcard_text(query);
    format!("(summary ~ \"{text}\" OR text ~ \"{text}\") ORDER BY updated DESC")
}

fn wildcard_text(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("{}*", term.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn looks_like_project_key(query: &str) -> bool {
    !query.is_empty()
        && query.len() <= 10
        && query
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

fn looks_like_key(query: &str) -> bool {
    query.split_once('-').is_some_and(|(project, number)| {
        !project.is_empty()
            && project
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            && !number.is_empty()
            && number.chars().all(|character| character.is_ascii_digit())
    })
}
