use ratatui::style::Style;
use tuicore::{ActivationMode, DataView, SelectionGlyphs, SelectionMode, SelectionTrigger, theme};

use crate::{
    components::work_item_rows::{ChangeBadge, WorkItemKind, WorkItemRow, work_item_column},
    store::composer::{ChangeKind, ComposerState, TicketChange, TicketKind},
};

pub(super) type TicketRow = WorkItemRow;

pub(super) fn ticket_data_view(state: &ComposerState) -> DataView<TicketRow, String> {
    let mut view = DataView::new(ticket_rows(state), |row: &TicketRow| row.id.clone())
        .headers(false)
        .columns(ticket_columns())
        .row_height(2)
        .activation_mode(ActivationMode::OnNavigate)
        .selection_mode(SelectionMode::Multi)
        .selection_trigger(SelectionTrigger::OnActivate)
        .selection_glyphs(SelectionGlyphs::NERD_FONT)
        .selection_disabled_by(|row| row.submitted)
        .selection_disabled_glyph("󱋭")
        .selected(
            state
                .active_set()
                .into_iter()
                .flat_map(|set| set.selected_ticket_ids.clone())
                .collect::<Vec<_>>(),
        );
    if let Some(selected) = state.selected_ticket.as_ref() {
        view.highlight_id(selected);
    }
    set_active_ticket_style(&mut view, state.selected_ticket.clone());
    view
}

pub(super) fn set_active_ticket_style(
    view: &mut DataView<TicketRow, String>,
    selected: Option<String>,
) {
    view.set_row_style_by(move |row| {
        (selected.as_deref() == Some(row.id.as_str())).then(|| {
            let theme = theme();
            Style::default()
                .fg(theme.selected_fg())
                .bg(theme.selected_bg())
        })
    });
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
    Some(WorkItemRow {
        id: change.id.clone(),
        key: ticket.key.clone(),
        title: ticket.title.clone(),
        kind: ticket_kind(ticket.kind),
        priority: ticket.priority.clone(),
        status: ticket.status.clone(),
        change_badge: Some(change_badge(change.kind)),
        submitted: change.is_submitted(),
    })
}

fn ticket_columns() -> Vec<tuicore::Column<TicketRow, String>> {
    vec![work_item_column()]
}

fn ticket_kind(kind: TicketKind) -> WorkItemKind {
    match kind {
        TicketKind::Epic => WorkItemKind::Epic,
        TicketKind::Story => WorkItemKind::Story,
        TicketKind::Task => WorkItemKind::Task,
        TicketKind::Subtask => WorkItemKind::Subtask,
        TicketKind::Bug => WorkItemKind::Bug,
    }
}

fn change_badge(change: ChangeKind) -> ChangeBadge {
    match change {
        ChangeKind::Added => ChangeBadge::Added,
        ChangeKind::Modified => ChangeBadge::Modified,
        ChangeKind::Deleted => ChangeBadge::Deleted,
        ChangeKind::Synced => ChangeBadge::Synced,
    }
}
