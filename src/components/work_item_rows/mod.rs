use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
};

#[derive(Clone)]
pub(crate) struct WorkItemRow {
    pub id: String,
    pub key: String,
    pub title: String,
    pub kind: WorkItemKind,
    pub priority: String,
    pub status: String,
    pub assignee: String,
    pub story_points: Option<f64>,
    pub show_story_points: bool,
    pub change_badge: Option<ChangeBadge>,
    pub submitted: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum WorkItemKind {
    Epic,
    Story,
    Task,
    Bug,
    Subtask,
    Other,
}

#[derive(Clone, Copy)]
pub(crate) enum ChangeBadge {
    Added,
    Modified,
    Deleted,
    Synced,
}

pub(crate) fn work_item_text(row: &WorkItemRow) -> Text<'static> {
    let theme = tuicore::theme();
    let (kind_icon, mut kind_color) = ticket_icon(row.kind);
    let (priority_icon, mut priority_color) = priority_icon(&row.priority);
    let (badge, mut badge_color) = row
        .change_badge
        .map(change_badge)
        .unwrap_or(("", theme.text_fg()));
    let text_color = if row.submitted {
        kind_color = theme.muted_fg();
        priority_color = theme.muted_fg();
        badge_color = theme.muted_fg();
        theme.muted_fg()
    } else {
        theme.text_fg()
    };
    let mut metadata = vec![Span::styled(
        row.key.clone(),
        Style::default().fg(text_color),
    )];
    if !row.status.is_empty() {
        metadata.extend([
            Span::styled(" • ", Style::default().fg(text_color)),
            Span::styled(row.status.clone(), Style::default().fg(text_color)),
        ]);
    }
    metadata.extend([
        Span::styled(" • ", Style::default().fg(text_color)),
        crate::components::avatar::bubble_span(&row.assignee),
    ]);
    if row.show_story_points {
        metadata.extend([
            Span::styled(" • ", Style::default().fg(text_color)),
            Span::styled(
                row.story_points
                    .map(|points| points.to_string())
                    .unwrap_or_else(|| "-".into()),
                Style::default().fg(if row.story_points.is_some() {
                    text_color
                } else {
                    theme.muted_fg()
                }),
            ),
        ]);
    }
    if row.submitted {
        metadata.push(Span::styled(
            " · submitted",
            Style::default().fg(theme.muted_fg()),
        ));
    }
    let first_line = vec![
        Span::styled(format!("{kind_icon} "), Style::default().fg(kind_color)),
        Span::styled(
            format!("{priority_icon} "),
            Style::default().fg(priority_color),
        ),
        Span::styled(row.title.clone(), Style::default().fg(text_color)),
    ];
    let mut second_line = Vec::new();
    if row.change_badge.is_some() {
        second_line.extend([
            Span::styled(
                format!("{badge} "),
                Style::default()
                    .fg(badge_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("• ", Style::default().fg(text_color)),
        ]);
    }
    second_line.append(&mut metadata);
    Text::from(vec![Line::from(first_line), Line::from(second_line)])
}

fn ticket_icon(kind: WorkItemKind) -> (&'static str, ratatui::style::Color) {
    let theme = tuicore::theme();
    match kind {
        WorkItemKind::Epic => ("", theme.warning_fg()),
        WorkItemKind::Story => ("", theme.accent_fg()),
        WorkItemKind::Task => ("", theme.success_fg()),
        WorkItemKind::Subtask => ("", theme.accent_fg()),
        WorkItemKind::Bug => ("", theme.error_fg()),
        WorkItemKind::Other => ("?", theme.muted_fg()),
    }
}

fn priority_icon(priority: &str) -> (&'static str, ratatui::style::Color) {
    let theme = tuicore::theme();
    match priority {
        "Highest" => ("󰄿", theme.error_fg()),
        "High" => ("󰅃", theme.warning_fg()),
        "Low" => ("󰅀", theme.success_fg()),
        "Lowest" => ("󰄼", theme.muted_fg()),
        _ => ("󰇼", theme.accent_fg()),
    }
}

fn change_badge(change: ChangeBadge) -> (&'static str, ratatui::style::Color) {
    let theme = tuicore::theme();
    match change {
        ChangeBadge::Added => ("A", theme.success_fg()),
        ChangeBadge::Modified => ("M", theme.warning_fg()),
        ChangeBadge::Deleted => ("D", theme.error_fg()),
        ChangeBadge::Synced => ("S", theme.text_fg()),
    }
}
