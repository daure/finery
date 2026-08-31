use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect, text::Text};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings,
};

use crate::{
    components::work_item_rows::{
        TICKET_MENU_WIDTH, TicketRowDetails, WorkItemKind, WorkItemRow, ticket_menu_max_height,
        ticket_summary_text, work_item_title_prefix_width_for,
    },
    jira::JiraProject,
    service::{AppService, ComposerSearchTicket},
    store::composer::{PlacementTarget, TicketKind},
};

const CHOICE_HOST_WIDTH: u16 = 52;
const CHOICE_FIELD_WIDTH: u16 = 46;
const EXISTING_WIDTH: u16 = TICKET_MENU_WIDTH;
const MENU_HOST_HEIGHT: u16 = 12;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AddItemId {
    New,
    Existing,
    Ticket(String),
    Project(String),
}

#[derive(Clone)]
enum AddItem {
    New,
    Existing,
    Ticket(ComposerSearchTicket),
    Project(JiraProject),
}

impl AddItem {
    fn id(&self) -> AddItemId {
        match self {
            Self::New => AddItemId::New,
            Self::Existing => AddItemId::Existing,
            Self::Ticket(ticket) => AddItemId::Ticket(ticket.ticket.key.clone()),
            Self::Project(project) => AddItemId::Project(project.key.clone()),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::New => "Add new".into(),
            Self::Existing => "Add existing".into(),
            Self::Ticket(ticket) => format!("{} · {}", ticket.ticket.key, ticket.ticket.title),
            Self::Project(project) => format!("{} · {}", project.key, project.name),
        }
    }
}

