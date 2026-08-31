use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
};
use tuicore::{
    AnimationSettings, AxisProposal, CellContext, ChildKey, Column, EventCtx, EventOutcome,
    EventRoute, FocusCtx, FocusId, FocusRequest, FocusTarget, Key, KeyEvent, KeyModifiers, KeySpec,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl,
    ListControlKeyBindings, RenderCtx, SearchMode, Spinner, TextInput, TickResult, TuiEvent,
    TuiNode, search_match,
};

use crate::{
    components::work_item_rows::{
        TICKET_MENU_WIDTH, TicketRowDetails, WorkItemKind, WorkItemRow, ticket_summary_text,
        work_item_title_prefix_width,
    },
    service::{AppService, RecentTickets},
    store::work_items::{SubtaskProgress, WorkItem},
};

const MENU_WIDTH: u16 = TICKET_MENU_WIDTH;
const MAX_VISIBLE_ROWS: u16 = 10;

enum RecentTicketsResult {
    Loaded {
        generation: u64,
        result: Result<RecentTickets, String>,
    },
}

#[derive(Clone)]
struct RecentTicketRow {
    item: WorkItemRow,
    epic_name: Option<String>,
    subtask_progress: Option<SubtaskProgress>,
    fix_versions: Vec<String>,
    alternate_background: bool,
}

pub(crate) struct RecentTicketsMenu {
    service: AppService,
    input: TextInput<()>,
    list: ListControl<RecentTicketRow, String>,
    query: Rc<RefCell<Option<String>>>,
    events: Vec<RecentTicketsMenuEvent>,
    sender: Sender<RecentTicketsResult>,
    receiver: Receiver<RecentTicketsResult>,
    generation: u64,
    loading: bool,
    spinner: Spinner,
    input_area: Rect,
    list_area: Rect,
}

pub(crate) enum RecentTicketsMenuEvent {
    OpenTicket(String),
    Closed,
}

impl RecentTicketsMenu {
    pub(crate) fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let query = Rc::new(RefCell::new(None));
        let query_changes = Rc::clone(&query);
        let input = TextInput::new()
            .placeholder("Search recent tickets…")
            .focused(true)
            .on_change(move |value| *query_changes.borrow_mut() = Some(value));
        Self {
            service,
            input,
            list: recent_ticket_list(),
            query,
            events: Vec::new(),
            sender,
            receiver,
            generation: 0,
            loading: false,
            spinner: Spinner::new(),
            input_area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    pub(crate) fn open(&mut self, ctx: &mut EventCtx<()>) {
        self.generation = self.generation.saturating_add(1);
        self.loading = true;
        self.events.clear();
        self.input.set_value("");
        self.input.set_insert_mode(true);
        self.input.move_cursor_to_end();
        self.query.borrow_mut().take();
        self.list.set_rows(Vec::new());
        self.list.data_view_mut().set_search_query("");
        self.list.data_view_mut().set_focused(true);
        ctx.focus(FocusRequest::Target(FocusId::new("input")));
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-recent-tickets".into())
            .spawn(move || {
                let _ = sender.send(RecentTicketsResult::Loaded {
                    generation,
                    result: service.load_recent_jira_tickets(),
                });
            })
        {
            self.loading = false;
            self.service
                .report_error(format!("Could not load recent Jira tickets: {error}"));
        }
        ctx.request_layout();
        ctx.request_redraw();
        ctx.request_tick();
    }

    pub(crate) fn take_events(&mut self) -> Vec<RecentTicketsMenuEvent> {
        std::mem::take(&mut self.events)
    }

    fn sync_query(&mut self) -> bool {
        let Some(query) = self.query.borrow_mut().take() else {
            return false;
        };
        self.list.data_view_mut().set_search_query(query);
        self.highlight_first_visible();
        true
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(RecentTicketsResult::Loaded { generation, result }) = self.receiver.try_recv()
        {
            if generation != self.generation {
                continue;
            }
            self.loading = false;
            match result {
                Ok(tickets) => {
                    let rows = tickets
                        .work_items
                        .into_iter()
                        .enumerate()
                        .map(|(index, ticket)| {
                            recent_ticket_row(
                                ticket,
                                tickets.story_points_configured,
                                tickets.assumed_story_points,
                                index % 2 == 0,
                            )
                        })
                        .collect::<Vec<_>>();
                    self.list.set_rows(rows);
                    self.highlight_first_visible();
                    self.input.set_focused(true);
                    self.input.set_insert_mode(true);
                    self.input.move_cursor_to_end();
                    self.list.data_view_mut().set_focused(true);
                }
                Err(error) => self
                    .service
                    .report_error(format!("Could not load recent Jira tickets: {error}")),
            }
            changed = true;
        }
        changed
    }

    fn open_highlighted_ticket(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::NONE).matches(*key))
        {
            return false;
        }
        let Some(key) = self.list.data_view().highlighted_id() else {
            return false;
        };
        self.events.push(RecentTicketsMenuEvent::OpenTicket(key));
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
        self.events.push(RecentTicketsMenuEvent::Closed);
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

