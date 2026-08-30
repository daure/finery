use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
};
use tuicore::{
    AnimationSettings, AxisProposal, CellContext, ChildKey, Column, EventCtx, EventOutcome,
    EventRoute, FocusCtx, FocusId, FocusTarget, Key, KeyEvent, KeyModifiers, KeySpec, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl,
    ListControlKeyBindings, RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    components::{
        avatar::bubble_span,
        work_item_rows::{
            WorkItemKind, WorkItemRow, append_release_chip, story_points_label,
            work_item_title_prefix_width, work_item_title_with_key_line_with_match,
        },
    },
    service::{AppService, RecentTickets},
    store::work_items::{SubtaskProgress, WorkItem},
};

const MENU_WIDTH: u16 = 56;
const MAX_VISIBLE_ROWS: u16 = 10;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

enum JiraSearchResult {
    Loaded {
        generation: u64,
        result: Result<RecentTickets, String>,
    },
}

#[derive(Clone)]
struct JiraSearchRow {
    item: WorkItemRow,
    epic_name: Option<String>,
    subtask_progress: Option<SubtaskProgress>,
    fix_versions: Vec<String>,
    match_query: String,
    alternate_background: bool,
}

pub(crate) struct JiraSearchMenu {
    service: AppService,
    input: TextInput<()>,
    list: ListControl<JiraSearchRow, String>,
    query: Rc<RefCell<Option<String>>>,
    events: Vec<JiraSearchMenuEvent>,
    sender: Sender<JiraSearchResult>,
    receiver: Receiver<JiraSearchResult>,
    generation: u64,
    last_query: String,
    pending_search: Option<Duration>,
    loading: bool,
    input_area: Rect,
    list_area: Rect,
}

pub(crate) enum JiraSearchMenuEvent {
    OpenTicket(String),
    Closed,
}

impl JiraSearchMenu {
    pub(crate) fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let query = Rc::new(RefCell::new(None));
        let query_changes = Rc::clone(&query);
        let input = TextInput::new()
            .placeholder("Search Jira tickets…")
            .focused(true)
            .on_change(move |value| *query_changes.borrow_mut() = Some(value));
        Self {
            service,
            input,
            list: jira_search_list(),
            query,
            events: Vec::new(),
            sender,
            receiver,
            generation: 0,
            last_query: String::new(),
            pending_search: None,
            loading: false,
            input_area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    pub(crate) fn open(&mut self, ctx: &mut EventCtx<()>) {
        self.events.clear();
        self.input.set_value("");
        self.query.borrow_mut().take();
        self.last_query.clear();
        self.pending_search = None;
        self.list.set_rows(Vec::new());
        self.list.data_view_mut().set_search_query("");
        self.list.data_view_mut().set_focused(true);
        self.start_search();
        ctx.request_layout();
        ctx.request_redraw();
        ctx.request_tick();
    }

    pub(crate) fn take_events(&mut self) -> Vec<JiraSearchMenuEvent> {
        std::mem::take(&mut self.events)
    }

    fn start_search(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.loading = true;
        let generation = self.generation;
        let query = self.last_query.clone();
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-search-{generation}"))
            .spawn(move || {
                let _ = sender.send(JiraSearchResult::Loaded {
                    generation,
                    result: service.search_jira_work_items(&query),
                });
            })
        {
            self.loading = false;
            self.service
                .report_error(format!("Could not start Jira search: {error}"));
        }
    }

    fn sync_query(&mut self) -> bool {
        let Some(query) = self.query.borrow_mut().take() else {
            return false;
        };
        if query == self.last_query {
            return false;
        }
        self.last_query = query;
        self.generation = self.generation.saturating_add(1);
        self.pending_search = Some(Duration::ZERO);
        self.loading = true;
        self.list.set_rows(Vec::new());
        self.list.data_view_mut().set_search_query("");
        true
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(JiraSearchResult::Loaded { generation, result }) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            self.loading = false;
            match result {
                Ok(tickets) => {
                    let match_query = self.last_query.clone();
                    self.list.set_rows(
                        tickets
                            .work_items
                            .into_iter()
                            .enumerate()
                            .map(|(index, ticket)| {
                                jira_search_row(
                                    ticket,
                                    tickets.story_points_configured,
                                    tickets.assumed_story_points,
                                    match_query.clone(),
                                    index % 2 == 0,
                                )
                            })
                            .collect::<Vec<_>>(),
                    );
                    self.highlight_first_ticket();
                    self.input.set_focused(true);
                    self.input.set_insert_mode(true);
                    self.input.move_cursor_to_end();
                    self.list.data_view_mut().set_focused(true);
                }
                Err(error) => self
                    .service
                    .report_error(format!("Jira search failed: {error}")),
            }
            changed = true;
        }
        changed
    }

