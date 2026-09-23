use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode,
};

use crate::service::{AppService, OpenCommandRequest};

const MENU_HOST_WIDTH: u16 = 69;
const MENU_HOST_HEIGHT: u16 = 18;
const MENU_FIELD_WIDTH: u16 = 54;

pub(crate) struct OpenCommandMenu {
    service: AppService,
    dropdown: Dropdown<String, String>,
    selected: Rc<RefCell<Vec<String>>>,
    key: Option<String>,
    title: Option<String>,
    area: Rect,
    close_requested: bool,
}

impl OpenCommandMenu {
    pub(crate) fn new(service: AppService) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_values = Rc::clone(&selected);
        let dropdown = Dropdown::single(
            Vec::<String>::new(),
            |value| value.clone(),
            |value| value.clone(),
        )
        .variant(DropdownVariant::Filled)
        .label("Open command")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(16)
        .on_select(move |values| selected_values.borrow_mut().extend(values));
        Self {
            service,
            dropdown,
            selected,
            key: None,
            title: None,
            area: Rect::default(),
            close_requested: false,
        }
    }

    pub(crate) fn open(&mut self, request: OpenCommandRequest, ctx: &mut EventCtx<()>) {
        self.key = Some(request.key);
        self.title = Some(request.title);
        self.close_requested = false;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown.set_rows(request.values);
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    pub(crate) fn take_close_requested(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    fn finish_event(&mut self, was_open: bool) {
        let selected = self.selected.borrow_mut().drain(..).next();
        if let (Some(key), Some(title), Some(value)) = (
            self.key.as_deref(),
            self.title.as_deref(),
            selected.as_deref(),
        ) {
            self.service.run_open_command_with_value(key, title, value);
            self.dropdown.close();
        }
        if was_open && !self.dropdown.is_open() {
            self.close_requested = true;
        }
    }
}

impl TuiNode for OpenCommandMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.area = Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(MENU_FIELD_WIDTH.min(area.width)) / 2),
            area.y.saturating_add(area.height.saturating_sub(3) / 2),
            MENU_FIELD_WIDTH.min(area.width),
            3.min(area.height),
        );
        <Dropdown<String, String> as TuiNode<()>>::layout(&mut self.dropdown, self.area, ctx);
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        self.finish_event(was_open);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.dispatch_event(route, event, ctx);
        self.finish_event(was_open);
        outcome
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<String, String> as TuiNode<()>>::tick(&mut self.dropdown, dt, settings)
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

#[cfg(test)]
#[path = "open_command_menu/tests.rs"]
mod tests;
