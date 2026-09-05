use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rmcp::{
    ErrorData, Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ResourceContents, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{StreamableHttpServerConfig, StreamableHttpService, stdio},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    service::{
        AppService,
        composer_attachments::{
            ATTACHMENT_BYTES_LIMIT, AttachmentRequest, AttachmentSource, TicketSnapshotView,
        },
        composer_service::{
            ChangeSetCatalogView, ChangeSetMutationResponse, ChangeSetPatchOperation,
            ChangeSetPatchResponse, ChangeSetView, ComposerService, DeleteChangeSetResponse,
            RecoveredCreate, ServiceError, SubmitChangeSetResponse, Versioned,
        },
    },
    store::work_items::{BacklogSnapshot, RankPlan, RunwayCapacitySource, WorkItem, rank_plan},
};

const MCP_INSTRUCTIONS: &str = "Read Composer change sets and Jira backlog order. Ticket snapshots include attachment metadata; call get_change_set_attachments with the current revision to inspect one or more files. Add attachments from a local file path or an http(s) URL through apply_change_set_patch; Finery stores the loaded bytes locally, and those changes remain local until submission. Remove attachments through apply_change_set_patch. Mermaid diagrams are local-only records with a title, type, and markup; add, update, or remove them through ordered apply_change_set_patch operations. Supported Mermaid types: flowchart, sequence, class, state, ER, gantt, mindmap, architecture, C4, kanban, git graph, pie, packet, timeline, journey, requirement, sankey, radar, info, treemap, block, quadrant chart, XY chart, tree view, ishikawa, event modeling, Venn. Finery validates markup and uploads only a generated PNG when the ticket is submitted. Add, update, or remove ticket web links through ordered apply_change_set_patch operations; new link IDs must start with local-. One atomic patch may add a draft and its links while changing links on other tickets. Before mutating an existing change set, reread it and send its current revision as expected_revision. Call get_change_set_guidance before reading or writing a Composer ticket description. Call lookup_jira_user before creating an @mention unless the account ID is already known. Use lookup_jira_label to discover existing Jira labels and lookup_jira_fix_version to discover project fix versions. create_change_set, apply_change_set_patch, and delete_change_set persist local edits only; delete_change_set never deletes Jira tickets. Before Jira reordering, state the exact move and get explicit user confirmation; reread the workspace afterward. submit_change_set is the only Composer Jira submission path, requires explicit ticket IDs, and may set updateTitle to rename the change set as part of submission. Submitted tickets cannot be changed or resubmitted. If a description cannot round-trip safely, state the identified formatting risk and get explicit user confirmation before setting accept_unsafe_description_overwrite. Recover marked draft creates with a confirmed Jira key, or explicitly mark them absent only after verifying they did not reach Jira; never retry ambiguous creates.";
// Agent-facing description contract. Any Jira ADF conversion, supported tag, validation, or
// overwrite-safety change MUST update this guidance so MCP agents receive accurate instructions.
const CHANGE_SET_GUIDANCE: &str = "Composer descriptions are Markdown transformed to and from Jira ADF. This is reference documentation, not a recommendation to add rich formatting. The syntax preserves Jira-specific source content when it is present.\n\nAvailable syntax: normal Markdown, nested ordered/bullet lists, basic tables (one header row; one paragraph per cell), underline (++text++), text colour ({color:#RRGGBB}text{/color}), emoji (:short_name:), UTC dates (@date(YYYY-MM-DD)), statuses (@status(\"Text\", color)), mentions (@mention(\"@Name\", \"ACCOUNT_ID\")), and cards (@card(https://example.com)). Jira background highlights are rejected because they break Jira's native editor. Status colors: green, blue, red, yellow, neutral, purple.\n\nJira blocks:\n- Panel: {{jira:panel {\"panelType\":\"info\"}}} … {{/jira:panel}}\n- Task list: {{jira:task-list}} with - [ ] or - [x] items … {{/jira:task-list}}\n- Decision list: {{jira:decision-list}} with plain - item entries … {{/jira:decision-list}}\n\nEscape literal {{ as \\{\\{ and any literal canonical inline opening with a leading backslash. Emoji syntax needs both colons, and content in inline code spans is literal. Old {{jira:mention ... /}} and {{jira:inline-card ... /}} forms are rejected. A patch with malformed Jira syntax is rejected atomically; malformed syntax also blocks a selected submission before Jira writes. For an unsafe existing Jira description, describe the formatting risk and get explicit approval before accept_unsafe_description_overwrite.";
const WORKSPACE_UNPLANNED_TICKET_LIMIT: usize = 50;
const WORKSPACE_VELOCITY_SPRINT_LIMIT: usize = 10;
const ATTACHMENT_BATCH_BYTES_LIMIT: usize = 20 * 1024 * 1024;

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
struct ListChangeSets {
    #[serde(default)]
    include_closed: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetChangeSetAttachments {
    change_set_id: String,
    expected_revision: i64,
    #[schemars(length(min = 1))]
    attachments: Vec<AttachmentRequest>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CreateChangeSet {
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookupJiraUser {
    search: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookupJiraLabel {
    search: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LookupJiraFixVersion {
    project_key: String,
    search: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JiraMentionUserView {
    account_id: String,
    display_name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JiraMentionUsersView {
    users: Vec<JiraMentionUserView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JiraLabelsView {
    labels: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JiraFixVersionView {
    id: String,
    name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct JiraFixVersionsView {
    fix_versions: Vec<JiraFixVersionView>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteChangeSet {
    change_set_id: String,
    expected_revision: i64,
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
    #[serde(rename = "updateTitle", alias = "update_title")]
    update_title: Option<String>,
    #[serde(default)]
    accept_unsafe_description_overwrite: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RecoverSubmissionAttempt {
    change_set_id: String,
    expected_revision: i64,
    #[serde(default)]
    recovered_creates: Vec<RecoveredCreate>,
    #[serde(default)]
    abandoned_creates: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ChangeSetGuidanceView {
    guidance: &'static str,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PlaceJiraItems {
    #[schemars(length(min = 1, max = 50))]
    issue_keys: Vec<String>,
    destination: JiraDestination,
    position: JiraPosition,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JiraDestination {
    Backlog,
    Sprint { sprint_id: u64 },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JiraPosition {
    Top,
    Bottom,
    Before { issue_key: String },
    After { issue_key: String },
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SwapJiraItems {
    first_issue_key: String,
    second_issue_key: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceView {
    backlog: WorkspaceBacklogView,
    change_sets: Vec<WorkspaceChangeSetView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceBacklogView {
    board_name: String,
    warnings: Vec<String>,
    story_points_configured: bool,
    velocity: Option<WorkspaceVelocityView>,
    capacity_guidance: Option<WorkspaceCapacityGuidanceView>,
    sprints: Vec<WorkspaceSprintView>,
    unplanned: WorkspaceUnplannedView,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceUnplannedView {
    total_count: usize,
    tickets: Vec<WorkspaceWorkItemView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceVelocityView {
    average_completed_points: Option<f64>,
    sprints: Vec<WorkspaceVelocitySprintView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceVelocitySprintView {
    name: String,
    completed_points: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    goal: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceCapacityGuidanceView {
    capacity: f64,
    source: String,
    first_unplanned_capacity_band: Vec<WorkspaceCapacityBandTicketView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceCapacityBandTicketView {
    key: String,
    effective_points: f64,
    points_source: WorkspacePointsSource,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WorkspacePointsSource {
    StoryPoints,
    FixedAssumption,
    AverageAssumption,
    UnestimatedBug,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceSprintView {
    id: u64,
    name: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_date: Option<String>,
    tickets: Vec<WorkspaceWorkItemView>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WorkspaceWorkItemView {
    key: String,
    title: String,
    kind: String,
    status: String,
    done: bool,
    priority: String,
    assignee: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<WorkspaceParentTicketView>,
    #[serde(skip_serializing_if = "is_false")]
    has_children: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_points: Option<f64>,
}

fn is_false(value: &bool) -> bool {
    !value
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
    has_attachments: bool,
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
        ServiceError::AlreadyExists { resource, id } => {
            format!("already_exists: {resource} '{id}' already exists")
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

fn attachment_contents(
    service: &AppService,
    input: GetChangeSetAttachments,
) -> Result<Vec<Content>, String> {
    if input.attachments.is_empty() {
        return Err("invalid_operation: request at least one attachment".into());
    }
    let resolved = service
        .composer_service()
        .attachment_sources(
            &input.change_set_id,
            input.expected_revision,
            &input.attachments,
        )
        .map_err(mcp_error)?;
    let mut content = Vec::with_capacity(resolved.len() * 2);
    let mut total_bytes = 0usize;
    for resolution in resolved {
        let attachment = match resolution {
            Ok(attachment) => attachment,
            Err(error) => {
                content.push(Content::text(format!("Attachment error: {error}")));
                continue;
            }
        };
        let remaining = ATTACHMENT_BATCH_BYTES_LIMIT.saturating_sub(total_bytes);
        let max_bytes = ATTACHMENT_BYTES_LIMIT.min(remaining);
        let bytes = match attachment.source {
            AttachmentSource::Local(bytes) if bytes.len() <= max_bytes => Ok(bytes),
            AttachmentSource::Local(bytes) => Err(format!(
                "attachment exceeds the {max_bytes}-byte remaining response limit ({} bytes)",
                bytes.len()
            )),
            AttachmentSource::Jira(url) if max_bytes > 0 => {
                service.download_jira_attachment_limited(&url, max_bytes)
            }
            AttachmentSource::Jira(_) => {
                Err("attachment exceeds the remaining batch response limit".into())
            }
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                content.push(Content::text(attachment_result_label(
                    &attachment.request,
                    &attachment.filename,
                    attachment.mime_type.as_deref(),
                    Some(&error),
                )));
                continue;
            }
        };
        total_bytes += bytes.len();
        content.push(Content::text(attachment_result_label(
            &attachment.request,
            &attachment.filename,
            attachment.mime_type.as_deref(),
            None,
        )));
        let uri = format!("finery://attachment/{}", attachment.request.attachment_id);
        if let Some(mime_type) = crate::service::image_mime_type(&bytes) {
            content.push(Content::image(BASE64_STANDARD.encode(bytes), mime_type));
        } else {
            let mime_type = attachment
                .mime_type
                .unwrap_or_else(|| "application/octet-stream".into());
            content.push(attachment_resource(bytes, uri, mime_type));
        }
    }
    Ok(content)
}

fn attachment_result_label(
    request: &AttachmentRequest,
    filename: &str,
    mime_type: Option<&str>,
    error: Option<&str>,
) -> String {
    let snapshot = match request.snapshot {
        TicketSnapshotView::Original => "original",
        TicketSnapshotView::Updated => "updated",
    };
    let mut metadata = serde_json::json!({
        "ticket_id": request.ticket_id,
        "snapshot": snapshot,
        "attachment_id": request.attachment_id,
        "filename": filename,
        "mime_type": mime_type,
    });
    if let Some(error) = error {
        metadata["error"] = error.into();
        format!("Attachment error: {metadata}")
    } else {
        format!("Attachment: {metadata}")
    }
}

fn attachment_resource(bytes: Vec<u8>, uri: String, mime_type: String) -> Content {
    let base_mime_type = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let textual = base_mime_type.starts_with("text/")
        || matches!(
            base_mime_type.as_str(),
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/javascript"
                | "application/x-yaml"
                | "application/yaml"
        );
    if textual && let Ok(text) = String::from_utf8(bytes.clone()) {
        return Content::resource(ResourceContents::TextResourceContents {
            uri,
            mime_type: Some(mime_type),
            text,
            meta: None,
        });
    }
    Content::resource(ResourceContents::BlobResourceContents {
        uri,
        mime_type: Some(mime_type),
        blob: BASE64_STANDARD.encode(bytes),
        meta: None,
    })
}

async fn run_workspace(service: AppService) -> Result<WorkspaceView, String> {
    tokio::task::spawn_blocking(move || {
        let composer = service.composer_service();
        let backlog = service
            .with_jira_reorder(|service| service.jira_backlog_while_reorder_locked())
            .map_err(|error| format!("jira_backlog_failed: {error}"))?;
        let change_sets = composer.change_set_catalog(true).map_err(mcp_error)?;
        workspace_view(backlog, change_sets)
    })
    .await
    .map_err(|_| "internal_error: background task failed".to_string())?
}

async fn run_jira_reorder(
    service: AppService,
    operation: impl FnOnce(&AppService) -> Result<BacklogSnapshot, String> + Send + 'static,
) -> Result<WorkspaceView, String> {
    let reorder_service = service.clone();
    let backlog = tokio::task::spawn_blocking(move || {
        reorder_service.with_jira_reorder(|service| operation(service))
    })
    .await
    .map_err(|_| "internal_error: background task failed".to_string())??;
    tokio::task::spawn_blocking(move || workspace_with_backlog(service, backlog))
        .await
        .map_err(|_| "internal_error: background task failed".to_string())?
}

fn workspace_with_backlog(
    service: AppService,
    backlog: BacklogSnapshot,
) -> Result<WorkspaceView, String> {
    let composer = service.composer_service();
    let change_sets = composer.change_set_catalog(true).map_err(mcp_error)?;
    workspace_view(backlog, change_sets)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JiraSection {
    Backlog,
    Sprint(u64),
}

impl JiraDestination {
    fn section(&self) -> JiraSection {
        match self {
            Self::Backlog => JiraSection::Backlog,
            Self::Sprint { sprint_id } => JiraSection::Sprint(*sprint_id),
        }
    }
}

fn section_order(snapshot: &BacklogSnapshot, section: JiraSection) -> Result<Vec<String>, String> {
    match section {
        JiraSection::Backlog => Ok(snapshot.top_level_backlog_keys.clone()),
        JiraSection::Sprint(sprint_id) => snapshot
            .sprints
            .iter()
            .find(|sprint| sprint.id == sprint_id)
            .map(|sprint| {
                sprint
                    .work_items
                    .iter()
                    .map(|item| item.key.clone())
                    .collect()
            })
            .ok_or_else(|| format!("Sprint {sprint_id} is not active or future on this board")),
    }
}

fn issue_sections(snapshot: &BacklogSnapshot) -> HashMap<String, JiraSection> {
    let mut sections = snapshot
        .top_level_backlog_keys
        .iter()
        .map(|key| (key.clone(), JiraSection::Backlog))
        .collect::<HashMap<_, _>>();
    for sprint in &snapshot.sprints {
        sections.extend(
            sprint
                .work_items
                .iter()
                .map(|item| (item.key.clone(), JiraSection::Sprint(sprint.id))),
        );
    }
    sections
}

fn validate_issue_keys(
    issue_keys: &[String],
    sections: &HashMap<String, JiraSection>,
) -> Result<(), String> {
    if issue_keys.is_empty() || issue_keys.len() > 50 {
        return Err("issue_keys must contain between 1 and 50 issues".into());
    }
    let unique = issue_keys.iter().collect::<HashSet<_>>();
    if unique.len() != issue_keys.len() {
        return Err("issue_keys must not contain duplicates".into());
    }
    if let Some(issue_key) = issue_keys
        .iter()
        .find(|issue_key| !sections.contains_key(*issue_key))
    {
        return Err(format!(
            "Issue '{issue_key}' is not in this board's active, future, or backlog sections"
        ));
    }
    Ok(())
}

fn final_order(
    destination_order: &[String],
    issue_keys: &[String],
    position: &JiraPosition,
) -> Result<Vec<String>, String> {
    let moved = issue_keys.iter().collect::<HashSet<_>>();
    let mut order = destination_order
        .iter()
        .filter(|key| !moved.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    let index = match position {
        JiraPosition::Top => 0,
        JiraPosition::Bottom => order.len(),
        JiraPosition::Before { issue_key } => order
            .iter()
            .position(|key| key == issue_key)
            .ok_or_else(|| {
                format!("Anchor '{issue_key}' is not in the destination or is being moved")
            })?,
        JiraPosition::After { issue_key } => order
            .iter()
            .position(|key| key == issue_key)
            .map(|index| index + 1)
            .ok_or_else(|| {
                format!("Anchor '{issue_key}' is not in the destination or is being moved")
            })?,
    };
    order.splice(index..index, issue_keys.iter().cloned());
    Ok(order)
}

fn placement_rank_plan(
    issue_keys: &[String],
    final_order: &[String],
) -> Result<Option<RankPlan>, String> {
    if issue_keys.len() == final_order.len() {
        return Ok((issue_keys.len() > 1).then(|| RankPlan {
            issues: issue_keys[1..].to_vec(),
            rank_before_issue: None,
            rank_after_issue: Some(issue_keys[0].clone()),
        }));
    }
    rank_plan(issue_keys.to_vec(), final_order)
}

fn place_jira_items(
    service: &AppService,
    input: PlaceJiraItems,
) -> Result<BacklogSnapshot, String> {
    let destination = input.destination.section();
    let snapshot = service.jira_backlog_while_reorder_locked()?;
    let sections = issue_sections(&snapshot);
    validate_issue_keys(&input.issue_keys, &sections)?;
    let destination_order = section_order(&snapshot, destination)?;
    final_order(&destination_order, &input.issue_keys, &input.position)?;
    let moving_sections = input
        .issue_keys
        .iter()
        .filter(|issue_key| sections.get(*issue_key) != Some(&destination))
        .cloned()
        .collect::<Vec<_>>();
    if !moving_sections.is_empty() {
        match destination {
            JiraSection::Backlog => {
                service.jira_move_to_backlog_while_reorder_locked(&moving_sections)?
            }
            JiraSection::Sprint(sprint_id) => {
                service.jira_move_to_sprint_while_reorder_locked(sprint_id, &moving_sections)?
            }
        }
    }
    let snapshot = service.jira_backlog_while_reorder_locked()?;
    let destination_order = section_order(&snapshot, destination)?;
    let final_order = final_order(&destination_order, &input.issue_keys, &input.position)?;
    if let Some(plan) = placement_rank_plan(&input.issue_keys, &final_order)? {
        service.jira_rank_while_reorder_locked(&plan)?;
    }
    service.jira_backlog_while_reorder_locked()
}

fn swap_jira_items(service: &AppService, input: SwapJiraItems) -> Result<BacklogSnapshot, String> {
    if input.first_issue_key == input.second_issue_key {
        return Err("Swap requires two distinct issue keys".into());
    }
    let snapshot = service.jira_backlog_while_reorder_locked()?;
    let sections = issue_sections(&snapshot);
    validate_issue_keys(
        &[
            input.first_issue_key.clone(),
            input.second_issue_key.clone(),
        ],
        &sections,
    )?;
    let first_section = sections[&input.first_issue_key];
    if sections[&input.second_issue_key] != first_section {
        return Err("Swap requires both issues to be in the same backlog or sprint section".into());
    }
    let order = section_order(&snapshot, first_section)?;
    let first_index = order
        .iter()
        .position(|key| key == &input.first_issue_key)
        .ok_or_else(|| {
            format!(
                "Issue '{}' is not in the rankable order for its section",
                input.first_issue_key
            )
        })?;
    let second_index = order
        .iter()
        .position(|key| key == &input.second_issue_key)
        .ok_or_else(|| {
            format!(
                "Issue '{}' is not in the rankable order for its section",
                input.second_issue_key
            )
        })?;
    let (earlier, later, earlier_index, later_index) = if first_index < second_index {
        (
            &input.first_issue_key,
            &input.second_issue_key,
            first_index,
            second_index,
        )
    } else {
        (
            &input.second_issue_key,
            &input.first_issue_key,
            second_index,
            first_index,
        )
    };
    service.jira_rank_while_reorder_locked(&RankPlan {
        issues: vec![later.clone()],
        rank_before_issue: Some(earlier.clone()),
        rank_after_issue: None,
    })?;
    if later_index > earlier_index + 1 {
        service.jira_rank_while_reorder_locked(&RankPlan {
            issues: vec![earlier.clone()],
            rank_before_issue: None,
            rank_after_issue: Some(order[later_index - 1].clone()),
        })?;
    }
    service.jira_backlog_while_reorder_locked()
}

fn workspace_view(
    backlog: BacklogSnapshot,
    change_sets: Versioned<ChangeSetCatalogView>,
) -> Result<WorkspaceView, String> {
    let unplanned_total_count = backlog.work_items.len();
    let capacity_guidance = backlog
        .runway
        .map(|runway| workspace_capacity_guidance_view(runway, &backlog.work_items))
        .transpose()?;
    Ok(WorkspaceView {
        backlog: WorkspaceBacklogView {
            board_name: backlog.board_name,
            warnings: backlog.warnings,
            story_points_configured: backlog.story_points_configured,
            velocity: backlog.velocity.map(workspace_velocity_view),
            capacity_guidance,
            sprints: backlog
                .sprints
                .into_iter()
                .map(|sprint| WorkspaceSprintView {
                    id: sprint.id,
                    name: sprint.name,
                    state: sprint.state,
                    goal: sprint.goal,
                    start_date: sprint.start_date,
                    end_date: sprint.end_date,
                    tickets: sprint.work_items.into_iter().map(Into::into).collect(),
                })
                .collect(),
            unplanned: WorkspaceUnplannedView {
                total_count: unplanned_total_count,
                tickets: backlog
                    .work_items
                    .into_iter()
                    .take(WORKSPACE_UNPLANNED_TICKET_LIMIT)
                    .map(Into::into)
                    .collect(),
            },
        },
        change_sets: change_sets
            .value
            .change_sets
            .into_iter()
            .filter(|change_set| !change_set.value.closed)
            .map(workspace_change_set_view)
            .collect(),
    })
}

fn workspace_velocity_view(
    velocity: crate::store::work_items::VelocityReport,
) -> WorkspaceVelocityView {
    WorkspaceVelocityView {
        average_completed_points: velocity.dynamic_capacity,
        sprints: velocity
            .sprints
            .into_iter()
            .take(
                velocity
                    .configured_sprints
                    .min(WORKSPACE_VELOCITY_SPRINT_LIMIT),
            )
            .map(|sprint| WorkspaceVelocitySprintView {
                name: sprint.name,
                completed_points: sprint.completed,
                goal: sprint.goal,
            })
            .collect(),
    }
}

fn workspace_capacity_guidance_view(
    runway: crate::store::work_items::BacklogRunway,
    work_items: &[WorkItem],
) -> Result<WorkspaceCapacityGuidanceView, String> {
    let mut work_items_by_key = HashMap::with_capacity(work_items.len());
    for ticket in work_items {
        if work_items_by_key
            .insert(ticket.key.as_str(), ticket)
            .is_some()
        {
            return Err(format!("duplicate backlog ticket key '{}'", ticket.key));
        }
    }
    let mut runway_keys = HashSet::with_capacity(runway.tickets.len());
    for runway_ticket in &runway.tickets {
        if !runway_keys.insert(runway_ticket.key.as_str()) {
            return Err(format!(
                "duplicate runway ticket key '{}'",
                runway_ticket.key
            ));
        }
        if !work_items_by_key.contains_key(runway_ticket.key.as_str()) {
            return Err(format!(
                "runway ticket '{}' is not in the loaded backlog",
                runway_ticket.key
            ));
        }
    }
    if let Some(key) = work_items_by_key
        .keys()
        .find(|key| !runway_keys.contains(*key))
    {
        return Err(format!("loaded backlog ticket '{key}' has no runway entry"));
    }
    let first_unplanned_capacity_band = runway
        .tickets
        .iter()
        .filter(|runway_ticket| runway_ticket.virtual_sprint == 1)
        .take(WORKSPACE_UNPLANNED_TICKET_LIMIT)
        .map(|runway_ticket| {
            let ticket = work_items_by_key
                .get(runway_ticket.key.as_str())
                .expect("runway tickets are validated against the loaded backlog");
            Ok(WorkspaceCapacityBandTicketView {
                key: runway_ticket.key.clone(),
                effective_points: runway_ticket.effective_points,
                points_source: workspace_points_source(ticket, runway_ticket),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(WorkspaceCapacityGuidanceView {
        capacity: runway.capacity,
        source: match runway.source {
            RunwayCapacitySource::JiraVelocity => "jira_velocity",
            RunwayCapacitySource::Fixed => "fixed",
            RunwayCapacitySource::FixedFallback => "fixed_fallback",
        }
        .into(),
        first_unplanned_capacity_band,
    })
}

fn workspace_points_source(
    ticket: &WorkItem,
    runway_ticket: &crate::store::work_items::RunwayTicket,
) -> WorkspacePointsSource {
    if ticket
        .story_points
        .is_some_and(|points| points.is_finite() && points >= 0.0)
    {
        WorkspacePointsSource::StoryPoints
    } else if ticket.kind.eq_ignore_ascii_case("bug") {
        WorkspacePointsSource::UnestimatedBug
    } else if runway_ticket.assumed_from_average {
        WorkspacePointsSource::AverageAssumption
    } else {
        WorkspacePointsSource::FixedAssumption
    }
}

fn workspace_change_set_view(
    change_set: Versioned<crate::service::composer_service::ChangeSetCatalogChangeSetView>,
) -> WorkspaceChangeSetView {
    WorkspaceChangeSetView {
        id: change_set.value.id,
        name: change_set.value.name,
        revision: change_set.revision,
        has_attachments: change_set.value.has_attachments,
    }
}

impl From<WorkItem> for WorkspaceWorkItemView {
    fn from(ticket: WorkItem) -> Self {
        Self {
            key: ticket.key,
            title: ticket.title,
            kind: ticket.kind,
            status: ticket.status,
            done: ticket.done,
            priority: ticket.priority,
            assignee: ticket.assignee,
            parent: workspace_parent(ticket.parent_key, ticket.parent_title),
            has_children: ticket.has_children,
            story_points: ticket.story_points,
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

#[tool_router]
impl McpServer {
    #[tool(
        description = "Look up Jira users for @mention Markdown. Returns display names and account IDs; use the account ID in @mention(\"@Name\", \"ACCOUNT_ID\")."
    )]
    async fn lookup_jira_user(
        &self,
        Parameters(input): Parameters<LookupJiraUser>,
    ) -> Result<Json<JiraMentionUsersView>, String> {
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.search_jira_users(&input.search))
            .await
            .map_err(|_| "internal_error: Jira user lookup failed".to_string())?
            .map(|users| {
                Json(JiraMentionUsersView {
                    users: users
                        .into_iter()
                        .map(|user| JiraMentionUserView {
                            account_id: user.account_id,
                            display_name: user.display_name,
                        })
                        .collect(),
                })
            })
    }

    #[tool(
        description = "Look up existing Jira labels by text. Use labels with change-set draft or update operations."
    )]
    async fn lookup_jira_label(
        &self,
        Parameters(input): Parameters<LookupJiraLabel>,
    ) -> Result<Json<JiraLabelsView>, String> {
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.search_jira_labels(&input.search))
            .await
            .map_err(|_| "internal_error: Jira label lookup failed".to_string())?
            .map(|labels| Json(JiraLabelsView { labels }))
    }

    #[tool(
        description = "Look up non-archived Jira fix versions in a project by text. Use version names with change-set draft or update operations."
    )]
    async fn lookup_jira_fix_version(
        &self,
        Parameters(input): Parameters<LookupJiraFixVersion>,
    ) -> Result<Json<JiraFixVersionsView>, String> {
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || {
            service.search_jira_fix_versions(&input.project_key, &input.search)
        })
        .await
        .map_err(|_| "internal_error: Jira fix-version lookup failed".to_string())?
        .map(|fix_versions| {
            Json(JiraFixVersionsView {
                fix_versions: fix_versions
                    .into_iter()
                    .map(|version| JiraFixVersionView {
                        id: version.id,
                        name: version.name,
                    })
                    .collect(),
            })
        })
    }

    #[tool(
        description = "Required before reading or writing a Composer ticket description. Get the concise canonical Markdown, Jira-tag, validation, and unsafe-overwrite rules for change sets."
    )]
    async fn get_change_set_guidance(&self) -> Json<ChangeSetGuidanceView> {
        Json(ChangeSetGuidanceView {
            guidance: CHANGE_SET_GUIDANCE,
        })
    }

    #[tool(
        description = "Get active/future sprints, top 50 rank-ordered unplanned tickets with total_count, up to 10 recent velocity sprints, empty-sprint capacity guidance, and open change-set summaries without changing Composer state. Tickets are compact and omit descriptions."
    )]
    async fn get_workspace(&self) -> Result<Json<WorkspaceView>, String> {
        run_workspace(self.service.clone()).await.map(Json)
    }

    #[tool(
        description = "Move one ordered block of up to 50 Jira issues to the backlog or an active/future sprint, then place it at the top, bottom, before, or after a destination issue. This writes directly to Jira. State the exact proposed move and get explicit user confirmation before calling it. A failure after a Jira write may leave a partial move; call get_workspace to inspect Jira before retrying."
    )]
    async fn place_jira_items(
        &self,
        Parameters(input): Parameters<PlaceJiraItems>,
    ) -> Result<Json<WorkspaceView>, String> {
        run_jira_reorder(self.service.clone(), move |service| {
            place_jira_items(service, input)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Swap two Jira issues in the same backlog or sprint section. This writes directly to Jira and can require two rank writes. State the exact proposed swap and get explicit user confirmation before calling it. A failure after a Jira write may leave a partial swap; call get_workspace to inspect Jira before retrying."
    )]
    async fn swap_jira_items(
        &self,
        Parameters(input): Parameters<SwapJiraItems>,
    ) -> Result<Json<WorkspaceView>, String> {
        run_jira_reorder(self.service.clone(), move |service| {
            swap_jira_items(service, input)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "List open Composer change sets with revisioned canonical data. Set include_closed to true to include closed change sets."
    )]
    async fn list_change_sets(
        &self,
        Parameters(input): Parameters<ListChangeSets>,
    ) -> Result<Json<Versioned<ChangeSetCatalogView>>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.change_set_catalog(input.include_closed)
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
        description = "Return one or more Composer attachments, limited to 5 MiB per file and 20 MiB per response. Images use native MCP image blocks, UTF-8 text uses text resources, and other files use blob resources. Read the change set first, then identify each file by ticket ID, original or updated snapshot, and attachment ID, and send the current revision as expected_revision. Local staged files use their stored bytes; Jira files are downloaded with Finery's configured authentication. Individual failures do not suppress successful attachments."
    )]
    async fn get_change_set_attachments(
        &self,
        Parameters(input): Parameters<GetChangeSetAttachments>,
    ) -> Result<CallToolResult, ErrorData> {
        let service = self.service.clone();
        let result = tokio::task::spawn_blocking(move || attachment_contents(&service, input))
            .await
            .map_err(|_| ErrorData::internal_error("attachment task failed", None))?;
        Ok(match result {
            Ok(content) => CallToolResult::success(content),
            Err(error) => CallToolResult::error(vec![Content::text(error)]),
        })
    }

    #[tool(
        description = "Create an empty open Composer change set locally. Finery assigns the next sequential CS-N ID; this never writes to Jira."
    )]
    async fn create_change_set(
        &self,
        Parameters(input): Parameters<CreateChangeSet>,
    ) -> Result<Json<ChangeSetMutationResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.create_change_set(input.name)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Delete one Composer change set and all of its local ticket snapshots. This never deletes Jira tickets. Reread the change set and send its current revision as expected_revision. Change sets with an unresolved submission attempt cannot be deleted."
    )]
    async fn delete_change_set(
        &self,
        Parameters(input): Parameters<DeleteChangeSet>,
    ) -> Result<Json<DeleteChangeSetResponse>, String> {
        run_composer(self.service.composer_service(), move |service| {
            service.delete_change_set(&input.change_set_id, input.expected_revision)
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Apply a nonempty ordered local patch atomically. Operations may add a draft and then add its web links, while also changing links on other tickets. New web-link IDs must start with local-. Attachment additions load up to 5 MiB from a local file path or an http(s) URL and store the bytes in the change set; optional filenames and MIME types default from the source where possible. Image signatures and filename extensions are validated. Removing a local addition drops it, while removing a synced attachment stages its Jira deletion. This persists once and never submits Jira."
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
        description = "Recover a durable abandoned submission attempt. Claimed attempts clear without contacting Jira. For every marked draft create, provide either its confirmed Jira key or its local ID in abandoned_creates after verifying it did not reach Jira. Finery fetches confirmed keys and never retries ambiguous creates."
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
                input.abandoned_creates,
            )
        })
        .await
        .map(Json)
    }

    #[tool(
        description = "Submit explicit selected tickets to Jira. Durable create-attempt markers are stored before Jira receives new-ticket creates, then results are reconciled and conditionally persisted. Set accept_unsafe_description_overwrite only after the user explicitly accepts that unsupported Jira formatting may be replaced."
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
                input.update_title,
                input.accept_unsafe_description_overwrite,
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
