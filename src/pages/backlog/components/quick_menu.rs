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

use crate::{components::avatar::initials, store::work_items::StatusTransition};

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 10;
const MENU_FIELD_WIDTH: u16 = 36;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) enum BacklogQuickAction {
    SetStatus(String),
    StatusLoading,
    SetStatusTo(StatusTransition),
    AssignUser(String),
    AssigneesLoading,
    AssignUserTo(BacklogAssignee),
    MoveToTop,
    MoveToBottom,
    MoveToSection(BacklogDestination),
    MoveToDestinationTop(BacklogDestination),
    MoveToDestinationBottom(BacklogDestination),
}

impl BacklogQuickAction {
    pub(in crate::pages::backlog) fn main_actions(status: &str, assignee: &str) -> [Self; 4] {
        [
            Self::AssignUser(assignee.to_owned()),
            Self::SetStatus(status.to_owned()),
            Self::MoveToTop,
            Self::MoveToBottom,
        ]
    }

    pub(in crate::pages::backlog) fn label(&self) -> String {
        match self {
            Self::SetStatus(status) if status.is_empty() => "Set status".into(),
            Self::SetStatus(status) => format!("Set status ({status})"),
            Self::StatusLoading => "Loading statuses…".into(),
            Self::SetStatusTo(status) => status.label.clone(),
            Self::AssignUser(assignee) => format!("Assign user (@{})", initials(assignee)),
            Self::AssigneesLoading => "Loading users…".into(),
            Self::AssignUserTo(assignee) => assignee.display_name.clone(),
            Self::MoveToTop => "Move to top".into(),
            Self::MoveToBottom => "Move to bottom".into(),
            Self::MoveToSection(destination) => format!("Move to {}", destination.label),
            Self::MoveToDestinationTop(destination) => format!("Top of {}", destination.label),
            Self::MoveToDestinationBottom(destination) => {
                format!("Bottom of {}", destination.label)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BacklogAssignee {
    pub account_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) struct BacklogDestination {
    pub section_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogQuickMenuEvent {
    LoadStatuses {
        keys: Vec<String>,
    },
    SetStatus {
        status: StatusTransition,
    },
    LoadAssignees,
    AssignUser {
        keys: Vec<String>,
        assignee: BacklogAssignee,
    },
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
        to_top: bool,
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
    #[cfg(test)]
    pub(in crate::pages::backlog) fn main_action_labels(
        status: &str,
        assignee: &str,
    ) -> Vec<String> {
        BacklogQuickAction::main_actions(status, assignee)
            .iter()
            .map(BacklogQuickAction::label)
            .collect()
    }

    pub(in crate::pages::backlog) fn new(move_locked: Rc<Cell<bool>>) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&selected);
        let dropdown = Dropdown::single(
            BacklogQuickAction::main_actions("", "Unassigned"),
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
        keys: Vec<String>,
        source_order: Vec<String>,
        status: String,
        assignee: String,
        destinations: Vec<BacklogDestination>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, true) {
            return false;
        }
        self.dropdown.set_rows(
            BacklogQuickAction::main_actions(&status, &assignee)
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

    pub(in crate::pages::backlog) fn open_status_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_statuses(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_assign_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_assignees(ctx);
        true
    }

    fn prepare_open(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        requires_move_unlocked: bool,
    ) -> bool {
        if requires_move_unlocked && self.move_locked.get() {
            self.events.push(BacklogQuickMenuEvent::MoveLocked);
            return false;
        }
        self.section_id = Some(section_id);
        self.keys = keys;
        self.source_order = source_order;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        true
    }

    pub(in crate::pages::backlog) fn set_statuses(&mut self, statuses: Vec<StatusTransition>) {
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows(statuses.into_iter().map(BacklogQuickAction::SetStatusTo));
        self.dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn set_assignees(&mut self, assignees: Vec<BacklogAssignee>) {
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows(assignees.into_iter().map(BacklogQuickAction::AssignUserTo));
        self.dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn take_events(&mut self) -> Vec<BacklogQuickMenuEvent> {
        std::mem::take(&mut self.events)
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn is_open_for_test(&self) -> bool {
        self.dropdown.is_open()
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

    fn finish_event(
        &mut self,
        was_open: bool,
        outcome: EventOutcome,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let selected = self.selected.borrow_mut().drain(..).collect::<Vec<_>>();
        for action in selected {
            let Some(section_id) = self.section_id.clone() else {
                continue;
            };
            let event = match action {
                BacklogQuickAction::SetStatus(_) => {
                    self.open_statuses(ctx);
                    continue;
                }
                BacklogQuickAction::StatusLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::SetStatusTo(status) => {
                    BacklogQuickMenuEvent::SetStatus { status }
                }
                BacklogQuickAction::AssignUser(_) => {
                    self.open_assignees(ctx);
                    continue;
                }
                BacklogQuickAction::AssigneesLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::AssignUserTo(assignee) => BacklogQuickMenuEvent::AssignUser {
                    keys: self.keys.clone(),
                    assignee,
                },
                BacklogQuickAction::MoveToTop => BacklogQuickMenuEvent::MoveToTop {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToBottom => BacklogQuickMenuEvent::MoveToBottom {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToSection(destination) => {
                    self.dropdown.clear_selection();
                    self.dropdown.set_rows([
                        BacklogQuickAction::MoveToDestinationTop(destination.clone()),
                        BacklogQuickAction::MoveToDestinationBottom(destination),
                    ]);
                    self.dropdown.set_search_query("");
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::MoveToDestinationTop(destination) => {
                    BacklogQuickMenuEvent::MoveToSection {
                        source_section_id: section_id,
                        destination,
                        keys: self.keys.clone(),
                        to_top: true,
                    }
                }
                BacklogQuickAction::MoveToDestinationBottom(destination) => {
                    BacklogQuickMenuEvent::MoveToSection {
                        source_section_id: section_id,
                        destination,
                        keys: self.keys.clone(),
                        to_top: false,
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

    fn open_statuses(&mut self, ctx: &mut EventCtx<()>) {
        self.dropdown.clear_selection();
        self.dropdown.set_rows([BacklogQuickAction::StatusLoading]);
        self.events.push(BacklogQuickMenuEvent::LoadStatuses {
            keys: self.keys.clone(),
        });
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn open_assignees(&mut self, ctx: &mut EventCtx<()>) {
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows([BacklogQuickAction::AssigneesLoading]);
        self.events.push(BacklogQuickMenuEvent::LoadAssignees);
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn close(&mut self, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.dropdown.close();
        self.selected.borrow_mut().clear();
        self.events.push(BacklogQuickMenuEvent::Closed);
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        EventOutcome::Handled
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
            return self.close(ctx);
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            return self.close(ctx);
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.dispatch_event(route, event, ctx);
        self.finish_event(was_open, outcome, ctx)
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