    fn highlight_first_ticket(&mut self) {
        if let Some(key) = self.list.items().first().map(|row| row.item.key.clone()) {
            self.list.set_highlighted_id(&key);
        }
    }

    fn open_highlighted_ticket(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::NONE).matches(*key))
        {
            return false;
        }
        let Some(key) = self.list.data_view().highlighted_id() else {
            return false;
        };
        self.events.push(JiraSearchMenuEvent::OpenTicket(key));
        ctx.stop_propagation();
        true
    }

    fn close(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if !tuicore::keybindings().focus().unfocus_matches(*key)
            && !KeySpec::key_with_modifiers(Key::Char('['), KeyModifiers::CONTROL).matches(*key)
        {
            return false;
        }
        self.events.push(JiraSearchMenuEvent::Closed);
        ctx.stop_propagation();
        true
    }

    fn navigate_list(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> Option<EventOutcome> {
        let TuiEvent::Key(key) = event else {
            return None;
        };
        let bindings = tuicore::keybindings();
        if bindings.page_up_matches(*key)
            || bindings.page_down_matches(*key)
            || bindings.home_matches(*key)
            || bindings.end_matches(*key)
        {
            return Some(self.list.data_view_mut().event(event, ctx));
        }
        let navigation_key = if bindings.dropdown().next_matches(*key) {
            Some(Key::Char('j'))
        } else if bindings.dropdown().previous_matches(*key) {
            Some(Key::Char('k'))
        } else {
            None
        }?;
        Some(self.list.data_view_mut().event(
            &TuiEvent::Key(KeyEvent {
                code: navigation_key,
                modifiers: KeyModifiers::NONE,
            }),
            ctx,
        ))
    }
}

fn jira_search_list() -> ListControl<JiraSearchRow, String> {
    let mut list = ListControl::new(
        Vec::new(),
        |row: &JiraSearchRow| row.item.key.clone(),
        |_, _| unreachable!(),
    )
    .headers(false)
    .columns(vec![jira_search_column()])
    .max_rows(usize::MAX)
    .panel_visible(false)
    .action_bar(false)
    .keybindings(
        ListControlKeyBindings::default()
            .add([])
            .add_child([])
            .edit([])
            .remove([]),
    )
    .empty_message("No results found.");
    list.data_view_mut()
        .set_row_height_by(jira_search_row_height);
    list.data_view_mut().set_wrap_cells(true);
    list.data_view_mut().set_row_style_by(|row| {
        row.alternate_background
            .then(|| Style::default().bg(tuicore::theme().surface_bg()))
    });
    list
}

fn jira_search_row_height(row: &JiraSearchRow) -> u16 {
    let title_width = usize::from(MENU_WIDTH)
        .saturating_sub(work_item_title_prefix_width(&row.item))
        .saturating_sub(2)
        .max(1);
    let title_lines = row.item.title.chars().count().div_ceil(title_width).max(1);
    u16::try_from(title_lines.saturating_add(1)).unwrap_or(u16::MAX)
}

fn jira_search_column() -> Column<JiraSearchRow, String> {
    Column::multiline(
        "jira-search-ticket",
        "",
        Constraint::Percentage(100),
        |row: &JiraSearchRow, _: &CellContext<String>| jira_search_text(row),
    )
    .constrained()
    .wrap_continuation_indent_by(|row| work_item_title_prefix_width(&row.item))
}

fn jira_search_text(row: &JiraSearchRow) -> Text<'static> {
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
        append_release_chip(&mut metadata, &row.fix_versions);
    }
    if let Some(epic_name) = &row.epic_name {
        append_metadata(
            &mut metadata,
            Span::styled(epic_name.clone(), Style::default().fg(theme.accent_fg())),
        );
    }
    Text::from(vec![
        work_item_title_with_key_line_with_match(&row.item, None, Some(&row.match_query)),
        Line::from(metadata),
    ])
}

fn append_metadata(metadata: &mut Vec<Span<'static>>, value: Span<'static>) {
    if !metadata.is_empty() {
        metadata.push(Span::raw(" • "));
    }
    metadata.push(value);
}

