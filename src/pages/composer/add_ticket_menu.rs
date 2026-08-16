use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings,
};

use crate::{service::AppService, store::composer::Ticket};

const CHOICE_HOST_WIDTH: u16 = 52;
const CHOICE_FIELD_WIDTH: u16 = 46;
const EXISTING_WIDTH: u16 = 120;
const MENU_HOST_HEIGHT: u16 = 12;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AddItemId {
    New,
    Existing,
    Ticket(String),
}

#[derive(Clone)]
enum AddItem {
    New,
    Existing,
    Ticket(Ticket),
}

impl AddItem {
    fn id(&self) -> AddItemId {
        match self {
            Self::New => AddItemId::New,
            Self::Existing => AddItemId::Existing,
            Self::Ticket(ticket) => AddItemId::Ticket(ticket.key.clone()),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::New => "Add new".into(),
            Self::Existing => "Add existing".into(),
            Self::Ticket(ticket) => format!("{} · {}", ticket.key, ticket.title),
        }
    }
}

#[derive(Clone)]
pub(super) enum AddTicketEvent {
    CreateNew,
    Include(Ticket),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddMenuMode {
    Choice,
    Existing,
}

pub(super) struct AddTicketMenu {
    service: AppService,
    dropdown: Dropdown<AddItem, AddItemId>,
    selected: Rc<RefCell<Vec<AddItemId>>>,
    tickets: Vec<Ticket>,
    events: Vec<AddTicketEvent>,
    mode: AddMenuMode,
    last_query: String,
    pending_search: Option<Duration>,
    generation: u64,
    sender: Sender<(u64, Result<Vec<Ticket>, String>)>,
    receiver: Receiver<(u64, Result<Vec<Ticket>, String>)>,
    field_area: Rect,
}

impl AddTicketMenu {
    pub(super) fn new(service: AppService) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_sink = Rc::clone(&selected);
        let dropdown = Dropdown::single(choice_items(), AddItem::id, AddItem::label)
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
            .max_popup_height(10)
            .on_select(move |ids| {
                if let Some(id) = ids.first() {
                    selected_sink.borrow_mut().push(id.clone());
                }
            });
        let (sender, receiver) = mpsc::channel();
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
            field_area: Rect::default(),
        }
    }

    pub(super) fn open(&mut self) {
        self.mode = AddMenuMode::Choice;
        self.pending_search = None;
        self.dropdown.set_search_mode(DropdownSearchMode::Fuzzy);
        self.dropdown.set_rows(choice_items());
        self.dropdown.clear_selection();
        self.dropdown.set_search_query("");
        self.dropdown.open();
    }

    pub(super) fn take_events(&mut self) -> Vec<AddTicketEvent> {
        std::mem::take(&mut self.events)
    }

    fn open_existing(&mut self, ctx: &mut EventCtx<()>) {
        self.mode = AddMenuMode::Existing;
        self.pending_search = None;
        self.dropdown.set_search_mode(DropdownSearchMode::External);
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
                let result = service.search_jira(&query);
                let _ = sender.send((generation, result));
            })
        {
            self.dropdown.set_external_loading(false);
            self.service
                .report_error(format!("could not start Jira search: {error}"));
        }
    }

    fn apply_search_result(&mut self, result: Result<Vec<Ticket>, String>) {
        self.dropdown.set_external_loading(false);
        match result {
            Ok(tickets) => {
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

    fn drain_selections(&mut self, ctx: &mut EventCtx<()>) {
        let selections = self.selected.borrow_mut().drain(..).collect::<Vec<_>>();
        for selection in selections {
            match selection {
                AddItemId::New => self.events.push(AddTicketEvent::CreateNew),
                AddItemId::Existing => self.open_existing(ctx),
                AddItemId::Ticket(id) => {
                    if let Some(ticket) = self.tickets.iter().find(|ticket| ticket.key == id) {
                        self.events.push(AddTicketEvent::Include(ticket.clone()));
                    }
                }
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
        outcome
    }
}

fn choice_items() -> [AddItem; 2] {
    [AddItem::New, AddItem::Existing]
}

impl TuiNode for AddTicketMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let width = match self.mode {
            AddMenuMode::Choice => CHOICE_HOST_WIDTH,
            AddMenuMode::Existing => EXISTING_WIDTH,
        };
        LayoutSizeHint::content(width, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let requested_width = match self.mode {
            AddMenuMode::Choice => CHOICE_FIELD_WIDTH,
            AddMenuMode::Existing => EXISTING_WIDTH,
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
