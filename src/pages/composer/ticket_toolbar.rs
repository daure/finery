use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Button, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    MainAlign, MenuButton, MenuItem, RenderCtx, TickResult, TuiEvent, TuiNode,
};

use crate::app_settings::{ComposerKeyBinding, ComposerKeyBindings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolbarEvent {
    OpenSibling,
    OpenChild,
    AddSiblingNew,
    AddSiblingExisting,
    AddChildNew,
    AddChildExisting,
    Refresh,
    Commit,
}

pub(super) type ToolbarEvents = Rc<RefCell<Vec<ToolbarEvent>>>;

#[derive(Clone)]
pub(super) struct ToolbarFeedback {
    refresh: Rc<Cell<bool>>,
}

impl ToolbarFeedback {
    pub(super) fn new() -> Self {
        Self {
            refresh: Rc::new(Cell::new(false)),
        }
    }
    pub(super) fn request_refresh(&self) {
        self.refresh.set(true);
    }
}

pub(super) fn toolbar(
    events: ToolbarEvents,
    feedback: ToolbarFeedback,
    can_change: Rc<Cell<bool>>,
    can_add_child: Rc<Cell<bool>>,
    can_refresh: Rc<Cell<bool>>,
    can_commit: Rc<Cell<bool>>,
    keys: ComposerKeyBindings,
) -> Flex<()> {
    let sibling = ToolbarMenu::new(
        "Add sibling",
        &keys.add_sibling,
        Rc::clone(&events),
        ToolbarEvent::OpenSibling,
        ToolbarEvent::AddSiblingNew,
        ToolbarEvent::AddSiblingExisting,
        can_change,
    );
    let child = ToolbarMenu::new(
        "Add child",
        &keys.add_child,
        Rc::clone(&events),
        ToolbarEvent::OpenChild,
        ToolbarEvent::AddChildNew,
        ToolbarEvent::AddChildExisting,
        can_add_child,
    );
    let refresh_events = Rc::clone(&events);
    let commit_events = events;
    let actions = Flex::row()
        .gap(1)
        .child(
            "refresh",
            BoundButton::new(
                Button::new(format!("Refresh ({})", keys.refresh.label()))
                    .hotkey(keys.refresh.sequence())
                    .hotkey_focus_enabled(false)
                    .on_press(move || refresh_events.borrow_mut().push(ToolbarEvent::Refresh)),
                can_refresh,
                Rc::clone(&feedback.refresh),
            ),
            FlexItem::fit_content(),
        )
        .child(
            "commit",
            BoundButton::new(
                Button::new(format!("Commit ({})", keys.commit.label()))
                    .hotkey(keys.commit.sequence())
                    .hotkey_focus_enabled(false)
                    .on_press(move || commit_events.borrow_mut().push(ToolbarEvent::Commit)),
                can_commit,
                Rc::new(Cell::new(false)),
            ),
            FlexItem::fit_content(),
        );
    Flex::row()
        .justify(MainAlign::SpaceBetween)
        .child(
            "add",
            Flex::row()
                .gap(1)
                .child("sibling", sibling, FlexItem::fit_content())
                .child("child", child, FlexItem::fit_content()),
            FlexItem::fit_content(),
        )
        .child("actions", actions, FlexItem::fit_content())
}

struct ToolbarMenu {
    menu: MenuButton<&'static str>,
    events: ToolbarEvents,
    opened: ToolbarEvent,
    new: ToolbarEvent,
    existing: ToolbarEvent,
    enabled: Rc<Cell<bool>>,
}

impl ToolbarMenu {
    fn new(
        label: &str,
        key: &ComposerKeyBinding,
        events: ToolbarEvents,
        opened: ToolbarEvent,
        new: ToolbarEvent,
        existing: ToolbarEvent,
        enabled: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            menu: MenuButton::new(
                format!("{label} ({})", key.label()),
                [
                    MenuItem::new("new", "New"),
                    MenuItem::new("existing", "Existing"),
                ],
            )
            .hotkey(key.sequence())
            .disabled(!enabled.get())
            .visible_items(2),
            events,
            opened,
            new,
            existing,
            enabled,
        }
    }

    fn drain(&mut self, was_open: bool) {
        if !was_open && self.menu.is_open() {
            self.events.borrow_mut().push(self.opened);
        }
        for item in self.menu.take_activated() {
            self.events.borrow_mut().push(match item {
                "new" => self.new,
                _ => self.existing,
            });
        }
    }

    fn sync(&mut self) -> bool {
        let disabled = !self.enabled.get();
        if disabled == self.menu.is_disabled() {
            return false;
        }
        self.menu.set_disabled(disabled);
        true
    }

    fn handle(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
        dispatch: impl FnOnce(
            &mut MenuButton<&'static str>,
            &TuiEvent,
            &mut EventCtx<()>,
        ) -> EventOutcome,
    ) -> EventOutcome {
        self.sync();
        let was_open = self.menu.is_open();
        let outcome = dispatch(&mut self.menu, event, ctx);
        self.drain(was_open);
        outcome
    }
}

impl TuiNode for ToolbarMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.menu.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.menu.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.menu.render(frame, area, ctx)
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.handle(event, ctx, |menu, event, ctx| menu.event(event, ctx))
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.handle(event, ctx, |menu, event, ctx| {
            menu.dispatch_event(route, event, ctx)
        })
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.menu.tick(dt, settings).merge(if self.sync() {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        if target.is_some() || self.menu.is_open() {
            self.menu.focus(target, focused, ctx);
        }
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.menu.dispatch_focus(target, focused, ctx)
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.menu.init(ctx)
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.menu.mount(ctx)
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.menu.unmount(ctx)
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.menu.destroy(ctx)
    }
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
        self.button.render(frame, area)
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
        self.button.focus(target, focused, ctx)
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.button.dispatch_focus(target, focused, ctx)
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.init(ctx)
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.mount(ctx)
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.unmount(ctx)
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.button.destroy(ctx)
    }
}
