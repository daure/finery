use std::{collections::HashMap, net::SocketAddr};

use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{StreamableHttpServerConfig, StreamableHttpService, stdio},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    service::{
        AppService,
        composer_service::{
            ChangeKindView, ChangeSetCatalogView, ChangeSetPatchOperation, ChangeSetPatchResponse,
            ChangeSetView, ComposerService, RecoveredCreate, ServiceError, SubmitChangeSetResponse,
            TicketChangeView, TicketKindView, TicketView, Versioned,
        },
    },
    store::work_items::{BacklogSnapshot, WorkItem},
};

const MCP_INSTRUCTIONS: &str = "Read and edit Finery Composer change sets. get_workspace returns compact active/future sprint tickets, plus the top 50 unplanned Jira tickets in rank order. Full sprint lists are never capped. backlog.unplanned_ticket_limit, backlog.unplanned_total_count, and backlog.unplanned_truncated make that limit explicit. Workspace tickets never include descriptions, include direct-parent metadata and has_children, and include only open Composer change sets. It refreshes Jira baselines for tickets in those open change sets through one batched Jira request. Revisions are optimistic-concurrency tokens: read current data before mutations and send its revision as expected_revision. apply_change_set_patch persists local edits only; it never submits Jira. refresh_change_set refreshes Jira baselines and returns the refreshed canonical change set. recover_submission_attempt clears safely claimed attempts or reconciles marked draft creates only after explicit Jira-key confirmation. submit_change_set is the only Jira submission tool and requires explicit ticket IDs. A submitted ticket cannot be edited or submitted again.";
const WORKSPACE_UNPLANNED_TICKET_LIMIT: usize = 50;

