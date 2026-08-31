use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ratatui::{layout::Constraint, style::Style};
use tuicore::{
    ActivationMode, CellContext, Column, DataView, SelectionGlyphs, SelectionMode,
    SelectionPropagation, SelectionTrigger, TreeAdapter, theme,
};

use crate::{
    components::{
        ticket_number_jump::TicketNumberJump,
        work_item_rows::{
            ChangeBadge, TicketRowDetails, WorkItemKind, WorkItemRow, ticket_summary_text,
            work_item_title_prefix_width,
        },
    },
    store::{
        composer::{ChangeKind, ComposerState, TicketChange, TicketKind},
        work_items::is_done_status,
    },
};

#[derive(Clone)]
pub(super) struct TicketRow {
    pub(super) item: WorkItemRow,
    parent_id: Option<String>,
    depth: usize,
    parent_delta: Option<String>,
    subtask_progress: Option<(usize, usize)>,
    fix_versions: Vec<String>,
    epic_name: Option<String>,
}

#[cfg(test)]
pub(super) fn ticket_data_view(state: &ComposerState) -> DataView<TicketRow, String> {
    ticket_data_view_with_number_jump(state, Rc::new(RefCell::new(TicketNumberJump::default())))
}

pub(super) fn ticket_data_view_with_number_jump(
    state: &ComposerState,
    number_jump: Rc<RefCell<TicketNumberJump>>,
) -> DataView<TicketRow, String> {
    let mut view = DataView::new(ticket_rows(state), |row: &TicketRow| row.item.id.clone())
        .headers(false)
        .columns(ticket_columns(number_jump))
        .wrap_cells()
        .row_height(2)
        .activation_mode(ActivationMode::OnNavigate)
        .selection_mode(SelectionMode::Multi)
        .selection_propagation(SelectionPropagation::CascadeDescendants)
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
    let mut rows = state
        .ordered_changes()
        .into_iter()
        .filter_map(|change| ticket_row(state, change))
        .collect::<Vec<_>>();
    let parents = rows
        .iter()
        .map(|row| (row.item.id.clone(), row.parent_id.clone()))
        .collect::<HashMap<_, _>>();
    for row in &mut rows {
        row.depth = ticket_row_depth(&row.parent_id, &parents);
    }
    rows
}

fn ticket_row_depth(
    ticket_id: &Option<String>,
    parents: &HashMap<String, Option<String>>,
) -> usize {
    let mut depth = 0;
    let mut parent_id = ticket_id.as_deref();
    while let Some(id) = parent_id {
        depth += 1;
        parent_id = parents.get(id).and_then(Option::as_deref);
    }
    depth
}

fn ticket_row(state: &ComposerState, change: &TicketChange) -> Option<TicketRow> {
    let ticket = state.ticket_for_change(change)?;
    let presentation = state.presentation_for_change(change);
    let estimated_story_points = matches!(ticket.kind, TicketKind::Story | TicketKind::Task)
        .then(|| presentation.map(|presentation| presentation.assumed_story_points))
        .flatten();
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
            done: is_done_status(&ticket.status),
            assignee: ticket.assignee.clone(),
            labels: presentation
                .map(|presentation| presentation.work_item.labels.clone())
                .unwrap_or_default(),
            story_points: presentation
                .and_then(|presentation| presentation.work_item.story_points)
                .or(estimated_story_points),
            show_story_points: presentation
                .is_some_and(|presentation| presentation.story_points_configured),
            story_points_estimated: presentation
                .is_some_and(|presentation| presentation.work_item.story_points.is_none())
                && estimated_story_points.is_some(),
            story_points_from_average: false,
            change_badge: Some(change_badge(change.kind)),
            submitted: change.is_submitted(),
        },
        parent_id,
        depth: 0,
        parent_delta,
        subtask_progress: presentation.and_then(|presentation| {
            presentation
                .work_item
                .subtask_progress
                .as_ref()
                .map(|progress| (progress.completed, progress.total))
        }),
        fix_versions: presentation
            .map(|presentation| presentation.work_item.fix_versions.clone())
            .unwrap_or_default(),
        epic_name: presentation.and_then(|presentation| presentation.work_item.epic_name.clone()),
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

fn ticket_columns(number_jump: Rc<RefCell<TicketNumberJump>>) -> Vec<Column<TicketRow, String>> {
    vec![
        Column::multiline(
            "ticket",
            "",
            Constraint::Percentage(100),
            move |row: &TicketRow, _: &CellContext<String>| {
                ticket_summary_text(
                    &row.item,
                    number_jump.borrow().query(),
                    None,
                    TicketRowDetails {
                        subtask_progress: row.subtask_progress,
                        fix_versions: &row.fix_versions,
                        epic_name: row.epic_name.as_deref(),
                        annotation: row.parent_delta.as_deref(),
                    },
                )
            },
        )
        .constrained()
        .wrap_continuation_indent_by(|row| {
            tuicore::preset()
                .data_view()
                .tree_indent_width()
                .saturating_mul(row.depth.saturating_add(1))
                .saturating_add(2)
                .saturating_add(work_item_title_prefix_width(&row.item))
        }),
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
