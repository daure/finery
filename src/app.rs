use std::{cell::Cell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dialog, DialogAction, DialogBackdrop, DialogHost, DialogLayer, EventCtx,
    EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusTarget, KeySpec, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, Tab, Tabs, TabsVariant,
    TickResult, ToastRack, TuiEvent, TuiNode,
};

use crate::{
    components::{self, settings_dialog::SettingsDialog},
    pages,
    service::AppService,
    store::composer::ChangeSet,
};

type SettingsHost = DialogHost<SettingsDialog, ()>;
type AppView = DialogLayer<Flex<()>, SettingsHost>;

pub(crate) struct App {
    view: AppView,
    open_settings: Rc<Cell<bool>>,
    close_dialog: Rc<Cell<bool>>,
    service: AppService,
    service_notifications: ToastRack,
}

pub(crate) fn root(service: AppService, change_sets: Vec<ChangeSet>) -> App {
    let settings = service.settings();
    let open_settings = Rc::new(Cell::new(false));
    let close_dialog = Rc::new(Cell::new(false));
    let pages = Tabs::new(vec![
        Tab::new("Backlog", pages::backlog::page(service.clone())),
        Tab::new("Sprint", pages::sprint::page()),
        Tab::new("Issues", pages::issues::page()),
        Tab::new(
            "Composer",
            pages::composer::page(change_sets, service.clone(), settings.clone()),
        ),
    ])
    .variant(TabsVariant::OneRow);
    let base = Flex::column()
        .child("pages", pages, FlexItem::fill(1))
        .child(
            "status",
            components::status_bar::status_bar(Rc::clone(&open_settings)),
            FlexItem::fixed(1),
        );
    let close_action = Rc::clone(&close_dialog);
    let close_event = Rc::clone(&close_dialog);
    let dialog = Dialog::new()
        .top_left("Settings")
        .actions([DialogAction::new("Close")
            .hotkey(KeySpec::plain('c'))
            .on_trigger(move || close_action.set(true))])
        .close_on_unfocus_from_descendants(true)
        .on_close(move |_| close_event.set(true))
        .host(SettingsDialog::new(settings, service.clone()));
    let view = DialogLayer::new(base, dialog)
        .active(false)
        .fit_content()
        .base_overlays_visible(true)
        .backdrop(DialogBackdrop::dim().amount(0.5));
    App {
        view,
        open_settings,
        close_dialog,
        service,
        service_notifications: ToastRack::new(),
    }
}

impl App {
    fn apply_dialog_signals(&mut self, ctx: &mut EventCtx<()>) {
        if self.open_settings.replace(false) {
            self.view.set_active_with_context(true, ctx);
        }
        if self.close_dialog.replace(false) {
            self.view.set_active_with_context(false, ctx);
        }
        if self.drain_service_notifications() {
            ctx.request_redraw();
            ctx.request_tick();
        }
    }

    fn drain_service_notifications(&mut self) -> bool {
        let errors = self.service.take_errors();
        let notifications = self.service.take_notifications();
        let has_notifications = !errors.is_empty() || !notifications.is_empty();
        for error in errors {
            self.service_notifications
                .push(tuicore::Notification::error(
                    "Background operation failed",
                    error,
                ));
        }
        for notification in notifications {
            self.service_notifications.push(notification);
        }
        has_notifications
    }
}

impl TuiNode for App {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.view.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.view.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.view.render(frame, area, ctx);
        self.service_notifications.render(frame, area);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.view.event(event, ctx);
        self.apply_dialog_signals(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.view.dispatch_event(route, event, ctx);
        self.apply_dialog_signals(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.view.dispatch_focus(target, focused, ctx);
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.view.focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let view_tick = self.view.tick(dt, settings);
        let notifications_added = self.drain_service_notifications();
        view_tick
            .merge(self.service_notifications.tick(dt, settings))
            .merge(if notifications_added {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.destroy(ctx);
    }
}

#[cfg(test)]
mod tests;
