use serde::{Deserialize, Serialize};

const IMAGE_PREFIX: &str = "<!-- finery:jira-image ";
const IMAGE_SUFFIX: &str = " -->";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TicketImage {
    pub url: String,
    pub alt: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TicketContentBlock {
    Markdown(String),
    Image(TicketImage),
}

pub(crate) fn ticket_image_marker(image: &TicketImage) -> String {
    let image = serde_json::to_string(image).expect("ticket image metadata should serialize");
    format!("{IMAGE_PREFIX}{image}{IMAGE_SUFFIX}")
}

pub(crate) fn ticket_content_blocks(source: &str) -> Vec<TicketContentBlock> {
    let mut blocks = Vec::new();
    let mut markdown = Vec::new();
    for line in source.lines() {
        let image = line
            .trim()
            .strip_prefix(IMAGE_PREFIX)
            .and_then(|value| value.strip_suffix(IMAGE_SUFFIX))
            .and_then(|value| serde_json::from_str::<TicketImage>(value).ok());
        if let Some(image) = image {
            push_markdown_block(&mut blocks, &mut markdown);
            blocks.push(TicketContentBlock::Image(image));
        } else {
            markdown.push(line);
        }
    }
    push_markdown_block(&mut blocks, &mut markdown);
    blocks
}

pub(crate) fn ticket_content_markdown(source: &str) -> String {
    ticket_content_blocks(source)
        .into_iter()
        .map(|block| match block {
            TicketContentBlock::Markdown(markdown) => markdown,
            TicketContentBlock::Image(image) => format!(
                "![{}](<{}>)",
                image
                    .alt
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]"),
                image.url.replace('>', "%3E")
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn push_markdown_block(blocks: &mut Vec<TicketContentBlock>, lines: &mut Vec<&str>) {
    let markdown = lines.join("\n");
    let markdown = markdown.trim_matches('\n');
    if !markdown.trim().is_empty() {
        blocks.push(TicketContentBlock::Markdown(markdown.to_owned()));
    }
    lines.clear();
}
