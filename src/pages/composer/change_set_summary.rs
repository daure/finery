use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    components::work_item_rows::{ChangeBadge, change_badge},
    store::composer::summary::ChangeSetSummary,
};

pub(super) fn summary_line(summary: &ChangeSetSummary) -> Line<'static> {
    let theme = tuicore::theme();
    let total = summary.reference + summary.created + summary.edited + summary.deleted;
    let submitted = summary.submitted;
    let icon = if submitted == 0 {
        "󰅙"
    } else if submitted == total {
        ""
    } else {
        ""
    };
    let mut spans = vec![Span::styled(
        format!("{icon} {submitted}/{total}"),
        Style::default().fg(theme.muted_fg()),
    )];
    let changes = [
        (ChangeBadge::Synced, summary.reference),
        (ChangeBadge::Added, summary.created),
        (ChangeBadge::Modified, summary.edited),
        (ChangeBadge::Deleted, summary.deleted),
    ]
    .map(|(change, count)| {
        let (label, color) = change_badge(change);
        (label, count, color)
    });
    for (label, count, color) in changes.into_iter().chain([
        ("", summary.diagrams, theme.text_fg()),
        ("", summary.uploads, theme.text_fg()),
        ("󰖟", summary.web_links, theme.text_fg()),
        ("", summary.external_links, theme.text_fg()),
    ]) {
        if count == 0 {
            continue;
        }
        spans.push(Span::styled(" · ", Style::default().fg(theme.subtle_fg())));
        spans.push(Span::styled(
            format!("{label} {count}"),
            Style::default().fg(color),
        ));
    }
    for (label, count) in [
        ("Cancelled", summary.cancelled),
        ("Concluded", summary.concluded),
    ] {
        if count > 0 {
            spans.push(Span::styled(
                format!(" · {label}"),
                Style::default().fg(theme.muted_fg()),
            ));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
#[path = "tests/change_set_summary.rs"]
mod tests;
