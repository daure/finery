use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::mpsc::Sender,
    time::Duration,
};

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tuicore::{
    Button, CellContext, ChildKey, Column, DataViewTransformMode, Dropdown, DropdownLabelPosition,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusRequest,
    FocusTarget, HotkeyEvent, Key, KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, ListControl, ListControlEvent, ListControlKeyBindings,
    MenuButton, MenuItem, RenderCtx, SearchMode, TickResult, Toggle, TreeAdapter, TuiEvent,
    TuiNode,
};

use crate::{
    components::{
        ticket_number_jump::{TicketNumberJump, exact_ticket_number_matches},
        work_item_rows::{
            TicketRowDetails, WorkItemKind, WorkItemRow, ticket_summary_text,
            work_item_title_prefix_width,
        },
    },
    jira::JiraOption,
    store::work_items::{
        BacklogSnapshot, RunwayCapacitySource, RunwayTicket, Sprint, SprintCapacityState,
        SubtaskProgress, WorkItem,
    },
};

#[derive(Clone)]
struct BacklogRow {
    id: String,
    parent_id: Option<String>,
    content: BacklogRowContent,
}

#[derive(Clone)]
enum BacklogRowContent {
    Section {
        title: Text<'static>,
        search_text: String,
    },
    WorkItem(BacklogWorkItem),
}

