use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::composer::{AttachmentChangeKind, ChangeSet, TicketAttachment};

pub(crate) const ATTACHMENT_BYTES_LIMIT: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentView {
    pub id: String,
    pub filename: String,
    pub created: String,
    pub size: u64,
    pub mime_type: Option<String>,
    pub change: AttachmentChangeKindView,
    pub kind: AttachmentKindView,
    pub content_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentChangeKindView {
    Synced,
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKindView {
    Image,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketSnapshotView {
    Original,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentRequest {
    pub ticket_id: String,
    pub snapshot: TicketSnapshotView,
    pub attachment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachmentSource {
    Local(Vec<u8>),
    Jira(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedAttachment {
    pub request: AttachmentRequest,
    pub filename: String,
    pub mime_type: Option<String>,
    pub source: AttachmentSource,
}

pub(crate) fn resolve_attachments(
    change_set: &ChangeSet,
    requests: &[AttachmentRequest],
) -> Vec<Result<ResolvedAttachment, String>> {
    requests
        .iter()
        .map(|request| resolve_attachment(change_set, request))
        .collect()
}

impl From<TicketAttachment> for AttachmentView {
    fn from(value: TicketAttachment) -> Self {
        let filename_image_mime_type = image_mime_type_for_filename(&value.filename);
        let mime_type = value
            .mime_type
            .clone()
            .or_else(|| filename_image_mime_type.map(str::to_owned));
        let kind = if mime_type
            .as_deref()
            .is_some_and(|mime_type| mime_type.starts_with("image/"))
            || filename_image_mime_type.is_some()
        {
            AttachmentKindView::Image
        } else {
            AttachmentKindView::Other
        };
        let content_available = value.local_data.is_some() || value.content_url.is_some();
        Self {
            id: value.id,
            filename: value.filename,
            created: value.created,
            size: value.size,
            mime_type,
            change: value.change.into(),
            kind,
            content_available,
        }
    }
}

impl From<AttachmentChangeKind> for AttachmentChangeKindView {
    fn from(value: AttachmentChangeKind) -> Self {
        match value {
            AttachmentChangeKind::Synced => Self::Synced,
            AttachmentChangeKind::Added => Self::Added,
            AttachmentChangeKind::Modified => Self::Modified,
            AttachmentChangeKind::Deleted => Self::Deleted,
        }
    }
}

fn resolve_attachment(
    change_set: &ChangeSet,
    request: &AttachmentRequest,
) -> Result<ResolvedAttachment, String> {
    let change = change_set
        .tickets
        .iter()
        .find(|change| change.id == request.ticket_id)
        .ok_or_else(|| format!("ticket not found: {}", request.ticket_id))?;
    let ticket = match request.snapshot {
        TicketSnapshotView::Original => change.original.as_ref(),
        TicketSnapshotView::Updated => change.updated.as_ref(),
    }
    .ok_or_else(|| {
        format!(
            "{} snapshot is unavailable for ticket {}",
            snapshot_name(request.snapshot),
            request.ticket_id
        )
    })?;
    let attachment = ticket
        .attachments
        .iter()
        .find(|attachment| attachment.id == request.attachment_id)
        .ok_or_else(|| {
            format!(
                "attachment not found in {} snapshot for ticket {}: {}",
                snapshot_name(request.snapshot),
                request.ticket_id,
                request.attachment_id
            )
        })?;
    let source = attachment
        .local_data
        .clone()
        .map(AttachmentSource::Local)
        .or_else(|| attachment.content_url.clone().map(AttachmentSource::Jira))
        .ok_or_else(|| format!("attachment content is unavailable: {}", attachment.filename))?;
    Ok(ResolvedAttachment {
        request: request.clone(),
        filename: attachment.filename.clone(),
        mime_type: attachment
            .mime_type
            .clone()
            .or_else(|| image_mime_type_for_filename(&attachment.filename).map(str::to_owned)),
        source,
    })
}

fn snapshot_name(snapshot: TicketSnapshotView) -> &'static str {
    match snapshot {
        TicketSnapshotView::Original => "original",
        TicketSnapshotView::Updated => "updated",
    }
}

pub(crate) fn image_mime_type_for_filename(filename: &str) -> Option<&'static str> {
    match filename.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "apng" | "png" => Some("image/png"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "gif" => Some("image/gif"),
        "ico" => Some("image/vnd.microsoft.icon"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}