#[derive(Clone)]
pub(super) enum AddTicketEvent {
    CreateNew {
        project_key: String,
        placement: PlacementTarget,
    },
    Include {
        ticket: ComposerSearchTicket,
        placement: PlacementTarget,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddMenuMode {
    Choice,
    Existing,
    Projects,
}

pub(super) struct AddTicketMenu {
    service: AppService,
    dropdown: Dropdown<AddItem, AddItemId>,
    selected: Rc<RefCell<Vec<AddItemId>>>,
    tickets: Vec<ComposerSearchTicket>,
    events: Vec<AddTicketEvent>,
    mode: AddMenuMode,
    last_query: String,
    pending_search: Option<Duration>,
    generation: u64,
    sender: Sender<(u64, Result<Vec<ComposerSearchTicket>, String>)>,
    receiver: Receiver<(u64, Result<Vec<ComposerSearchTicket>, String>)>,
    project_sender: Sender<(u64, Result<Vec<JiraProject>, String>)>,
    project_receiver: Receiver<(u64, Result<Vec<JiraProject>, String>)>,
    project_hint: Option<String>,
    placement: PlacementTarget,
    legal_kinds: Vec<TicketKind>,
    field_area: Rect,
}

impl AddTicketMenu {
    pub(super) fn new(service: AppService) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_sink = Rc::clone(&selected);
        let mut dropdown =
            Dropdown::single_rich(choice_items(), AddItem::id, AddItem::label, add_item_text)
                .variant(DropdownVariant::Filled)
                .label("Add ticket")
                .label_position(DropdownLabelPosition::Inline)
                .search_mode(DropdownSearchMode::Fuzzy)
                .external_loading_message("Searching Jira")
                .commit_mode(DropdownCommitMode::Explicit)
                .centered(true)
                .show_field_when_open(false)
                .backdrop_amount(0.0)
                .tab_stop(false)
                .on_select(move |ids| {
                    if let Some(id) = ids.first() {
                        selected_sink.borrow_mut().push(id.clone());
                    }
                });
        dropdown.set_wrap_cells(true);
        let dropdown = dropdown.wrap_continuation_indent_by(add_item_title_prefix_width);
        let (sender, receiver) = mpsc::channel();
        let (project_sender, project_receiver) = mpsc::channel();
        Self {
            service,
            dropdown,
            selected,
            tickets: Vec::new(),
            events: Vec::new(),
            mode: AddMenuMode::Choice,
            last_query: String::new(),
            pending_search: None,
            generation: 0,
            sender,
            receiver,
            project_sender,
            project_receiver,
            project_hint: None,
            placement: PlacementTarget::Root,
            legal_kinds: Vec::new(),
            field_area: Rect::default(),
        }
    }

    pub(super) fn open_existing(
        &mut self,
        project_hint: Option<String>,
        placement: PlacementTarget,
        legal_kinds: Vec<TicketKind>,
        ctx: &mut EventCtx<()>,
    ) {
        self.placement = placement;
        self.legal_kinds = legal_kinds;
        self.project_hint = project_hint;
        self.open_existing_search(ctx);
    }

    pub(super) fn open_new_project_selector(
        &mut self,
        placement: PlacementTarget,
        legal_kinds: Vec<TicketKind>,
        ctx: &mut EventCtx<()>,
    ) {
        self.placement = placement;
        self.legal_kinds = legal_kinds;
        self.open_projects(ctx);
    }

    pub(super) fn open_projects(&mut self, ctx: &mut EventCtx<()>) {
        self.mode = AddMenuMode::Projects;
        self.dropdown.set_search_mode(DropdownSearchMode::Fuzzy);
        self.dropdown.set_row_height(1);
        self.dropdown.set_wrap_cells(false);
        self.dropdown.set_rows([]);
        self.dropdown.clear_selection();
        self.dropdown.set_search_query("");
        self.dropdown.set_external_loading(true);
        self.dropdown.open_with_context(ctx);
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.project_sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-projects-{generation}"))
            .spawn(move || {
                let _ = sender.send((generation, service.jira_projects()));
            })
        {
            self.dropdown.set_external_loading(false);
            self.service
                .report_error(format!("could not load Jira projects: {error}"));
        }
    }

    pub(super) fn take_events(&mut self) -> Vec<AddTicketEvent> {
        std::mem::take(&mut self.events)
    }

    fn open_existing_search(&mut self, ctx: &mut EventCtx<()>) {
        self.mode = AddMenuMode::Existing;
        self.pending_search = None;
        self.dropdown.set_search_mode(DropdownSearchMode::External);
        self.dropdown.set_row_height(2);
        self.dropdown.set_wrap_cells(true);
        self.tickets.clear();
        self.dropdown.set_rows([]);
        self.dropdown.clear_selection();
        self.dropdown.set_search_query("");
        self.last_query.clear();
        self.dropdown.open_with_context(ctx);
        self.start_search();
    }

    fn start_search(&mut self) {
        self.dropdown.set_external_loading(true);
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let query = self.last_query.clone();
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-search-{generation}"))
            .spawn(move || {
                let result = service.search_jira_for_composer(&query);
                let _ = sender.send((generation, result));
            })
        {
            self.dropdown.set_external_loading(false);
            self.service
                .report_error(format!("could not start Jira search: {error}"));
        }
    }

    fn apply_search_result(&mut self, result: Result<Vec<ComposerSearchTicket>, String>) {
        self.dropdown.set_external_loading(false);
        match result {
            Ok(tickets) => {
                let tickets = tickets
                    .into_iter()
                    .filter(|ticket| self.legal_kinds.contains(&ticket.ticket.kind))
                    .collect::<Vec<_>>();
                self.tickets = tickets.clone();
                self.dropdown
                    .set_rows(tickets.into_iter().map(AddItem::Ticket));
            }
            Err(error) => {
                self.tickets.clear();
                self.dropdown.set_rows([]);
                self.service
                    .report_error(format!("Jira search failed: {error}"));
            }
        }
    }

    fn drain_search(&mut self) -> bool {
        if self.mode != AddMenuMode::Existing {
            return false;
        }
        let query = self.dropdown.search_query().to_owned();
        let mut changed = false;
        if query != self.last_query {
            self.last_query = query;
            self.generation = self.generation.saturating_add(1);
            self.pending_search = Some(Duration::ZERO);
            self.tickets.clear();
            self.dropdown.set_rows([]);
            self.dropdown.set_external_loading(true);
            changed = true;
        }
        while let Ok((generation, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            self.apply_search_result(result);
            changed = true;
        }
        changed
    }

    fn drain_projects(&mut self) -> bool {
        if self.mode != AddMenuMode::Projects {
            return false;
        }
        let mut changed = false;
        while let Ok((generation, result)) = self.project_receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            self.dropdown.set_external_loading(false);
            match result {
                Ok(projects) => self
                    .dropdown
                    .set_rows(projects.into_iter().map(AddItem::Project)),
                Err(error) => {
                    self.dropdown.set_rows([]);
                    self.service
                        .report_error(format!("Jira project load failed: {error}"));
                }
            }
            changed = true;
        }
        changed
    }

    fn drain_selections(&mut self, ctx: &mut EventCtx<()>) {
        let selections = self.selected.borrow_mut().drain(..).collect::<Vec<_>>();
        for selection in selections {
            match selection {
                AddItemId::New => {
                    if let Some(project) = self.project_hint.clone() {
                        self.events.push(AddTicketEvent::CreateNew {
                            project_key: project,
                            placement: self.placement.clone(),
                        });
                    } else {
                        self.open_projects(ctx);
                    }
                }
                AddItemId::Existing => self.open_existing_search(ctx),
                AddItemId::Ticket(id) => {
                    if let Some(ticket) = self.tickets.iter().find(|ticket| ticket.ticket.key == id)
                    {
                        self.events.push(AddTicketEvent::Include {
                            ticket: ticket.clone(),
                            placement: self.placement.clone(),
                        });
                    }
                }
                AddItemId::Project(key) => self.events.push(AddTicketEvent::CreateNew {
                    project_key: key,
                    placement: self.placement.clone(),
                }),
            }
        }
    }

    fn event_outcome(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
        dispatch: impl FnOnce(&mut Self, &TuiEvent, &mut EventCtx<()>) -> EventOutcome,
    ) -> EventOutcome {
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            self.events.push(AddTicketEvent::Closed);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let outcome = dispatch(self, event, ctx);
        self.drain_selections(ctx);
        self.drain_search();
        self.drain_projects();
        outcome
    }
}

fn add_item_title_prefix_width(item: &AddItem) -> usize {
    let AddItem::Ticket(ticket) = item else {
        return 0;
    };
    work_item_title_prefix_width_for(
        ticket_kind(ticket.ticket.kind),
        &ticket.work_item.priority,
        &ticket.work_item.key,
    )
}

fn choice_items() -> [AddItem; 2] {
    [AddItem::New, AddItem::Existing]
}

fn add_item_text(item: &AddItem, query: &str, _mode: DropdownSearchMode) -> Text<'static> {
    let AddItem::Ticket(ticket) = item else {
        return Text::raw(item.label());
    };
    let estimated_story_points = matches!(ticket.ticket.kind, TicketKind::Story | TicketKind::Task)
        .then_some(ticket.assumed_story_points);
    let row = WorkItemRow {
        id: ticket.work_item.key.clone(),
        key: ticket.work_item.key.clone(),
        title: ticket.work_item.title.clone(),
        kind: ticket_kind(ticket.ticket.kind),
        priority: ticket.work_item.priority.clone(),
        status: ticket.work_item.status.clone(),
        done: ticket.work_item.done,
        assignee: ticket.work_item.assignee.clone(),
        labels: ticket.work_item.labels.clone(),
        story_points: ticket.work_item.story_points.or(estimated_story_points),
        show_story_points: ticket.story_points_configured,
        story_points_estimated: ticket.work_item.story_points.is_none()
            && estimated_story_points.is_some(),
        story_points_from_average: false,
        change_badge: None,
        submitted: false,
    };
    ticket_summary_text(
        &row,
        None,
        (!query.is_empty()).then_some(query),
        TicketRowDetails {
            subtask_progress: ticket
                .work_item
                .subtask_progress
                .as_ref()
                .map(|progress| (progress.completed, progress.total)),
            fix_versions: &ticket.work_item.fix_versions,
            epic_name: ticket.work_item.epic_name.as_deref(),
            annotation: None,
        },
    )
}

