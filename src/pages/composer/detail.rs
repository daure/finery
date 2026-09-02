use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    AnimationSettings, AxisProposal, BorderKind, EventCtx, EventOutcome, EventRoute, Flex,
    FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, LayoutCtx, LayoutProposal,
    LayoutResult, LayoutSizeHint, LifecycleCtx, Panel, PanelHost, RenderCtx, SeasonalEmptyState,
    Split, Tab, Tabs, TabsBodyBorderStyle, TabsVariant, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::ComposerKeyBindings,
    service::AppService,
    store::composer::{ChangeKind, ComposerAction, ComposerState, ComposerViewMode},
};

use super::fields::{
    BoundDescription, BoundDescriptionDiffStyle, BoundTextField, BoundViewMode, DescriptionAction,
    DescriptionEditRequest, PendingDescriptionActions,
};
use super::property_fields::PropertyFields;

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type WideDescription = PanelHost<BoundDescription, ()>;
type WideProperties = PanelHost<PropertyFields, ()>;
type WideDetails = Split<WideDescription, WideProperties>;
type TicketFields = Split<BoundTextField, ResponsiveDetails>;
type TicketDetail = Split<Flex<()>, TicketFields>;

const WIDE_BREAKPOINT: u16 = 100;

struct ResponsiveDetails {
    narrow: Tabs<()>,
    wide: WideDetails,
    is_wide: bool,
    description_focus_key: String,
    description_editor_key: String,
    description_reader_key: String,
}

pub(super) struct DetailPane {
    state: Rc<RefCell<ComposerState>>,
    description_actions: PendingDescriptionActions,
    description_edit_request: DescriptionEditRequest,
    external_editor_pending: bool,
    service: AppService,
    detail: TicketDetail,
    empty: SeasonalEmptyState,
}

