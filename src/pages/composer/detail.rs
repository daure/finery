use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusRequest,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    SeasonalEmptyState, Split, Tab, Tabs, TabsBodyBorderStyle, TabsVariant, TickResult, TuiEvent,
    TuiNode,
};

use crate::store::composer::{ChangeKind, ComposerAction, ComposerState};

use super::fields::{
    BoundDescription, BoundTextField, BoundVersionToggle, DescriptionAction,
    DescriptionEditRequest, PendingDescriptionActions, properties_form,
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type TicketFields = Split<BoundTextField, Tabs<()>>;
type TicketDetail = Split<BoundVersionToggle, TicketFields>;

pub(super) struct DetailPane {
    state: Rc<RefCell<ComposerState>>,
    description_actions: PendingDescriptionActions,
    description_edit_request: DescriptionEditRequest,
    external_editor_pending: bool,
    detail: TicketDetail,
    empty: SeasonalEmptyState,
}

impl DetailPane {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        description_actions: PendingDescriptionActions,
    ) -> Self {
        let title = BoundTextField::title(Rc::clone(&state), Rc::clone(&pending));
        let description_edit_request = Rc::new(std::cell::Cell::new(false));
        let description = BoundDescription::new(
            Rc::clone(&state),
            Rc::clone(&pending),
            Rc::clone(&description_edit_request),
        );
        let tabs = Tabs::new(vec![
            Tab::new("Description", description).hotkey("shift+d"),
            Tab::new(
                "Properties",
                properties_form(Rc::clone(&state), Rc::clone(&pending)),
            )
            .hotkey("shift+p"),
        ])
        .action_hotkey(
            "dd",
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Focus,
            ),
        )
        .action_hotkey(
            "do",
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Editor,
            ),
        )
        .action_hotkey(
            "ds",
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::SpeedReader,
            ),
        )
        .variant(TabsVariant::Underline)
        .bordered(true);
        let fields =
            Split::vertical(title, tabs).constraints(Constraint::Length(3), Constraint::Fill(1));
        let version = BoundVersionToggle::new(Rc::clone(&state), Rc::clone(&pending));
        Self {
            state,
            description_actions,
            description_edit_request,
            external_editor_pending: false,
            detail: Split::vertical(version, fields)
                .constraints(Constraint::Length(1), Constraint::Fill(1)),
            empty: SeasonalEmptyState::new("No issue selected"),
        }
    }

    fn active(&self) -> &dyn TuiNode<()> {
        if self.state.borrow().selected_ticket.is_some() {
            &self.detail
        } else {
            &self.empty
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        if self.state.borrow().selected_ticket.is_some() {
            &mut self.detail
        } else {
            &mut self.empty
        }
    }

    fn process_description_actions(&mut self, ctx: &mut EventCtx<()>) {
        let actions = self
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        let mut deferred = Vec::new();
        for action in actions {
            let tabs = self.detail.second_mut().second_mut();
            match action {
                DescriptionAction::Focus { edit } => {
                    self.description_edit_request.set(edit);
                    tabs.select_index(0);
                    ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
                    ctx.request_layout();
                    ctx.request_redraw();
                }
                DescriptionAction::OpenExternalEditor(description) => {
                    self.external_editor_pending = true;
                    ctx.request_external_editor_with_extension(description, 1, 1, "md");
                }
                action => deferred.push(action),
            }
        }
        self.description_actions.borrow_mut().extend(deferred);
    }

    pub(super) fn focus_description(&mut self, edit: bool, ctx: &mut EventCtx<()>) {
        self.description_edit_request.set(edit);
        self.detail.second_mut().second_mut().select_index(0);
        ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn sync(&mut self) {
        let state = self.state.borrow();
        let editable = state.selected_is_editable();
        let deleted = state
            .selected_change()
            .is_some_and(|change| change.kind == ChangeKind::Deleted);
        let tabs = self.detail.second_mut().second_mut();
        tabs.set_action_hotkey_enabled("dd", !deleted);
        tabs.set_action_hotkey_enabled("do", editable);
        tabs.set_body_border_style(if deleted {
            TabsBodyBorderStyle::Dashed
        } else {
            TabsBodyBorderStyle::Solid
        });
    }

    fn handle_external_editor(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        let TuiEvent::ExternalEditor(response) = event else {
            return None;
        };
        if !self.external_editor_pending {
            return None;
        }
        self.external_editor_pending = false;
        let adf = crate::store::composer::jira_adf::markdown_to_adf(&response.value);
        let markdown = crate::store::composer::jira_adf::adf_to_markdown(&adf);
        self.state
            .borrow_mut()
            .dispatch(ComposerAction::UpdateDescription(markdown));
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DescriptionTabAction {
    Focus,
    Editor,
    SpeedReader,
}

fn description_tab_action(
    state: Rc<RefCell<ComposerState>>,
    actions: PendingDescriptionActions,
    action: DescriptionTabAction,
) -> impl Fn(usize) + 'static {
    move |selected| {
        if selected != 0 {
            if action != DescriptionTabAction::Editor || state.borrow().selected_is_editable() {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::Focus { edit: false });
            }
            return;
        }
        let state = state.borrow();
        let description = state
            .selected_ticket()
            .map(|ticket| ticket.description.clone())
            .unwrap_or_default();
        match action {
            DescriptionTabAction::Focus => {
                actions.borrow_mut().push(DescriptionAction::Focus {
                    edit: state.selected_is_editable(),
                });
            }
            DescriptionTabAction::Editor if state.selected_is_editable() => {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenExternalEditor(description));
            }
            DescriptionTabAction::SpeedReader => {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenSpeedReader(description));
            }
            DescriptionTabAction::Editor => {}
        }
    }
}

impl TuiNode for DetailPane {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.active().measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active().render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if let Some(outcome) = self.handle_external_editor(event, ctx) {
            return outcome;
        }
        let outcome = self.active_mut().event(event, ctx);
        self.process_description_actions(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let Some(outcome) = self.handle_external_editor(event, ctx) {
            return outcome;
        }
        let outcome = self.active_mut().dispatch_event(route, event, ctx);
        self.process_description_actions(ctx);
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.sync();
        self.active_mut().tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.detail.init(ctx);
        self.empty.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.detail.mount(ctx);
        self.empty.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.empty.unmount(ctx);
        self.detail.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.empty.destroy(ctx);
        self.detail.destroy(ctx);
    }
}
