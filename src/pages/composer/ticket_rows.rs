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
            ChangeBadge, TicketRowDetails, WorkItemKind, WorkItemRow, attachment_summary_text,
            mermaid_diagram_summary_text, ticket_summary_text, work_item_title_prefix_width,
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
    attachments: Vec<crate::store::composer::TicketAttachment>,
    attachment: Option<crate::store::composer::TicketAttachment>,
    mermaid_diagrams: Vec<crate::store::composer::MermaidDiagram>,
    mermaid_diagram: Option<crate::store::composer::MermaidDiagram>,
}

#[cfg(test)]
pub(super) fn ticket_data_view(state: &ComposerState) -> DataView<TicketRow, String> {
    ticket_data_view_with_number_jump(
        state,
        Rc::new(RefCell::new(TicketNumberJump::default())),
        None,
    )
}

pub(super) fn ticket_data_view_with_number_jump(
    state: &ComposerState,
    number_jump: Rc<RefCell<TicketNumberJump>>,
    jira_base_url: Option<String>,
) -> DataView<TicketRow, String> {
    let mut view = DataView::new(ticket_rows(state), |row: &TicketRow| row.item.id.clone())
        .headers(false)
        .columns(ticket_columns(number_jump))
        .wrap_cells()
        .row_height_by(|row| row.row_height())
        .activation_mode(ActivationMode::OnNavigate)
        .selection_mode(SelectionMode::Multi)
        .selection_propagation(SelectionPropagation::CascadeDescendants)
        .selection_trigger(SelectionTrigger::OnActivate)
        .selection_glyphs(SelectionGlyphs::NERD_FONT)
        .selection_disabled_by(|row| row.item.submitted)
        .selection_glyph_hidden_by(|row| row.attachment.is_some() || row.mermaid_diagram.is_some())
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
        )
        .copy_hotkey("yu", move |row| {
            (!row.item.key.starts_with("NEW-")).then(|| {
                jira_base_url
                    .as_ref()
                    .map(|base_url| format!("{base_url}/browse/{}", row.item.key))
            })?
        });
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
    let changes = state.ordered_changes();
    let temporary_keys = temporary_draft_keys(state, &changes);
    let mut rows =
        changes
            .into_iter()
            .filter_map(|change| ticket_row(state, change, temporary_keys.get(&change.id)))
            .flat_map(|row| {
                let mut attachment_rows = row
                    .attachments
                    .iter()
                    .cloned()
                    .enumerate()
                    .filter(|(_, attachment)| {
                        !row.mermaid_diagrams.iter().any(|diagram| {
                            diagram.published_attachment_id.as_deref()
                                == Some(attachment.id.as_str())
                        })
                    })
                    .map(|(index, attachment)| TicketRow::attachment_child(&row, attachment, index))
                    .collect::<Vec<_>>();
                attachment_rows.extend(row.mermaid_diagrams.iter().cloned().enumerate().map(
                    |(index, diagram)| TicketRow::mermaid_diagram_child(&row, diagram, index),
                ));
                attachment_rows.insert(0, row);
                attachment_rows
            })
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

pub(super) fn display_key_for_ticket(state: &ComposerState, ticket_id: &str) -> Option<String> {
    let changes = state.ordered_changes();
    let temporary_keys = temporary_draft_keys(state, &changes);
    let change = changes.into_iter().find(|change| change.id == ticket_id)?;
    temporary_keys.get(ticket_id).cloned().or_else(|| {
        state
            .ticket_for_change(change)
            .map(|ticket| ticket.key.clone())
    })
}

