use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings,
};

use crate::store::composer::{Ticket, demo_jira_tickets};

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 10;
const MENU_FIELD_WIDTH: u16 = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AddChoice {
    New,
    Existing,
}

struct AddOption {
    choice: AddChoice,
    label: &'static str,
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
    choice: Dropdown<AddOption, AddChoice>,
    existing: Dropdown<Ticket, String>,
    selected_choices: Rc<RefCell<Vec<AddChoice>>>,
    selected_tickets: Rc<RefCell<Vec<String>>>,
    jira_tickets: Vec<Ticket>,
    events: Vec<AddTicketEvent>,
    mode: AddMenuMode,
    field_area: Rect,
}

impl AddTicketMenu {
    pub(super) fn new() -> Self {
        let selected_choices = Rc::new(RefCell::new(Vec::new()));
        let choice_sink = Rc::clone(&selected_choices);
        let choice = Dropdown::single(
            [
                AddOption {
                    choice: AddChoice::New,
                    label: "Add new",
                },
                AddOption {
                    choice: AddChoice::Existing,
                    label: "Add existing",
                },
            ],
            |option| option.choice,
            |option| option.label.into(),
        )
        .variant(DropdownVariant::Filled)
        .label("Add ticket")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(6)
        .on_select(move |ids| {
            if let Some(choice) = ids.first() {
                choice_sink.borrow_mut().push(*choice);
            }
        });

        let jira_tickets = demo_jira_tickets();
        let selected_tickets = Rc::new(RefCell::new(Vec::new()));
        let ticket_sink = Rc::clone(&selected_tickets);
        let existing = Dropdown::single(
            jira_tickets.clone(),
            |ticket: &Ticket| ticket.key.clone(),
            |ticket: &Ticket| format!("{} · {}", ticket.key, ticket.title),
        )
        .variant(DropdownVariant::Filled)
        .label("Add existing Jira ticket")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(8)
        .min_search_chars(2)
        .on_select(move |ids| {
            if let Some(id) = ids.first() {
                ticket_sink.borrow_mut().push(id.clone());
            }
        });

        Self {
            choice,
            existing,
            selected_choices,
            selected_tickets,
            jira_tickets,
            events: Vec::new(),
            mode: AddMenuMode::Choice,
            field_area: Rect::default(),
        }
    }

    pub(super) fn open(&mut self) {
        self.mode = AddMenuMode::Choice;
        self.choice.clear_selection();
        self.choice.open();
    }

    pub(super) fn take_events(&mut self) -> Vec<AddTicketEvent> {
        std::mem::take(&mut self.events)
    }

    fn active_dropdown(&self) -> &dyn TuiNode<()> {
        match self.mode {
            AddMenuMode::Choice => &self.choice,
            AddMenuMode::Existing => &self.existing,
        }
    }

    fn centered_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let hint = self
            .active_dropdown()
            .measure(LayoutProposal::at_most(width, area.height));
        let height = hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    fn drain_selections(&mut self, ctx: &mut EventCtx<()>) {
        for choice in self.selected_choices.borrow_mut().drain(..) {
            match choice {
                AddChoice::New => self.events.push(AddTicketEvent::CreateNew),
                AddChoice::Existing => {
                    self.mode = AddMenuMode::Existing;
                    self.existing.clear_selection();
                    self.existing.open_with_context(ctx);
                    ctx.request_layout();
                }
            }
        }
        for id in self.selected_tickets.borrow_mut().drain(..) {
            if let Some(ticket) = self.jira_tickets.iter().find(|ticket| ticket.key == id) {
                self.events.push(AddTicketEvent::Include(ticket.clone()));
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
        outcome
    }
}

impl TuiNode for AddTicketMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.field_area = self.centered_field_area(area);
        match self.mode {
            AddMenuMode::Choice => <Dropdown<AddOption, AddChoice> as TuiNode<()>>::layout(
                &mut self.choice,
                self.field_area,
                ctx,
            ),
            AddMenuMode::Existing => <Dropdown<Ticket, String> as TuiNode<()>>::layout(
                &mut self.existing,
                self.field_area,
                ctx,
            ),
        };
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        match self.mode {
            AddMenuMode::Choice => self.choice.render(frame, self.field_area, ctx),
            AddMenuMode::Existing => self.existing.render(frame, self.field_area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.event_outcome(event, ctx, |menu, event, ctx| match menu.mode {
            AddMenuMode::Choice => menu.choice.event(event, ctx),
            AddMenuMode::Existing => menu.existing.event(event, ctx),
        })
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.event_outcome(event, ctx, |menu, event, ctx| match menu.mode {
            AddMenuMode::Choice => menu.choice.dispatch_event(route, event, ctx),
            AddMenuMode::Existing => menu.existing.dispatch_event(route, event, ctx),
        })
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        match self.mode {
            AddMenuMode::Choice => self.choice.dispatch_focus(target, focused, ctx),
            AddMenuMode::Existing => self.existing.dispatch_focus(target, focused, ctx),
        }
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        match self.mode {
            AddMenuMode::Choice => self.choice.focus(target, focused, ctx),
            AddMenuMode::Existing => self.existing.focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<AddOption, AddChoice> as TuiNode<()>>::tick(&mut self.choice, dt, settings).merge(
            <Dropdown<Ticket, String> as TuiNode<()>>::tick(&mut self.existing, dt, settings),
        )
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.choice.init(ctx);
        self.existing.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.choice.mount(ctx);
        self.existing.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.existing.unmount(ctx);
        self.choice.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.existing.destroy(ctx);
        self.choice.destroy(ctx);
    }
}
