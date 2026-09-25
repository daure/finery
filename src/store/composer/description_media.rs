use std::ops::Range;

use serde_json::{Value, json};

use super::{AttachmentChangeKind, Ticket, TicketAttachment, jira_adf::adf_to_markdown};

#[derive(Default, PartialEq, Eq)]
pub(crate) struct DescriptionPresentation {
    pub text: String,
    pub references: Vec<ImageReference>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct ImageReference {
    pub characters: Range<usize>,
    pub attachment_id: String,
}

impl DescriptionPresentation {
    pub(crate) fn for_ticket(ticket: Option<&Ticket>) -> Self {
        let Some(ticket) = ticket else {
            return Self::default();
        };
        let plain = || Self {
            text: ticket.description.clone(),
            references: Vec::new(),
        };
        if ticket.description_safe_to_overwrite {
            return plain();
        }
        let Some(adf) = ticket
            .jira_metadata
            .as_ref()
            .and_then(|metadata| metadata.description_adf.as_ref())
        else {
            return plain();
        };
        // Media identities belong to the exact source snapshot, never to edited placeholder text.
        if adf_to_markdown(adf) != ticket.description {
            return plain();
        }
        let mut prefix = "FINERYIMAGEREFERENCE".to_owned();
        let serialized = adf.to_string();
        while serialized.contains(&prefix)
            || ticket
                .attachments
                .iter()
                .any(|attachment| attachment.filename.contains(&prefix))
        {
            prefix.push('X');
        }
        let mut replacements = Vec::new();
        let mut display_adf = adf.clone();
        replace_media(
            &mut display_adf,
            &ticket.attachments,
            &prefix,
            &mut replacements,
        );
        let mut text = adf_to_markdown(&display_adf);
        let mut references = Vec::new();
        for (token, attachment) in replacements {
            let Some(start) = text.find(&token) else {
                continue;
            };
            let start_char = text[..start].chars().count();
            let filename = attachment
                .filename
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>();
            let fence = "`".repeat(
                filename
                    .split(|c| c != '`')
                    .map(str::len)
                    .max()
                    .unwrap_or(0)
                    + 1,
            );
            let padding = if filename.starts_with('`') || filename.ends_with('`') {
                " "
            } else {
                ""
            };
            let label = format!("Image: {fence}{padding}{filename}{padding}{fence}");
            text.replace_range(start..start + token.len(), &label);
            references.push(ImageReference {
                characters: start_char..start_char + label.chars().count(),
                attachment_id: attachment.id.clone(),
            });
        }
        Self { text, references }
    }

    pub(crate) fn attachment_at(&self, character: usize) -> Option<&str> {
        self.references
            .iter()
            .find(|reference| reference.characters.contains(&character))
            .map(|reference| reference.attachment_id.as_str())
    }

    pub(crate) fn character_at(&self, line: usize, column: usize) -> Option<usize> {
        let mut offset = 0;
        for (index, text) in self.text.split('\n').enumerate() {
            if index + 1 == line {
                return (column < text.chars().count()).then_some(offset + column);
            }
            offset += text.chars().count() + 1;
        }
        None
    }
}

fn replace_media<'a>(
    node: &mut Value,
    attachments: &'a [TicketAttachment],
    prefix: &str,
    replacements: &mut Vec<(String, &'a TicketAttachment)>,
) {
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("mediaSingle" | "mediaGroup")
    ) {
        let mut separated = Vec::new();
        for media in node
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !separated.is_empty() {
                separated.push(json!({"type": "hardBreak"}));
            }
            let text = if let Some(attachment) = resolve_image(media, attachments) {
                let token = format!("{prefix}{}END", replacements.len());
                replacements.push((token.clone(), attachment));
                token
            } else {
                "Unsupported media".into()
            };
            separated.push(json!({"type": "text", "text": text}));
        }
        if separated.is_empty() {
            separated.push(json!({"type":"text", "text":"Unsupported media"}));
        }
        *node = json!({"type": "paragraph", "content": separated});
    } else if let Some(content) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in content {
            replace_media(child, attachments, prefix, replacements);
        }
    }
}

fn resolve_image<'a>(
    media: &Value,
    attachments: &'a [TicketAttachment],
) -> Option<&'a TicketAttachment> {
    if media.get("type")?.as_str()? != "media" || media.pointer("/attrs/type")?.as_str()? != "file"
    {
        return None;
    }
    let candidates = attachments
        .iter()
        .filter(|attachment| {
            attachment.change != AttachmentChangeKind::Deleted
                && !attachment.id.is_empty()
                && attachment
                    .mime_type
                    .as_deref()
                    .is_some_and(|mime| mime.starts_with("image/"))
                && (attachment.local_data.is_some()
                    || attachment
                        .content_url
                        .as_deref()
                        .is_some_and(|url| !url.is_empty()))
        })
        .collect::<Vec<_>>();
    if let Some(id) = media.pointer("/attrs/id").and_then(Value::as_str)
        && let Some(attachment) = candidates.iter().find(|attachment| attachment.id == id)
    {
        return Some(attachment);
    }
    let alt = media.pointer("/attrs/alt")?.as_str()?;
    let mut matches = candidates
        .into_iter()
        .filter(|attachment| attachment.filename == alt);
    let attachment = matches.next()?;
    matches.next().is_none().then_some(attachment)
}

#[cfg(test)]
#[path = "tests/description_media.rs"]
pub(crate) mod tests;
