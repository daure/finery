use std::{cell::Cell, collections::HashMap, rc::Rc, sync::mpsc::Sender, time::Duration};

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tuicore::{
    Animated, Button, CellContext, ChildKey, Column, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusId, FocusTarget, Key, KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, ListControl, ListControlEvent, ListControlKeyBindings, RenderCtx,
    SearchMode, Spinner, TickResult, TreeAdapter, TuiEvent, TuiNode,
};

use crate::{
    components::{
        avatar::bubble_span,
        work_item_rows::{
            WorkItemKind, WorkItemRow, story_points_label, work_item_title_prefix_width,
            work_item_title_with_key_line,
        },
    },
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
    MoveLocked,
    OpenTicket {
        key: String,
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
    let runway_markers_visible = Rc::new(Cell::new(true));
    let row_marker_visible = Rc::clone(&runway_markers_visible);
    control.data_view_mut().set_row_height_by(|row| {
        if matches!(row.content, BacklogRowContent::Section { .. }) {
            u16::from(row.id != section_row_id("backlog")) + 1
        } else {
            2
        }
    });
    control.data_view_mut().set_wrap_cells(true);
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
    BacklogTree {
        control,
        refresh: Button::new("Refresh").hotkey("shift+r").on_press(move || {
            let _ = refresh_events.send(BacklogSectionEvent::Refresh);
        }),
        spinner: Spinner::new(),
        loading: false,
        refresh_area: ratatui::layout::Rect::default(),
        spinner_area: ratatui::layout::Rect::default(),
        control_area: ratatui::layout::Rect::default(),
        events,
        move_locked,
        runway_markers_visible,
    }
}

pub(in crate::pages::backlog) struct BacklogTree {
    control: ListControl<BacklogRow, String>,
    refresh: Button<()>,
    spinner: Spinner,
    loading: bool,
    refresh_area: ratatui::layout::Rect,
    spinner_area: ratatui::layout::Rect,
    control_area: ratatui::layout::Rect,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    runway_markers_visible: Rc<Cell<bool>>,
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

    pub(in crate::pages::backlog) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        self.refresh.set_disabled(loading);
    }

    pub(in crate::pages::backlog) fn reset_to_backlog_parent(
        &mut self,
        snapshot: &BacklogSnapshot,
    ) {
        *self = backlog_tree(snapshot, self.events.clone(), self.move_locked.clone());
        self.highlight(&section_row_id("backlog"));
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
        if self.open_highlighted_ticket(event, ctx) {
            return EventOutcome::Handled;
        }
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
        let show_runway_bands = self
            .control
            .data_view()
            .transform_state()
            .search
            .trim()
            .is_empty();
        if self.runway_markers_visible.replace(show_runway_bands) != show_runway_bands {
            ctx.request_redraw();
        }
        self.drain_events(source_parents);
        outcome
    }
}