impl DetailPane {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        description_actions: PendingDescriptionActions,
        service: AppService,
        keys: ComposerKeyBindings,
    ) -> Self {
        let title =
            BoundTextField::title(Rc::clone(&state), Rc::clone(&pending), keys.title.clone());
        let description_edit_request = Rc::new(std::cell::Cell::new(false));
        let narrow_description = BoundDescription::new(
            Rc::clone(&state),
            Rc::clone(&pending),
            Rc::clone(&description_edit_request),
            &keys,
            Rc::clone(&description_actions),
        );
        let tabs = Tabs::new(vec![
            Tab::new("Description", narrow_description).hotkey(keys.description_tab.sequence()),
            Tab::new(
                "Properties",
                PropertyFields::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.clone(),
                ),
            )
            .hotkey(keys.properties_tab.sequence()),
        ])
        .action_hotkey(
            keys.description_focus.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Focus,
            ),
        )
        .action_hotkey(
            keys.description_editor.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Editor,
            ),
        )
        .action_hotkey(
            keys.description_reader.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::SpeedReader,
            ),
        )
        .variant(TabsVariant::Underline)
        .bordered(true);
        let wide_description = Panel::new()
            .top_left("Description")
            .action_hotkey(
                keys.description_focus.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::Focus,
                ),
            )
            .action_hotkey(
                keys.description_editor.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::Editor,
                ),
            )
            .action_hotkey(
                keys.description_reader.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::SpeedReader,
                ),
            )
            .host(BoundDescription::new(
                Rc::clone(&state),
                Rc::clone(&pending),
                Rc::clone(&description_edit_request),
                &keys,
                Rc::clone(&description_actions),
            ));
        let wide_properties = Panel::new()
            .top_left("Properties")
            .hotkey(keys.properties_tab.sequence())
            .host(PropertyFields::new(
                Rc::clone(&state),
                Rc::clone(&pending),
                service.clone(),
                keys.clone(),
            ));
        let details = ResponsiveDetails {
            narrow: tabs,
            wide: Split::horizontal(wide_description, wide_properties).ratio(70, 30),
            is_wide: false,
            description_focus_key: keys.description_focus.sequence().into(),
            description_editor_key: keys.description_editor.sequence().into(),
            description_reader_key: keys.description_reader.sequence().into(),
        };
        let fields =
            Split::vertical(title, details).constraints(Constraint::Length(3), Constraint::Fill(1));
        let mode = Flex::row()
            .gap(2)
            .child(
                "mode",
                BoundViewMode::new(Rc::clone(&state), Rc::clone(&pending), keys.view.clone()),
                FlexItem::fit_content(),
            )
            .child(
                "description-diff-style",
                BoundDescriptionDiffStyle::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    keys.description_inline.clone(),
                ),
                FlexItem::fit_content(),
            );
        Self {
            state,
            description_actions,
            description_edit_request,
            external_editor_pending: false,
            service,
            detail: Split::vertical(mode, fields)
                .constraints(Constraint::Length(1), Constraint::Fill(1)),
            empty: SeasonalEmptyState::new("No tickets added"),
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
            let details = self.detail.second_mut().second_mut();
            match action {
                DescriptionAction::ShowChanges => {
                    let _ = self
                        .state
                        .borrow_mut()
                        .dispatch(ComposerAction::SetViewMode(ComposerViewMode::Changes));
                }
                DescriptionAction::Focus { edit } => {
                    self.description_edit_request.set(edit);
                    details.focus_description();
                    ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
                    ctx.request_layout();
                    ctx.request_redraw();
                }
                DescriptionAction::FocusDiff => {
                    details.focus_description();
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
        self.detail.second_mut().second_mut().focus_description();
        ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
        ctx.request_layout();
        ctx.request_redraw();
    }

    pub(super) fn focus_diff(&mut self, ctx: &mut EventCtx<()>) {
        self.detail.second_mut().second_mut().focus_description();
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn sync(&mut self) {
        let fields = self.detail.second_mut();
        let title_height = fields.first().height();
        fields.set_constraints(Constraint::Length(title_height), Constraint::Fill(1));
        let state = self.state.borrow();
        let editable = state.selected_is_editable();
        let deleted = state
            .selected_change()
            .is_some_and(|change| change.kind == ChangeKind::Deleted);
        let details = self.detail.second_mut().second_mut();
        let (focus, editor, reader) = match state.view_mode {
            ComposerViewMode::Changes => (!deleted, editable, true),
            ComposerViewMode::Source => (true, false, true),
            ComposerViewMode::Diff => (true, false, false),
        };
        details.set_action_hotkeys(focus, editor, reader);
        let submitted = state
            .selected_change()
            .is_some_and(|change| change.is_submitted());
        details.set_dashed(
            deleted
                || submitted
                || state.view_mode == ComposerViewMode::Source
                || state.view_mode == ComposerViewMode::Diff,
        );
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
        if let Err(error) = self
            .state
            .borrow_mut()
            .dispatch(ComposerAction::UpdateDescription(response.value.clone()))
        {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Change blocked",
                    error.to_string(),
                ));
        }
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    #[cfg(test)]
    pub(super) fn detail_panel_areas(&self) -> (Rect, Rect) {
        self.detail.second().second().wide_areas()
    }

    #[cfg(test)]
    pub(super) fn narrow_border_style(&self) -> TabsBodyBorderStyle {
        self.detail.second().second().narrow_border_style()
    }
}

impl ResponsiveDetails {
    fn active(&self) -> &dyn TuiNode<()> {
        if self.is_wide {
            &self.wide
        } else {
            &self.narrow
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        if self.is_wide {
            &mut self.wide
        } else {
            &mut self.narrow
        }
    }

    fn focus_description(&mut self) {
        if !self.is_wide {
            self.narrow.select_index(0);
        }
    }

    fn set_action_hotkeys(&mut self, description: bool, editor: bool, reader: bool) {
        self.narrow
            .set_action_hotkey_enabled(&self.description_focus_key, description);
        self.narrow
            .set_action_hotkey_enabled(&self.description_editor_key, editor);
        self.narrow
            .set_action_hotkey_enabled(&self.description_reader_key, reader);
        let description_panel = self.wide.first_mut().panel_mut();
        description_panel.set_action_hotkey_enabled(&self.description_focus_key, description);
        description_panel.set_action_hotkey_enabled(&self.description_editor_key, editor);
        description_panel.set_action_hotkey_enabled(&self.description_reader_key, reader);
    }

    fn set_dashed(&mut self, dashed: bool) {
        self.narrow.set_body_border_style(if dashed {
            TabsBodyBorderStyle::Dashed
        } else {
            TabsBodyBorderStyle::Solid
        });
        let border = if dashed {
            BorderKind::AsciiDashed
        } else {
            tuicore::preset().border()
        };
        self.wide.first_mut().panel_mut().set_border(border);
        self.wide.second_mut().panel_mut().set_border(border);
    }

    #[cfg(test)]
    fn wide_areas(&self) -> (Rect, Rect) {
        self.wide.child_areas()
    }

    #[cfg(test)]
    fn narrow_border_style(&self) -> TabsBodyBorderStyle {
        self.narrow.current_body_border_style()
    }
}

fn panel_description_action(
    state: Rc<RefCell<ComposerState>>,
    actions: PendingDescriptionActions,
    action: DescriptionTabAction,
) -> impl Fn() + 'static {
    let trigger = description_tab_action(state, actions, action);
    move || trigger(0)
}

impl TuiNode for ResponsiveDetails {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let wide = match proposal.width {
            AxisProposal::Exact(width) | AxisProposal::AtMost(width) => width >= WIDE_BREAKPOINT,
            AxisProposal::Unbounded => true,
        };
        if wide {
            self.wide.measure(proposal)
        } else {
            self.narrow.measure(proposal)
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.is_wide = area.width >= WIDE_BREAKPOINT;
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active().render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.active_mut().event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.active_mut().dispatch_event(route, event, ctx)
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
        self.narrow.init(ctx);
        self.wide.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.narrow.mount(ctx);
        self.wide.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.wide.unmount(ctx);
        self.narrow.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.wide.destroy(ctx);
        self.narrow.destroy(ctx);
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
        let state = state.borrow();
        let view_mode = state.view_mode;
        if view_mode == ComposerViewMode::Changes
            && matches!(
                action,
                DescriptionTabAction::Focus | DescriptionTabAction::SpeedReader
            )
        {
            actions.borrow_mut().push(DescriptionAction::ShowChanges);
        }
        if selected != 0 {
            if action != DescriptionTabAction::Editor || state.selected_is_editable() {
                actions
                    .borrow_mut()
                    .push(description_focus_action(view_mode, false));
            }
            return;
        }
        let description = match view_mode {
            ComposerViewMode::Source => state.selected_source(),
            ComposerViewMode::Changes | ComposerViewMode::Diff => state.selected_changes(),
        }
        .map(|ticket| ticket.description.clone())
        .unwrap_or_default();
        match action {
            DescriptionTabAction::Focus => {
                actions.borrow_mut().push(description_focus_action(
                    view_mode,
                    state.selected_is_editable(),
                ));
            }
            DescriptionTabAction::Editor
                if view_mode == ComposerViewMode::Changes && state.selected_is_editable() =>
            {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenExternalEditor(description));
            }
            DescriptionTabAction::SpeedReader if view_mode != ComposerViewMode::Diff => {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenSpeedReader(description));
            }
            DescriptionTabAction::SpeedReader => {}
            DescriptionTabAction::Editor => {}
        }
    }
}

fn description_focus_action(view_mode: ComposerViewMode, edit: bool) -> DescriptionAction {
    match view_mode {
        ComposerViewMode::Diff => DescriptionAction::FocusDiff,
        ComposerViewMode::Source | ComposerViewMode::Changes => DescriptionAction::Focus { edit },
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
        if self.state.borrow().selected_ticket.is_none() {
            let offset = area.height / 6;
            let shifted = Rect::new(
                area.x,
                area.y.saturating_sub(offset),
                area.width,
                area.height,
            );
            <SeasonalEmptyState as TuiNode<()>>::render(&self.empty, frame, shifted, ctx);
        } else {
            self.detail.render(frame, area, ctx);
        }
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
