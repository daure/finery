use std::{collections::HashMap, time::Duration};

use reqwest::{StatusCode, blocking::Client};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    app_settings::AppSettings,
    store::composer::{
        ChangeKind, SubmissionSnapshot, Ticket, TicketChange, TicketKind, change_parent,
        jira_adf::markdown_to_adf,
    },
    store::work_items::{
        BacklogSnapshot, RankPlan, RunwayCapacitySource, Sprint, WorkItem, apply_capacity,
        loaded_story_point_average,
    },
};

mod mapping;

use mapping::*;

const ISSUE_FIELDS: [&str; 9] = [
    "summary",
    "description",
    "issuetype",
    "status",
    "priority",
    "assignee",
    "project",
    "parent",
    "subtasks",
];

const BACKLOG_FIELDS: [&str; 8] = [
    "summary",
    "issuetype",
    "status",
    "priority",
    "assignee",
    "parent",
    "subtasks",
    "fixVersions",
];
const BACKLOG_JQL: &str = "issuetype not in subTaskIssueTypes() AND issuetype != Epic";

pub(crate) struct BacklogLoad {
    pub snapshot: BacklogSnapshot,
    pub discovered_story_points: Option<DiscoveredStoryPoints>,
}

pub(crate) struct DiscoveredStoryPoints {
    pub board_id: String,
    pub field_id: String,
    pub discovery_complete: bool,
}

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
    #[serde(default, rename = "isLast")]
    is_last: bool,
    #[serde(default, rename = "startAt")]
    start_at: usize,
    #[serde(default, rename = "maxResults")]
    max_results: usize,
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
    #[serde(rename = "startDate")]
    start_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date: Option<String>,
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
pub(super) struct JiraIssue {
    pub(super) key: String,
    pub(super) fields: Value,
}

#[derive(Debug, Deserialize)]
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

pub(crate) fn fetch_tickets(
    settings: &AppSettings,
    keys: &[String],
) -> Result<HashMap<String, Ticket>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let (client, base_url, email, token) = configured_client(settings)?;
    let tickets = bulk_fetch_tickets(&client, &base_url, &email, &token, keys)?;
    let missing = keys
        .iter()
        .filter(|key| !tickets.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(tickets)
    } else {
        Err(format!(
            "Jira did not return requested tickets: {}",
            missing.join(", ")
        ))
    }
}

