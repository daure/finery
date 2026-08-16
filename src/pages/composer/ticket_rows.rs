use ratatui::{
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tuicore::{
    ActivationMode, CellContext, Column, DataView, SelectionGlyphs, SelectionMode, SelectionTrigger,
};

use crate::store::composer::{ChangeKind, ComposerState, TicketChange, TicketKind};

#[derive(Clone)]
pub(super) struct TicketRow {
    pub(super) id: String,
    key: String,
    title: String,
    kind: TicketKind,
    priority: String,
    status: String,
    pub(super) change: ChangeKind,
    pub(super) submitted: bool,
}

pub(super) fn ticket_data_view(state: &ComposerState) -> DataView<TicketRow, String> {
    let mut view = DataView::new(ticket_rows(state), |row: &TicketRow| row.id.clone())
        .headers(false)
        .columns(ticket_columns())
        .row_height(2)
        .activation_mode(ActivationMode::Manual)
        .selection_mode(SelectionMode::Multi)
        .selection_trigger(SelectionTrigger::OnActivate)
        .selection_glyphs(SelectionGlyphs::NERD_FONT)
        .selection_disabled_by(|row| row.submitted)
        .selection_disabled_glyph("󱋭");
    if let Some(selected) = state.selected_ticket.as_ref() {
        view.highlight_id(selected);
    }
    view
}

pub(super) fn ticket_rows(state: &ComposerState) -> Vec<TicketRow> {
    state
        .active_set()
        .into_iter()
        .flat_map(|set| &set.tickets)
        .filter_map(|change| ticket_row(state, change))
        .collect()
}

fn ticket_row(state: &ComposerState, change: &TicketChange) -> Option<TicketRow> {
    let ticket = state.ticket_for_change(change)?;
    Some(TicketRow {
        id: change.id.clone(),
        key: ticket.key.clone(),
        title: ticket.title.clone(),
        kind: ticket.kind,
        priority: ticket.priority.clone(),
        status: ticket.status.clone(),
        change: change.kind,
        submitted: change.is_submitted(),
    })
}

fn ticket_columns() -> Vec<Column<TicketRow, String>> {
    vec![Column::multiline(
        "ticket",
        "",
        Constraint::Percentage(100),
        |row: &TicketRow, _: &CellContext<String>| {
            let theme = tuicore::theme();
            let (kind_icon, mut kind_color) = ticket_icon(row.kind);
            let (priority_icon, mut priority_color) = priority_icon(&row.priority);
            let (badge, mut badge_color) = change_badge(row.change);
            let text_color = if row.submitted {
                kind_color = theme.muted_fg();
                priority_color = theme.muted_fg();
                badge_color = theme.muted_fg();
                theme.muted_fg()
            } else {
                theme.text_fg()
            };
            Text::from(vec![
                Line::from(vec![
                    Span::styled(format!("{kind_icon} "), Style::default().fg(kind_color)),
                    Span::styled(
                        format!("{priority_icon} "),
                        Style::default().fg(priority_color),
                    ),
                    Span::styled(row.title.clone(), Style::default().fg(text_color)),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("{badge} "),
                        Style::default()
                            .fg(badge_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("• ", Style::default().fg(text_color)),
                    Span::styled(row.key.clone(), Style::default().fg(text_color)),
                    Span::styled(" • ", Style::default().fg(text_color)),
                    Span::styled(row.status.clone(), Style::default().fg(text_color)),
                    Span::styled(
                        if row.submitted { " · submitted" } else { "" },
                        Style::default().fg(theme.muted_fg()),
                    ),
                ]),
            ])
        },
    )]
}

fn ticket_icon(kind: TicketKind) -> (&'static str, ratatui::style::Color) {
    let theme = tuicore::theme();
    match kind {
        TicketKind::Epic => ("", theme.warning_fg()),
        TicketKind::Story => ("", theme.accent_fg()),
        TicketKind::Task => ("", theme.success_fg()),
        TicketKind::Subtask => ("", theme.accent_fg()),
        TicketKind::Bug => ("", theme.error_fg()),
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

fn change_badge(change: ChangeKind) -> (&'static str, ratatui::style::Color) {
    let theme = tuicore::theme();
    match change {
        ChangeKind::Added => ("A", theme.success_fg()),
        ChangeKind::Modified => ("M", theme.warning_fg()),
        ChangeKind::Deleted => ("D", theme.error_fg()),
        ChangeKind::Synced => ("S", theme.text_fg()),
    }
}