    fn visible_row_count(&self) -> usize {
        let query = self.input.current_value().trim();
        self.list
            .items()
            .iter()
            .filter(|row| {
                search_match(
                    query,
                    &format!(
                        "{} {} {}",
                        row.item.key,
                        row.item.title,
                        row.epic_name.as_deref().unwrap_or_default()
                    ),
                    SearchMode::Contains,
                )
                .is_some()
            })
            .count()
    }

    fn highlight_first_visible(&mut self) {
        let query = self.input.current_value().trim();
        let first_ticket = self.list.items().iter().find_map(|row| {
            search_match(
                query,
                &format!(
                    "{} {} {}",
                    row.item.key,
                    row.item.title,
                    row.epic_name.as_deref().unwrap_or_default()
                ),
                SearchMode::Contains,
            )
            .is_some()
            .then(|| row.item.key.clone())
        });
        if let Some(first_ticket) = first_ticket {
            self.list.set_highlighted_id(&first_ticket);
        }
    }
}

fn recent_ticket_list() -> ListControl<RecentTicketRow, String> {
    let mut list = ListControl::new(
        Vec::new(),
        |row: &RecentTicketRow| row.item.key.clone(),
        |_, _| unreachable!(),
    )
    .headers(false)
    .columns(vec![recent_ticket_column()])
    .max_rows(usize::MAX)
    .panel_visible(false)
    .action_bar(false)
    .search_mode(SearchMode::Contains)
    .keybindings(
        ListControlKeyBindings::default()
            .add([])
            .add_child([])
            .edit([])
            .remove([]),
    )
    .empty_message("No results found.");
    list.data_view_mut().set_row_height(2);
    list.data_view_mut().set_wrap_cells(true);
    list.data_view_mut().set_row_style_by(|row| {
        row.alternate_background
            .then(|| Style::default().bg(tuicore::theme().surface_bg()))
    });
    list
}

fn recent_ticket_column() -> Column<RecentTicketRow, String> {
    Column::multiline(
        "recent-ticket",
        "",
        Constraint::Percentage(100),
        |row: &RecentTicketRow, _: &CellContext<String>| recent_ticket_text(row),
    )
    .constrained()
    .wrap_continuation_indent_by(|row| work_item_title_prefix_width(&row.item))
    .search_key(|row| {
        format!(
            "{} {} {}",
            row.item.key,
            row.item.title,
            row.epic_name.as_deref().unwrap_or_default()
        )
    })
}

fn recent_ticket_text(row: &RecentTicketRow) -> Text<'static> {
    ticket_summary_text(
        &row.item,
        None,
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

fn recent_ticket_row(
    ticket: WorkItem,
    story_points_configured: bool,
    assumed_story_points: f64,
    alternate_background: bool,
) -> RecentTicketRow {
    let missing_estimate = ticket.story_points.is_none();
    let estimated_story_points = matches!(
        work_item_kind(&ticket.kind),
        WorkItemKind::Story | WorkItemKind::Task
    )
    .then_some(assumed_story_points);
    RecentTicketRow {
        item: WorkItemRow {
            id: ticket.key.clone(),
            key: ticket.key,
            title: ticket.title,
            kind: work_item_kind(&ticket.kind),
            priority: ticket.priority,
            status: ticket.status,
            done: ticket.done,
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

#[cfg(test)]
mod tests;

impl TuiNode for RecentTicketsMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let preferred_height = if self.loading {
            1
        } else {
            let rows = self.visible_row_count().max(1).min(MAX_VISIBLE_ROWS.into());
            1 + u16::try_from(rows).unwrap_or(MAX_VISIBLE_ROWS) * 2
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
            let spinner_area = Rect::new(self.list_area.x, self.list_area.y, 1, 1);
            self.spinner.render(frame, spinner_area);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Loading recent tickets...",
                    Style::default().fg(tuicore::theme().text_fg()),
                ))),
                Rect::new(
                    self.list_area.x.saturating_add(2),
                    self.list_area.y,
                    self.list_area.width.saturating_sub(2),
                    1,
                ),
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
        if let Some(search_route) = route.path.without_first_if(&ChildKey::new("search")) {
            let outcome = self
                .input
                .dispatch_event(&EventRoute::new(search_route), event, ctx);
            if self.sync_query() {
                ctx.request_layout();
                ctx.request_redraw();
            }
            return outcome;
        }
        let outcome = self.input.event(event, ctx);
        if self.sync_query() {
            ctx.request_layout();
            ctx.request_redraw();
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
        let result = self
            .input
            .tick(dt, settings)
            .merge(<Spinner as TuiNode<()>>::tick(
                &mut self.spinner,
                dt,
                settings,
            ))
            .merge(self.list.tick(dt, settings));
        if self.drain_results() {
            result.merge(TickResult {
                changed: true,
                layout: true,
                active: false,
                next_tick: None,
            })
        } else if self.loading {
            result.merge(TickResult::scheduled_after(Duration::from_millis(50)))
        } else {
            result
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
        self.list.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
        self.list.mount(ctx);
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
