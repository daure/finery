use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tuicore::{Chip, ChipColorRole, MatchSpan, SearchMode, search_match};

pub(crate) const TICKET_MENU_WIDTH: u16 = 84;
pub(crate) const TICKET_MENU_MAX_HEIGHT_PERCENT: u16 = 60;
const LABEL_CHIP_TEXT_MAX_WIDTH: usize = 16;
const LABEL_TEXT_MAX_WIDTH: usize = 10;

pub(crate) fn ticket_menu_max_height(viewport_height: u16) -> u16 {
    viewport_height.saturating_mul(TICKET_MENU_MAX_HEIGHT_PERCENT) / 100
}

#[derive(Clone)]
pub(crate) struct WorkItemRow {
    pub id: String,
    pub key: String,
    pub title: String,
    pub kind: WorkItemKind,
    pub priority: String,
    pub status: String,
    pub done: bool,
    pub assignee: String,
    pub labels: Vec<String>,
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

pub(crate) struct TicketRowDetails<'a> {
    pub subtask_progress: Option<(usize, usize)>,
    pub fix_versions: &'a [String],
    pub epic_name: Option<&'a str>,
    pub annotation: Option<&'a str>,
}

pub(crate) fn ticket_summary_text(
    row: &WorkItemRow,
    number_query: Option<&str>,
    text_query: Option<&str>,
    details: TicketRowDetails<'_>,
) -> Text<'static> {
    let theme = tuicore::theme();
    let text_style = Style::default().fg(if row.submitted {
        theme.muted_fg()
    } else {
        theme.text_fg()
    });
    let muted_style = Style::default().fg(theme.muted_fg());
    let mut metadata = Vec::new();
    if let Some(change) = row.change_badge {
        let (badge, color) = change_badge(change);
        metadata.push(Span::styled(
            badge,
            Style::default()
                .fg(if row.submitted {
                    theme.muted_fg()
                } else {
                    color
                })
                .add_modifier(Modifier::BOLD),
        ));
    }
    if row.show_story_points {
        let style = (row.story_points.is_some() && !row.story_points_estimated)
            .then_some(text_style)
            .unwrap_or(muted_style);
        append_metadata(&mut metadata, Span::styled(story_points_label(row), style));
    }
    append_metadata(
        &mut metadata,
        crate::components::avatar::bubble_span(&row.assignee),
    );
    if let Some((completed, total)) = details.subtask_progress {
        append_metadata(
            &mut metadata,
            Span::styled(format!("{completed}/{total} "), text_style),
        );
    }
    append_labels_chip(&mut metadata, &row.labels);
    if !row.status.is_empty() {
        append_metadata(&mut metadata, Span::styled(row.status.clone(), text_style));
    }
    append_release_text(&mut metadata, details.fix_versions);
    if let Some(epic_name) = details.epic_name {
        append_metadata(
            &mut metadata,
            Span::styled(
                epic_name.to_owned(),
                Style::default()
                    .fg(theme.warning_fg())
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }
    if row.submitted {
        metadata.push(Span::styled(
            " · submitted",
            Style::default().fg(theme.muted_fg()),
        ));
    }
    if let Some(annotation) = details.annotation {
        append_metadata(
            &mut metadata,
            Span::styled(annotation.to_owned(), Style::default().fg(theme.muted_fg())),
        );
    }
    Text::from(vec![
        work_item_title_with_key_line_with_match(row, number_query, text_query),
        Line::from(metadata),
    ])
}

pub(crate) fn work_item_title_with_key_line_with_match(
    row: &WorkItemRow,
    number_query: Option<&str>,
    text_query: Option<&str>,
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
    let key_style = ticket_key_style(
        row,
        Style::default()
            .fg(theme.muted_fg())
            .add_modifier(Modifier::BOLD),
    );
    if text_query.is_some() {
        spans.extend(search_match_spans(&row.key, text_query, key_style));
    } else {
        spans.extend(ticket_key_spans(&row.key, number_query, key_style));
    }
    spans.push(Span::raw(" "));
    spans.extend(search_match_spans(
        &row.title,
        text_query,
        Style::default().fg(text_color).add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

fn ticket_key_style(row: &WorkItemRow, style: Style) -> Style {
    row.done
        .then(|| style.add_modifier(Modifier::CROSSED_OUT))
        .unwrap_or(style)
}

fn search_match_spans(text: &str, query: Option<&str>, style: Style) -> Vec<Span<'static>> {
    let mut matches = query
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|term| search_match(term, text, SearchMode::Contains))
        .flat_map(|matched| matched.spans)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return vec![Span::styled(text.to_owned(), style)];
    }
    matches.sort_by_key(|matched| matched.start);
    let matches = merge_match_spans(matches);
    let mut spans = Vec::with_capacity(matches.len().saturating_mul(2).saturating_add(1));
    let mut cursor = 0;
    for matched in matches {
        if cursor < matched.start {
            spans.push(Span::styled(text[cursor..matched.start].to_owned(), style));
        }
        spans.push(Span::styled(
            text[matched.start..matched.end].to_owned(),
            style.add_modifier(Modifier::UNDERLINED),
        ));
        cursor = matched.end;
    }
    if cursor < text.len() {
        spans.push(Span::styled(text[cursor..].to_owned(), style));
    }
    spans
}

fn merge_match_spans(matches: Vec<MatchSpan>) -> Vec<MatchSpan> {
    let mut merged = Vec::<MatchSpan>::new();
    for matched in matches {
        if let Some(previous) = merged.last_mut()
            && matched.start <= previous.end
        {
            previous.end = previous.end.max(matched.end);
        } else {
            merged.push(matched);
        }
    }
    merged
}

fn ticket_key_spans(key: &str, number_query: Option<&str>, style: Style) -> Vec<Span<'static>> {
    let Some(number) = number_query
        .filter(|number| crate::components::ticket_number_jump::ticket_number_matches(key, number))
    else {
        return vec![Span::styled(key.to_owned(), style)];
    };
    let (_, suffix) = key
        .rsplit_once('-')
        .expect("matching ticket key has a number suffix");
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
    work_item_title_prefix_width_for(row.kind, &row.priority, &row.key)
}

