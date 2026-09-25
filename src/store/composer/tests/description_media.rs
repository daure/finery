use super::*;
use crate::store::composer::{demo_jira_tickets, jira_adf};

pub(crate) fn image_ticket() -> Ticket {
    let mut ticket = demo_jira_tickets().remove(0);
    let adf = json!({"type":"doc", "version":1, "content":[
        {"type":"paragraph", "content":[{"type":"text", "text":"Before"}]},
        {"type":"mediaSingle", "attrs":{"layout":"center"}, "content":[
            {"type":"media", "attrs":{"type":"file", "id":"media-uuid", "collection":"jira", "alt":"screenshot.png"}}
        ]},
        {"type":"paragraph", "content":[{"type":"text", "text":"After"}]}
    ]});
    ticket.description = adf_to_markdown(&adf);
    ticket.description_safe_to_overwrite = jira_adf::adf_is_safe_to_overwrite(&adf);
    ticket.description_overwrite_warning = jira_adf::adf_overwrite_warning(&adf);
    ticket.jira_metadata.as_mut().unwrap().description_adf = Some(adf);
    ticket.attachments = vec![TicketAttachment {
        id: "42".into(),
        filename: "screenshot.png".into(),
        created: String::new(),
        size: 1,
        mime_type: Some("image/png".into()),
        content_url: Some("https://jira.example/attachment/42".into()),
        change: AttachmentChangeKind::Synced,
        local_data: None,
    }];
    ticket
}

#[test]
fn image_references_are_display_only_and_preserve_adf_and_overwrite_guards() {
    let ticket = image_ticket();
    let original = ticket.clone();
    let display = DescriptionPresentation::for_ticket(Some(&ticket));
    assert_eq!(display.text, "Before\n\nImage: `screenshot.png`\n\nAfter");
    assert_eq!(display.attachment_at(8), Some("42"));
    assert_eq!(display.attachment_at(0), None);
    assert_eq!(ticket, original);
    assert!(!ticket.description_safe_to_overwrite);
    assert_eq!(
        ticket.description_overwrite_warning.as_deref(),
        Some("Jira media")
    );
    let restored: Ticket = serde_json::from_value(serde_json::to_value(&ticket).unwrap()).unwrap();
    assert_eq!(restored, ticket);
    let mut edited = ticket;
    edited.description.push_str("\nEdited");
    let display = DescriptionPresentation::for_ticket(Some(&edited));
    assert_eq!(display.text, edited.description);
    assert!(display.references.is_empty());
}

#[test]
fn media_groups_preserve_unresolved_items_and_ambiguous_names_are_not_openable() {
    let mut ticket = image_ticket();
    let mut adf = ticket
        .jira_metadata
        .as_ref()
        .unwrap()
        .description_adf
        .clone()
        .unwrap();
    adf["content"][1]["type"] = json!("mediaGroup");
    adf["content"][1]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"media", "attrs":{"type":"file", "alt":"missing.mov"}}));
    ticket.description = adf_to_markdown(&adf);
    ticket.jira_metadata.as_mut().unwrap().description_adf = Some(adf);
    let display = DescriptionPresentation::for_ticket(Some(&ticket));
    assert_eq!(
        display.text,
        "Before\n\nImage: `screenshot.png`  \nUnsupported media\n\nAfter"
    );
    assert_eq!(display.references.len(), 1);
    let mut duplicate = ticket.attachments[0].clone();
    duplicate.id = "43".into();
    ticket.attachments.push(duplicate);
    let ambiguous = DescriptionPresentation::for_ticket(Some(&ticket));
    assert!(ambiguous.references.is_empty());
    assert!(ambiguous.text.contains("Unsupported media"));
}

#[test]
fn image_filenames_receive_markdown_code_color_and_handle_embedded_backticks() {
    let mut ticket = image_ticket();
    let display = DescriptionPresentation::for_ticket(Some(&ticket));
    let highlighted = tuicore::SyntaxHighlighter::new(display.text, tuicore::Language::Markdown)
        .highlighted_text();
    let spans = highlighted
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .collect::<Vec<_>>();
    let prose = spans
        .iter()
        .find(|span| span.content.contains("Image:"))
        .unwrap();
    let filename = spans
        .iter()
        .find(|span| span.content.contains("screenshot.png"))
        .unwrap();
    assert_ne!(filename.style.fg, prose.style.fg);

    ticket.attachments[0].filename = "`screen``shot`.png".into();
    ticket
        .jira_metadata
        .as_mut()
        .unwrap()
        .description_adf
        .as_mut()
        .unwrap()["content"][1]["content"][0]["attrs"]["id"] = json!("42");
    let original = ticket.clone();
    let display = DescriptionPresentation::for_ticket(Some(&ticket));
    assert_eq!(
        display.text,
        "Before\n\nImage: ``` `screen``shot`.png ```\n\nAfter"
    );
    let reference = &display.references[0];
    assert_eq!(
        display.attachment_at(reference.characters.end - 1),
        Some("42")
    );
    assert_eq!(display.attachment_at(reference.characters.end), None);
    assert_eq!(ticket, original);
}