#[derive(Clone)]
struct McpServer {
    service: AppService,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    fn new(service: AppService) -> Self {
        let mut tool_router = Self::tool_router();
        for route in tool_router.map.values_mut() {
            route.attr.output_schema = None;
        }
        Self {
            service,
            tool_router,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChangeSetId {
    change_set_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PatchChangeSet {
    change_set_id: String,
    expected_revision: i64,
    #[schemars(length(min = 1))]
    operations: Vec<ChangeSetPatchOperation>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RefreshChangeSet {
    change_set_id: String,
    expected_revision: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SubmitChangeSet {
    change_set_id: String,
    expected_revision: i64,
    #[schemars(length(min = 1))]
    selected_ticket_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecoverSubmissionAttempt {
    change_set_id: String,
    expected_revision: i64,
    #[serde(default)]
    recovered_creates: Vec<RecoveredCreate>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceView {
    backlog: WorkspaceBacklogView,
    change_set_catalog_revision: i64,
    change_sets: Vec<WorkspaceChangeSetView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceBacklogView {
    board_name: String,
    warnings: Vec<String>,
    sprints: Vec<WorkspaceSprintView>,
    unplanned_tickets: Vec<WorkspaceWorkItemView>,
    unplanned_ticket_limit: usize,
    unplanned_total_count: usize,
    unplanned_truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceSprintView {
    id: u64,
    name: String,
    state: String,
    tickets: Vec<WorkspaceWorkItemView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceWorkItemView {
    key: String,
    title: String,
    kind: String,
    status: String,
    priority: String,
    assignee: String,
    parent: Option<WorkspaceParentTicketView>,
    has_children: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_points: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceParentTicketView {
    key: String,
    title: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceChangeSetView {
    id: String,
    name: String,
    revision: i64,
    closed: bool,
    tickets: Vec<WorkspaceChangeTicketView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceChangeTicketView {
    id: String,
    kind: ChangeKindView,
    original: Option<WorkspaceComposerTicketView>,
    updated: Option<WorkspaceComposerTicketView>,
    submitted: bool,
    selected_for_commit: bool,
    children: Vec<WorkspaceChangeTicketView>,
}

fn mcp_error(error: ServiceError) -> String {
    match error {
        ServiceError::NotFound { resource, id } => {
            format!("not_found: {resource} '{id}' was not found")
        }
        ServiceError::ClosedChangeSet { change_set_id } => {
            format!("closed_change_set: '{change_set_id}' is closed")
        }
        ServiceError::SubmittedTicket { ticket_id } => {
            format!("submitted_ticket: '{ticket_id}' was already submitted")
        }
        ServiceError::SubmissionClaimed { ticket_id } => {
            format!("submission_claimed: '{ticket_id}' has a durable submission claim")
        }
        ServiceError::InvalidOperation { message } => format!("invalid_operation: {message}"),
        ServiceError::StaleRevision { change_set_id } => {
            format!("stale_revision: reread change set '{change_set_id}' and retry")
        }
        ServiceError::JiraLookup { jira_key, .. } => {
            format!("jira_lookup_failed: could not load Jira ticket '{jira_key}'")
        }
        ServiceError::Storage { .. } => "storage_error: local persistence failed".into(),
        ServiceError::Submission { .. } => {
            "jira_submission_failed: Jira returned incomplete results".into()
        }
    }
}

async fn run_composer<T>(
    service: ComposerService,
    operation: impl FnOnce(ComposerService) -> Result<T, ServiceError> + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || operation(service))
        .await
        .map_err(|_| "internal_error: background task failed".to_string())?
        .map_err(mcp_error)
}

async fn run_workspace(service: AppService) -> Result<WorkspaceView, String> {
    tokio::task::spawn_blocking(move || {
        let composer = service.composer_service();
        let jira_ticket_keys = composer
            .open_change_set_jira_ticket_keys()
            .map_err(mcp_error)?;
        let backlog = service
            .jira_backlog()
            .map_err(|error| format!("jira_backlog_failed: {error}"))?;
        let refreshed_tickets = service.fetch_jira_tickets(&jira_ticket_keys)?;
        let change_sets = composer
            .refresh_open_change_set_baselines(&refreshed_tickets)
            .map_err(mcp_error)?;
        Ok(workspace_view(backlog, change_sets))
    })
    .await
    .map_err(|_| "internal_error: background task failed".to_string())?
}

fn workspace_view(
    backlog: BacklogSnapshot,
    change_sets: Versioned<ChangeSetCatalogView>,
) -> WorkspaceView {
    let unplanned_total_count = backlog.work_items.len();
    WorkspaceView {
        backlog: WorkspaceBacklogView {
            board_name: backlog.board_name,
            warnings: backlog.warnings,
            sprints: backlog
                .sprints
                .into_iter()
                .map(|sprint| WorkspaceSprintView {
                    id: sprint.id,
                    name: sprint.name,
                    state: sprint.state,
                    tickets: sprint.work_items.into_iter().map(Into::into).collect(),
                })
                .collect(),
            unplanned_tickets: backlog
                .work_items
                .into_iter()
                .take(WORKSPACE_UNPLANNED_TICKET_LIMIT)
                .map(Into::into)
                .collect(),
            unplanned_ticket_limit: WORKSPACE_UNPLANNED_TICKET_LIMIT,
            unplanned_total_count,
            unplanned_truncated: unplanned_total_count > WORKSPACE_UNPLANNED_TICKET_LIMIT,
        },
        change_set_catalog_revision: change_sets.revision,
        change_sets: change_sets
            .value
            .change_sets
            .into_iter()
            .filter(|change_set| !change_set.value.closed)
            .map(workspace_change_set_view)
            .collect(),
    }
}

fn workspace_change_set_view(change_set: Versioned<ChangeSetView>) -> WorkspaceChangeSetView {
    let ChangeSetView {
        id,
        name,
        closed,
        tickets,
        ..
    } = change_set.value;
    WorkspaceChangeSetView {
        id,
        name,
        revision: change_set.revision,
        closed,
        tickets: workspace_change_ticket_tree(tickets),
    }
}

fn workspace_change_ticket_tree(tickets: Vec<TicketChangeView>) -> Vec<WorkspaceChangeTicketView> {
    let aliases = tickets
        .iter()
        .flat_map(|ticket| {
            let id = ticket.id.clone();
            std::iter::once((id.clone(), id.clone())).chain(
                ticket
                    .updated
                    .as_ref()
                    .or(ticket.original.as_ref())
                    .map(|ticket| (ticket.key.clone(), id)),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut children = HashMap::<String, Vec<TicketChangeView>>::new();
    let mut roots = Vec::new();
    for ticket in tickets {
        let parent = ticket
            .updated
            .as_ref()
            .or(ticket.original.as_ref())
            .and_then(|ticket| ticket.parent_key.as_deref());
        if let Some(parent) = parent.and_then(|parent| aliases.get(parent)) {
            children.entry(parent.into()).or_default().push(ticket);
        } else {
            roots.push(ticket);
        }
    }
    roots
        .into_iter()
        .map(|ticket| workspace_change_ticket_view(ticket, &mut children))
        .collect()
}

fn workspace_change_ticket_view(
    ticket: TicketChangeView,
    children_by_parent: &mut HashMap<String, Vec<TicketChangeView>>,
) -> WorkspaceChangeTicketView {
    let children = children_by_parent
        .remove(&ticket.id)
        .unwrap_or_default()
        .into_iter()
        .map(|child| workspace_change_ticket_view(child, children_by_parent))
        .collect::<Vec<_>>();
    let has_children = !children.is_empty();
    let original = ticket
        .original
        .map(|ticket| WorkspaceComposerTicketView::from(ticket, has_children));
    WorkspaceChangeTicketView {
        id: ticket.id,
        kind: ticket.kind,
        original,
        updated: ticket
            .updated
            .map(|ticket| WorkspaceComposerTicketView::from(ticket, has_children)),
        submitted: ticket.submitted,
        selected_for_commit: ticket.selected_for_commit,
        children,
    }
}

impl From<WorkItem> for WorkspaceWorkItemView {
    fn from(ticket: WorkItem) -> Self {
        Self {
            key: ticket.key,
            title: ticket.title,
            kind: ticket.kind,
            status: ticket.status,
            priority: ticket.priority,
            assignee: ticket.assignee,
            parent: workspace_parent(ticket.parent_key, ticket.parent_title),
            has_children: ticket.has_children,
            story_points: ticket.story_points,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceComposerTicketView {
    key: String,
    title: String,
    kind: String,
    status: String,
    priority: String,
    assignee: String,
    parent: Option<WorkspaceParentTicketView>,
    has_children: bool,
}

impl WorkspaceComposerTicketView {
    fn from(ticket: TicketView, has_children: bool) -> Self {
        Self {
            key: ticket.key,
            title: ticket.title,
            kind: workspace_ticket_kind(ticket.kind).into(),
            status: ticket.status,
            priority: ticket.priority,
            assignee: ticket.assignee,
            parent: workspace_parent(ticket.parent_key, ticket.parent_title),
            has_children: ticket.has_children || has_children,
        }
    }
}

fn workspace_parent(
    key: Option<String>,
    title: Option<String>,
) -> Option<WorkspaceParentTicketView> {
    key.map(|key| WorkspaceParentTicketView {
        title: title.unwrap_or_else(|| key.clone()),
        key,
    })
}

fn workspace_ticket_kind(kind: TicketKindView) -> &'static str {
    match kind {
        TicketKindView::Epic => "Epic",
        TicketKindView::Story => "Story",
        TicketKindView::Task => "Task",
        TicketKindView::Bug => "Bug",
        TicketKindView::Subtask => "Subtask",
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        description = "Get active/future sprint tickets and the top 50 unplanned Jira tickets in rank order. Full sprint lists are never capped. backlog.unplanned_ticket_limit, backlog.unplanned_total_count, and backlog.unplanned_truncated describe the unplanned limit. Tickets include compact metadata, never descriptions. Only open Composer change sets are returned."
    )]
    async fn get_workspace(&self) -> Result<Json<WorkspaceView>, String> {
        run_workspace(self.service.clone()).await.map(Json)
    }

    #[tool(description = "List Composer change sets with revisioned canonical data")]
    async fn list_change_sets(&self) -> Result<Json<Versioned<ChangeSetCatalogView>>, String> {
        run_composer(self.service.composer_service(), |service| {
            service.change_set_catalog()
        })
        .await
        .map(Json)
    }

    #[tool(description = "Get one Composer change set with its current revision")]
    async fn get_change_set(
        &self,
        Parameters(input): Parameters<ChangeSetId>,
    ) -> Result<Json<Versioned<ChangeSetView>>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.change_set(&input.change_set_id)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Apply a nonempty ordered local patch atomically. This persists once and never submits Jira."
    )]
    async fn apply_change_set_patch(
        &self,
        Parameters(input): Parameters<PatchChangeSet>,
    ) -> Result<Json<ChangeSetPatchResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.apply_change_set_patch(
                &input.change_set_id,
                input.expected_revision,
                input.operations,
            )
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Refresh Jira baselines for an open change set. This persists once, returns the refreshed canonical change set, and does not submit Jira changes."
    )]
    async fn refresh_change_set(
        &self,
        Parameters(input): Parameters<RefreshChangeSet>,
    ) -> Result<Json<ChangeSetPatchResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.refresh_change_set(&input.change_set_id, input.expected_revision)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Recover a durable abandoned submission attempt. Claimed attempts clear without contacting Jira. Marked draft creates require every local draft ID and its confirmed Jira key; Finery fetches those keys before reconciliation and never retries ambiguous creates."
    )]
    async fn recover_submission_attempt(
        &self,
        Parameters(input): Parameters<RecoverSubmissionAttempt>,
    ) -> Result<Json<ChangeSetPatchResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.recover_submission_attempt(
                &input.change_set_id,
                input.expected_revision,
                input.recovered_creates,
            )
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Submit explicit selected tickets to Jira. Durable create-attempt markers are stored before Jira receives new-ticket creates, then results are reconciled and conditionally persisted."
    )]
    async fn submit_change_set(
        &self,
        Parameters(input): Parameters<SubmitChangeSet>,
    ) -> Result<Json<SubmitChangeSetResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.submit_change_set(
                &input.change_set_id,
                input.expected_revision,
                input.selected_ticket_ids,
            )
        })
        .await
        .map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(MCP_INSTRUCTIONS.into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub(crate) async fn run_stdio(service: AppService) -> Result<(), Box<dyn std::error::Error>> {
    McpServer::new(service)
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

pub(crate) async fn run_http(
    service: AppService,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    run_http_inner(service, addr, None).await
}

pub(crate) async fn run_http_with_startup(
    service: AppService,
    addr: SocketAddr,
    startup: std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_http_inner(service, addr, Some(startup)).await
}

async fn run_http_inner(
    service: AppService,
    addr: SocketAddr,
    startup: Option<std::sync::mpsc::Sender<Result<(), String>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !addr.ip().is_loopback() {
        if let Some(startup) = startup {
            let _ = startup.send(Err("MCP HTTP bind must be loopback".into()));
        }
        return Err("MCP HTTP bind must be loopback".into());
    }

    let mcp: StreamableHttpService<McpServer> = StreamableHttpService::new(
        move || Ok(McpServer::new(service.clone())),
        Default::default(),
        StreamableHttpServerConfig {
            stateful_mode: false,
            sse_keep_alive: None,
            ..Default::default()
        },
    );
    let router = axum::Router::new().nest_service("/mcp", mcp);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            if let Some(startup) = startup {
                let _ = startup.send(Err(error.to_string()));
            }
            return Err(Box::new(error));
        }
    };
    if let Some(startup) = startup {
        let _ = startup.send(Ok(()));
    }
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
