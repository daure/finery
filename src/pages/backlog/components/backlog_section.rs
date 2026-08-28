use std::{cell::Cell, rc::Rc, sync::mpsc::Sender, time::Duration};

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Text},
};
use tuicore::{
    CellContext, Column, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, ListControlKeyBindings, RenderCtx, SelectionMode,
    SelectionTrigger, TickResult, TreeAdapter, TuiEvent, TuiNode,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogSectionEvent {
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

#[derive(Clone)]
enum BacklogRowContent {
    Parent(String),
    WorkItem(WorkItemRow),
}

#[derive(Clone, Copy)]
enum NavigationEndpoint {
    First,
    Last,
}

#[derive(Clone, Default)]
pub(in crate::pages::backlog) struct BacklogSectionNavigation {
    entry: Rc<Cell<Option<NavigationEndpoint>>>,
    previous: Option<Rc<Cell<Option<NavigationEndpoint>>>>,
    next: Option<Rc<Cell<Option<NavigationEndpoint>>>>,
}

impl BacklogSectionNavigation {
    fn new(
        entry: Rc<Cell<Option<NavigationEndpoint>>>,
        previous: Option<Rc<Cell<Option<NavigationEndpoint>>>>,
        next: Option<Rc<Cell<Option<NavigationEndpoint>>>>,
    ) -> Self {
        Self {
            entry,
            previous,
            next,
        }
    }

    pub(in crate::pages::backlog) fn sequence(count: usize) -> Vec<Self> {
        let entries = (0..count)
            .map(|_| Rc::new(Cell::new(None)))
            .collect::<Vec<_>>();
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Self::new(
                    entry.clone(),
                    index
                        .checked_sub(1)
                        .map(|previous| entries[previous].clone()),
                    entries.get(index + 1).cloned(),
                )
            })
            .collect()
    }
}

pub(in crate::pages::backlog) fn backlog_section(
    section_id: impl Into<String>,
    title: impl Into<String>,
    work_items: &[WorkItem],
    hotkey: Option<&str>,
    expanded: bool,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    navigation: BacklogSectionNavigation,
) -> BacklogSection {
    let section_id = section_id.into();
    let parent_id = format!("parent:{section_id}");
    let mut control = ListControl::new(
        backlog_rows(section_id.clone(), title.into(), work_items),
        |row: &BacklogRow| row.id.clone(),
        |_, _| unreachable!("backlog sections do not add rows"),
    )
    .headers(false)
    .columns(vec![backlog_column()])
    .tree(TreeAdapter::mutable_parent_id(
        |row: &BacklogRow| row.parent_id.clone(),
        |row, parent_id| row.parent_id = parent_id,
    ))
    .expanded(expanded.then_some(parent_id))
    .selection_mode(SelectionMode::None)
    .selection_trigger(SelectionTrigger::Manual)
    .max_rows(usize::MAX)
    .panel_visible(false)
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
        .set_row_height_by(|row| match &row.content {
            BacklogRowContent::Parent(_) => 1,
            BacklogRowContent::WorkItem(_) => 2,
        });
    let control = match hotkey {
        Some(hotkey) => control.hotkey(hotkey),
        None => control,
    };
    BacklogSection {
        section_id,
        parent_id: control
            .items()
            .first()
            .expect("backlog section has parent row")
            .id
            .clone(),
        control,
        events,
        move_locked,
        navigation,
    }
}

pub(in crate::pages::backlog) struct BacklogSection {
    section_id: String,
    parent_id: String,
    control: ListControl<BacklogRow, String>,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    navigation: BacklogSectionNavigation,
}