fn jira_search_row(
    ticket: WorkItem,
    story_points_configured: bool,
    assumed_story_points: f64,
    match_query: String,
    alternate_background: bool,
) -> JiraSearchRow {
    let missing_estimate = ticket.story_points.is_none();
    let estimated_story_points = matches!(
        work_item_kind(&ticket.kind),
        WorkItemKind::Story | WorkItemKind::Task
    )
    .then_some(assumed_story_points);
    JiraSearchRow {
        item: WorkItemRow {
            id: ticket.key.clone(),
            key: ticket.key,
            title: ticket.title,
            kind: work_item_kind(&ticket.kind),
            priority: ticket.priority,
            status: ticket.status,
            assignee: ticket.assignee,
            story_points: ticket.story_points.or(estimated_story_points),
            show_story_points: story_points_configured,
            story_points_estimated: missing_estimate && estimated_story_points.is_some(),
            story_points_from_average: false,
            change_badge: None,
            submitted: false,
        },
        epic_name: ticket.epic_name,
        subtask_progress: ticket.subtask_progress,
        fix_versions: ticket.fix_versions,
        match_query,
        alternate_background,
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

impl TuiNode for JiraSearchMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let preferred_height = if self.loading {
            1
        } else {
            1 + self
                .list
                .items()
                .iter()
                .take(MAX_VISIBLE_ROWS.into())
                .map(jira_search_row_height)
                .sum::<u16>()
                .max(1)
        };
        let max_height = match proposal.height {
            AxisProposal::AtMost(height) | AxisProposal::Exact(height) => {
                height.saturating_mul(4) / 5
            }
            AxisProposal::Unbounded => preferred_height,
        };
        LayoutSizeHint::content(MENU_WIDTH, preferred_height.min(max_height)).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input_area = Rect::new(area.x, area.y, area.width, u16::from(!area.is_empty()));
        self.list_area = Rect::new(
            area.x,
            area.y.saturating_add(self.input_area.height),
            area.width,
            area.height.saturating_sub(self.input_area.height),
        );
        ctx.push_slot(ChildKey::new("search"), self.input_area, |ctx| {
            self.input.layout(self.input_area, ctx)
        });
        ctx.push_slot(ChildKey::new("list"), self.list_area, |ctx| {
            self.list.layout(self.list_area, ctx)
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default().style(Style::default().bg(tuicore::theme().surface_bg())),
            area,
        );
        self.input.render(frame, self.input_area);
        if self.loading {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Searching Jira…",
                    Style::default().fg(tuicore::theme().muted_fg()),
                )))
                .alignment(Alignment::Right),
                self.input_area,
            );
        } else {
            self.list.render(frame, self.list_area, ctx);
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.close(event, ctx) || self.open_highlighted_ticket(event, ctx) {
            return EventOutcome::Handled;
        }
        if let Some(outcome) = self.navigate_list(event, ctx) {
            return outcome;
        }
        let outcome = self.input.event(event, ctx);
        if self.sync_query() {
            ctx.request_layout();
            ctx.request_redraw();
            ctx.request_tick();
        }
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.close(event, ctx) || self.open_highlighted_ticket(event, ctx) {
            return EventOutcome::Handled;
        }
        if let Some(outcome) = self.navigate_list(event, ctx) {
            return outcome;
        }
        let outcome =
            if let Some(search_route) = route.path.without_first_if(&ChildKey::new("search")) {
                self.input
                    .dispatch_event(&EventRoute::new(search_route), event, ctx)
            } else {
                self.input.event(event, ctx)
            };
        if self.sync_query() {
            ctx.request_layout();
            ctx.request_redraw();
            ctx.request_tick();
        }
        outcome
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
        self.list.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(search_target) = target.for_child(&ChildKey::new("search")) {
            self.input.dispatch_focus(&search_target, focused, ctx);
            return;
        }
        if let Some(list_target) = target.for_child(&ChildKey::new("list")) {
            self.list.dispatch_focus(&list_target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut changed = self.drain_results();
        if let Some(elapsed) = &mut self.pending_search {
            *elapsed += dt;
            if *elapsed >= SEARCH_DEBOUNCE {
                self.pending_search = None;
                self.start_search();
                changed = true;
            }
        }
        self.input
            .tick(dt, settings)
            .merge(self.list.tick(dt, settings))
            .merge(if changed {
                TickResult {
                    changed: true,
                    layout: true,
                    active: false,
                    next_tick: None,
                }
            } else {
                TickResult::IDLE
            })
            .merge(TickResult::scheduled_after(POLL_INTERVAL))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
        self.list.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
        self.list.mount(ctx);
        ctx.request_tick();
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
        self.list.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
        self.list.destroy(ctx);
    }
}

#[cfg(test)]
mod tests;
