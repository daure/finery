use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::store::composer::summary::ChangeSetSummary;

pub(super) fn summary_line(summary: &ChangeSetSummary) -> Line<'static> {
    let theme = tuicore::theme();
    let mut spans = Vec::new();
    for (icon, count, color, show_zero) in [
        ("", summary.created, theme.success_fg(), false),
        ("", summary.edited, theme.warning_fg(), false),
        ("", summary.deleted, theme.error_fg(), false),
        ("", summary.reference, theme.muted_fg(), false),
        ("", summary.diagrams, theme.accent_fg(), false),
        ("", summary.uploads, theme.text_fg(), false),
        ("󰖟", summary.web_links, theme.accent_fg(), false),
        ("", summary.external_links, theme.accent_fg(), false),
        ("", summary.submitted, theme.muted_fg(), true),
    ] {
        if count == 0 && !show_zero {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(theme.subtle_fg())));
        }
        spans.push(Span::styled(
            format!("{icon} {count}"),
            Style::default().fg(color),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tests/change_set_summary.rs"]
mod tests;
