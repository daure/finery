use std::{cell::RefCell, rc::Rc};

use chrono::{DateTime, FixedOffset};
use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};
use tuicore::{LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, RenderCtx, TuiNode, theme};

use crate::store::composer::{ComposerState, JiraTicketMetadata};

pub(super) struct JiraMetadata {
    state: Rc<RefCell<ComposerState>>,
}

impl JiraMetadata {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>) -> Self {
        Self { state }
    }

    fn text(&self) -> String {
        let state = self.state.borrow();
        let ticket = state.selected_source().or_else(|| state.selected_changes());
        let Some(ticket) = ticket.filter(|ticket| !ticket.key.starts_with("NEW-")) else {
            return "Metadata is available after Jira creates this ticket.".into();
        };
        let Some(metadata) = ticket.jira_metadata.as_ref() else {
            return "Jira metadata is unavailable. Refresh this ticket to load it.".into();
        };
        metadata_text(metadata)
    }
}

fn metadata_text(metadata: &JiraTicketMetadata) -> String {
    [
        format!("Reporter     {}", value_or_unavailable(&metadata.reporter)),
        format!("Created      {}", format_timestamp(&metadata.created)),
        format!("Last updated {}", format_timestamp(&metadata.updated)),
    ]
    .join("\n")
}

fn value_or_unavailable(value: &str) -> &str {
    if value.trim().is_empty() {
        "Unavailable"
    } else {
        value
    }
}

fn format_timestamp(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::<FixedOffset>::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M %Z").to_string())
        .unwrap_or_else(|_| value_or_unavailable(value).into())
}

impl TuiNode for JiraMetadata {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(32, 3).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        frame.render_widget(
            Paragraph::new(self.text()).style(Style::default().fg(theme().text_fg())),
            area,
        );
    }
}
