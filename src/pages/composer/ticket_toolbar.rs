use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Button, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    MainAlign, RenderCtx, TickResult, TuiEvent, TuiNode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolbarEvent {
    NewTicket,
    Refresh,
    Submit,
}

pub(super) type ToolbarEvents = Rc<RefCell<Vec<ToolbarEvent>>>;

#[derive(Clone)]
pub(super) struct ToolbarFeedback {
    refresh: Rc<Cell<bool>>,
    submit: Rc<Cell<bool>>,
}

impl ToolbarFeedback {
    pub(super) fn new() -> Self {
        Self {
            refresh: Rc::new(Cell::new(false)),
            submit: Rc::new(Cell::new(false)),
        }
    }

    pub(super) fn request_refresh(&self) {
        self.refresh.set(true);
    }

    pub(super) fn request_submit(&self) {
        self.submit.set(true);
    }
}

pub(super) fn toolbar(
    events: ToolbarEvents,
    feedback: ToolbarFeedback,
    can_change: Rc<Cell<bool>>,
    can_refresh: Rc<Cell<bool>>,
    can_submit: Rc<Cell<bool>>,
) -> Flex<()> {
    let new_events = Rc::clone(&events);
    let submit_events = events;
    let refresh_events = Rc::clone(&submit_events);
    let actions = Flex::row()
        .gap(1)
        .child(
            "refresh",
            BoundButton::new(
                Button::new("Refresh")
                    .hotkey("shift+r")
                    .hotkey_focus_enabled(false)
                    .on_press(move || refresh_events.borrow_mut().push(ToolbarEvent::Refresh)),
                can_refresh,
                Rc::clone(&feedback.refresh),
            ),
            FlexItem::fit_content(),
        )
        .child(
            "submit",
            BoundButton::new(
                Button::new("Submit")
                    .hotkey("shift+s")
                    .hotkey_focus_enabled(false)
                    .on_press(move || submit_events.borrow_mut().push(ToolbarEvent::Submit)),
                can_submit,
                Rc::clone(&feedback.submit),
            ),
            FlexItem::fit_content(),
        );
    Flex::row()
        .justify(MainAlign::SpaceBetween)
        .child(
            "new-ticket",
            BoundButton::new(
                Button::new("New ticket")
                    .hotkey("shift+n")
                    .on_press(move || new_events.borrow_mut().push(ToolbarEvent::NewTicket)),
                can_change,
                Rc::new(Cell::new(false)),
            ),
            FlexItem::fit_content(),
        )
        .child("actions", actions, FlexItem::fit_content())
}

struct BoundButton {
    button: Button<()>,
    enabled: Rc<Cell<bool>>,
    feedback_requested: Rc<Cell<bool>>,
}

impl BoundButton {
    fn new(
        button: Button<()>,
        enabled: Rc<Cell<bool>>,
        feedback_requested: Rc<Cell<bool>>,
    ) -> Self {
        let mut button = button;
        button.set_disabled(!enabled.get());
        Self {
            button,
            enabled,
            feedback_requested,
        }
    }

    fn sync(&mut self) -> bool {
        let disabled = !self.enabled.get();
        let changed = disabled != self.button.is_disabled();
        self.button.set_disabled(disabled);
        changed
    }
}

impl TuiNode for BoundButton {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.button.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.button.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        self.button.render(frame, area);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.button.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.button.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let feedback = if self.feedback_requested.replace(false) {
            self.button.press(settings);
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        };
        self.button
            .tick(dt, settings)
            .merge(feedback)
            .merge(if self.sync() {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.button.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.button.dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.destroy(ctx);
    }
}
