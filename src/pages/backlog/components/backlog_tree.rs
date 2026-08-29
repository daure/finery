use std::{cell::Cell, collections::HashMap, rc::Rc, sync::mpsc::Sender, time::Duration};

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Text},
};
use tuicore::{
    CellContext, Column, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, ListControlKeyBindings, RenderCtx, SearchMode, TickResult,
    TreeAdapter, TuiEvent, TuiNode,
};

use crate::{
    components::work_item_rows::{WorkItemKind, WorkItemRow, work_item_text},
    store::work_items::{BacklogSnapshot, WorkItem},
};

#[derive(Clone)]
struct BacklogRow {
    id: String,
    parent_id: Option<String>,
    content: BacklogRowContent,
}

#[derive(Clone)]
enum BacklogRowContent {
    Section { title: String },
    WorkItem(WorkItemRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogSectionEvent {
    MoveLocked,
    OpenQuickMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    Moved {
        section_id: String,
        moved_keys: Vec<String>,
        final_order: Vec<String>,
    },
    Rejected {
        section_id: String,
        message: String,
    },
}

pub(in crate::pages::backlog) fn backlog_tree(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
) -> BacklogTree {
    let mut control = ListControl::new(
        backlog_rows(snapshot),
        |row: &BacklogRow| row.id.clone(),
        |_, _| unreachable!("backlog does not add rows"),
    )
    .headers(false)
    .columns(vec![backlog_column()])
    .tree(TreeAdapter::mutable_parent_id(
        |row: &BacklogRow| row.parent_id.clone(),
        |row, parent_id| row.parent_id = parent_id,
    ))
    .expanded([section_row_id("backlog")])
    .allow_horizontal_moving(false)
    .max_rows(usize::MAX)
    .panel_visible(false)
    .action_bar(true)
    .search_mode(SearchMode::Contains)
    .keybindings(
        ListControlKeyBindings::default()
            .add([])
            .add_child([])
            .edit([])
            .remove([]),
    )
    .empty_message("No stories");
    control
        .data_view_mut()
        .set_row_height_by(|row| match row.content {
            BacklogRowContent::Section { .. } => 1,
            BacklogRowContent::WorkItem(_) => 2,
        });
    control.data_view_mut().set_wrap_cells(true);
    BacklogTree {
        control,
        events,
        move_locked,
    }
}

pub(in crate::pages::backlog) struct BacklogTree {
    control: ListControl<BacklogRow, String>,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
}

impl BacklogTree {
    pub(in crate::pages::backlog) fn set_snapshot(&mut self, snapshot: &BacklogSnapshot) {
        self.control.set_rows(backlog_rows(snapshot));
        let parents = self
            .control
            .transient_selected_ids()
            .into_iter()
            .filter_map(|id| self.control.items().iter().find(|row| row.id == id))
            .filter_map(|row| row.parent_id.clone())
            .collect::<std::collections::HashSet<_>>();
        if parents.len() > 1 {
            self.control.clear_transient_selection();
        }
    }

    pub(in crate::pages::backlog) fn highlight(&mut self, row_id: &str) {
        self.control
            .data_view_mut()
            .highlight_id(&row_id.to_owned());
    }

    fn is_section(&self, id: &str) -> bool {
        self.control
            .items()
            .iter()
            .any(|row| row.id == id && matches!(row.content, BacklogRowContent::Section { .. }))
    }

    fn highlighted_section(&self) -> bool {
        self.control
            .data_view()
            .highlighted_id()
            .is_some_and(|id| self.is_section(&id))
    }

    fn selected_issue_keys(&self) -> Option<(String, Vec<String>, Vec<String>)> {
        let selected = self.control.transient_selected_ids();
        let ids = if selected.is_empty() {
            self.control
                .data_view()
                .highlighted_id()
                .into_iter()
                .collect()
        } else {
            selected
        };
        let mut section = None;
        let mut keys = Vec::new();
        for id in ids {
            let row = self.control.items().iter().find(|row| row.id == id)?;
            let BacklogRowContent::WorkItem(item) = &row.content else {
                return None;
            };
            let parent = row.parent_id.as_ref()?.strip_prefix("section:")?.to_owned();
            if section.as_ref().is_some_and(|current| current != &parent) {
                return None;
            }
            section = Some(parent);
            keys.push(item.key.clone());
        }
        let section = section?;
        let order = self.issue_keys_in_section(&section);
        keys.sort_by_key(|key| order.iter().position(|candidate| candidate == key));
        Some((section, keys, order))
    }

    fn issue_keys_in_section(&self, section: &str) -> Vec<String> {
        let parent = section_row_id(section);
        self.control
            .items()
            .iter()
            .filter_map(|row| {
                (row.parent_id.as_deref() == Some(parent.as_str())).then(|| match &row.content {
                    BacklogRowContent::WorkItem(item) => Some(item.key.clone()),
                    BacklogRowContent::Section { .. } => None,
                })?
            })
            .collect()
    }

    fn blocks_locked_gesture(&self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        let modifiers = key.modifiers;
        let selection_navigation = (modifiers == KeyModifiers::SHIFT
            || modifiers == KeyModifiers::CONTROL)
            && (tuicore::keybindings().line_up_matches(*key)
                || tuicore::keybindings().line_down_matches(*key));
        KeySpec::key_with_modifiers(Key::Char('m'), KeyModifiers::CONTROL).matches(*key)
            || KeySpec::plain('<').matches(*key)
            || KeySpec::plain('>').matches(*key)
            || KeySpec::plain(' ').matches(*key)
            || selection_navigation
    }

    fn blocks_section_gesture(&self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        ((key.modifiers == KeyModifiers::SHIFT || key.modifiers == KeyModifiers::CONTROL)
            && (tuicore::keybindings().line_up_matches(*key)
                || tuicore::keybindings().line_down_matches(*key)))
            || KeySpec::key_with_modifiers(Key::Char('m'), KeyModifiers::CONTROL).matches(*key)
    }

    fn open_quick_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Key(key) if KeySpec::plain('.').matches(*key)) {
            return false;
        }
        if self.move_locked.get() {
            let _ = self.events.send(BacklogSectionEvent::MoveLocked);
        } else if !self.control.is_reordering() {
            if let Some((section_id, keys, source_order)) = self.selected_issue_keys() {
                let _ = self.events.send(BacklogSectionEvent::OpenQuickMenu {
                    section_id,
                    keys,
                    source_order,
                });
            }
        }
        ctx.stop_propagation();
        true
    }