impl BacklogSection {
    pub(in crate::pages::backlog) fn blocks_move_gesture(&self, event: &TuiEvent) -> bool {
        blocks_move_gesture(self.move_locked.get(), event)
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn highlighted_id(&self) -> Option<String> {
        self.control.data_view().highlighted_id()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn transient_selected_ids(&self) -> Vec<String> {
        self.control.transient_selected_ids()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn is_reordering(&self) -> bool {
        self.control.is_reordering()
    }

    fn issue_keys(&self) -> Vec<String> {
        self.control
            .items()
            .iter()
            .filter_map(|row| match &row.content {
                BacklogRowContent::WorkItem(work_item) => Some(work_item.key.clone()),
                BacklogRowContent::Parent(_) => None,
            })
            .collect()
    }

    fn drain_events(&mut self, source_order: Vec<String>) {
        let final_order = self.issue_keys();
        for event in self.control.take_events() {
            let (row_ids, parent_id) = match event {
                ListControlEvent::TreeMoved {
                    row_id, parent_id, ..
                } => (vec![row_id], parent_id),
                ListControlEvent::TreeBlockMoved {
                    row_ids, parent_id, ..
                } => (row_ids, parent_id),
                _ => continue,
            };
            if parent_id.as_deref() != Some(self.parent_id.as_str()) {
                let _ = self.events.send(BacklogSectionEvent::Rejected {
                    section_id: self.section_id.clone(),
                    message: "Backlog tickets must remain in their section".into(),
                });
                continue;
            }
            let moved_ids = row_ids
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
            let moved_keys = source_order
                .iter()
                .filter(|key| moved_ids.contains(&work_item_row_id(&self.parent_id, key)))
                .cloned()
                .collect::<Vec<_>>();
            let event = if moved_keys.len() > crate::store::work_items::MAX_RANK_ISSUES {
                BacklogSectionEvent::Rejected {
                    section_id: self.section_id.clone(),
                    message: format!(
                        "Jira can rank at most {} issues at once",
                        crate::store::work_items::MAX_RANK_ISSUES
                    ),
                }
            } else {
                BacklogSectionEvent::Moved {
                    section_id: self.section_id.clone(),
                    moved_keys,
                    final_order: final_order.clone(),
                }
            };
            let _ = self.events.send(event);
        }
    }

    fn move_focus_at_boundary(&self, endpoint: NavigationEndpoint, ctx: &mut EventCtx<()>) {
        let target = match endpoint {
            NavigationEndpoint::First => &self.navigation.next,
            NavigationEndpoint::Last => &self.navigation.previous,
        };
        let Some(target) = target else {
            return;
        };
        target.set(Some(endpoint));
        match endpoint {
            NavigationEndpoint::First => ctx.focus_next(),
            NavigationEndpoint::Last => ctx.focus_previous(),
        }
    }

    fn apply_pending_navigation(&mut self, ctx: &mut FocusCtx<()>) {
        let Some(endpoint) = self.navigation.entry.take() else {
            return;
        };
        let parent_id = self.parent_id.clone();
        let target_id = match endpoint {
            NavigationEndpoint::First => parent_id.clone(),
            NavigationEndpoint::Last => self
                .issue_keys()
                .last()
                .map(|key| work_item_row_id(&parent_id, key))
                .unwrap_or_else(|| parent_id.clone()),
        };
        let view = self.control.data_view_mut();
        view.highlight_id(&target_id);
        if view.highlighted_id().as_ref() != Some(&target_id) {
            view.highlight_id(&parent_id);
        }
        ctx.request_redraw();
    }

    fn handle_navigation_boundary(
        &self,
        event: &TuiEvent,
        highlighted_before: Option<String>,
        outcome: EventOutcome,
        ctx: &mut EventCtx<()>,
    ) {
        if self.control.is_reordering() {
            return;
        }
        let Some(endpoint) = navigation_endpoint(event) else {
            return;
        };
        if highlighted_before == self.control.data_view().highlighted_id()
            && matches!(outcome, EventOutcome::Handled)
        {
            self.move_focus_at_boundary(endpoint, ctx);
        }
    }
}

pub(in crate::pages::backlog) fn blocks_move_gesture(move_locked: bool, event: &TuiEvent) -> bool {
    const MOVE_GESTURES: [KeySpec; 3] = [
        KeySpec::key_with_modifiers(Key::Char('m'), KeyModifiers::CONTROL),
        KeySpec::plain('<'),
        KeySpec::plain('>'),
    ];

    move_locked
        && matches!(
            event,
            TuiEvent::Key(key)
                if MOVE_GESTURES.into_iter().any(|binding| binding.matches(*key))
        )
}

fn blocks_selection_gesture(event: &TuiEvent) -> bool {
    let TuiEvent::Key(key) = event else {
        return false;
    };
    if KeySpec::plain(' ').matches(*key) {
        return true;
    }
    let is_range_modifier = matches!(key.modifiers, KeyModifiers::SHIFT | KeyModifiers::CONTROL);
    if !is_range_modifier {
        return false;
    }
    let bindings = tuicore::keybindings();
    bindings.line_up_matches(*key) || bindings.line_down_matches(*key)
}

fn navigation_endpoint(event: &TuiEvent) -> Option<NavigationEndpoint> {
    let TuiEvent::Key(key) = event else {
        return None;
    };
    let bindings = tuicore::keybindings();
    if bindings.line_down_matches(*key) || bindings.page_down_matches(*key) {
        Some(NavigationEndpoint::First)
    } else if bindings.line_up_matches(*key) || bindings.page_up_matches(*key) {
        Some(NavigationEndpoint::Last)
    } else {
        None
    }
}

impl TuiNode for BacklogSection {
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
        if self.blocks_move_gesture(event) || blocks_selection_gesture(event) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let source_order = self.issue_keys();
        let highlighted_before = self.control.data_view().highlighted_id();
        let outcome = self.control.event(event, ctx);
        self.drain_events(source_order);
        self.handle_navigation_boundary(event, highlighted_before, outcome, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.blocks_move_gesture(event) || blocks_selection_gesture(event) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let source_order = self.issue_keys();
        let highlighted_before = self.control.data_view().highlighted_id();
        let outcome = self.control.dispatch_event(route, event, ctx);
        self.drain_events(source_order);
        self.handle_navigation_boundary(event, highlighted_before, outcome, ctx);
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: tuicore::AnimationSettings) -> TickResult {
        self.control.tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
        if focused {
            self.apply_pending_navigation(ctx);
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.dispatch_focus(target, focused, ctx);
        if focused {
            self.apply_pending_navigation(ctx);
        }
    }

    fn focus_reveal_area(&self, target: &FocusTarget) -> Option<ratatui::layout::Rect> {
        self.control.focus_reveal_area(target)
    }

    fn focus_reveal_centered(&self, target: &FocusTarget) -> bool {
        self.control.focus_reveal_centered(target)
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
        id: work_item_row_id(&parent_id, &work_item.key),
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

fn work_item_row_id(parent_id: &str, key: &str) -> String {
    format!("{parent_id}:work-item:{key}")
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
