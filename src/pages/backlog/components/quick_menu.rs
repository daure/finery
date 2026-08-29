use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings,
};

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 10;
const MENU_FIELD_WIDTH: u16 = 36;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) enum BacklogQuickAction {
    MoveToTop {
        section_label: String,
        ticket_count: usize,
    },
    MoveToBottom {
        section_label: String,
        ticket_count: usize,
    },
    MoveToSection(BacklogDestination),
}

impl BacklogQuickAction {
    pub(in crate::pages::backlog) fn top_bottom(
        section_label: impl Into<String>,
        ticket_count: usize,
    ) -> [Self; 2] {
        let section_label = section_label.into();
        [
            Self::MoveToTop {
                section_label: section_label.clone(),
                ticket_count,
            },
            Self::MoveToBottom {
                section_label,
                ticket_count,
            },
        ]
    }

    pub(in crate::pages::backlog) fn label(&self) -> String {
        match self {
            Self::MoveToTop {
                section_label,
                ticket_count,
            } => move_label("top", section_label, *ticket_count),
            Self::MoveToBottom {
                section_label,
                ticket_count,
            } => move_label("bottom", section_label, *ticket_count),
            Self::MoveToSection(destination) => format!("Move tickets to {}", destination.label),
        }
    }
}

fn move_label(position: &str, section_label: &str, ticket_count: usize) -> String {
    let ticket = if ticket_count == 1 {
        "ticket"
    } else {
        "tickets"
    };
    format!("Move {ticket} to {position} of {section_label}")
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) struct BacklogDestination {
    pub section_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogQuickMenuEvent {
    MoveToTop {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    MoveToBottom {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    MoveToSection {
        source_section_id: String,
        destination: BacklogDestination,
        keys: Vec<String>,
    },
    MoveLocked,
    Closed,
}

pub(in crate::pages::backlog) struct BacklogQuickMenu {
    dropdown: Dropdown<BacklogQuickAction, BacklogQuickAction>,
    selected: Rc<RefCell<Vec<BacklogQuickAction>>>,
    keys: Vec<String>,
    section_id: Option<String>,
    source_order: Vec<String>,
    events: Vec<BacklogQuickMenuEvent>,
    field_area: Rect,
    move_locked: Rc<Cell<bool>>,
}

impl BacklogQuickMenu {
    pub(in crate::pages::backlog) fn new(move_locked: Rc<Cell<bool>>) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&selected);
        let dropdown = Dropdown::single(
            BacklogQuickAction::top_bottom("backlog", 0),
            |action| action.clone(),
            |action| action.label(),
        )
        .variant(DropdownVariant::Filled)
        .label("Backlog actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(8)
        .on_select(move |actions| {
            if let Some(action) = actions.first() {
                selected_actions.borrow_mut().push(action.clone());
            }
        });
        Self {
            dropdown,
            selected,
            keys: Vec::new(),
            section_id: None,
            source_order: Vec::new(),
            events: Vec::new(),
            field_area: Rect::default(),
            move_locked,
        }
    }

    pub(in crate::pages::backlog) fn open(
        &mut self,
        section_id: String,
        section_label: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        destinations: Vec<BacklogDestination>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if self.move_locked.get() {
            self.events.push(BacklogQuickMenuEvent::MoveLocked);
            return false;
        }
        self.section_id = Some(section_id);
        self.keys = keys;
        self.source_order = source_order;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown.set_rows(
            BacklogQuickAction::top_bottom(section_label, self.keys.len())
                .into_iter()
                .chain(
                    destinations
                        .into_iter()
                        .map(BacklogQuickAction::MoveToSection),
                ),
        );
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
        true
    }

    pub(in crate::pages::backlog) fn take_events(&mut self) -> Vec<BacklogQuickMenuEvent> {
        std::mem::take(&mut self.events)
    }

    fn centered_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let hint = <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::measure(
            &self.dropdown,
            LayoutProposal::at_most(width, area.height),
        );
        let height = hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    fn finish_event(&mut self, was_open: bool, outcome: EventOutcome) -> EventOutcome {
        if self.move_locked.get() {
            self.selected.borrow_mut().clear();
            self.events.push(BacklogQuickMenuEvent::MoveLocked);
            return outcome;
        }
        for action in self.selected.borrow_mut().drain(..) {
            let Some(section_id) = self.section_id.clone() else {
                continue;
            };
            let event = match action {
                BacklogQuickAction::MoveToTop { .. } => BacklogQuickMenuEvent::MoveToTop {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToBottom { .. } => BacklogQuickMenuEvent::MoveToBottom {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToSection(destination) => {
                    BacklogQuickMenuEvent::MoveToSection {
                        source_section_id: section_id,
                        destination,
                        keys: self.keys.clone(),
                    }
                }
            };
            self.events.push(event);
        }
        if was_open && !self.dropdown.is_open() && self.events.is_empty() {
            self.events.push(BacklogQuickMenuEvent::Closed);
        }
        outcome
    }
}

impl TuiNode for BacklogQuickMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.field_area = self.centered_field_area(area);
        <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::layout(
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
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            self.events.push(BacklogQuickMenuEvent::Closed);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        self.finish_event(was_open, outcome)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.dispatch_event(route, event, ctx);
        self.finish_event(was_open, outcome)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::tick(
            &mut self.dropdown,
            dt,
            settings,
        )
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.destroy(ctx);
    }
}