#[derive(Clone)]
struct BacklogWorkItem {
    item: WorkItemRow,
    runway: Option<RunwayTicket>,
    alternate_background: bool,
    subtask_progress: Option<SubtaskProgress>,
    fix_versions: Vec<String>,
    epic_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogSectionEvent {
    Refresh,
    EstimatedChanged(bool),
    IssueTypesChanged(Vec<String>),
    OpenVelocity,
    OpenReports,
    OpenTimeline,
    OpenBoard,
    OpenReleases,
    WebMenuClosed,
    MoveLocked,
    OpenTicket {
        key: String,
    },
    YankTicketUrl {
        key: String,
    },
    YankSprintGoal {
        goal: String,
    },
    YankSprintReport {
        sprint_id: u64,
    },
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum WebMenuItem {
    Board,
    Timeline,
    Releases,
    Reports,
}

#[cfg(test)]
pub(in crate::pages::backlog) fn backlog_tree(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
) -> BacklogTree {
    backlog_tree_with_issue_types(snapshot, events, move_locked, Vec::new())
}

pub(in crate::pages::backlog) fn backlog_tree_with_issue_types(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    issue_types: Vec<JiraOption>,
) -> BacklogTree {
    let number_jump = Rc::new(RefCell::new(TicketNumberJump::default()));
    let filters = BacklogFilters::default();
    let issue_types = selectable_issue_types(issue_types);
    let mut control = ListControl::new(
        backlog_rows(snapshot, &filters),
        |row: &BacklogRow| row.id.clone(),
        |_, _| unreachable!("backlog does not add rows"),
    )
    .headers(false)
    .columns(vec![backlog_column(Rc::clone(&number_jump))])
    .tree(TreeAdapter::mutable_parent_id(
        |row: &BacklogRow| row.parent_id.clone(),
        |row, parent_id| row.parent_id = parent_id,
    ))
    .expanded(initially_expanded_rows(snapshot))
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
    let runway_markers_visible = Rc::new(Cell::new(!filters.is_active()));
    let row_marker_visible = Rc::clone(&runway_markers_visible);
    control.data_view_mut().set_row_height_by(|row| {
        if matches!(row.content, BacklogRowContent::Section { .. }) {
            u16::from(row.id != section_row_id("backlog")) + 1
        } else {
            2
        }
    });
    control.data_view_mut().set_wrap_cells(true);
    control.data_view_mut().set_show_inactive_highlight(true);
    control
        .data_view_mut()
        .set_row_style_by(|row| match &row.content {
            BacklogRowContent::WorkItem(item) if item.alternate_background => {
                Some(Style::default().bg(tuicore::theme().surface_bg()))
            }
            _ => None,
        });
    control
        .data_view_mut()
        .set_left_gutter_marker_by(move |row| match &row.content {
            BacklogRowContent::WorkItem(item) => Some(
                (row_marker_visible.get()
                    && item
                        .runway
                        .as_ref()
                        .is_some_and(|runway| runway.virtual_sprint % 2 == 1))
                .then(|| Span::styled("┃", Style::default().fg(tuicore::theme().accent_fg())))
                .unwrap_or_else(|| Span::raw(" ")),
            ),
            _ => None,
        });
    let refresh_events = events.clone();
    let velocity_events = events.clone();
    let estimated_events = events.clone();
    let issue_type_events = events.clone();
    let issue_type_labels = Rc::new(RefCell::new(issue_type_labels(&issue_types)));
    let selected_issue_type_labels = Rc::clone(&issue_type_labels);
    BacklogTree {
        control,
        refresh: Button::new("󰑓").hotkey("shift+r").on_press(move || {
            let _ = refresh_events.send(BacklogSectionEvent::Refresh);
        }),
        velocity: Button::new("󰓅").hotkey("shift+v").on_press(move || {
            let _ = velocity_events.send(BacklogSectionEvent::OpenVelocity);
        }),
        estimated: Toggle::new("󰑭")
            .checked(true)
            .hotkey("shift+e")
            .preserve_focus_on_hotkey(true)
            .on_change(move |estimated| {
                let _ = estimated_events.send(BacklogSectionEvent::EstimatedChanged(estimated));
            }),
        issue_types: Dropdown::multi(
            issue_types,
            |issue_type: &JiraOption| issue_type.id.clone(),
            |issue_type| issue_type.label.clone(),
        )
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder(" Type")
        .hotkey("shift+t")
        .on_select(move |selected| {
            let issue_types = selected
                .into_iter()
                .filter_map(|id| selected_issue_type_labels.borrow().get(&id).cloned())
                .collect();
            let _ = issue_type_events.send(BacklogSectionEvent::IssueTypesChanged(issue_types));
        }),
        web: MenuButton::new(
            "Web",
            [
                MenuItem::new(WebMenuItem::Board, "Board"),
                MenuItem::new(WebMenuItem::Timeline, "Timeline"),
                MenuItem::new(WebMenuItem::Releases, "Releases"),
                MenuItem::new(WebMenuItem::Reports, "Reports"),
            ],
        )
        .hotkey("shift+w"),
        loading: false,
        refresh_area: ratatui::layout::Rect::default(),
        velocity_area: ratatui::layout::Rect::default(),
        estimated_area: ratatui::layout::Rect::default(),
        issue_types_area: ratatui::layout::Rect::default(),
        web_area: ratatui::layout::Rect::default(),
        control_area: ratatui::layout::Rect::default(),
        events,
        move_locked,
        runway_markers_visible,
        filters,
        issue_type_labels,
        snapshot: snapshot.clone(),
        number_jump,
    }
}

pub(in crate::pages::backlog) struct BacklogTree {
    control: ListControl<BacklogRow, String>,
    refresh: Button<()>,
    velocity: Button<()>,
    estimated: Toggle<()>,
    issue_types: Dropdown<JiraOption, String>,
    web: MenuButton<WebMenuItem>,
    loading: bool,
    refresh_area: ratatui::layout::Rect,
    velocity_area: ratatui::layout::Rect,
    estimated_area: ratatui::layout::Rect,
    issue_types_area: ratatui::layout::Rect,
    web_area: ratatui::layout::Rect,
    control_area: ratatui::layout::Rect,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    runway_markers_visible: Rc<Cell<bool>>,
    filters: BacklogFilters,
    issue_type_labels: Rc<RefCell<HashMap<String, String>>>,
    snapshot: BacklogSnapshot,
    number_jump: Rc<RefCell<TicketNumberJump>>,
}

impl BacklogTree {
    pub(in crate::pages::backlog) fn set_snapshot(&mut self, snapshot: &BacklogSnapshot) {
        self.snapshot = snapshot.clone();
        let highlighted = self.control.data_view().highlighted_id();
        let expanded = self.control.data_view().tree_expansion_snapshot();
        let highlighted_parent = highlighted.as_ref().and_then(|id| {
            self.control
                .items()
                .iter()
                .find(|row| &row.id == id)
                .and_then(|row| row.parent_id.clone())
        });
        self.control.set_rows(backlog_rows(snapshot, &self.filters));
        self.sync_search_results();
        let expanded = expanded
            .into_iter()
            .filter(|id| self.is_expandable(id))
            .collect();
        self.control
            .data_view_mut()
            .restore_tree_expansion(expanded);
        if highlighted
            .as_ref()
            .is_some_and(|id| !self.control.items().iter().any(|row| &row.id == id))
        {
            let fallback = highlighted_parent
                .filter(|id| self.control.items().iter().any(|row| &row.id == id))
                .unwrap_or_else(|| section_row_id("backlog"));
            self.highlight(&fallback);
        }
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

    pub(in crate::pages::backlog) fn set_estimated(&mut self, estimated: bool) {
        self.filters.estimated = estimated;
        self.estimated.set_value(estimated);
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_issue_types_filter(&mut self, issue_types: Vec<String>) {
        self.filters.issue_types = issue_types;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_issue_types(&mut self, issue_types: Vec<JiraOption>) {
        let issue_types = selectable_issue_types(issue_types);
        *self.issue_type_labels.borrow_mut() = issue_type_labels(&issue_types);
        self.issue_types.set_rows(issue_types);
    }

    pub(in crate::pages::backlog) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        self.refresh.set_disabled(loading);
        self.velocity.set_disabled(loading);
        self.estimated.set_disabled(loading);
        self.issue_types.set_disabled(loading);
    }

    pub(in crate::pages::backlog) fn highlight(&mut self, row_id: &str) {
        self.control
            .data_view_mut()
            .highlight_id(&row_id.to_owned());
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn highlighted_id_for_test(&self) -> Option<String> {
        self.control.data_view().highlighted_id()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn is_reordering_for_test(&self) -> bool {
        self.control.is_reordering()
    }

    fn is_expandable(&self, id: &str) -> bool {
        self.control.items().iter().any(|row| {
            row.id == id
                && (matches!(row.content, BacklogRowContent::Section { .. })
                    || self
                        .control
                        .items()
                        .iter()
                        .any(|child| child.parent_id.as_deref() == Some(id)))
        })
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

    fn highlighted_subtask(&self) -> bool {
        self.control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
            .and_then(|row| row.parent_id.as_deref())
            .is_some_and(|parent_id| parent_id.starts_with("ticket:"))
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
            keys.push(item.item.key.clone());
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
                    BacklogRowContent::WorkItem(item) => Some(item.item.key.clone()),
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

    fn open_highlighted_ticket(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return false;
        }
        let Some(row) = self
            .control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
        else {
            return false;
        };
        let BacklogRowContent::WorkItem(item) = &row.content else {
            return false;
        };
        let _ = self.events.send(BacklogSectionEvent::OpenTicket {
            key: item.item.key.clone(),
        });
        ctx.stop_propagation();
        true
    }

    fn handle_yank(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) = event else {
            return false;
        };
        let Some(id) = self.control.data_view().highlighted_id() else {
            return false;
        };
        let event = match sequence.as_str() {
            "yu" => self
                .control
                .items()
                .iter()
                .find(|row| row.id == id)
                .and_then(|row| match &row.content {
                    BacklogRowContent::WorkItem(item) => Some(BacklogSectionEvent::YankTicketUrl {
                        key: item.item.key.clone(),
                    }),
                    BacklogRowContent::Section { .. } => None,
                }),
            "yg" => self.sprint_for_section(&id).and_then(|sprint| {
                sprint
                    .goal
                    .clone()
                    .filter(|goal| !goal.trim().is_empty())
                    .map(|goal| BacklogSectionEvent::YankSprintGoal { goal })
            }),
            "yv" => {
                self.sprint_for_section(&id)
                    .map(|sprint| BacklogSectionEvent::YankSprintReport {
                        sprint_id: sprint.id,
                    })
            }
            _ => None,
        };
        let Some(event) = event else {
            return false;
        };
        let _ = self.events.send(event);
        ctx.stop_propagation();
        true
    }

    fn sprint_for_section(&self, id: &str) -> Option<&Sprint> {
        let sprint_id = id.strip_prefix("section:sprint-")?.parse::<u64>().ok()?;
        self.snapshot
            .sprints
            .iter()
            .find(|sprint| sprint.id == sprint_id)
    }

    fn handle_ticket_number_jump(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if self.control.data_view().is_searching() {
            return false;
        }
        if self.number_jump.borrow().cancels(*key) {
            self.number_jump.borrow_mut().clear();
            ctx.request_redraw();
            ctx.stop_propagation();
            return true;
        }
        if self.number_jump.borrow().accepts(*key) {
            let number = self
                .number_jump
                .borrow()
                .query()
                .unwrap_or_default()
                .to_owned();
            let row_id = self.exact_ticket_row_id(&number);
            self.number_jump.borrow_mut().clear();
            if let Some(row_id) = row_id {
                self.jump_to_ticket(&row_id);
            }
            ctx.request_redraw();
            ctx.stop_propagation();
            return true;
        }
        if !self.number_jump.borrow_mut().push(*key) {
            return false;
        }
        let number = self
            .number_jump
            .borrow()
            .query()
            .unwrap_or_default()
            .to_owned();
        self.control.data_view_mut().expand_all();
        let matching_count = self
            .control
            .items()
            .iter()
            .filter(|row| matches!(&row.content, BacklogRowContent::WorkItem(item) if crate::components::ticket_number_jump::ticket_number_matches(&item.item.key, &number)))
            .count();
        if matching_count == 1 {
            if let Some(row_id) = self.exact_ticket_row_id(&number) {
                self.number_jump.borrow_mut().clear();
                self.jump_to_ticket(&row_id);
            }
        }
        ctx.request_redraw();
        ctx.request_tick();
        ctx.stop_propagation();
        true
    }

    fn exact_ticket_row_id(&self, number: &str) -> Option<String> {
        self.control
            .items()
            .iter()
            .find_map(|row| match &row.content {
                BacklogRowContent::WorkItem(item)
                    if exact_ticket_number_matches(&item.item.key, number) =>
                {
                    Some(row.id.clone())
                }
                _ => None,
            })
    }

    fn jump_to_ticket(&mut self, row_id: &str) {
        let view = self.control.data_view_mut();
        view.highlight_id(&row_id.to_owned());
        view.reveal_highlighted_centered();
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
                        BacklogRowContent::WorkItem(item) => Some(item.item.key.clone()),
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
        if self.handle_yank(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.handle_ticket_number_jump(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_highlighted_ticket(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_quick_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if (self.move_locked.get() && self.blocks_locked_gesture(event))
            || ((self.highlighted_section() || self.highlighted_subtask())
                && self.blocks_section_gesture(event))
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
        let search = self.control.data_view().transform_state().search.clone();
        let outcome = dispatch(&mut self.control, ctx);
        if self.control.data_view().transform_state().search != search {
            self.sync_search_results();
        }
        let show_runway_bands = self.show_runway_bands();
        if self.runway_markers_visible.replace(show_runway_bands) != show_runway_bands {
            ctx.request_redraw();
        }
        self.drain_events(source_parents);
        outcome
    }

    fn show_runway_bands(&self) -> bool {
        !self.filters.is_active()
            && self
                .control
                .data_view()
                .transform_state()
                .search
                .trim()
                .is_empty()
    }

    fn sync_search_results(&mut self) {
        let search = self
            .control
            .data_view()
            .transform_state()
            .search
            .trim()
            .to_owned();
        if search.is_empty() {
            self.control
                .data_view_mut()
                .set_transform_mode(DataViewTransformMode::Local);
            self.control.data_view_mut().clear_visible_row_ids();
            return;
        }

        self.control
            .data_view_mut()
            .set_transform_mode(DataViewTransformMode::External);

        let rows = self.control.items();
        let row_ids = rows
            .iter()
            .map(|row| row.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let parent_ids = rows
            .iter()
            .map(|row| (row.id.as_str(), row.parent_id.as_deref()))
            .collect::<HashMap<_, _>>();
        let mut visible_ids = std::collections::HashSet::new();

        for row in rows.iter().filter(|row| {
            tuicore::search_match(&search, &backlog_search_text(row), SearchMode::Contains)
                .is_some()
        }) {
            visible_ids.insert(row.id.as_str());
            visible_ids.extend(descendant_row_ids(row.id.as_str(), rows));

            let mut parent_id = row.parent_id.as_deref();
            while let Some(id) = parent_id {
                if !visible_ids.insert(id) {
                    break;
                }
                parent_id = parent_ids.get(id).copied().flatten();
            }
        }

        let visible_row_ids = rows
            .iter()
            .filter(|row| row_ids.contains(&row.id) && visible_ids.contains(row.id.as_str()))
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        self.control
            .data_view_mut()
            .set_visible_row_ids(visible_row_ids);
    }

    fn drain_web_menu(&mut self, was_open: bool) {
        let activated = self.web.take_activated();
        for item in activated.iter().copied() {
            let event = match item {
                WebMenuItem::Board => BacklogSectionEvent::OpenBoard,
                WebMenuItem::Timeline => BacklogSectionEvent::OpenTimeline,
                WebMenuItem::Releases => BacklogSectionEvent::OpenReleases,
                WebMenuItem::Reports => BacklogSectionEvent::OpenReports,
            };
            let _ = self.events.send(event);
        }
        if was_open && activated.is_empty() && !self.web.is_open() {
            let _ = self.events.send(BacklogSectionEvent::WebMenuClosed);
        }
    }

    fn refocus_data_view_after_unfocus(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if !tuicore::keybindings().focus().unfocus_matches(*key) {
            return false;
        }
        ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
        ctx.stop_propagation();
        true
    }
}

impl TuiNode for BacklogTree {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: ratatui::layout::Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let header_height = u16::from(!area.is_empty());
        let button_width = |button: &Button<()>| {
            button
                .measure(LayoutProposal::at_most(area.width, header_height))
                .preferred
                .width
        };
        let estimated_width = <Toggle<()> as TuiNode<()>>::measure(
            &self.estimated,
            LayoutProposal::at_most(area.width, header_height),
        )
        .preferred
        .width
        .min(area.width);
        let issue_types_width = <Dropdown<JiraOption, String> as TuiNode<()>>::measure(
            &self.issue_types,
            LayoutProposal::at_most(area.width, header_height),
        )
        .preferred
        .width
        .min(area.width.saturating_sub(estimated_width));
        let refresh_width = button_width(&self.refresh).min(
            area.width
                .saturating_sub(estimated_width)
                .saturating_sub(issue_types_width)
                .saturating_sub(u16::from(estimated_width > 0))
                .saturating_sub(u16::from(issue_types_width > 0)),
        );
        let velocity_width = button_width(&self.velocity).min(
            area.width
                .saturating_sub(refresh_width)
                .saturating_sub(estimated_width)
                .saturating_sub(issue_types_width)
                .saturating_sub(u16::from(refresh_width > 0)),
        );
        let web_width = self
            .web
            .measure(LayoutProposal::at_most(area.width, header_height))
            .preferred
            .width
            .min(
                area.width
                    .saturating_sub(refresh_width)
                    .saturating_sub(velocity_width)
                    .saturating_sub(estimated_width)
                    .saturating_sub(issue_types_width)
                    .saturating_sub(u16::from(refresh_width > 0))
                    .saturating_sub(u16::from(velocity_width > 0))
                    .saturating_sub(u16::from(estimated_width > 0)),
            );
        self.web_area = ratatui::layout::Rect::new(area.x, area.y, web_width, header_height);
        self.estimated_area = ratatui::layout::Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(estimated_width)),
            area.y,
            estimated_width,
            header_height,
        );
        self.issue_types_area = ratatui::layout::Rect::new(
            self.estimated_area
                .x
                .saturating_sub(issue_types_width)
                .saturating_sub(u16::from(issue_types_width > 0)),
            area.y,
            issue_types_width,
            header_height,
        );
        self.refresh_area = ratatui::layout::Rect::new(
            self.issue_types_area
                .x
                .saturating_sub(refresh_width)
                .saturating_sub(u16::from(refresh_width > 0)),
            area.y,
            refresh_width,
            header_height,
        );
        self.velocity_area = ratatui::layout::Rect::new(
            self.refresh_area
                .x
                .saturating_sub(velocity_width)
                .saturating_sub(u16::from(velocity_width > 0)),
            area.y,
            velocity_width,
            header_height,
        );
        self.control_area = ratatui::layout::Rect::new(
            area.x,
            area.y.saturating_add(header_height),
            area.width,
            area.height.saturating_sub(header_height),
        );
        let (result, _) = ctx.with_focus_fallback_hotkey_sequences_status(
            FocusId::new("data-view"),
            self.control_area,
            ["yu".to_owned(), "yg".to_owned(), "yv".to_owned()],
            |ctx| self.control.layout(self.control_area, ctx),
        );
        ctx.push_slot(ChildKey::new("refresh"), self.refresh_area, |ctx| {
            self.refresh.layout(self.refresh_area, ctx)
        });
        ctx.push_slot(ChildKey::new("velocity"), self.velocity_area, |ctx| {
            self.velocity.layout(self.velocity_area, ctx)
        });
        ctx.push_slot(ChildKey::new("estimated"), self.estimated_area, |ctx| {
            <Toggle<()> as TuiNode<()>>::layout(&mut self.estimated, self.estimated_area, ctx)
        });
        ctx.push_slot(ChildKey::new("issue-types"), self.issue_types_area, |ctx| {
            <Dropdown<JiraOption, String> as TuiNode<()>>::layout(
                &mut self.issue_types,
                self.issue_types_area,
                ctx,
            )
        });
        ctx.push_slot(ChildKey::new("web"), self.web_area, |ctx| {
            self.web.layout(self.web_area, ctx)
        });
        result
    }
    fn render<'a>(
        &'a self,
        frame: &mut Frame,
        _area: ratatui::layout::Rect,
        ctx: &mut RenderCtx<'a>,
    ) {
        self.refresh.render(frame, self.refresh_area);
        self.velocity.render(frame, self.velocity_area);
        self.estimated.render(frame, self.estimated_area);
        self.issue_types.render(frame, self.issue_types_area, ctx);
        self.web.render(frame, self.web_area, ctx);
        self.control.render(frame, self.control_area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let web_was_open = self.web.is_open();
        if matches!(event, TuiEvent::Mouse(_))
            && (self.refresh.event(event, ctx) == EventOutcome::Handled
                || self.velocity.event(event, ctx) == EventOutcome::Handled
                || self.estimated.event(event, ctx) == EventOutcome::Handled
                || self.issue_types.event(event, ctx) == EventOutcome::Handled
                || self.web.event(event, ctx) == EventOutcome::Handled)
        {
            self.drain_web_menu(web_was_open);
            return EventOutcome::Handled;
        }
        let outcome = self.handle_event(event, ctx, |control, ctx| control.event(event, ctx));
        self.drain_web_menu(web_was_open);
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let Some(refresh_path) = route.path.without_first_if(&ChildKey::new("refresh")) {
            let outcome = self
                .refresh
                .dispatch_event(&EventRoute::new(refresh_path), event, ctx);
            return self
                .refocus_data_view_after_unfocus(event, ctx)
                .then_some(EventOutcome::Handled)
                .unwrap_or(outcome);
        }
        if let Some(velocity_path) = route.path.without_first_if(&ChildKey::new("velocity")) {
            let outcome = self
                .velocity
                .dispatch_event(&EventRoute::new(velocity_path), event, ctx);
            return self
                .refocus_data_view_after_unfocus(event, ctx)
                .then_some(EventOutcome::Handled)
                .unwrap_or(outcome);
        }
        if let Some(estimated_path) = route.path.without_first_if(&ChildKey::new("estimated")) {
            let outcome =
                self.estimated
                    .dispatch_event(&EventRoute::new(estimated_path), event, ctx);
            return self
                .refocus_data_view_after_unfocus(event, ctx)
                .then_some(EventOutcome::Handled)
                .unwrap_or(outcome);
        }
        if let Some(issue_types_path) = route.path.without_first_if(&ChildKey::new("issue-types")) {
            let outcome =
                self.issue_types
                    .dispatch_event(&EventRoute::new(issue_types_path), event, ctx);
            return self
                .refocus_data_view_after_unfocus(event, ctx)
                .then_some(EventOutcome::Handled)
                .unwrap_or(outcome);
        }
        if let Some(web_path) = route.path.without_first_if(&ChildKey::new("web")) {
            let web_was_open = self.web.is_open();
            let outcome = self
                .web
                .dispatch_event(&EventRoute::new(web_path), event, ctx);
            self.drain_web_menu(web_was_open);
            return self
                .refocus_data_view_after_unfocus(event, ctx)
                .then_some(EventOutcome::Handled)
                .unwrap_or(outcome);
        }
        let keeps_data_focus = route
            .path
            .without_first_if(&ChildKey::new("data"))
            .is_some()
            && matches!(event, TuiEvent::Key(key) if tuicore::keybindings().focus().unfocus_matches(*key));
        let outcome = self.handle_event(event, ctx, |control, ctx| {
            control.dispatch_event(route, event, ctx)
        });
        if keeps_data_focus && outcome == EventOutcome::Ignored {
            ctx.stop_propagation();
            EventOutcome::Handled
        } else {
            outcome
        }
    }
    fn tick(&mut self, dt: Duration, settings: tuicore::AnimationSettings) -> TickResult {
        let number_jump = {
            let mut jump = self.number_jump.borrow_mut();
            if jump.advance(dt) {
                TickResult::CHANGED
            } else {
                jump.remaining()
                    .map_or(TickResult::IDLE, TickResult::scheduled_after)
            }
        };
        self.control
            .tick(dt, settings)
            .merge(<Button<()> as TuiNode<()>>::tick(
                &mut self.refresh,
                dt,
                settings,
            ))
            .merge(<Button<()> as TuiNode<()>>::tick(
                &mut self.velocity,
                dt,
                settings,
            ))
            .merge(<Toggle<()> as TuiNode<()>>::tick(
                &mut self.estimated,
                dt,
                settings,
            ))
            .merge(<Dropdown<JiraOption, String> as TuiNode<()>>::tick(
                &mut self.issue_types,
                dt,
                settings,
            ))
            .merge(self.web.tick(dt, settings))
            .merge(number_jump)
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(refresh_target) = target.for_child(&ChildKey::new("refresh")) {
            self.refresh.dispatch_focus(&refresh_target, focused, ctx);
            return;
        }
        if let Some(velocity_target) = target.for_child(&ChildKey::new("velocity")) {
            self.velocity.dispatch_focus(&velocity_target, focused, ctx);
            return;
        }
        if let Some(estimated_target) = target.for_child(&ChildKey::new("estimated")) {
            self.estimated
                .dispatch_focus(&estimated_target, focused, ctx);
            return;
        }
        if let Some(issue_types_target) = target.for_child(&ChildKey::new("issue-types")) {
            self.issue_types
                .dispatch_focus(&issue_types_target, focused, ctx);
            return;
        }
        if let Some(web_target) = target.for_child(&ChildKey::new("web")) {
            self.web.dispatch_focus(&web_target, focused, ctx);
            return;
        }
        self.control.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.init(ctx);
        self.estimated.init(ctx);
        self.issue_types.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.mount(ctx);
        self.estimated.mount(ctx);
        self.issue_types.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
        self.estimated.unmount(ctx);
        self.issue_types.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
        self.estimated.destroy(ctx);
        self.issue_types.destroy(ctx);
    }
}

struct BacklogFilters {
    estimated: bool,
    issue_types: Vec<String>,
}

impl Default for BacklogFilters {
    fn default() -> Self {
        Self {
            estimated: true,
            issue_types: Vec::new(),
        }
    }
}

impl BacklogFilters {
    fn is_active(&self) -> bool {
        !self.estimated || !self.issue_types.is_empty()
    }
}

pub(in crate::pages::backlog) fn selectable_issue_types(
    issue_types: Vec<JiraOption>,
) -> Vec<JiraOption> {
    issue_types
        .into_iter()
        .filter(|issue_type| {
            !matches!(
                issue_type.label.to_ascii_lowercase().as_str(),
                "subtask" | "sub-task" | "epic"
            )
        })
        .collect()
}

fn issue_type_labels(issue_types: &[JiraOption]) -> HashMap<String, String> {
    issue_types
        .iter()
        .map(|issue_type| (issue_type.id.clone(), issue_type.label.clone()))
        .collect()
}

fn backlog_rows(snapshot: &BacklogSnapshot, filters: &BacklogFilters) -> Vec<BacklogRow> {
    let mut rows = Vec::new();
    for sprint in &snapshot.sprints {
        let section = format!("sprint-{}", sprint.id);
        rows.push(sprint_section_row(&section, sprint));
        let visible_items = visible_work_items(&sprint.work_items, filters);
        let item_keys = visible_items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<std::collections::HashSet<_>>();
        let is_active_sprint = sprint.state == "active";
        rows.extend(visible_items.into_iter().enumerate().map(|(index, item)| {
            work_item_row(
                item,
                &section,
                is_active_sprint,
                &item_keys,
                snapshot.story_points_configured,
                None,
                sprint.capacity.as_ref().map(|capacity| {
                    (
                        capacity.assumed_ticket_size,
                        capacity.assumed_ticket_size_from_average,
                    )
                }),
                index % 2 == 0,
            )
        }));
    }
    rows.push(backlog_section_row(snapshot));
    let visible_items = visible_work_items(&snapshot.work_items, filters);
    let item_keys = visible_items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<std::collections::HashSet<_>>();
    rows.extend(visible_items.into_iter().enumerate().map(|(index, item)| {
        work_item_row(
            item,
            "backlog",
            false,
            &item_keys,
            snapshot.story_points_configured,
            snapshot
                .runway
                .as_ref()
                .and_then(|runway| runway.tickets.iter().find(|ticket| ticket.key == item.key))
                .cloned(),
            None,
            index % 2 == 0,
        )
    }));
    rows
}

fn visible_work_items<'a>(items: &'a [WorkItem], filters: &BacklogFilters) -> Vec<&'a WorkItem> {
    let items_by_key = items
        .iter()
        .map(|item| (item.key.as_str(), item))
        .collect::<std::collections::HashMap<_, _>>();
    items
        .iter()
        .filter(|item| {
            (!matches!(work_item_kind(&item.kind), WorkItemKind::Subtask)
                || item
                    .parent_key
                    .as_deref()
                    .is_some_and(|parent| items_by_key.contains_key(parent)))
                && matches_filters(item, filters)
                && ancestors_match_filters(item, &items_by_key, filters)
        })
        .collect()
}

fn ancestors_match_filters(
    item: &WorkItem,
    items_by_key: &std::collections::HashMap<&str, &WorkItem>,
    filters: &BacklogFilters,
) -> bool {
    let mut ancestor = item;
    let mut seen = std::collections::HashSet::from([item.key.as_str()]);
    while let Some(parent_key) = ancestor.parent_key.as_deref() {
        let Some(parent) = items_by_key.get(parent_key).copied() else {
            return true;
        };
        if !seen.insert(parent.key.as_str()) || !matches_filters(parent, filters) {
            return false;
        }
        ancestor = parent;
    }
    true
}

fn matches_filters(item: &WorkItem, filters: &BacklogFilters) -> bool {
    (filters.estimated || (estimation_eligible(item) && item.story_points.is_none()))
        && (filters.issue_types.is_empty()
            || filters
                .issue_types
                .iter()
                .any(|issue_type| item.kind.eq_ignore_ascii_case(issue_type)))
}

fn initially_expanded_rows(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut expanded = vec![section_row_id("backlog")];
    for sprint in &snapshot.sprints {
        expanded.extend(parent_row_ids(&sprint.work_items));
    }
    expanded.extend(parent_row_ids(&snapshot.work_items));
    expanded
}

fn parent_row_ids(items: &[WorkItem]) -> Vec<String> {
    let item_keys = items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<std::collections::HashSet<_>>();
    items
        .iter()
        .filter_map(|item| {
            item.parent_key
                .as_deref()
                .filter(|parent| item_keys.contains(parent))
                .map(|parent| format!("ticket:{parent}"))
        })
        .collect()
}

fn section_row(section: &str, title: Text<'static>, search_text: String) -> BacklogRow {
    BacklogRow {
        id: section_row_id(section),
        parent_id: None,
        content: BacklogRowContent::Section { title, search_text },
    }
}

fn sprint_section_row(section: &str, sprint: &Sprint) -> BacklogRow {
    let theme = tuicore::theme();
    let icon = sprint_icon(&sprint.state);
    let mut title = vec![
        Span::styled(icon, Style::default().fg(theme.accent_fg())),
        Span::raw(" "),
        Span::styled(
            sprint.name.clone(),
            Style::default()
                .fg(theme.text_fg())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(date_range) = sprint_date_range(sprint) {
        title.extend([
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(date_range, Style::default().fg(theme.text_fg())),
        ]);
    }
    let search_text = sprint_title(sprint);
    let is_active = sprint.state == "active";
    let Some(capacity) = sprint.capacity.as_ref() else {
        title.extend([
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!(
                    "{} items",
                    sprint_item_count_label(&sprint.work_items, is_active)
                ),
                Style::default().fg(theme.muted_fg()),
            ),
        ]);
        return section_row(section, Text::from(Line::from(title)), search_text);
    };
    let (coverage, coverage_style) = sprint_estimation_coverage(sprint);
    section_row(
        section,
        Text::from(vec![
            Line::from(title),
            Line::from(vec![
                Span::styled(
                    sprint_capacity_icon(capacity.state),
                    Style::default().fg(theme.accent_fg()),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{} pts", capacity_load_label(capacity, is_active)),
                    Style::default().fg(theme.text_fg()),
                ),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(coverage, coverage_style),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(
                    format!(
                        "{} items",
                        sprint_item_count_label(&sprint.work_items, is_active)
                    ),
                    Style::default().fg(theme.muted_fg()),
                ),
            ]),
        ]),
        search_text,
    )
}

fn backlog_section_row(snapshot: &BacklogSnapshot) -> BacklogRow {
    let theme = tuicore::theme();
    section_row(
        "backlog",
        Text::from(Line::from(vec![
            Span::styled("", Style::default().fg(theme.accent_fg())),
            Span::raw(" "),
            Span::styled(
                "Backlog",
                Style::default()
                    .fg(theme.text_fg())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!("{} items", root_item_count_label(&snapshot.work_items)),
                Style::default().fg(theme.muted_fg()),
            ),
        ])),
        backlog_title(snapshot),
    )
}
fn section_row_id(section: &str) -> String {
    format!("section:{section}")
}
fn work_item_row(
    item: &WorkItem,
    section: &str,
    is_active_sprint: bool,
    item_keys: &std::collections::HashSet<&str>,
    show_story_points: bool,
    runway: Option<RunwayTicket>,
    assumed_ticket_size: Option<(f64, bool)>,
    alternate_background: bool,
) -> BacklogRow {
    let assumed_ticket_size = matches!(
        work_item_kind(&item.kind),
        WorkItemKind::Story | WorkItemKind::Task
    )
    .then(|| {
        runway
            .as_ref()
            .filter(|runway| runway.assumed)
            .map(|runway| (runway.effective_points, runway.assumed_from_average))
            .or(assumed_ticket_size)
    })
    .flatten();
    BacklogRow {
        id: format!("ticket:{}", item.key),
        parent_id: item
            .parent_key
            .as_deref()
            .filter(|parent| item_keys.contains(parent))
            .map(|parent| format!("ticket:{parent}"))
            .or_else(|| Some(section_row_id(section))),
        content: BacklogRowContent::WorkItem(BacklogWorkItem {
            item: WorkItemRow {
                id: item.key.clone(),
                key: item.key.clone(),
                title: item.title.clone(),
                kind: work_item_kind(&item.kind),
                priority: item.priority.clone(),
                status: item.status.clone(),
                done: item.done,
                assignee: item.assignee.clone(),
                labels: item.labels.clone(),
                story_points: item
                    .story_points
                    .or_else(|| assumed_ticket_size.map(|(points, _)| points)),
                show_story_points: show_story_points || assumed_ticket_size.is_some(),
                story_points_estimated: item.story_points.is_none()
                    && assumed_ticket_size.is_some(),
                story_points_from_average: item.story_points.is_none()
                    && assumed_ticket_size.is_some_and(|(_, from_average)| from_average),
                change_badge: None,
                submitted: false,
                status_changed_at: item.status_changed_at,
                show_time_in_status: is_active_sprint,
            },
            runway,
            alternate_background,
            subtask_progress: item.subtask_progress.clone(),
            fix_versions: item.fix_versions.clone(),
            epic_name: item.epic_name.clone(),
        }),
    }
}
fn backlog_column(number_jump: Rc<RefCell<TicketNumberJump>>) -> Column<BacklogRow, String> {
    Column::multiline(
        "backlog",
        "",
        Constraint::Percentage(100),
        move |row: &BacklogRow, _: &CellContext<String>| match &row.content {
            BacklogRowContent::Section { title, .. } => title.clone(),
            BacklogRowContent::WorkItem(item) => {
                backlog_work_item_text(item, number_jump.borrow().query())
            }
        },
    )
    .constrained()
    .wrap_continuation_indent_by(|row| match &row.content {
        BacklogRowContent::Section { .. } => 0,
        BacklogRowContent::WorkItem(item) => tuicore::preset()
            .data_view()
            .tree_indent_width()
            .saturating_add(1)
            .saturating_add(2)
            .saturating_add(work_item_title_prefix_width(&item.item)),
    })
    .search_key(backlog_search_text)
}

fn backlog_search_text(row: &BacklogRow) -> String {
    match &row.content {
        BacklogRowContent::Section { search_text, .. } => search_text.clone(),
        BacklogRowContent::WorkItem(item) => format!(
            "{} {} {}",
            item.item.key,
            item.item.title,
            item.epic_name.as_deref().unwrap_or_default(),
        ),
    }
}

fn descendant_row_ids<'a>(id: &'a str, rows: &'a [BacklogRow]) -> Vec<&'a str> {
    let mut descendants = Vec::new();
    let mut parents = vec![id];
    while let Some(parent) = parents.pop() {
        for child in rows
            .iter()
            .filter(|row| row.parent_id.as_deref() == Some(parent))
        {
            descendants.push(child.id.as_str());
            parents.push(child.id.as_str());
        }
    }
    descendants
}

fn backlog_work_item_text(row: &BacklogWorkItem, number_query: Option<&str>) -> Text<'static> {
    ticket_summary_text(
        &row.item,
        number_query,
        None,
        TicketRowDetails {
            subtask_progress: row
                .subtask_progress
                .as_ref()
                .map(|progress| (progress.completed, progress.total)),
            fix_versions: &row.fix_versions,
            epic_name: row.epic_name.as_deref(),
            annotation: None,
        },
    )
}

fn sprint_title(sprint: &Sprint) -> String {
    let icon = sprint_icon(&sprint.state);
    let date_range = sprint_date_range(sprint)
        .map(|range| format!(" • {range}"))
        .unwrap_or_default();
    let is_active = sprint.state == "active";
    let Some(capacity) = sprint.capacity.as_ref() else {
        return format!("{icon} {}{date_range}", sprint.name);
    };
    format!(
        "{icon} {}{date_range}\n{} {} pts • {} • {} items",
        sprint.name,
        sprint_capacity_icon(capacity.state),
        capacity_load_label(capacity, is_active),
        sprint_estimation_coverage(sprint).0,
        sprint_item_count_label(&sprint.work_items, is_active),
    )
}

fn sprint_estimation_coverage(sprint: &Sprint) -> (String, Style) {
    let theme = tuicore::theme();
    let eligible_items = sprint
        .work_items
        .iter()
        .filter(|item| estimation_eligible(item))
        .count();
    let estimated_items = sprint
        .work_items
        .iter()
        .filter(|item| estimation_eligible(item) && item.story_points.is_some())
        .count();
    if estimated_items == eligible_items {
        (
            format!("✓ {eligible_items}/{eligible_items}"),
            Style::default().fg(theme.success_fg()),
        )
    } else {
        (
            format!("󰄰 {estimated_items}/{eligible_items}"),
            Style::default().fg(theme.warning_fg()),
        )
    }
}

fn estimation_eligible(item: &WorkItem) -> bool {
    matches!(item.kind.to_ascii_lowercase().as_str(), "task" | "story")
}

fn capacity_load_label(
    capacity: &crate::store::work_items::SprintCapacity,
    is_active: bool,
) -> String {
    let prefix = matches!(capacity.source, RunwayCapacitySource::JiraVelocity)
        .then_some("~")
        .unwrap_or("");
    if is_active {
        let source_suffix = match capacity.source {
            RunwayCapacitySource::JiraVelocity => "v",
            RunwayCapacitySource::Fixed | RunwayCapacitySource::FixedFallback => "c",
        };
        format!(
            "{prefix}{}/{} ({}{source_suffix})",
            points_label(capacity.completed_points),
            points_label(capacity.effective_points),
            points_label(capacity.capacity)
        )
    } else {
        format!(
            "{prefix}{}/{}",
            points_label(capacity.effective_points),
            points_label(capacity.capacity)
        )
    }
}

fn backlog_title(snapshot: &BacklogSnapshot) -> String {
    format!(
        " Backlog • {} items",
        root_item_count_label(&snapshot.work_items)
    )
}

fn sprint_item_count_label(items: &[WorkItem], is_active: bool) -> String {
    if is_active {
        let (completed, total) = root_item_counts(items);
        format!("{completed}/{total}")
    } else {
        root_item_count_label(items)
    }
}

fn root_item_count_label(items: &[WorkItem]) -> String {
    let (_, total) = root_item_counts(items);
    format!("{total}")
}

fn root_item_counts(items: &[WorkItem]) -> (usize, usize) {
    let keys = items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut completed = 0;
    let mut total = 0;
    for item in items {
        if !is_subtask(item)
            && item
                .parent_key
                .as_deref()
                .is_none_or(|parent| !keys.contains(parent))
        {
            total += 1;
            if item.done || crate::store::work_items::is_done_status(&item.status) {
                completed += 1;
            }
        }
    }
    (completed, total)
}

fn is_subtask(item: &WorkItem) -> bool {
    matches!(
        item.kind.to_ascii_lowercase().as_str(),
        "sub-task" | "subtask"
    )
}

fn sprint_icon(state: &str) -> &'static str {
    match state {
        "active" => "",
        "future" => "",
        _ => "•",
    }
}

fn sprint_capacity_icon(state: SprintCapacityState) -> &'static str {
    match state {
        SprintCapacityState::OnTarget => "",
        SprintCapacityState::OverCommitted => "",
        SprintCapacityState::UnderCommitted => "",
    }
}

fn sprint_date_range(sprint: &Sprint) -> Option<String> {
    Some(format!(
        "{} – {}",
        sprint_date_label(sprint.start_date.as_deref()?)?,
        sprint_date_label(sprint.end_date.as_deref()?)?,
    ))
}

fn sprint_date_label(date: &str) -> Option<String> {
    let mut parts = date.get(..10)?.split('-');
    let year = parts.next()?.parse::<u16>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    let month = match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => return None,
    };
    (year > 0 && (1..=31).contains(&day) && parts.next().is_none())
        .then(|| format!("{day} {month}"))
}

fn points_label(points: f64) -> String {
    if points.fract().abs() < f64::EPSILON {
        format!("{points:.0}")
    } else {
        format!("{points:.1}")
    }
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