    fn drain_events(&mut self, source_parents: HashMap<String, String>) {
        for event in self.control.take_events() {
            let (ids, parent) = match event {
                ListControlEvent::TreeMoved {
                    row_id, parent_id, ..
                } => (vec![row_id], parent_id),
                ListControlEvent::TreeBlockMoved {
                    row_ids, parent_id, ..
                } => (row_ids, parent_id),
                _ => continue,
            };
            let Some(section_id) = ids
                .first()
                .and_then(|id| source_parents.get(id))
                .and_then(|id| id.strip_prefix("section:"))
                .map(str::to_owned)
            else {
                continue;
            };
            let source_parent = section_row_id(&section_id);
            let valid = !ids.is_empty()
                && ids
                    .iter()
                    .all(|id| source_parents.get(id) == Some(&source_parent))
                && parent.as_deref() == Some(source_parent.as_str())
                && ids.iter().all(|id| !self.is_section(id));
            if !valid {
                let _ = self.events.send(BacklogSectionEvent::Rejected {
                    section_id,
                    message: "Backlog tickets must remain in their section".into(),
                });
                continue;
            }
            let moved_keys = self.issue_keys_for_ids(&ids, &source_parents);
            if moved_keys.len() > crate::store::work_items::MAX_RANK_ISSUES {
                let _ = self.events.send(BacklogSectionEvent::Rejected {
                    section_id,
                    message: format!(
                        "Jira can rank at most {} issues at once",
                        crate::store::work_items::MAX_RANK_ISSUES
                    ),
                });
            } else {
                let final_order = self.issue_keys_in_section(&section_id);
                let _ = self.events.send(BacklogSectionEvent::Moved {
                    section_id,
                    moved_keys,
                    final_order,
                });
            }
        }
    }

