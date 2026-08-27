use std::{collections::HashMap, time::Duration};

use reqwest::{StatusCode, blocking::Client};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    app_settings::AppSettings,
    store::composer::{
        ChangeKind, SubmissionSnapshot, Ticket, TicketChange, TicketKind,
        jira_adf::{adf_is_safe_to_overwrite, adf_to_markdown, markdown_to_adf},
    },
    store::work_items::{BacklogSnapshot, Sprint, WorkItem},
};

const ISSUE_FIELDS: [&str; 8] = [
    "summary",
    "description",
    "issuetype",
    "status",
    "priority",
    "assignee",
    "project",
    "parent",
];

const BACKLOG_FIELDS: [&str; 5] = ["summary", "issuetype", "status", "priority", "assignee"];
const BACKLOG_JQL: &str = "issuetype not in subTaskIssueTypes() AND issuetype != Epic";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JiraProject {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JiraOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JiraFieldOptions {
    pub issue_types: Vec<JiraOption>,
    pub statuses: Vec<JiraOption>,
    pub priorities: Vec<JiraOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JiraAssignee {
    pub account_id: String,
    pub display_name: String,
}

pub(crate) enum SubmitBatchOutcome {
    PreflightError(String),
    Conflict(Vec<String>),
    Completed(Vec<TicketSubmitOutcome>),
}

pub(crate) struct TicketSubmitOutcome {
    pub id: String,
    pub result: Result<SubmissionSnapshot, SubmitFailure>,
}

pub(crate) struct SubmitFailure {
    pub message: String,
    pub refresh: Option<Box<(Ticket, Ticket)>>,
    pub retry_blocked: bool,
}

#[derive(Deserialize)]
struct SearchResponse {
    issues: Vec<JiraIssue>,
}

#[derive(Deserialize)]
struct AgileBoardPage {
    values: Vec<AgileBoard>,
}

#[derive(Deserialize)]
struct AgileBoard {
    id: u64,
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct SprintPage {
    values: Vec<JiraSprint>,
    #[serde(rename = "isLast")]
    is_last: bool,
    #[serde(rename = "startAt")]
    start_at: usize,
    #[serde(rename = "maxResults")]
    max_results: usize,
}

#[derive(Deserialize)]
struct JiraSprint {
    id: u64,
    name: String,
    state: String,
}

#[derive(Deserialize)]
struct AgileIssuePage {
    issues: Vec<JiraIssue>,
    #[serde(default, rename = "isLast")]
    is_last: bool,
    #[serde(default, rename = "startAt")]
    start_at: usize,
    #[serde(default, rename = "maxResults")]
    max_results: usize,
    #[serde(default)]
    total: usize,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct JiraIssue {
    key: String,
    fields: Value,
}

#[derive(Deserialize)]
struct CreatedIssue {
    key: String,
}

#[derive(Deserialize)]
struct ProjectPage {
    values: Vec<ProjectValue>,
    #[serde(rename = "isLast")]
    is_last: bool,
    #[serde(rename = "startAt")]
    start_at: usize,
    #[serde(rename = "maxResults")]
    max_results: usize,
}

#[derive(Deserialize)]
struct ProjectValue {
    key: String,
    name: String,
}

#[derive(Deserialize)]
struct TransitionPage {
    transitions: Vec<Transition>,
}

#[derive(Deserialize)]
struct Transition {
    id: String,
    to: NamedValue,
}

#[derive(Deserialize)]
struct NamedValue {
    name: String,
}

#[derive(Deserialize)]
struct JiraUser {
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

pub(crate) fn search(settings: &AppSettings, query: &str) -> Result<Vec<Ticket>, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let response = match request_search(&client, &base_url, &email, &token, &search_jql(query)) {
        Err((Some(400), _)) if looks_like_project_key(query.trim()) => request_search(
            &client,
            &base_url,
            &email,
            &token,
            &text_search_jql(query.trim()),
        ),
        result => result,
    }
    .map_err(|(_, error)| error)?;
    Ok(response.issues.into_iter().map(to_ticket).collect())
}

pub(crate) fn backlog(settings: &AppSettings) -> Result<BacklogSnapshot, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let board = backlog_board(&client, &base_url, &email, &token, settings)?;
    let sprints = board_sprints(&client, &base_url, &email, &token, board.id)?;
    let sprints = sprints
        .into_iter()
        .map(|sprint| {
            sprint_issues(&client, &base_url, &email, &token, sprint.id).map(|work_items| Sprint {
                id: sprint.id,
                name: sprint.name,
                state: sprint.state,
                work_items,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let work_items = board_backlog(&client, &base_url, &email, &token, board.id)?;
    Ok(BacklogSnapshot {
        board_name: board.name,
        sprints,
        work_items,
    })
}

fn backlog_board(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    settings: &AppSettings,
) -> Result<AgileBoard, String> {
    if !settings.jira_default_board.trim().is_empty() {
        let board_id = settings
            .jira_default_board
            .trim()
            .parse::<u64>()
            .map_err(|_| "Default Jira board ID must be a whole number".to_string())?;
        let response = client
            .get(format!("{base_url}/rest/agile/1.0/board/{board_id}"))
            .basic_auth(email, Some(token))
            .send()
            .map_err(|error| error.to_string())?;
        return response_json(response);
    }

    let mut query = vec![("maxResults", "50".to_string())];
    if !settings.jira_default_project.trim().is_empty() {
        query.push((
            "projectKeyOrId",
            settings.jira_default_project.trim().to_owned(),
        ));
    }
    let response = client
        .get(format!("{base_url}/rest/agile/1.0/board"))
        .basic_auth(email, Some(token))
        .query(&query)
        .send()
        .map_err(|error| error.to_string())?;
    select_backlog_board(response_json::<AgileBoardPage>(response)?.values).ok_or_else(|| {
        if settings.jira_default_project.trim().is_empty() {
            "No Jira boards are available; set a default board ID in Settings".into()
        } else {
            format!(
                "No Jira board is available for project {}",
                settings.jira_default_project.trim()
            )
        }
    })
}

fn select_backlog_board(mut boards: Vec<AgileBoard>) -> Option<AgileBoard> {
    let index = boards
        .iter()
        .position(|board| board.kind == "scrum")
        .unwrap_or(0);
    (!boards.is_empty()).then(|| boards.remove(index))
}

fn board_sprints(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    board_id: u64,
) -> Result<Vec<JiraSprint>, String> {
    let mut start_at = 0;
    let mut sprints = Vec::new();
    loop {
        let response = client
            .get(format!("{base_url}/rest/agile/1.0/board/{board_id}/sprint"))
            .basic_auth(email, Some(token))
            .query(&[
                ("startAt", start_at.to_string()),
                ("maxResults", "50".into()),
                ("state", "active,future".into()),
            ])
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<SprintPage>(response)?;
        sprints.extend(page.values);
        if page.is_last || page.max_results == 0 {
            return Ok(sprints);
        }
        start_at = page.start_at.saturating_add(page.max_results);
    }
}

fn sprint_issues(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    sprint_id: u64,
) -> Result<Vec<WorkItem>, String> {
    let mut next_page_token = None;
    let mut work_items = Vec::new();
    loop {
        let query = vec![
            ("maxResults", "100".to_string()),
            ("fields", BACKLOG_FIELDS.join(",")),
        ];
        let mut request = client
            .get(format!(
                "{base_url}/rest/software/1.0/sprint/{sprint_id}/issue"
            ))
            .basic_auth(email, Some(token))
            .query(&query);
        if let Some(page_token) = next_page_token.as_deref() {
            request = request.query(&[("nextPageToken", page_token)]);
        }
        let response = request.send().map_err(|error| error.to_string())?;
        let page = response_json::<AgileIssuePage>(response)?;
        work_items.extend(page.issues.into_iter().map(to_work_item));
        let Some(page_token) = page.next_page_token else {
            return Ok(work_items);
        };
        next_page_token = Some(page_token);
    }
}

fn board_backlog(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    board_id: u64,
) -> Result<Vec<WorkItem>, String> {
    let mut start_at = 0;
    let mut work_items = Vec::new();
    loop {
        let response = client
            .get(format!(
                "{base_url}/rest/agile/1.0/board/{board_id}/backlog"
            ))
            .basic_auth(email, Some(token))
            .query(&board_backlog_query(start_at))
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<AgileIssuePage>(response)?;
        let loaded = page.issues.len();
        let complete = backlog_page_complete(&page, loaded);
        work_items.extend(page.issues.into_iter().map(to_work_item));
        if complete {
            return Ok(work_items);
        }
        start_at = page.start_at.saturating_add(page.max_results.max(loaded));
    }
}

fn backlog_page_complete(page: &AgileIssuePage, loaded: usize) -> bool {
    page.is_last
        || loaded == 0
        || (page.total > 0 && page.start_at.saturating_add(page.max_results) >= page.total)
}

fn board_backlog_query(start_at: usize) -> [(&'static str, String); 4] {
    [
        ("startAt", start_at.to_string()),
        ("maxResults", "100".into()),
        ("fields", BACKLOG_FIELDS.join(",")),
        ("jql", BACKLOG_JQL.into()),
    ]
}

pub(crate) fn fetch(settings: &AppSettings, key: &str) -> Result<Ticket, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    fetch_ticket(&client, &base_url, &email, &token, key)
}

pub(crate) fn field_options(
    settings: &AppSettings,
    ticket: &Ticket,
) -> Result<JiraFieldOptions, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let issue_types = if ticket.key.starts_with("NEW-") {
        create_issue_types(&client, &base_url, &email, &token, &ticket.project_key)?
    } else {
        edit_issue_types(&client, &base_url, &email, &token, &ticket.key)?
    };
    let statuses = if ticket.key.starts_with("NEW-") {
        vec![JiraOption {
            id: ticket.status.clone(),
            label: ticket.status.clone(),
        }]
    } else {
        available_statuses(&client, &base_url, &email, &token, ticket)?
    };
    let priorities = priorities(&client, &base_url, &email, &token)?;
    Ok(JiraFieldOptions {
        issue_types,
        statuses,
        priorities,
    })
}

pub(crate) fn assignees(
    settings: &AppSettings,
    project_key: &str,
    query: &str,
) -> Result<Vec<JiraAssignee>, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let response = client
        .get(format!("{base_url}/rest/api/3/user/assignable/search"))
        .basic_auth(email, Some(token))
        .query(&[
            ("project", project_key),
            ("query", query),
            ("maxResults", "50"),
        ])
        .send()
        .map_err(|error| error.to_string())?;
    response_json::<Vec<JiraUser>>(response).map(|users| {
        users
            .into_iter()
            .map(|user| JiraAssignee {
                account_id: user.account_id,
                display_name: user.display_name,
            })
            .collect()
    })
}

fn create_issue_types(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    project_key: &str,
) -> Result<Vec<JiraOption>, String> {
    let response = client
        .get(format!(
            "{base_url}/rest/api/3/issue/createmeta/{project_key}/issuetypes"
        ))
        .basic_auth(email, Some(token))
        .query(&[("maxResults", "100")])
        .send()
        .map_err(|error| error.to_string())?;
    let value = response_json::<Value>(response)?;
    Ok(create_issue_types_from_value(&value))
}

fn create_issue_types_from_value(value: &Value) -> Vec<JiraOption> {
    options_from_values(value.get("values").and_then(Value::as_array))
}

fn create_issue_type(issue_types: Vec<JiraOption>, ticket: &Ticket) -> Result<JiraOption, String> {
    issue_types
        .into_iter()
        .find(|issue_type| ticket_kind(&issue_type.label) == ticket.kind)
        .ok_or_else(|| {
            format!(
                "Project {} cannot create {} issues",
                ticket.project_key,
                ticket_kind_name(ticket.kind)
            )
        })
}

fn edit_issue_types(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
) -> Result<Vec<JiraOption>, String> {
    let response = client
        .get(format!("{base_url}/rest/api/3/issue/{key}/editmeta"))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    let value = response_json::<Value>(response)?;
    Ok(options_from_values(
        value
            .pointer("/fields/issuetype/allowedValues")
            .and_then(Value::as_array),
    ))
}

fn priorities(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
) -> Result<Vec<JiraOption>, String> {
    let response = client
        .get(format!("{base_url}/rest/api/3/priority/search"))
        .basic_auth(email, Some(token))
        .query(&[("maxResults", "100")])
        .send()
        .map_err(|error| error.to_string())?;
    let value = response_json::<Value>(response)?;
    Ok(options_from_values(
        value.get("values").and_then(Value::as_array),
    ))
}

fn available_statuses(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    ticket: &Ticket,
) -> Result<Vec<JiraOption>, String> {
    let response = client
        .get(format!(
            "{base_url}/rest/api/3/issue/{}/transitions",
            ticket.key
        ))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    let transitions = response_json::<TransitionPage>(response)?;
    let mut options = vec![JiraOption {
        id: ticket.status.clone(),
        label: ticket.status.clone(),
    }];
    options.extend(
        transitions
            .transitions
            .into_iter()
            .map(|transition| JiraOption {
                id: transition.id,
                label: transition.to.name,
            }),
    );
    options.sort_by(|left, right| left.label.cmp(&right.label));
    options.dedup_by(|left, right| left.label == right.label);
    Ok(options)
}

fn options_from_values(values: Option<&Vec<Value>>) -> Vec<JiraOption> {
    values
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let label = value.get("name")?.as_str()?.to_owned();
            Some(JiraOption {
                id: value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&label)
                    .to_owned(),
                label,
            })
        })
        .collect()
}

pub(crate) fn projects(settings: &AppSettings) -> Result<Vec<JiraProject>, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let mut start_at = 0;
    let mut projects = Vec::new();
    loop {
        let response = client
            .get(format!("{base_url}/rest/api/3/project/search"))
            .basic_auth(&email, Some(&token))
            .query(&[
                ("startAt", start_at.to_string()),
                ("maxResults", "50".into()),
                ("orderBy", "name".into()),
                ("action", "create".into()),
            ])
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<ProjectPage>(response)?;
        projects.extend(page.values.into_iter().map(|project| JiraProject {
            key: project.key,
            name: project.name,
        }));
        if page.is_last || page.max_results == 0 {
            return Ok(projects);
        }
        start_at = page.start_at.saturating_add(page.max_results);
    }
}

pub(crate) fn submit_changes(
    settings: &AppSettings,
    changes: &[TicketChange],
) -> SubmitBatchOutcome {
    let result = (|| {
        commit_order(changes)?;
        let (client, base_url, email, token) = configured_client(settings)?;
        let existing_keys = changes
            .iter()
            .filter(|change| change.kind != ChangeKind::Added)
            .filter_map(|change| change.original.as_ref().map(|ticket| ticket.key.clone()))
            .collect::<Vec<_>>();
        let current = fetch_tickets(&client, &base_url, &email, &token, &existing_keys)?;
        let conflicts = changes
            .iter()
            .filter(|change| change.kind != ChangeKind::Added)
            .filter_map(|change| {
                let original = change.original.as_ref()?;
                let remote = current.get(&original.key)?;
                (!same_jira_content(original, remote)).then(|| original.key.clone())
            })
            .collect::<Vec<_>>();
        let missing = existing_keys
            .iter()
            .filter(|key| !current.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        if !conflicts.is_empty() || !missing.is_empty() {
            return Ok(SubmitBatchOutcome::Conflict(
                conflicts.into_iter().chain(missing).collect(),
            ));
        }

        submit_ordered_changes(changes, |change| {
            submit_change(&client, &base_url, &email, &token, change, &current)
        })
        .map(SubmitBatchOutcome::Completed)
    })();
    result.unwrap_or_else(SubmitBatchOutcome::PreflightError)
}

fn submit_ordered_changes(
    changes: &[TicketChange],
    mut submit: impl FnMut(&TicketChange) -> Result<SubmissionSnapshot, SubmitFailure>,
) -> Result<Vec<TicketSubmitOutcome>, String> {
    let order = commit_order(changes)?;
    let mut created_keys = HashMap::<String, String>::new();
    let mut failed = std::collections::HashSet::new();
    let mut outcomes = Vec::with_capacity(changes.len());
    for index in order {
        let change = &changes[index];
        let parent = change_parent(change);
        if let Some(parent) = parent.as_deref()
            && parent.starts_with("NEW-")
            && failed.contains(parent)
        {
            failed.insert(change.id.clone());
            outcomes.push(TicketSubmitOutcome {
                id: change.id.clone(),
                result: Err(submit_failure(format!(
                    "Commit skipped: parent {parent} failed"
                ))),
            });
            continue;
        }
        let mut resolved = change.clone();
        if let Some(ticket) = resolved.updated.as_mut()
            && let Some(parent) = ticket.parent_key.as_mut()
            && let Some(key) = created_keys.get(parent)
        {
            *parent = key.clone();
        }
        let result = submit(&resolved);
        if let Ok(snapshot) = &result
            && let Some(ticket) = snapshot.updated.as_ref().or(snapshot.original.as_ref())
            && change.id.starts_with("NEW-")
        {
            created_keys.insert(change.id.clone(), ticket.key.clone());
        }
        if result.is_err() {
            failed.insert(change.id.clone());
        }
        outcomes.push(TicketSubmitOutcome {
            id: change.id.clone(),
            result,
        });
    }
    Ok(outcomes)
}

fn commit_order(changes: &[TicketChange]) -> Result<Vec<usize>, String> {
    let indexes = changes
        .iter()
        .enumerate()
        .map(|(index, change)| (change.id.as_str(), index))
        .collect::<HashMap<_, _>>();
    for change in changes {
        if let Some(parent) = change_parent(change)
            && parent.starts_with("NEW-")
            && !indexes.contains_key(parent.as_str())
        {
            return Err(format!(
                "Commit blocked: {} needs unsent local parent {parent} selected",
                change.id
            ));
        }
    }
    let mut visiting = vec![false; changes.len()];
    let mut visited = vec![false; changes.len()];
    let mut ordered = Vec::with_capacity(changes.len());
    for index in 0..changes.len() {
        visit_commit_change(
            index,
            changes,
            &indexes,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }
    let mut deletion_order = ordered
        .iter()
        .rev()
        .copied()
        .filter(|index| changes[*index].kind == ChangeKind::Deleted)
        .collect::<Vec<_>>();
    let mut commit_order = ordered
        .into_iter()
        .filter(|index| changes[*index].kind != ChangeKind::Deleted)
        .collect::<Vec<_>>();
    commit_order.append(&mut deletion_order);
    Ok(commit_order)
}

fn visit_commit_change(
    index: usize,
    changes: &[TicketChange],
    indexes: &HashMap<&str, usize>,
    visiting: &mut [bool],
    visited: &mut [bool],
    ordered: &mut Vec<usize>,
) -> Result<(), String> {
    if visited[index] {
        return Ok(());
    }
    if visiting[index] {
        return Err("Commit blocked: local parent relationship contains a cycle".into());
    }
    visiting[index] = true;
    if let Some(parent) = change_parent(&changes[index])
        && let Some(parent_index) = indexes.get(parent.as_str())
    {
        visit_commit_change(*parent_index, changes, indexes, visiting, visited, ordered)?;
    }
    visiting[index] = false;
    visited[index] = true;
    ordered.push(index);
    Ok(())
}

fn change_parent(change: &TicketChange) -> Option<String> {
    let ticket = if change.kind == ChangeKind::Deleted {
        change.original.as_ref()
    } else {
        change.updated.as_ref().or(change.original.as_ref())
    };
    ticket.and_then(|ticket| ticket.parent_key.clone())
}

fn submit_failure(message: String) -> SubmitFailure {
    SubmitFailure {
        message,
        refresh: None,
        retry_blocked: false,
    }
}

fn configured_client(settings: &AppSettings) -> Result<(Client, String, String, String), String> {
    let (base_url, email, token) = settings.configured_jira().ok_or_else(|| {
        "Jira is not configured; add URL, email, and API token in Settings".to_string()
    })?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    Ok((
        client,
        base_url.to_owned(),
        email.to_owned(),
        token.to_owned(),
    ))
}

fn submit_change(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    change: &TicketChange,
    current: &HashMap<String, Ticket>,
) -> Result<SubmissionSnapshot, SubmitFailure> {
    match change.kind {
        ChangeKind::Synced => {
            let ticket = change
                .original
                .as_ref()
                .and_then(|ticket| current.get(&ticket.key))
                .cloned()
                .ok_or_else(|| SubmitFailure {
                    message: "Jira ticket was not found during commit".into(),
                    refresh: None,
                    retry_blocked: false,
                })?;
            Ok(SubmissionSnapshot {
                original: Some(ticket.clone()),
                updated: Some(ticket),
            })
        }
        ChangeKind::Deleted => {
            let original = change
                .original
                .as_ref()
                .expect("deleted ticket has original");
            delete_issue(client, base_url, email, token, &original.key).map_err(|message| {
                SubmitFailure {
                    message,
                    refresh: None,
                    retry_blocked: false,
                }
            })?;
            Ok(SubmissionSnapshot {
                original: current.get(&original.key).cloned(),
                updated: None,
            })
        }
        ChangeKind::Modified => {
            let original = change
                .original
                .as_ref()
                .expect("modified ticket has original");
            let desired = change.updated.as_ref().expect("modified ticket has update");
            let current = current
                .get(&original.key)
                .expect("current ticket was checked");
            match update_issue(client, base_url, email, token, current, desired) {
                Ok(updated) => Ok(SubmissionSnapshot {
                    original: Some(current.clone()),
                    updated: Some(updated),
                }),
                Err(message) => Err(failed_with_refresh(
                    client,
                    base_url,
                    email,
                    token,
                    &original.key,
                    desired,
                    message,
                )),
            }
        }
        ChangeKind::Added => {
            let desired = change.updated.as_ref().expect("added ticket has update");
            create_issue(client, base_url, email, token, desired).map(|updated| {
                SubmissionSnapshot {
                    original: None,
                    updated: Some(updated),
                }
            })
        }
    }
}

fn fetch_tickets(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    keys: &[String],
) -> Result<HashMap<String, Ticket>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let response = client
        .post(format!("{base_url}/rest/api/3/issue/bulkfetch"))
        .basic_auth(email, Some(token))
        .header("Accept", "application/json")
        .json(&json!({ "issueIdsOrKeys": keys, "fields": ISSUE_FIELDS }))
        .send()
        .map_err(|error| error.to_string())?;
    let response = response_json::<SearchResponse>(response)?;
    Ok(response
        .issues
        .into_iter()
        .map(to_ticket)
        .map(|ticket| (ticket.key.clone(), ticket))
        .collect())
}

fn fetch_ticket(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
) -> Result<Ticket, String> {
    let response = client
        .get(format!("{base_url}/rest/api/3/issue/{key}"))
        .basic_auth(email, Some(token))
        .query(&[("fields", ISSUE_FIELDS.join(","))])
        .send()
        .map_err(|error| error.to_string())?;
    response_json::<JiraIssue>(response).map(to_ticket)
}

fn create_issue(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    desired: &Ticket,
) -> Result<Ticket, SubmitFailure> {
    if desired.project_key.trim().is_empty() {
        return Err(SubmitFailure {
            message: "Choose a Jira project before committing the new ticket".into(),
            refresh: None,
            retry_blocked: false,
        });
    }
    if desired.kind == TicketKind::Subtask && desired.parent_key.is_none() {
        return Err(SubmitFailure {
            message: "A sub-task needs a parent before it can be committed".into(),
            refresh: None,
            retry_blocked: false,
        });
    }
    let issue_type = create_issue_types(client, base_url, email, token, &desired.project_key)
        .and_then(|issue_types| create_issue_type(issue_types, desired))
        .map_err(|message| SubmitFailure {
            message,
            refresh: None,
            retry_blocked: false,
        })?;
    let account_id =
        resolve_assignee(client, base_url, email, token, desired).map_err(|message| {
            SubmitFailure {
                message,
                refresh: None,
                retry_blocked: false,
            }
        })?;
    let fields = create_issue_fields(desired, account_id.as_deref(), &issue_type);
    let response = client
        .post(format!("{base_url}/rest/api/3/issue"))
        .basic_auth(email, Some(token))
        .header("Accept", "application/json")
        .json(&json!({ "fields": fields }))
        .send()
        .map_err(|error| ambiguous_create_failure(error.to_string()))?;
    if !response.status().is_success() {
        return Err(create_response_failure(
            response.status(),
            response_json::<CreatedIssue>(response).unwrap_err(),
        ));
    }
    let created = response_json::<CreatedIssue>(response).map_err(ambiguous_create_failure)?;
    let mut created_desired = desired.clone();
    created_desired.key = created.key.clone();
    let created_ticket = fetch_ticket(client, base_url, email, token, &created.key)
        .map_err(|message| created_issue_failure(message, created_desired.clone(), None))?;
    if created_ticket.status != desired.status
        && let Err(message) = transition_issue(
            client,
            base_url,
            email,
            token,
            &created.key,
            &desired.status,
        )
    {
        return Err(failed_created_with_refresh(
            client,
            base_url,
            email,
            token,
            &created.key,
            &created_desired,
            message,
        ));
    }
    fetch_ticket(client, base_url, email, token, &created.key)
        .map_err(|message| created_issue_failure(message, created_desired, None))
}

fn update_issue(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    original: &Ticket,
    desired: &Ticket,
) -> Result<Ticket, String> {
    if desired.kind == TicketKind::Subtask && desired.parent_key.is_none() {
        return Err("A sub-task cannot be moved to Root in Jira".into());
    }
    ensure_description_can_be_overwritten(original, desired)?;
    let account_id = resolve_assignee(client, base_url, email, token, desired)?;
    let payload = update_payload(original, desired, account_id.as_deref())?;
    let response = client
        .put(format!("{base_url}/rest/api/3/issue/{}", original.key))
        .basic_auth(email, Some(token))
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .map_err(|error| error.to_string())?;
    ensure_success(response)?;
    if original.status != desired.status {
        transition_issue(
            client,
            base_url,
            email,
            token,
            &original.key,
            &desired.status,
        )?;
    }
    fetch_ticket(client, base_url, email, token, &original.key)
}

fn delete_issue(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
) -> Result<(), String> {
    let response = client
        .delete(format!("{base_url}/rest/api/3/issue/{key}"))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    ensure_success(response)
}

fn transition_issue(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
    desired_status: &str,
) -> Result<(), String> {
    if desired_status.trim().is_empty() {
        return Ok(());
    }
    let response = client
        .get(format!("{base_url}/rest/api/3/issue/{key}/transitions"))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    let transitions = response_json::<TransitionPage>(response)?;
    let Some(transition) = transitions
        .transitions
        .iter()
        .find(|transition| transition.to.name.eq_ignore_ascii_case(desired_status))
    else {
        return Err(format!(
            "Jira offers no transition from {key} to {desired_status}"
        ));
    };
    let response = client
        .post(format!("{base_url}/rest/api/3/issue/{key}/transitions"))
        .basic_auth(email, Some(token))
        .json(&json!({ "transition": { "id": transition.id } }))
        .send()
        .map_err(|error| error.to_string())?;
    ensure_success(response)
}

fn resolve_assignee(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    ticket: &Ticket,
) -> Result<Option<String>, String> {
    if ticket.assignee.trim().is_empty() || ticket.assignee.eq_ignore_ascii_case("unassigned") {
        return Ok(None);
    }
    if !ticket.assignee_account_id.is_empty() {
        return Ok(Some(ticket.assignee_account_id.clone()));
    }
    let response = client
        .get(format!("{base_url}/rest/api/3/user/assignable/search"))
        .basic_auth(email, Some(token))
        .query(&[
            ("project", ticket.project_key.as_str()),
            ("query", ticket.assignee.as_str()),
            ("maxResults", "50"),
        ])
        .send()
        .map_err(|error| error.to_string())?;
    let users = response_json::<Vec<JiraUser>>(response)?;
    users
        .into_iter()
        .find(|user| user.display_name.eq_ignore_ascii_case(&ticket.assignee))
        .map(|user| Some(user.account_id))
        .ok_or_else(|| format!("No assignable Jira user matches {}", ticket.assignee))
}

fn issue_fields(ticket: &Ticket, account_id: Option<&str>, include_project: bool) -> Value {
    let mut fields = Map::new();
    if include_project {
        fields.insert("project".into(), json!({ "key": ticket.project_key }));
    }
    fields.insert("summary".into(), json!(ticket.title));
    fields.insert("description".into(), markdown_to_adf(&ticket.description));
    fields.insert(
        "issuetype".into(),
        json!({ "name": ticket_kind_name(ticket.kind) }),
    );
    if !ticket.priority.trim().is_empty() {
        fields.insert("priority".into(), json!({ "name": ticket.priority }));
    }
    fields.insert(
        "assignee".into(),
        account_id.map_or(Value::Null, |id| json!({ "accountId": id })),
    );
    if let Some(parent) = ticket.parent_key.as_deref() {
        fields.insert("parent".into(), json!({ "key": parent }));
    }
    Value::Object(fields)
}

fn create_issue_fields(
    ticket: &Ticket,
    account_id: Option<&str>,
    issue_type: &JiraOption,
) -> Value {
    let mut fields = issue_fields(ticket, account_id, true)
        .as_object()
        .expect("issue fields must be an object")
        .clone();
    fields.insert("issuetype".into(), json!({ "id": issue_type.id }));
    Value::Object(fields)
}

fn update_payload(
    original: &Ticket,
    desired: &Ticket,
    account_id: Option<&str>,
) -> Result<Value, String> {
    if desired.kind == TicketKind::Subtask && desired.parent_key.is_none() {
        return Err("A sub-task cannot be moved to Root in Jira".into());
    }
    ensure_description_can_be_overwritten(original, desired)?;
    let mut payload = Map::new();
    let mut fields = issue_fields(desired, account_id, false)
        .as_object()
        .expect("issue fields must be an object")
        .clone();
    if original.description == desired.description {
        fields.remove("description");
    }
    payload.insert("fields".into(), Value::Object(fields));
    if original.parent_key.is_some() && desired.parent_key.is_none() {
        payload.insert(
            "update".into(),
            json!({ "parent": [{ "set": Value::Null }] }),
        );
    }
    Ok(Value::Object(payload))
}

fn ensure_description_can_be_overwritten(
    original: &Ticket,
    desired: &Ticket,
) -> Result<(), String> {
    if original.description != desired.description && !original.description_safe_to_overwrite {
        return Err(
            "Jira description contains unsupported formatting and cannot be edited safely".into(),
        );
    }
    Ok(())
}

fn failed_with_refresh(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
    desired: &Ticket,
    message: String,
) -> SubmitFailure {
    let refresh = fetch_ticket(client, base_url, email, token, key)
        .ok()
        .map(|current| {
            let mut desired = desired.clone();
            desired.key = current.key.clone();
            desired.project_key = current.project_key.clone();
            Box::new((current, desired))
        });
    SubmitFailure {
        message,
        refresh,
        retry_blocked: false,
    }
}

fn ambiguous_create_failure(message: String) -> SubmitFailure {
    SubmitFailure {
        message: format!(
            "Jira create outcome is unknown: {message}. Jira may have created the ticket; retry is blocked to prevent a duplicate. Search Jira, then remove or reconcile this draft."
        ),
        refresh: None,
        retry_blocked: true,
    }
}

fn create_response_failure(status: StatusCode, message: String) -> SubmitFailure {
    if status.is_server_error() {
        ambiguous_create_failure(message)
    } else {
        submit_failure(message)
    }
}

fn failed_created_with_refresh(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    key: &str,
    desired: &Ticket,
    message: String,
) -> SubmitFailure {
    created_issue_failure(
        message,
        desired.clone(),
        fetch_ticket(client, base_url, email, token, key).ok(),
    )
}

fn created_issue_failure(
    message: String,
    desired: Ticket,
    recovered: Option<Ticket>,
) -> SubmitFailure {
    let original = recovered.unwrap_or_else(|| desired.clone());
    SubmitFailure {
        message,
        refresh: Some(Box::new((original, desired))),
        retry_blocked: false,
    }
}

fn same_jira_content(left: &Ticket, right: &Ticket) -> bool {
    left.key == right.key
        && left.title == right.title
        && left.description == right.description
        && left.description_safe_to_overwrite == right.description_safe_to_overwrite
        && left.kind == right.kind
        && left.status == right.status
        && left.priority == right.priority
        && left.assignee == right.assignee
        && left.assignee_account_id == right.assignee_account_id
        && left.parent_key == right.parent_key
}

fn response_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "unknown Jira error".into());
        return Err(format!("Jira returned {}: {body}", status.as_u16()));
    }
    response.json::<T>().map_err(|error| error.to_string())
}

fn ensure_success(response: reqwest::blocking::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = response
            .text()
            .unwrap_or_else(|_| "unknown Jira error".into());
        Err(format!("Jira returned {}: {body}", status.as_u16()))
    }
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
            "fields": ISSUE_FIELDS,
            "maxResults": 100
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
        parent_key: field("parent")
            .get("key")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parent_kind: field("parent")
            .pointer("/fields/issuetype/name")
            .and_then(Value::as_str)
            .map(ticket_kind),
    }
}

fn to_work_item(issue: JiraIssue) -> WorkItem {
    let field = |name: &str| issue.fields.get(name).unwrap_or(&Value::Null);
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

fn ticket_kind_name(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Epic => "Epic",
        TicketKind::Story => "Story",
        TicketKind::Task => "Task",
        TicketKind::Bug => "Bug",
        TicketKind::Subtask => "Sub-task",
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