pub(crate) fn backlog(settings: &AppSettings) -> Result<BacklogLoad, String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let board = backlog_board(&client, &base_url, &email, &token, settings)?;
    let (discovered_story_points, discovery_warning) =
        if should_discover_story_points(settings, board.id) {
            discover_story_points(
                settings,
                board.id,
                board_story_points_field(&client, &base_url, &email, &token, board.id),
            )
        } else {
            (None, None)
        };
    let story_points_field_id =
        story_points_field_for_load(settings, discovered_story_points.as_ref());
    let velocity = settings
        .backlog_runway
        .use_jira_velocity
        .then(|| board_velocity(&client, &base_url, &email, &token, board.id));
    let sprints = board_sprints(&client, &base_url, &email, &token, board.id)?;
    let (sprints, work_items) = std::thread::scope(|scope| -> Result<_, String> {
        let backlog_client = client.clone();
        let backlog_base_url = base_url.clone();
        let backlog_email = email.clone();
        let backlog_token = token.clone();
        let board_id = board.id;
        let story_points_field_id = story_points_field_id.map(str::to_owned);
        let backlog_story_points_field_id = story_points_field_id.clone();
        let backlog = scope.spawn(move || {
            board_backlog(
                &backlog_client,
                &backlog_base_url,
                &backlog_email,
                &backlog_token,
                board_id,
                backlog_story_points_field_id.as_deref(),
            )
        });
        let sprint_requests = sprints
            .iter()
            .map(|sprint| {
                let sprint_id = sprint.id;
                let client = client.clone();
                let base_url = base_url.clone();
                let email = email.clone();
                let token = token.clone();
                let story_points_field_id = story_points_field_id.clone();
                scope.spawn(move || {
                    sprint_issues(
                        &client,
                        &base_url,
                        &email,
                        &token,
                        sprint_id,
                        story_points_field_id.as_deref(),
                    )
                })
            })
            .collect::<Vec<_>>();
        let work_items = backlog
            .join()
            .map_err(|_| "Jira backlog request panicked".to_string())??;
        let sprints = sprints
            .into_iter()
            .zip(sprint_requests)
            .map(|(sprint, request)| {
                let work_items = request
                    .join()
                    .map_err(|_| "Jira sprint request panicked".to_string())??;
                Ok(Sprint {
                    id: sprint.id,
                    name: sprint.name,
                    state: sprint.state,
                    start_date: sprint.start_date,
                    end_date: sprint.end_date,
                    work_items,
                    capacity: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((sprints, work_items))
    })?;
    let mut snapshot = BacklogSnapshot {
        board_name: board.name,
        story_points_configured: story_points_field_id.is_some(),
        sprints,
        work_items,
        warnings: Vec::new(),
        runway: None,
    };
    if let Some(warning) = discovery_warning {
        snapshot.warnings.push(warning);
    }
    if let Some(warning) = story_points_warning(&snapshot) {
        snapshot.warnings.push(warning);
    }
    let (capacity, source) = match velocity {
        Some(Ok(capacity)) => (capacity, RunwayCapacitySource::JiraVelocity),
        Some(Err(error)) => {
            snapshot.warnings.push(format!(
                "Could not load Jira velocity; using the fixed sprint capacity instead: {error}"
            ));
            (
                settings.backlog_runway.fixed_sprint_capacity,
                RunwayCapacitySource::FixedFallback,
            )
        }
        None => (
            settings.backlog_runway.fixed_sprint_capacity,
            RunwayCapacitySource::Fixed,
        ),
    };
    let assumed_ticket_size = if settings.backlog_runway.use_average_ticket_size {
        loaded_story_point_average(&snapshot).map(|size| (size, true))
    } else {
        Some((settings.backlog_runway.fixed_ticket_size, false))
    };
    if assumed_ticket_size.is_none() {
        snapshot.warnings.push(
            "Could not calculate an assumed ticket size because no loaded tickets have story points. Set a fixed assumed ticket size in Settings.".into(),
        );
    }
    apply_capacity(
        &mut snapshot,
        capacity,
        assumed_ticket_size,
        source,
        settings.backlog_runway.sprint_tolerance_percent,
    );
    Ok(BacklogLoad {
        snapshot,
        discovered_story_points,
    })
}

pub(crate) fn rank(settings: &AppSettings, plan: &RankPlan) -> Result<(), String> {
    let (client, base_url, email, token) = configured_client(settings)?;
    let response = client
        .put(format!("{base_url}/rest/agile/1.0/issue/rank"))
        .basic_auth(email, Some(token))
        .json(&rank_payload(plan))
        .send()
        .map_err(|error| error.to_string())?;
    ensure_rank_success(response)
}

pub(crate) fn move_to_sprint(
    settings: &AppSettings,
    sprint_id: u64,
    issue_keys: &[String],
) -> Result<(), String> {
    move_issues(
        settings,
        format!("/rest/agile/1.0/sprint/{sprint_id}/issue"),
        issue_keys,
    )
}

pub(crate) fn move_to_backlog(settings: &AppSettings, issue_keys: &[String]) -> Result<(), String> {
    move_issues(settings, "/rest/agile/1.0/backlog/issue".into(), issue_keys)
}

fn move_issues(settings: &AppSettings, path: String, issue_keys: &[String]) -> Result<(), String> {
    if issue_keys.is_empty() {
        return Ok(());
    }
    if issue_keys.len() > crate::store::work_items::MAX_RANK_ISSUES {
        return Err(format!(
            "Jira can move at most {} issues at once",
            crate::store::work_items::MAX_RANK_ISSUES
        ));
    }
    let (client, base_url, email, token) = configured_client(settings)?;
    let response = client
        .post(format!("{base_url}{path}"))
        .basic_auth(email, Some(token))
        .json(&move_payload(issue_keys))
        .send()
        .map_err(|error| error.to_string())?;
    ensure_success(response)
}

fn move_payload(issue_keys: &[String]) -> Value {
    json!({ "issues": issue_keys })
}

fn rank_payload(plan: &RankPlan) -> Value {
    let mut payload = Map::new();
    payload.insert("issues".into(), json!(plan.issues));
    if let Some(anchor) = plan.rank_before_issue.as_ref() {
        payload.insert("rankBeforeIssue".into(), json!(anchor));
    } else if let Some(anchor) = plan.rank_after_issue.as_ref() {
        payload.insert("rankAfterIssue".into(), json!(anchor));
    }
    Value::Object(payload)
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
    let mut boards = Vec::new();
    let mut start_at = 0;
    loop {
        let mut page_query = query.clone();
        page_query.push(("startAt", start_at.to_string()));
        let response = client
            .get(format!("{base_url}/rest/agile/1.0/board"))
            .basic_auth(email, Some(token))
            .query(&page_query)
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<AgileBoardPage>(response)?;
        boards.extend(page.values);
        if page.is_last || page.max_results == 0 {
            break;
        }
        start_at = page.start_at.saturating_add(page.max_results);
    }
    select_backlog_board(boards).ok_or_else(|| {
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

fn board_velocity(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    board_id: u64,
) -> Result<f64, String> {
    let response = client
        .get(format!(
            "{base_url}/rest/greenhopper/1.0/rapid/charts/velocity.json"
        ))
        .basic_auth(email, Some(token))
        .query(&[("rapidViewId", board_id.to_string())])
        .send()
        .map_err(|error| error.to_string())?;
    response_json::<Value>(response).and_then(velocity_average)
}

fn velocity_average(chart: Value) -> Result<f64, String> {
    let entries = chart
        .get("velocityStatEntries")
        .and_then(|entries| match entries {
            Value::Array(entries) => Some(entries.iter().collect::<Vec<_>>()),
            Value::Object(entries) => Some(entries.values().collect::<Vec<_>>()),
            _ => None,
        })
        .ok_or_else(|| "Jira velocity chart did not contain sprint entries".to_string())?;
    let completed = entries
        .into_iter()
        .filter_map(|entry| {
            entry
                .pointer("/completed/value")
                .or_else(|| entry.get("completed"))
                .and_then(Value::as_f64)
        })
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect::<Vec<_>>();
    (!completed.is_empty())
        .then(|| completed.iter().sum::<f64>() / completed.len() as f64)
        .ok_or_else(|| "Jira velocity chart did not contain completed estimates".to_string())
}

fn board_story_points_field(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    board_id: u64,
) -> Result<String, String> {
    let response = client
        .get(format!(
            "{base_url}/rest/agile/1.0/board/{board_id}/configuration"
        ))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    response_json::<Value>(response).map(|configuration| story_points_field_id(&configuration))
}

fn story_points_field_id(configuration: &Value) -> String {
    (configuration
        .pointer("/estimation/type")
        .and_then(Value::as_str)
        == Some("field"))
    .then(|| {
        configuration
            .pointer("/estimation/field/fieldId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    })
    .unwrap_or_default()
}

fn should_discover_story_points(settings: &AppSettings, board_id: u64) -> bool {
    if settings.story_points_field_is_manual() {
        return false;
    }
    let known_board = settings.jira_story_points_board_id.trim();
    if known_board.is_empty() {
        return true;
    }
    known_board != board_id.to_string() || !settings.jira_story_points_discovery_complete
}

fn story_points_field_for_load<'a>(
    settings: &'a AppSettings,
    discovery: Option<&'a DiscoveredStoryPoints>,
) -> Option<&'a str> {
    discovery
        .map(|discovery| discovery.field_id.trim())
        .filter(|field_id| !field_id.is_empty())
        .or_else(|| {
            let field_id = settings.jira_story_points_field_id.trim();
            (!field_id.is_empty()).then_some(field_id)
        })
}

fn discover_story_points(
    settings: &AppSettings,
    board_id: u64,
    result: Result<String, String>,
) -> (Option<DiscoveredStoryPoints>, Option<String>) {
    if !should_discover_story_points(settings, board_id) {
        return (None, None);
    }
    match result {
        Ok(field_id) => (
            Some(DiscoveredStoryPoints {
                board_id: board_id.to_string(),
                field_id,
                discovery_complete: true,
            }),
            None,
        ),
        Err(error) => (
            None,
            Some(format!(
                "Could not discover the Jira story-points field: {error}. Loaded tickets using any stored field ID; retry on the next backlog refresh."
            )),
        ),
    }
}

fn sprint_issues(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    sprint_id: u64,
    story_points_field_id: Option<&str>,
) -> Result<Vec<WorkItem>, String> {
    let mut start_at = 0;
    let mut work_items = Vec::new();
    loop {
        let query = vec![
            ("startAt", start_at.to_string()),
            ("maxResults", "100".to_string()),
            ("fields", backlog_fields(story_points_field_id).join(",")),
        ];
        let response = client
            .get(format!(
                "{base_url}/rest/agile/1.0/sprint/{sprint_id}/issue"
            ))
            .basic_auth(email, Some(token))
            .query(&query)
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<AgileIssuePage>(response)?;
        let loaded = page.issues.len();
        let complete = backlog_page_complete(&page, loaded);
        work_items.extend(
            page.issues
                .into_iter()
                .map(|issue| to_work_item(issue, story_points_field_id)),
        );
        if complete {
            return Ok(work_items);
        }
        start_at = page.start_at.saturating_add(page.max_results.max(loaded));
    }
}

fn board_backlog(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    board_id: u64,
    story_points_field_id: Option<&str>,
) -> Result<Vec<WorkItem>, String> {
    let mut start_at = 0;
    let mut work_items = Vec::new();
    loop {
        let response = client
            .get(format!(
                "{base_url}/rest/agile/1.0/board/{board_id}/backlog"
            ))
            .basic_auth(email, Some(token))
            .query(&board_backlog_query(start_at, story_points_field_id))
            .send()
            .map_err(|error| error.to_string())?;
        let page = response_json::<AgileIssuePage>(response)?;
        let loaded = page.issues.len();
        let complete = backlog_page_complete(&page, loaded);
        work_items.extend(
            page.issues
                .into_iter()
                .map(|issue| to_work_item(issue, story_points_field_id)),
        );
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
        || (page.max_results == 0 && page.next_page_token.is_none())
}

fn board_backlog_query(
    start_at: usize,
    story_points_field_id: Option<&str>,
) -> Vec<(&'static str, String)> {
    vec![
        ("startAt", start_at.to_string()),
        ("maxResults", "100".into()),
        ("fields", backlog_fields(story_points_field_id).join(",")),
        ("jql", BACKLOG_JQL.into()),
    ]
}

fn backlog_fields(story_points_field_id: Option<&str>) -> Vec<&str> {
    let mut fields = BACKLOG_FIELDS.to_vec();
    if let Some(field_id) = story_points_field_id.filter(|field_id| !field_id.trim().is_empty()) {
        fields.push(field_id);
    }
    fields
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
        create_available_statuses(&client, &base_url, &email, &token, ticket)?
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
    options_from_values(
        value
            .get("issueTypes")
            .or_else(|| value.get("values"))
            .and_then(Value::as_array),
    )
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

fn create_available_statuses(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    ticket: &Ticket,
) -> Result<Vec<JiraOption>, String> {
    let response = client
        .get(format!(
            "{base_url}/rest/api/3/project/{}/statuses",
            ticket.project_key
        ))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    let statuses = response_json::<Value>(response)?;
    Ok(create_available_statuses_from_value(&statuses, ticket.kind))
}

fn create_available_statuses_from_value(value: &Value, kind: TicketKind) -> Vec<JiraOption> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .find(|issue_type| {
            issue_type
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| ticket_kind(name) == kind)
        })
        .and_then(|issue_type| issue_type.get("statuses").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|status| {
            let label = status.get("name")?.as_str()?.to_owned();
            Some(JiraOption {
                id: status
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(&label)
                    .to_owned(),
                label,
            })
        })
        .collect()
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
        let current = bulk_fetch_tickets(&client, &base_url, &email, &token, &existing_keys)?;
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

fn bulk_fetch_tickets(
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

fn ensure_rank_success(response: reqwest::blocking::Response) -> Result<(), String> {
    let status = response.status();
    if status == StatusCode::NO_CONTENT {
        return Ok(());
    }
    let body = response
        .text()
        .unwrap_or_else(|_| "unknown Jira error".into());
    Err(format!("Jira returned {}: {body}", status.as_u16()))
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

#[cfg(test)]
mod tests;
