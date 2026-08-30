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
    pub story_points_estimated: bool,
    pub story_points_from_average: bool,
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

pub(crate) fn work_item_text(row: &WorkItemRow, number_query: Option<&str>) -> Text<'static> {
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
    let mut metadata = ticket_key_spans(&row.key, number_query, Style::default().fg(text_color));
    metadata.extend([
        Span::styled(" • ", Style::default().fg(text_color)),
        crate::components::avatar::bubble_span(&row.assignee),
    ]);
    if row.show_story_points {
        metadata.extend([
            Span::styled(" • ", Style::default().fg(text_color)),
            Span::styled(
                story_points_label(row),
                Style::default().fg(
                    if row.story_points.is_some() && !row.story_points_estimated {
                        text_color
                    } else {
                        theme.muted_fg()
                    },
                ),
            ),
        ]);
    }
    if !row.status.is_empty() {
        metadata.extend([
            Span::styled(" • ", Style::default().fg(text_color)),
            Span::styled(row.status.clone(), Style::default().fg(text_color)),
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

pub(crate) fn work_item_title_with_key_line(
    row: &WorkItemRow,
    number_query: Option<&str>,
) -> Line<'static> {
    let theme = tuicore::theme();
    let (kind_icon, mut kind_color) = ticket_icon(row.kind);
    let (priority_icon, mut priority_color) = priority_icon(&row.priority);
    let text_color = if row.submitted {
        kind_color = theme.muted_fg();
        priority_color = theme.muted_fg();
        theme.muted_fg()
    } else {
        theme.text_fg()
    };
    let mut spans = vec![
        Span::styled(format!("{kind_icon} "), Style::default().fg(kind_color)),
        Span::styled(
            format!("{priority_icon} "),
            Style::default().fg(priority_color),
        ),
    ];
    let key_style = Style::default()
        .fg(theme.muted_fg())
        .add_modifier(Modifier::BOLD);
    spans.extend(ticket_key_spans(&row.key, number_query, key_style));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        row.title.clone(),
        Style::default().fg(text_color).add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

fn ticket_key_spans(key: &str, number_query: Option<&str>, style: Style) -> Vec<Span<'static>> {
    let Some(number) = number_query.filter(|number| {
        crate::components::ticket_number_jump::ticket_number_matches(key, number)
    }) else {
        return vec![Span::styled(key.to_owned(), style)];
    };
    let (_, suffix) = key.rsplit_once('-').expect("matching ticket key has a number suffix");
    let prefix_len = key.len().saturating_sub(suffix.len());
    let underline_end = prefix_len.saturating_add(number.len());
    vec![
        Span::styled(key[..prefix_len].to_owned(), style),
        Span::styled(
            key[prefix_len..underline_end].to_owned(),
            style.add_modifier(Modifier::UNDERLINED),
        ),
        Span::styled(key[underline_end..].to_owned(), style),
    ]
}

pub(crate) fn work_item_title_prefix_width(row: &WorkItemRow) -> usize {
    let (kind_icon, _) = ticket_icon(row.kind);
    let (priority_icon, _) = priority_icon(&row.priority);
    Line::from(format!("{kind_icon} {priority_icon} {} ", row.key)).width()
}

pub(crate) fn story_points_label(row: &WorkItemRow) -> String {
    let Some(points) = row.story_points else {
        return "-".into();
    };
    let prefix = (row.story_points_estimated && row.story_points_from_average)
        .then_some("~")
        .unwrap_or("");
    format!("{prefix}{points}")
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