fn temporary_draft_keys(
    state: &ComposerState,
    changes: &[&TicketChange],
) -> HashMap<String, String> {
    let mut counters = HashMap::<String, usize>::new();
    changes
        .iter()
        .filter_map(|change| {
            let ticket = state.ticket_for_change(change)?;
            (change.kind == ChangeKind::Added
                && !change.is_submitted()
                && ticket.key.starts_with("NEW-"))
            .then(|| {
                let number = counters.entry(ticket.project_key.clone()).or_default();
                *number += 1;
                (
                    change.id.clone(),
                    format!("{}-TMP-{number}", ticket.project_key),
                )
            })
        })
        .collect()
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

fn ticket_row(
    state: &ComposerState,
    change: &TicketChange,
    temporary_key: Option<&String>,
) -> Option<TicketRow> {
    let ticket = state.ticket_for_change(change)?;
    let presentation = state.presentation_for_change(change);
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
            key: temporary_key.cloned().unwrap_or_else(|| ticket.key.clone()),
            title: ticket.title.clone(),
            kind: ticket_kind(ticket.kind),
            priority: ticket.priority.clone(),
            status: ticket.status.clone(),
            done: is_done_status(&ticket.status),
            assignee: ticket.assignee.clone(),
            labels: ticket.labels.clone(),
            story_points: ticket.story_points,
            show_story_points: presentation
                .is_some_and(|presentation| presentation.story_points_configured),
            story_points_estimated: false,
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
        fix_versions: ticket.fix_versions.clone(),
        epic_name: presentation.and_then(|presentation| presentation.work_item.epic_name.clone()),
        attachments: ticket.attachments.clone(),
        attachment: None,
        mermaid_diagrams: state
            .changes_for_change(change)
            .map(|ticket| ticket.mermaid_diagrams.clone())
            .unwrap_or_default(),
        mermaid_diagram: None,
    })
}

fn ticket_columns(number_jump: Rc<RefCell<TicketNumberJump>>) -> Vec<Column<TicketRow, String>> {
    vec![
        Column::multiline(
            "ticket",
            "",
            Constraint::Percentage(100),
            move |row: &TicketRow, context: &CellContext<String>| {
                if let Some(attachment) = row.attachment.as_ref() {
                    return attachment_summary_text(
                        attachment.change,
                        &attachment.filename,
                        &attachment.created,
                        attachment.size,
                        context.highlighted,
                        row.item.submitted,
                    );
                }
                if let Some(diagram) = row.mermaid_diagram.as_ref() {
                    return mermaid_diagram_summary_text(
                        &diagram.title,
                        &diagram.diagram_type,
                        context.highlighted,
                        row.item.submitted,
                        diagram.published_attachment_id.is_some(),
                    );
                }
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

impl TicketRow {
    fn attachment_child(
        parent: &Self,
        attachment: crate::store::composer::TicketAttachment,
        index: usize,
    ) -> Self {
        Self {
            item: WorkItemRow {
                id: format!("{}:attachment:{index}", parent.item.id),
                key: String::new(),
                title: String::new(),
                kind: WorkItemKind::Other,
                priority: String::new(),
                status: String::new(),
                done: false,
                assignee: String::new(),
                labels: Vec::new(),
                story_points: None,
                show_story_points: false,
                story_points_estimated: false,
                story_points_from_average: false,
                change_badge: None,
                submitted: parent.item.submitted,
            },
            parent_id: Some(parent.item.id.clone()),
            depth: 0,
            parent_delta: None,
            subtask_progress: None,
            fix_versions: Vec::new(),
            epic_name: None,
            attachments: Vec::new(),
            attachment: Some(attachment),
            mermaid_diagrams: Vec::new(),
            mermaid_diagram: None,
        }
    }

    fn mermaid_diagram_child(
        parent: &Self,
        diagram: crate::store::composer::MermaidDiagram,
        index: usize,
    ) -> Self {
        Self {
            item: WorkItemRow {
                id: format!("{}:diagram:{index}", parent.item.id),
                key: String::new(),
                title: String::new(),
                kind: WorkItemKind::Other,
                priority: String::new(),
                status: String::new(),
                done: false,
                assignee: String::new(),
                labels: Vec::new(),
                story_points: None,
                show_story_points: false,
                story_points_estimated: false,
                story_points_from_average: false,
                change_badge: None,
                submitted: parent.item.submitted,
            },
            parent_id: Some(parent.item.id.clone()),
            depth: 0,
            parent_delta: None,
            subtask_progress: None,
            fix_versions: Vec::new(),
            epic_name: None,
            attachments: Vec::new(),
            attachment: None,
            mermaid_diagrams: Vec::new(),
            mermaid_diagram: Some(diagram),
        }
    }

    fn row_height(&self) -> u16 {
        if self.attachment.is_some() || self.mermaid_diagram.is_some() {
            1
        } else {
            2
        }
    }
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
