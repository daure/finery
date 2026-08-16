use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult,
    TuiEvent, TuiNode,
};

use crate::{
    app_settings::AppSettings,
    service::AppService,
    store::composer::{ChangeSet, ComposerState},
};

use super::{change_set_list::ChangeSetListView, ticket_editor::TicketEditor};

pub(crate) fn page(
    change_sets: Vec<ChangeSet>,
    service: AppService,
    settings: Arc<RwLock<AppSettings>>,
) -> ComposerPage {
    ComposerPage::new(change_sets, service, settings)
}

pub(crate) struct ComposerPage {
    state: Rc<RefCell<ComposerState>>,
    change_sets: ChangeSetListView,
    editor: TicketEditor,
}

impl ComposerPage {
    pub(super) fn new(
        change_sets: Vec<ChangeSet>,
        service: AppService,
        settings: Arc<RwLock<AppSettings>>,
    ) -> Self {
        let state = Rc::new(RefCell::new(ComposerState::from_change_sets(change_sets)));
        Self {
            change_sets: ChangeSetListView::new(Rc::clone(&state), service.clone()),
            editor: TicketEditor::new(Rc::clone(&state), settings, service),
            state,
        }
    }

    fn in_change_set(&self) -> bool {
        self.state.borrow().active_change_set.is_some()
    }

    fn active(&self) -> &dyn TuiNode<()> {
        if self.in_change_set() {
            &self.editor
        } else {
            &self.change_sets
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        if self.in_change_set() {
            &mut self.editor
        } else {
            &mut self.change_sets
        }
    }

    fn handle_active_view_change(&mut self, was_open: bool, ctx: &mut EventCtx<()>) {
        let is_open = self.in_change_set();
        if was_open == is_open {
            return;
        }
        self.editor.sync();
        if !is_open {
            self.change_sets.sync();
        }
        ctx.request_layout();
        ctx.request_redraw();
        if is_open {
            TicketEditor::focus_tickets(ctx);
        }
    }

    #[cfg(test)]
    pub(super) fn create_ticket(&mut self, title: &str) {
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::CreateTicket(
                title.into(),
            ));
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn mark_selected_deleted(&mut self) {
        let selected = self.state.borrow().selected_ticket.clone().unwrap();
        self.state.borrow_mut().dispatch(
            crate::store::composer::ComposerAction::MarkTicketDeleted(selected),
        );
        self.editor.sync();
    }
}

impl TuiNode for ComposerPage {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.active().measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.editor.sync();
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active().render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let was_open = self.in_change_set();
        let outcome = self.active_mut().event(event, ctx);
        self.handle_active_view_change(was_open, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let was_open = self.in_change_set();
        let outcome = self.active_mut().dispatch_event(route, event, ctx);
        self.handle_active_view_change(was_open, ctx);
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.active_mut().tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.change_sets.init(ctx);
        self.editor.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.change_sets.mount(ctx);
        self.editor.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.editor.unmount(ctx);
        self.change_sets.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.editor.destroy(ctx);
        self.change_sets.destroy(ctx);
    }
}