    fn issue_keys_for_ids(
        &self,
        ids: &[String],
        source_parents: &HashMap<String, String>,
    ) -> Vec<String> {
        self.control
            .items()
            .iter()
            .filter_map(|row| {
                (ids.contains(&row.id) && source_parents.contains_key(&row.id)).then(
                    || match &row.content {
                        BacklogRowContent::WorkItem(item) => Some(item.key.clone()),
                        BacklogRowContent::Section { .. } => None,
                    },
                )?
            })
            .collect()
    }

    fn handle_event(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
        dispatch: impl FnOnce(&mut ListControl<BacklogRow, String>, &mut EventCtx<()>) -> EventOutcome,
    ) -> EventOutcome {
        if self.open_quick_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if (self.move_locked.get() && self.blocks_locked_gesture(event))
            || (self.highlighted_section() && self.blocks_section_gesture(event))
        {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let source_parents = self
            .control
            .items()
            .iter()
            .map(|row| (row.id.clone(), row.parent_id.clone().unwrap_or_default()))
            .collect();
        let outcome = dispatch(&mut self.control, ctx);
        self.drain_events(source_parents);
        outcome
    }
}

impl TuiNode for BacklogTree {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: ratatui::layout::Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.control.layout(area, ctx)
    }
    fn render<'a>(
        &'a self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        ctx: &mut RenderCtx<'a>,
    ) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.handle_event(event, ctx, |control, ctx| control.event(event, ctx))
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.handle_event(event, ctx, |control, ctx| {
            control.dispatch_event(route, event, ctx)
        })
    }
    fn tick(&mut self, dt: Duration, settings: tuicore::AnimationSettings) -> TickResult {
        self.control.tick(dt, settings)
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
    }
}

fn backlog_rows(snapshot: &BacklogSnapshot) -> Vec<BacklogRow> {
    let mut rows = Vec::new();
    for sprint in &snapshot.sprints {
        let section = format!("sprint-{}", sprint.id);
        rows.push(section_row(
            &section,
            format!(
                "{} · {} ({})",
                snapshot.board_name, sprint.name, sprint.state
            ),
        ));
        rows.extend(
            sprint
                .work_items
                .iter()
                .map(|item| work_item_row(item, &section, snapshot.story_points_configured)),
        );
    }
    rows.push(section_row(
        "backlog",
        format!("{} · Backlog", snapshot.board_name),
    ));
    rows.extend(
        snapshot
            .work_items
            .iter()
            .map(|item| work_item_row(item, "backlog", snapshot.story_points_configured)),
    );
    rows
}

fn section_row(section: &str, title: String) -> BacklogRow {
    BacklogRow {
        id: section_row_id(section),
        parent_id: None,
        content: BacklogRowContent::Section { title },
    }
}
fn section_row_id(section: &str) -> String {
    format!("section:{section}")
}
fn work_item_row(item: &WorkItem, section: &str, show_story_points: bool) -> BacklogRow {
    BacklogRow {
        id: format!("ticket:{}", item.key),
        parent_id: Some(section_row_id(section)),
        content: BacklogRowContent::WorkItem(WorkItemRow {
            id: item.key.clone(),
            key: item.key.clone(),
            title: item.title.clone(),
            kind: work_item_kind(&item.kind),
            priority: item.priority.clone(),
            status: item.status.clone(),
            assignee: item.assignee.clone(),
            story_points: item.story_points,
            show_story_points,
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
            BacklogRowContent::Section { title, .. } => Text::from(Line::styled(
                title.clone(),
                Style::default()
                    .fg(tuicore::theme().accent_fg())
                    .add_modifier(Modifier::BOLD),
            )),
            BacklogRowContent::WorkItem(item) => work_item_text(item),
        },
    )
    .constrained()
    .search_key(|row| match &row.content {
        BacklogRowContent::Section { title } => title.clone(),
        BacklogRowContent::WorkItem(item) => format!("{} {}", item.key, item.title),
    })
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