impl TuiNode for BacklogTree {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: ratatui::layout::Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let header_height = u16::from(!area.is_empty());
        let refresh_width = self
            .refresh
            .measure(LayoutProposal::at_most(area.width, header_height))
            .preferred
            .width
            .min(area.width);
        self.refresh_area = ratatui::layout::Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(refresh_width)),
            area.y,
            refresh_width,
            header_height,
        );
        self.spinner_area = if self.loading {
            ratatui::layout::Rect::new(
                self.refresh_area.x.saturating_sub(2),
                area.y,
                1,
                header_height,
            )
        } else {
            ratatui::layout::Rect::default()
        };
        self.control_area = ratatui::layout::Rect::new(
            area.x,
            area.y.saturating_add(header_height),
            area.width,
            area.height.saturating_sub(header_height),
        );
        let result = self.control.layout(self.control_area, ctx);
        ctx.push_slot(ChildKey::new("refresh"), self.refresh_area, |ctx| {
            self.refresh.layout(self.refresh_area, ctx)
        });
        if self.loading {
            <Spinner as TuiNode<()>>::layout(&mut self.spinner, self.spinner_area, ctx);
        }
        result
    }
    fn render<'a>(
        &'a self,
        frame: &mut Frame,
        _area: ratatui::layout::Rect,
        ctx: &mut RenderCtx<'a>,
    ) {
        self.refresh.render(frame, self.refresh_area);
        if self.loading {
            self.spinner.render(frame, self.spinner_area);
        }
        self.control.render(frame, self.control_area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if matches!(event, TuiEvent::Mouse(_))
            && self.refresh.event(event, ctx) == EventOutcome::Handled
        {
            return EventOutcome::Handled;
        }
        self.handle_event(event, ctx, |control, ctx| control.event(event, ctx))
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let Some(refresh_path) = route.path.without_first_if(&ChildKey::new("refresh")) {
            return self
                .refresh
                .dispatch_event(&EventRoute::new(refresh_path), event, ctx);
        }
        self.handle_event(event, ctx, |control, ctx| {
            control.dispatch_event(route, event, ctx)
        })
    }
    fn tick(&mut self, dt: Duration, settings: tuicore::AnimationSettings) -> TickResult {
        self.control
            .tick(dt, settings)
            .merge(<Button<()> as TuiNode<()>>::tick(
                &mut self.refresh,
                dt,
                settings,
            ))
            .merge(if self.loading {
                Animated::tick(&mut self.spinner, dt, settings)
            } else {
                TickResult::IDLE
            })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(refresh_target) = target.for_child(&ChildKey::new("refresh")) {
            self.refresh.dispatch_focus(&refresh_target, focused, ctx);
            return;
        }
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
        rows.push(sprint_section_row(&section, sprint));
        rows.extend(sprint.work_items.iter().enumerate().map(|(index, item)| {
            work_item_row(
                item,
                &section,
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
    rows.extend(snapshot.work_items.iter().enumerate().map(|(index, item)| {
        work_item_row(
            item,
            "backlog",
            snapshot.story_points_configured,
            snapshot
                .runway
                .as_ref()
                .and_then(|runway| runway.tickets.get(index))
                .cloned(),
            None,
            index % 2 == 0,
        )
    }));
    rows
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
    let Some(capacity) = sprint.capacity.as_ref() else {
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
                    format!("{} pts", capacity_load_label(capacity)),
                    Style::default().fg(theme.text_fg()),
                ),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(coverage, coverage_style),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(
                    format!("{} items", sprint.work_items.len()),
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
                format!("{} items", snapshot.work_items.len()),
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
    show_story_points: bool,
    runway: Option<RunwayTicket>,
    assumed_ticket_size: Option<(f64, bool)>,
    alternate_background: bool,
) -> BacklogRow {
    let assumed_ticket_size = runway
        .as_ref()
        .filter(|runway| runway.assumed)
        .map(|runway| (runway.effective_points, runway.assumed_from_average))
        .or(assumed_ticket_size)
        .filter(|_| !item.kind.eq_ignore_ascii_case("bug"));
    BacklogRow {
        id: format!("ticket:{}", item.key),
        parent_id: Some(section_row_id(section)),
        content: BacklogRowContent::WorkItem(BacklogWorkItem {
            item: WorkItemRow {
                id: item.key.clone(),
                key: item.key.clone(),
                title: item.title.clone(),
                kind: work_item_kind(&item.kind),
                priority: item.priority.clone(),
                status: item.status.clone(),
                assignee: item.assignee.clone(),
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
            },
            runway,
            alternate_background,
            subtask_progress: item.subtask_progress.clone(),
            fix_versions: item.fix_versions.clone(),
            epic_name: item.epic_name.clone(),
        }),
    }
}
fn backlog_column() -> Column<BacklogRow, String> {
    Column::multiline(
        "backlog",
        "",
        Constraint::Percentage(100),
        |row: &BacklogRow, _: &CellContext<String>| match &row.content {
            BacklogRowContent::Section { title, .. } => title.clone(),
            BacklogRowContent::WorkItem(item) => backlog_work_item_text(item),
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
    .search_key(|row| match &row.content {
        BacklogRowContent::Section { search_text, .. } => search_text.clone(),
        BacklogRowContent::WorkItem(item) => format!("{} {}", item.item.key, item.item.title),
    })
}

fn backlog_work_item_text(row: &BacklogWorkItem) -> Text<'static> {
    let theme = tuicore::theme();
    let text_style = Style::default().fg(theme.text_fg());
    let muted_style = Style::default().fg(theme.muted_fg());
    let mut metadata = Vec::new();
    if row.item.show_story_points {
        let style = (row.item.story_points.is_some() && !row.item.story_points_estimated)
            .then_some(text_style)
            .unwrap_or(muted_style);
        metadata.push(Span::styled(story_points_label(&row.item), style));
    }
    append_metadata(&mut metadata, bubble_span(&row.item.assignee));
    if let Some(progress) = &row.subtask_progress {
        append_metadata(
            &mut metadata,
            Span::styled(
                format!("{}/{} ", progress.completed, progress.total),
                text_style,
            ),
        );
    }
    if !row.item.status.is_empty() {
        append_metadata(
            &mut metadata,
            Span::styled(row.item.status.clone(), text_style),
        );
    }
    if !row.fix_versions.is_empty() {
        append_metadata(
            &mut metadata,
            Span::styled(
                row.fix_versions.join(", "),
                Style::default()
                    .fg(theme.highlight_fg())
                    .bg(theme.highlight_bg()),
            ),
        );
    }
    if let Some(epic_name) = &row.epic_name {
        append_metadata(
            &mut metadata,
            Span::styled(epic_name.clone(), Style::default().fg(theme.accent_fg())),
        );
    }
    Text::from(vec![
        work_item_title_with_key_line(&row.item),
        Line::from(metadata),
    ])
}

fn append_metadata(metadata: &mut Vec<Span<'static>>, value: Span<'static>) {
    if !metadata.is_empty() {
        metadata.push(Span::raw(" • "));
    }
    metadata.push(value);
}

fn sprint_title(sprint: &Sprint) -> String {
    let icon = sprint_icon(&sprint.state);
    let date_range = sprint_date_range(sprint)
        .map(|range| format!(" • {range}"))
        .unwrap_or_default();
    let Some(capacity) = sprint.capacity.as_ref() else {
        return format!("{icon} {}{date_range}", sprint.name);
    };
    format!(
        "{icon} {}{date_range}\n{} {} pts • {} • {} items",
        sprint.name,
        sprint_capacity_icon(capacity.state),
        capacity_load_label(capacity),
        sprint_estimation_coverage(sprint).0,
        sprint.work_items.len(),
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

fn capacity_load_label(capacity: &crate::store::work_items::SprintCapacity) -> String {
    let prefix = matches!(capacity.source, RunwayCapacitySource::JiraVelocity)
        .then_some("~")
        .unwrap_or("");
    format!(
        "{prefix}{}/{}",
        points_label(capacity.effective_points),
        points_label(capacity.capacity)
    )
}

fn backlog_title(snapshot: &BacklogSnapshot) -> String {
    format!(" Backlog • {} items", snapshot.work_items.len())
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