fn ticket_kind(kind: TicketKind) -> WorkItemKind {
    match kind {
        TicketKind::Epic => WorkItemKind::Epic,
        TicketKind::Story => WorkItemKind::Story,
        TicketKind::Task => WorkItemKind::Task,
        TicketKind::Bug => WorkItemKind::Bug,
        TicketKind::Subtask => WorkItemKind::Subtask,
    }
}

impl TuiNode for AddTicketMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let width = match self.mode {
            AddMenuMode::Choice => CHOICE_HOST_WIDTH,
            AddMenuMode::Existing => EXISTING_WIDTH,
            AddMenuMode::Projects => CHOICE_HOST_WIDTH,
        };
        LayoutSizeHint::content(width, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let overlay_bounds = ctx.overlay_bounds();
        let viewport_height = if overlay_bounds.is_empty() {
            area.height
        } else {
            overlay_bounds.height
        };
        self.dropdown
            .set_max_popup_height(ticket_menu_max_height(viewport_height));
        let requested_width = match self.mode {
            AddMenuMode::Choice => CHOICE_FIELD_WIDTH,
            AddMenuMode::Existing => EXISTING_WIDTH,
            AddMenuMode::Projects => CHOICE_FIELD_WIDTH,
        };
        let width = requested_width.min(area.width);
        let height = MENU_HOST_HEIGHT.min(area.height);
        self.field_area = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        );
        <Dropdown<AddItem, AddItemId> as TuiNode<()>>::layout(
            &mut self.dropdown,
            self.field_area,
            ctx,
        );
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.field_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.event_outcome(event, ctx, |menu, event, ctx| {
            menu.dropdown.event(event, ctx)
        })
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.event_outcome(event, ctx, |menu, event, ctx| {
            menu.dropdown.dispatch_event(route, event, ctx)
        })
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut changed = self.drain_search();
        changed |= self.drain_projects();
        if let Some(elapsed) = &mut self.pending_search {
            *elapsed += dt;
            if *elapsed >= SEARCH_DEBOUNCE {
                self.pending_search = None;
                self.start_search();
                changed = true;
            }
        }
        <Dropdown<AddItem, AddItemId> as TuiNode<()>>::tick(&mut self.dropdown, dt, settings)
            .merge(if changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
            .merge(TickResult::scheduled_after(POLL_INTERVAL))
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.mount(ctx);
        ctx.request_tick();
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.destroy(ctx);
    }
}

#[cfg(test)]
mod tests;