pub(crate) fn work_item_title_prefix_width_for(
    kind: WorkItemKind,
    priority: &str,
    key: &str,
) -> usize {
    let (kind_icon, _) = ticket_icon(kind);
    let (priority_icon, _) = priority_icon(priority);
    Line::from(format!("{kind_icon} {priority_icon} {key} ")).width()
}

pub(crate) fn story_points_label(row: &WorkItemRow) -> String {
    let Some(points) = row.story_points else {
        return "-".into();
    };
    let prefix = (row.story_points_estimated && row.story_points_from_average)
        .then_some("~")
        .unwrap_or("");
    if row.story_points_estimated && row.story_points_from_average {
        return format!("{prefix}{points:.1}");
    }
    format!("{prefix}{points}")
}

pub(crate) fn append_release_text(metadata: &mut Vec<Span<'static>>, fix_versions: &[String]) {
    if fix_versions.is_empty() {
        return;
    }
    if !metadata.is_empty() {
        metadata.push(Span::raw(" • "));
    }
    metadata.push(Span::styled(
        fix_versions.join(", "),
        Style::default()
            .fg(tuicore::theme().accent_fg())
            .add_modifier(Modifier::BOLD),
    ));
}

fn append_labels_chip(metadata: &mut Vec<Span<'static>>, labels: &[String]) {
    if labels.is_empty() {
        return;
    }
    if !metadata.is_empty() {
        metadata.push(Span::raw(" • "));
    }
    metadata.extend(
        Chip::new(compact_labels_text(labels))
            .color_role(ChipColorRole::Highlight)
            .line()
            .spans,
    );
}

fn compact_labels_text(labels: &[String]) -> String {
    let mut displayed = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let label = truncate_label(label);
        let candidate = displayed
            .iter()
            .chain(std::iter::once(&label))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("|");
        let hidden = labels.len().saturating_sub(index + 1);
        let overflow = (hidden > 0)
            .then(|| format!("|+{hidden}"))
            .unwrap_or_default();
        if Line::from(format!("{candidate}{overflow}")).width() > LABEL_CHIP_TEXT_MAX_WIDTH {
            break;
        }
        displayed.push(label);
    }
    let hidden = labels.len().saturating_sub(displayed.len());
    format!(
        "{}{}",
        displayed.join("|"),
        (hidden > 0)
            .then(|| format!("|+{hidden}"))
            .unwrap_or_default()
    )
}

fn truncate_label(label: &str) -> String {
    if Line::from(label).width() <= LABEL_TEXT_MAX_WIDTH {
        return label.into();
    }
    let mut truncated = String::new();
    for character in label.chars() {
        let candidate = format!("{truncated}{character}…");
        if Line::from(candidate).width() > LABEL_TEXT_MAX_WIDTH {
            break;
        }
        truncated.push(character);
    }
    format!("{truncated}…")
}

fn append_metadata(metadata: &mut Vec<Span<'static>>, value: Span<'static>) {
    if !metadata.is_empty() {
        metadata.push(Span::raw(" • "));
    }
    metadata.push(value);
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

#[cfg(test)]
mod tests;
