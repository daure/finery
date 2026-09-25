use chrono::NaiveDate;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
};

use super::{
    WorkItemGroup, capacity_indicator_style, estimation_coverage, points_label,
    root_item_counts_refs,
};
use crate::store::work_items::{
    BacklogSnapshot, SprintCapacityState,
    release::{self, ReleaseStatus},
};

#[cfg(test)]
#[path = "../tests/release_row.rs"]
mod tests;

pub(super) fn title(
    snapshot: &BacklogSnapshot,
    group: &WorkItemGroup<'_>,
    today: NaiveDate,
    hides_tickets: bool,
) -> Text<'static> {
    let theme = tuicore::theme();
    let muted = Style::default().fg(theme.muted_fg());
    let text = Style::default().fg(theme.text_fg());
    let separator = || Span::styled(" • ", muted);
    let (completed, total) = root_item_counts_refs(&group.items);
    let forecast = release::forecast(
        snapshot,
        &group.items,
        &group.label,
        total > 0 && completed == total,
        today,
    );
    let (icon, status, status_style) = match forecast.status {
        ReleaseStatus::Underplanned => (
            "󰸂",
            "Underplanned",
            capacity_indicator_style(SprintCapacityState::UnderCommitted),
        ),
        ReleaseStatus::Planned => (
            "󱩿",
            "Planned",
            capacity_indicator_style(SprintCapacityState::OnTarget),
        ),
        ReleaseStatus::Overplanned => (
            "󰸁",
            "Overplanned",
            capacity_indicator_style(SprintCapacityState::OverCommitted),
        ),
        ReleaseStatus::InProgress => ("", "In progress", muted),
        ReleaseStatus::OnTarget => (
            "󱩿",
            "On target",
            capacity_indicator_style(SprintCapacityState::OnTarget),
        ),
        ReleaseStatus::AtRisk => (
            "󰸁",
            "At risk",
            capacity_indicator_style(SprintCapacityState::OverCommitted),
        ),
        ReleaseStatus::Delivered => ("", "Delivered", Style::default().fg(theme.success_fg())),
        ReleaseStatus::MissedTarget => (
            "󰸁",
            "Missed target",
            capacity_indicator_style(SprintCapacityState::OverCommitted),
        ),
        ReleaseStatus::Unavailable => ("?", "Forecast unavailable", muted),
    };
    let mut heading = vec![
        Span::styled(" ", Style::default().fg(theme.accent_fg())),
        Span::styled(group.label.clone(), text.add_modifier(Modifier::BOLD)),
        separator(),
    ];
    if let (Some(start), Some(end)) = (forecast.start, forecast.end) {
        heading.extend([
            Span::styled(
                format!("{} – {}", start.format("%-d %b"), end.format("%-d %b")),
                text,
            ),
            separator(),
        ]);
        if forecast.status == ReleaseStatus::Delivered {
            heading.push(Span::styled(format!("{icon} {status}"), status_style));
        } else if today > end {
            if let Some((needed, _)) = forecast.sprints {
                heading.push(Span::styled(
                    format!("󰑮 {} unfinished ", points_label(needed)),
                    text,
                ));
            }
            heading.push(Span::styled(icon, status_style));
            heading.push(Span::styled(
                format!(" {}d overdue", (today - end).num_days()),
                text,
            ));
        } else if let Some((needed, available)) = forecast.sprints {
            let (work_label, time_label) = if today < start {
                ("planned", "available")
            } else {
                ("todo", "left")
            };
            heading.push(Span::styled(
                format!("󰑮 {} {work_label} ", points_label(needed)),
                text,
            ));
            heading.push(Span::styled(icon, status_style));
            heading.push(Span::styled(
                format!(" {} {time_label}", points_label(available)),
                text,
            ));
        } else {
            heading.push(Span::styled(format!("{icon} {status}"), status_style));
        }
    } else {
        let warning = match (forecast.start, forecast.end) {
            (None, Some(_)) => " Start date missing",
            (Some(_), None) => " End date missing",
            _ => " Start and end date missing",
        };
        heading.push(Span::styled(
            warning,
            Style::default().fg(theme.warning_fg()),
        ));
    }
    if hides_tickets {
        heading.push(Span::styled(" (some tickets hidden by filters)", muted));
    }
    let (coverage, coverage_style) = estimation_coverage(&group.items);
    let remaining_points = if forecast.points_known {
        points_label((forecast.total_points - forecast.completed_points).max(0.0))
    } else {
        "?".into()
    };
    let assumption = if forecast.assumed { "~" } else { "" };
    Text::from(vec![
        Line::from(heading),
        Line::from(vec![
            Span::styled(format!("{coverage} est"), coverage_style),
            separator(),
            Span::styled(format!("{} open", total - completed), muted),
            separator(),
            Span::styled(
                format!("{assumption}{remaining_points} pts remaining"),
                text,
            ),
        ]),
    ])
}
