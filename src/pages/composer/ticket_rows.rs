use ratatui::{layout::Constraint, style::Style, text::Line};
use tuicore::{
    ActivationMode, CellContext, Column, DataView, SelectionGlyphs, SelectionMode,
    SelectionTrigger, TreeAdapter, theme,
};

use crate::{
    components::work_item_rows::{ChangeBadge, WorkItemKind, WorkItemRow, work_item_text},
    store::composer::{ChangeKind, ComposerState, TicketChange, TicketKind},
};

#[derive(Clone)]
pub(super) struct TicketRow {
    pub(super) item: WorkItemRow,
    parent_id: Option<String>,
    parent_delta: Option<String>,
}

pub(super) fn ticket_data_view(state: &ComposerState) -> DataView<TicketRow, String> {
    let mut view = DataView::new(ticket_rows(state), |row: &TicketRow| row.item.id.clone())
        .headers(false)
        .columns(ticket_columns())
        .wrap_cells()
        .row_height(2)
        .activation_mode(ActivationMode::OnNavigate)
        .selection_mode(SelectionMode::Multi)
        .selection_trigger(SelectionTrigger::OnActivate)
        .selection_glyphs(SelectionGlyphs::NERD_FONT)
        .selection_disabled_by(|row| row.item.submitted)
        .selection_disabled_glyph("󱋭")
        .tree(TreeAdapter::parent_id(|row: &TicketRow| {
            row.parent_id.clone()
        }))
        .expanded(
            ticket_rows(state)
                .into_iter()
                .map(|row| row.item.id)
                .collect::<Vec<_>>(),
        )
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
        (selected.as_deref() == Some(row.item.id.as_str())).then(|| {
            let theme = theme();
            Style::default()
                .fg(theme.selected_fg())
                .bg(theme.selected_bg())
        })
    });
}

pub(super) fn ticket_rows(state: &ComposerState) -> Vec<TicketRow> {
    state
        .ordered_changes()
        .into_iter()
        .filter_map(|change| ticket_row(state, change))
        .collect()
}

fn ticket_row(state: &ComposerState, change: &TicketChange) -> Option<TicketRow> {
    let ticket = state.ticket_for_change(change)?;
    let active_ids = state
        .active_set()?
        .tickets
        .iter()
        .flat_map(|change| {
            std::iter::once((change.id.as_str(), change.id.as_str())).chain(
                state
                    .changes_for_change(change)
                    .map(|candidate| (candidate.key.as_str(), change.id.as_str())),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let parent_id = ticket
        .parent_key
        .as_ref()
        .and_then(|parent| active_ids.get(parent.as_str()))
        .map(|parent| (*parent).to_owned());
    let old_parent = change
        .original
        .as_ref()
        .or_else(|| state.source_for_change(change))
        .and_then(|ticket| ticket.parent_key.as_ref())
        .cloned();
    let parent_delta = (change.kind == ChangeKind::Modified && old_parent != ticket.parent_key)
        .then(|| {
            format!(
                "{} -> {}",
                old_parent.unwrap_or_else(|| "Root".into()),
                ticket.parent_key.clone().unwrap_or_else(|| "Root".into())
            )
        });
    Some(TicketRow {
        item: WorkItemRow {
            id: change.id.clone(),
            key: display_key(change, ticket),
            title: ticket.title.clone(),
            kind: ticket_kind(ticket.kind),
            priority: ticket.priority.clone(),
            status: ticket.status.clone(),
            assignee: ticket.assignee.clone(),
            story_points: None,
            show_story_points: false,
            story_points_estimated: false,
            story_points_from_average: false,
            change_badge: Some(change_badge(change.kind)),
            submitted: change.is_submitted(),
        },
        parent_id,
        parent_delta,
    })
}

fn display_key(change: &TicketChange, ticket: &crate::store::composer::Ticket) -> String {
    if change.kind == ChangeKind::Added && !change.is_submitted() && ticket.key.starts_with("NEW-")
    {
        format!("{}-DRAFT", ticket.project_key)
    } else {
        ticket.key.clone()
    }
}

fn ticket_columns() -> Vec<Column<TicketRow, String>> {
    vec![
        Column::multiline(
            "ticket",
            "",
            Constraint::Percentage(100),
            |row: &TicketRow, _: &CellContext<String>| {
                let mut text = work_item_text(&row.item);
                if let Some(delta) = &row.parent_delta {
                    text.lines[1] = Line::raw(delta.clone());
                }
                text
            },
        )
        .constrained(),
    ]
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
