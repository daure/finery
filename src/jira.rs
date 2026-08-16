use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    app_settings::AppSettings,
    store::composer::{Ticket, TicketKind, jira_adf::adf_to_markdown},
};

#[derive(Deserialize)]
struct SearchResponse {
    issues: Vec<JiraIssue>,
}

#[derive(Deserialize)]
struct JiraIssue {
    key: String,
    fields: Value,
}

pub(crate) fn search(settings: &AppSettings, query: &str) -> Result<Vec<Ticket>, String> {
    let (base_url, email, token) = settings.configured_jira().ok_or_else(|| {
        "Jira is not configured; add URL, email, and API token in Settings".to_string()
    })?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = match request_search(&client, base_url, email, token, &search_jql(query)) {
        Err((Some(400), _)) if looks_like_project_key(query.trim()) => request_search(
            &client,
            base_url,
            email,
            token,
            &text_search_jql(query.trim()),
        ),
        result => result,
    }
    .map_err(|(_, error)| error)?;
    Ok(response.issues.into_iter().map(to_ticket).collect())
}

fn request_search(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    jql: &str,
) -> Result<SearchResponse, (Option<u16>, String)> {
    let response = client
        .post(format!("{base_url}/rest/api/3/search/jql"))
        .basic_auth(email, Some(token))
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "jql": jql,
            "fields": ["summary", "description", "issuetype", "status", "priority", "assignee"],
            "maxResults": 10
        }))
        .send()
        .map_err(|error| (None, error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "unknown Jira error".into());
        return Err((
            Some(status.as_u16()),
            format!("Jira returned {}: {body}", status.as_u16()),
        ));
    }
    response
        .json::<SearchResponse>()
        .map_err(|error| (Some(status.as_u16()), error.to_string()))
}

fn to_ticket(issue: JiraIssue) -> Ticket {
    let field = |name: &str| issue.fields.get(name).unwrap_or(&Value::Null);
    Ticket {
        key: issue.key.clone(),
        title: field("summary").as_str().unwrap_or(&issue.key).into(),
        description: adf_to_markdown(field("description")),
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
    }
}

fn named_field(value: &Value) -> Option<String> {
    value.get("name").and_then(Value::as_str).map(str::to_owned)
}

fn ticket_kind(name: &str) -> TicketKind {
    match name.to_ascii_lowercase().as_str() {
        "epic" => TicketKind::Epic,
        "story" => TicketKind::Story,
        "bug" => TicketKind::Bug,
        "subtask" | "sub-task" => TicketKind::Subtask,
        _ => TicketKind::Task,
    }
}

fn search_jql(query: &str) -> String {
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

fn text_search_jql(query: &str) -> String {
    let text = wildcard_text(query);
    format!("(summary ~ \"{text}\" OR text ~ \"{text}\") ORDER BY updated DESC")
}

fn wildcard_text(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| {
            let escaped = term.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{escaped}*")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_project_key(query: &str) -> bool {
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

#[cfg(test)]
mod tests;
