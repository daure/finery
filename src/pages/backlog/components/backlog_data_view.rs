use ratatui::{
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Text},
};
use tuicore::{
    ActivationMode, CellContext, Column, DataView, ScrollbarConfig, ScrollbarVisibility,
    SelectionGlyphs, SelectionMode, SelectionPropagation, SelectionTrigger, TreeAdapter,
};

use crate::{
    components::work_item_rows::{WorkItemKind, WorkItemRow, work_item_text},
    store::work_items::WorkItem,
};

#[derive(Clone)]
pub(in crate::pages::backlog) struct BacklogRow {
    id: String,
    parent_id: Option<String>,
    content: BacklogRowContent,
}

#[derive(Clone)]
enum BacklogRowContent {
    Parent(String),
    WorkItem(WorkItemRow),
}

pub(in crate::pages::backlog) fn backlog_data_view(
    section_id: impl Into<String>,
    title: impl Into<String>,
    work_items: &[WorkItem],
    hotkey: Option<&str>,
    expanded: bool,
) -> DataView<BacklogRow, String> {
    let section_id = section_id.into();
    let parent_id = format!("parent:{section_id}");
    let view = DataView::new(
        backlog_rows(section_id, title.into(), work_items),
        |row: &BacklogRow| row.id.clone(),
    )
    .headers(false)
    .columns(vec![backlog_column()])
    .row_height_by(|row| match &row.content {
        BacklogRowContent::Parent(_) => 1,
        BacklogRowContent::WorkItem(_) => 2,
    })
    .tree(TreeAdapter::parent_id(|row: &BacklogRow| {
        row.parent_id.clone()
    }))
    .expanded(expanded.then_some(parent_id))
    .activation_mode(ActivationMode::Manual)
    .selection_mode(SelectionMode::Multi)
    .selection_trigger(SelectionTrigger::OnActivate)
    .selection_propagation(SelectionPropagation::CascadeDescendants)
    .selection_glyphs(SelectionGlyphs::NERD_FONT)
    .scrollbars(ScrollbarConfig {
        vertical: ScrollbarVisibility::Never,
        horizontal: ScrollbarVisibility::Never,
        ..ScrollbarConfig::default()
    })
    .parent_vertical_scroll()
    .empty_message("No stories");
    match hotkey {
        Some(hotkey) => view.hotkey(hotkey),
        None => view,
    }
}

fn backlog_rows(section_id: String, title: String, work_items: &[WorkItem]) -> Vec<BacklogRow> {
    let parent_id = format!("parent:{section_id}");
    let mut rows = vec![BacklogRow {
        id: parent_id.clone(),
        parent_id: None,
        content: BacklogRowContent::Parent(title),
    }];
    rows.extend(
        work_items
            .iter()
            .map(|work_item| work_item_row(work_item, parent_id.clone())),
    );
    rows
}

fn work_item_row(work_item: &WorkItem, parent_id: String) -> BacklogRow {
    BacklogRow {
        id: format!("{parent_id}:work-item:{}", work_item.key),
        parent_id: Some(parent_id),
        content: BacklogRowContent::WorkItem(WorkItemRow {
            id: work_item.key.clone(),
            key: work_item.key.clone(),
            title: work_item.title.clone(),
            kind: work_item_kind(&work_item.kind),
            priority: work_item.priority.clone(),
            status: work_item.status.clone(),
            change_badge: None,
            submitted: false,
        }),
    }
}

fn backlog_column() -> Column<BacklogRow, String> {
    Column::multiline(
        "backlog",
        "",
        Constraint::Percentage(100),
        |row: &BacklogRow, _: &CellContext<String>| match &row.content {
            BacklogRowContent::Parent(title) => Text::from(Line::styled(
                title.clone(),
                Style::default()
                    .fg(tuicore::theme().accent_fg())
                    .add_modifier(Modifier::BOLD),
            )),
            BacklogRowContent::WorkItem(work_item) => work_item_text(work_item),
        },
    )
}

fn work_item_kind(kind: &str) -> WorkItemKind {
    match kind.to_ascii_lowercase().as_str() {
        "epic" => WorkItemKind::Epic,
        "story" => WorkItemKind::Story,
        "task" => WorkItemKind::Task,
        "bug" => WorkItemKind::Bug,
        "subtask" | "sub-task" => WorkItemKind::Subtask,
        _ => WorkItemKind::Other,
    }
}
