use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    AnimationSettings, ChildKey, CrossAlign, DataView, DataViewTypedEvent, Dialog, DialogAction,
    DialogBackdrop, DialogLayer, Dropdown, DropdownPopupDirection, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, Key, KeyEvent,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, MainAlign, Panel,
    PanelHost, Paragraph, RenderCtx, ScrollContainer, SpeedReader, Spinner, Split, TextInput,
    TextInputKeyBindings, TickResult, TuiEvent, TuiNode, keybindings,
};

use crate::{
    app_settings::{AppSettings, ComposerKeyBinding, ComposerKeyBindings},
    components::ticket_number_jump::{TicketNumberJump, exact_ticket_number_matches},
    service::{AppService, ComposerSearchTicket},
    speed_reader_settings::SpeedReaderSettings,
    store::composer::{
        ChangeKind, ComposerAction, ComposerState, PlacementTarget, TicketKind, TicketPresentation,
    },
};

use super::{
    add_ticket_menu::{AddTicketEvent, AddTicketMenu},
    detail::DetailPane,
    fields::{DescriptionAction, PendingDescriptionActions},
    source::SourceController,
    submission::SubmissionController,
    ticket_rows::{
        TicketRow, set_active_ticket_style, ticket_data_view_with_number_jump, ticket_rows,
    },
    ticket_toolbar::{ToolbarEvent, ToolbarEvents, ToolbarFeedback, toolbar},
    title_guidance::{TitleFeedback, format_title},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type TicketList = PanelHost<DataView<TicketRow, String>>;
type Body = Split<TicketList, DetailPane>;
type Workspace = Split<Flex<()>, Body>;
type CreateDialog = tuicore::DialogHost<CreateTicketForm, ()>;
type CreateLayer = DialogLayer<Workspace, CreateDialog>;
type AddLayer = DialogLayer<CreateLayer, AddTicketMenu>;
type TicketEditorView = DialogLayer<AddLayer, Dialog<()>>;
type DescriptionReader = tuicore::DialogHost<SpeedReader, ()>;

const TITLE_INPUT_SLOT: &str = "title-input";
const ISSUE_TYPE_SLOT: &str = "issue-type";
type EditorView = DialogLayer<TicketEditorView, DescriptionReader>;

struct CreateTicketForm {
    input: TextInput<()>,
    kind: Dropdown<TicketKind, TicketKind>,
    selected_kind: Rc<Cell<TicketKind>>,
    feedback: TitleFeedback,
    submit_key: ComposerKeyBinding,
    on_ctrl_enter: Box<dyn Fn(String)>,
    input_area: Rect,
    feedback_area: Rect,
}

impl CreateTicketForm {
    fn new(
        input: TextInput<()>,
        kind: Dropdown<TicketKind, TicketKind>,
        selected_kind: Rc<Cell<TicketKind>>,
        feedback: TitleFeedback,
        submit_key: ComposerKeyBinding,
        on_ctrl_enter: impl Fn(String) + 'static,
    ) -> Self {
        let mut input = input;
        input.set_insert_mode(true);
        Self {
            input,
            kind,
            selected_kind,
            feedback,
            submit_key,
            on_ctrl_enter: Box::new(on_ctrl_enter),
            input_area: Rect::default(),
            feedback_area: Rect::default(),
        }
    }

    fn clear(&mut self) {
        self.input.set_value("");
        self.input.set_insert_mode(true);
        self.feedback.clear();
    }

    fn set_kinds(&mut self, kinds: Vec<TicketKind>) {
        self.kind.set_rows(kinds.clone());
        let selected = if kinds.contains(&TicketKind::Story) {
            TicketKind::Story
        } else {
            kinds[0]
        };
        self.kind.set_selected_one(selected);
        self.selected_kind.set(selected);
    }

    fn submit_on_ctrl_enter(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if !self.submit_key.matches(*key) {
            return false;
        }
        (self.on_ctrl_enter)(self.input.current_value().to_owned());
        ctx.request_redraw();
        ctx.stop_propagation();
        true
    }

    fn open_kind_on_enter(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(
            event,
            TuiEvent::Key(KeyEvent {
                code: Key::Enter,
                ..
            })
        ) || !self.input.insert_mode()
        {
            return false;
        }
        self.kind.open_with_context(ctx);
        ctx.stop_propagation();
        true
    }
}

impl TuiNode for CreateTicketForm {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let input = self.input.measure(proposal);
        let feedback = self.feedback.measure(proposal);
        LayoutSizeHint::content(
            input
                .preferred
                .width
                .max(
                    <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::measure(
                        &self.kind, proposal,
                    )
                    .preferred
                    .width,
                )
                .max(feedback.preferred.width)
                .max(80),
            input
                .preferred
                .height
                .saturating_add(
                    <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::measure(
                        &self.kind, proposal,
                    )
                    .preferred
                    .height,
                )
                .saturating_add(1)
                .saturating_add(feedback.preferred.height),
        )
        .normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let input_height = self
            .input
            .measure(LayoutProposal::unbounded())
            .preferred
            .height
            .min(area.height);
        self.input_area = Rect::new(area.x, area.y, area.width, input_height);
        let kind_height = <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::measure(
            &self.kind,
            LayoutProposal::unbounded(),
        )
        .preferred
        .height
        .min(area.height.saturating_sub(input_height));
        let kind_area = Rect::new(
            area.x,
            area.y.saturating_add(input_height),
            area.width,
            kind_height,
        );
        let feedback_y = kind_area.bottom().saturating_add(1);
        self.feedback_area = Rect::new(
            area.x,
            feedback_y,
            area.width,
            area.bottom().saturating_sub(feedback_y),
        );
        ctx.push_slot(ChildKey::new(TITLE_INPUT_SLOT), self.input_area, |ctx| {
            self.input.layout(self.input_area, ctx);
        });
        ctx.push_slot(ChildKey::new(ISSUE_TYPE_SLOT), kind_area, |ctx| {
            <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::layout(
                &mut self.kind,
                kind_area,
                ctx,
            );
        });
        self.feedback.layout(self.feedback_area, ctx);
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        TuiNode::render(&self.input, frame, self.input_area, ctx);
        self.kind.render(
            frame,
            Rect::new(
                self.input_area.x,
                self.input_area.bottom(),
                self.input_area.width,
                <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::measure(
                    &self.kind,
                    LayoutProposal::unbounded(),
                )
                .preferred
                .height,
            ),
            ctx,
        );
        self.feedback.render(frame, self.feedback_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.submit_on_ctrl_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_kind_on_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        let outcome = self.input.event(event, ctx);
        if outcome == EventOutcome::Handled {
            outcome
        } else {
            self.kind.event(event, ctx)
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.submit_on_ctrl_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_kind_on_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        if let Some(path) = route
            .path
            .without_first_if(&ChildKey::new(TITLE_INPUT_SLOT))
        {
            return self
                .input
                .dispatch_event(&EventRoute::new(path), event, ctx);
        }
        if let Some(path) = route.path.without_first_if(&ChildKey::new(ISSUE_TYPE_SLOT)) {
            return self.kind.dispatch_event(&EventRoute::new(path), event, ctx);
        }
        EventOutcome::Ignored
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings).merge(
            <Dropdown<TicketKind, TicketKind> as TuiNode<()>>::tick(&mut self.kind, dt, settings),
        )
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
        self.kind.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(target) = target.for_child(&ChildKey::new(TITLE_INPUT_SLOT)) {
            self.input.dispatch_focus(&target, focused, ctx);
        }
        if let Some(target) = target.for_child(&ChildKey::new(ISSUE_TYPE_SLOT)) {
            self.kind.dispatch_focus(&target, focused, ctx);
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
        self.kind.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
        self.kind.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
        self.kind.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
        self.kind.destroy(ctx);
    }
}

pub(super) struct TicketEditor {
    state: Rc<RefCell<ComposerState>>,
    pending: PendingActions,
    description_actions: PendingDescriptionActions,
    settings: Arc<RwLock<AppSettings>>,
    service: AppService,
    view: EditorView,
    toolbar_events: ToolbarEvents,
    toolbar_feedback: ToolbarFeedback,
    can_change: Rc<Cell<bool>>,
    can_add_child: Rc<Cell<bool>>,
    can_refresh: Rc<Cell<bool>>,
    can_submit: Rc<Cell<bool>>,
    pending_project: Rc<RefCell<Option<String>>>,
    pending_placement: Rc<RefCell<PlacementTarget>>,
    create_dialog_close_requested: Rc<Cell<bool>>,
    ticket_dialog_close_requested: Rc<Cell<bool>>,
    submit_confirmation_requested: Rc<Cell<bool>>,
    reparent_confirmation_requested: Rc<Cell<bool>>,
    pending_reparent: Rc<RefCell<Option<ComposerAction>>>,
    submission: SubmissionController,
    source: SourceController,
    loading_view: ScrollContainer<Flex<()>>,
    opening_loading: bool,
    pending_focus_tickets: bool,
    number_jump: Rc<RefCell<TicketNumberJump>>,
}

impl TicketEditor {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        settings: Arc<RwLock<AppSettings>>,
        service: AppService,
    ) -> Self {
        let pending = Rc::new(RefCell::new(Vec::new()));
        let description_actions = Rc::new(RefCell::new(Vec::new()));
        let keys = settings
            .read()
            .expect("settings lock poisoned")
            .composer_keys
            .clone();
        let number_jump = Rc::new(RefCell::new(TicketNumberJump::default()));
        let ticket_list = Panel::new().top_left("Change sets").one_row(true).host(
            ticket_data_view_with_number_jump(&state.borrow(), Rc::clone(&number_jump)),
        );

        let detail = DetailPane::new(
            Rc::clone(&state),
            Rc::clone(&pending),
            Rc::clone(&description_actions),
            service.clone(),
            keys.clone(),
        );
        let body = Split::vertical(ticket_list, detail).ratio(1, 2);
        let toolbar_events = Rc::new(RefCell::new(Vec::new()));
        let toolbar_feedback = ToolbarFeedback::new();
        let can_change = Rc::new(Cell::new(true));
        let can_add_child = Rc::new(Cell::new(false));
        let can_refresh = Rc::new(Cell::new(false));
        let can_submit = Rc::new(Cell::new(false));
        let workspace = Split::vertical(
            toolbar(
                Rc::clone(&toolbar_events),
                toolbar_feedback.clone(),
                Rc::clone(&can_change),
                Rc::clone(&can_add_child),
                Rc::clone(&can_refresh),
                Rc::clone(&can_submit),
                keys.clone(),
            ),
            body,
        )
        .constraints(Constraint::Length(1), Constraint::Min(1));

        let create_sink = Rc::clone(&pending);
        let pending_project = Rc::new(RefCell::new(None::<String>));
        let create_project = Rc::clone(&pending_project);
        let pending_placement = Rc::new(RefCell::new(PlacementTarget::Root));
        let create_placement = Rc::clone(&pending_placement);
        let selected_kind = Rc::new(Cell::new(TicketKind::Story));
        let create_kind = Rc::clone(&selected_kind);
        let title = Rc::new(RefCell::new(String::new()));
        let title_feedback = TitleFeedback::new(Rc::clone(&title));
        let title_input = Rc::clone(&title);
        let create_dialog_close_requested = Rc::new(Cell::new(false));
        let create_keys = TextInputKeyBindings::default();
        let submit_ticket = Rc::new(move |title: String| {
            let title = format_title(&title);
            if !title.is_empty()
                && let Some(project_key) = create_project.borrow().clone()
            {
                create_sink
                    .borrow_mut()
                    .push(ComposerAction::CreateTicketAt {
                        title,
                        project_key,
                        kind: create_kind.get(),
                        placement: create_placement.borrow().clone(),
                    });
            }
        });
        let ok_submit = Rc::clone(&submit_ticket);
        let ok_title = Rc::clone(&title);
        let kind_selection = Rc::clone(&selected_kind);
        let cancel_action = Rc::clone(&create_dialog_close_requested);
        let close_action = Rc::clone(&create_dialog_close_requested);
        let create_dialog = Dialog::new()
            .top_left("Create ticket")
            .actions([
                DialogAction::new("OK")
                    .hotkey(keys.create_confirm.spec())
                    .on_trigger(move || ok_submit(ok_title.borrow().clone())),
                DialogAction::new("Cancel")
                    .hotkey(keys.dialog_cancel.spec())
                    .on_trigger(move || cancel_action.set(true)),
            ])
            .close_on_unfocus_from_descendants(true)
            .on_close(move |_| close_action.set(true))
            .host(CreateTicketForm::new(
                TextInput::new()
                    .keybindings(create_keys)
                    .panel("Title")
                    .placeholder("Ticket title")
                    .focused(true)
                    .on_change(move |value| *title_input.borrow_mut() = value),
                Dropdown::single(
                    [TicketKind::Story],
                    |kind| *kind,
                    |kind| ticket_kind_label(*kind).into(),
                )
                .label("Issue type")
                .popup_direction(DropdownPopupDirection::Down)
                .selected_one(TicketKind::Story)
                .on_select(move |kinds| {
                    if let Some(kind) = kinds.first() {
                        kind_selection.set(*kind);
                    }
                }),
                Rc::clone(&selected_kind),
                title_feedback,
                keys.create_submit.clone(),
                move |title| submit_ticket(title),
            ));
        let create_layer = DialogLayer::new(workspace, create_dialog)
            .active(false)
            .layer_percent(60)
            .layer_cross_percent(50)
            .fit_content()
            .child_overlays_use_base_bounds(true)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let add_layer = DialogLayer::new(create_layer, AddTicketMenu::new(service.clone()))
            .active(false)
            .fit_content()
            .fit_content_max(120, 12)
            .child_overlays_use_base_bounds(true)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let ticket_dialog_close_requested = Rc::new(Cell::new(false));
        let submit_confirmation_requested = Rc::new(Cell::new(false));
        let reparent_confirmation_requested = Rc::new(Cell::new(false));
        let pending_reparent = Rc::new(RefCell::new(None));
        let close_ticket_dialog = Rc::clone(&ticket_dialog_close_requested);
        let ticket_action_dialog = Dialog::new()
            .top_left("Ticket action")
            .content(["Choose what should happen to the selected ticket."])
            .on_close(move |_| close_ticket_dialog.set(true));
        let ticket_view = DialogLayer::new(add_layer, ticket_action_dialog)
            .active(false)
            .fit_content()
            .fit_content_max(72, 8)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let view = DialogLayer::new(
            ticket_view,
            description_reader(
                "",
                Rc::clone(&description_actions),
                settings
                    .read()
                    .expect("settings lock poisoned")
                    .speed_reader,
            ),
        )
        .active(false)
        .fit_content()
        .fit_content_max(72, 16)
        .backdrop(DialogBackdrop::dim().amount(0.55));
        let submission = SubmissionController::new(Rc::clone(&state), service.clone());
        let source = SourceController::new(Rc::clone(&state), service.clone());

        Self {
            state,
            pending,
            description_actions,
            settings,
            service,
            view,
            toolbar_events,
            toolbar_feedback,
            can_change,
            can_add_child,
            can_refresh,
            can_submit,
            pending_project,
            pending_placement,
            create_dialog_close_requested,
            ticket_dialog_close_requested,
            submit_confirmation_requested,
            reparent_confirmation_requested,
            pending_reparent,
            submission,
            source,
            loading_view: loading_view(),
            opening_loading: false,
            pending_focus_tickets: false,
            number_jump,
        }
    }

    fn table(&self) -> &DataView<TicketRow, String> {
        self.view
            .base()
            .base()
            .base()
            .base()
            .second()
            .first()
            .child()
    }

    fn table_mut(&mut self) -> &mut DataView<TicketRow, String> {
        self.view
            .base_mut()
            .base_mut()
            .base_mut()
            .base_mut()
            .second_mut()
            .first_mut()
            .child_mut()
    }

    fn detail_mut(&mut self) -> &mut DetailPane {
        self.view
            .base_mut()
            .base_mut()
            .base_mut()
            .base_mut()
            .second_mut()
            .second_mut()
    }

    fn body_mut(&mut self) -> &mut Body {
        self.view
            .base_mut()
            .base_mut()
            .base_mut()
            .base_mut()
            .second_mut()
    }

    pub(super) fn sync(&mut self) {
        let (breadcrumb, rows, selected, selected_for_submission, is_open) = {
            let state = self.state.borrow();
            let breadcrumb = state
                .active_set()
                .map_or_else(|| "Change sets".into(), |set| set.name.clone());
            let selected_for_submission = state
                .active_set()
                .into_iter()
                .flat_map(|set| set.selected_ticket_ids.clone())
                .collect::<Vec<_>>();
            (
                breadcrumb,
                ticket_rows(&state),
                state.selected_ticket.clone(),
                selected_for_submission,
                state.active_set().is_some_and(|set| !set.closed),
            )
        };
        self.view
            .base_mut()
            .base_mut()
            .base_mut()
            .base_mut()
            .second_mut()
            .first_mut()
            .panel_mut()
            .set_top_left(breadcrumb);
        let table = self.table_mut();
        table.set_rows(rows);
        table.expand_all();
        set_active_ticket_style(table, selected.clone());
        restore_ticket_selection(table, selected_for_submission);
        if let Some(selected) = &selected {
            table.highlight_id(selected);
        }
        self.can_change
            .set(is_open && !self.submission.is_submitting());
        let can_add_child = {
            let state = self.state.borrow();
            state
                .selected_change()
                .is_some_and(|change| change.kind != ChangeKind::Deleted)
                && state
                    .selected_ticket
                    .as_ref()
                    .is_some_and(|id| !state.legal_child_kinds(Some(id)).is_empty())
        };
        self.can_add_child
            .set(is_open && !self.submission.is_submitting() && can_add_child);
        let can_refresh = {
            let state = self.state.borrow();
            state.remote_queries_allowed() && state.has_remote_tickets()
        };
        self.can_refresh
            .set(can_refresh && !self.submission.is_submitting());
        let selected = self.table().selected_ids();
        self.can_submit.set(
            is_open
                && !self.submission.is_submitting()
                && self.state.borrow().changes_ready_for_submit(&selected),
        );
    }

    pub(super) fn is_submitting(&self) -> bool {
        self.submission.is_submitting()
    }

    #[cfg(test)]
    pub(super) fn detail_panel_areas(&self) -> (Rect, Rect) {
        self.view
            .base()
            .base()
            .base()
            .base()
            .second()
            .second()
            .detail_panel_areas()
    }

    #[cfg(test)]
    pub(super) fn ticket_detail_areas(&self) -> (Rect, Rect) {
        self.view.base().base().base().base().second().child_areas()
    }

    #[cfg(test)]
    pub(super) fn narrow_border_style(&self) -> tuicore::TabsBodyBorderStyle {
        self.view
            .base()
            .base()
            .base()
            .base()
            .second()
            .second()
            .narrow_border_style()
    }

    #[cfg(test)]
    pub(super) fn create_kind_menu_is_open(&self) -> bool {
        self.view
            .base()
            .base()
            .base()
            .layer()
            .child()
            .kind
            .is_open()
    }

    #[cfg(test)]
    pub(super) fn create_dialog_is_open(&self) -> bool {
        self.view.base().base().base().is_active()
    }

    fn drain_description_actions(&mut self, ctx: &mut EventCtx<()>) {
        let actions = self
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for action in actions {
            match action {
                DescriptionAction::ShowChanges => {
                    let _ = self
                        .state
                        .borrow_mut()
                        .dispatch(ComposerAction::SetViewMode(
                            crate::store::composer::ComposerViewMode::Changes,
                        ));
                }
                DescriptionAction::Focus { edit } => {
                    self.detail_mut().focus_description(edit, ctx);
                }
                DescriptionAction::FocusDiff => {
                    self.detail_mut().focus_diff(ctx);
                }
                DescriptionAction::OpenSpeedReader(description) => {
                    self.view.replace_layer(
                        description_reader(
                            description,
                            Rc::clone(&self.description_actions),
                            self.settings
                                .read()
                                .expect("settings lock poisoned")
                                .speed_reader,
                        ),
                        ctx,
                    );
                    self.view.set_active_with_context(true, ctx);
                }
                DescriptionAction::OpenExternalEditor(_) => {}
                DescriptionAction::CloseSpeedReader => {
                    self.view.set_active_with_context(false, ctx);
                }
            }
        }
    }

    fn drain_outputs(&mut self, ctx: &mut EventCtx<()>) {
        self.drain_description_actions(ctx);
        if self.create_dialog_close_requested.replace(false) {
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .set_active_with_context(false, ctx);
        }
        let submit_confirmed = self.submit_confirmation_requested.replace(false);
        let reparent_confirmed = self.reparent_confirmation_requested.replace(false);
        if self.ticket_dialog_close_requested.replace(false)
            || submit_confirmed
            || reparent_confirmed
        {
            self.view.base_mut().set_active_with_context(false, ctx);
        }
        if submit_confirmed {
            self.start_submit(ctx);
        }
        if reparent_confirmed && let Some(action) = self.pending_reparent.borrow_mut().take() {
            if let Err(error) = self.state.borrow_mut().dispatch(action) {
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Change blocked",
                        error.to_string(),
                    ));
            }
            if let Some(set) = self.state.borrow().active_set().cloned() {
                self.service.save_change_set(set);
            }
        }
        let add_events = self.view.base_mut().base_mut().layer_mut().take_events();
        for event in add_events {
            match event {
                AddTicketEvent::CreateNew {
                    project_key,
                    placement,
                } => {
                    *self.pending_project.borrow_mut() = Some(project_key);
                    *self.pending_placement.borrow_mut() = placement.clone();
                    self.view
                        .base_mut()
                        .base_mut()
                        .set_active_with_context(false, ctx);
                    self.view
                        .base_mut()
                        .base_mut()
                        .base_mut()
                        .layer_mut()
                        .child_mut()
                        .clear();
                    let kinds = self.legal_kinds(&placement);
                    self.view
                        .base_mut()
                        .base_mut()
                        .base_mut()
                        .layer_mut()
                        .child_mut()
                        .set_kinds(kinds);
                    self.view
                        .base_mut()
                        .base_mut()
                        .base_mut()
                        .set_active_with_context(true, ctx);
                }
                AddTicketEvent::Include { ticket, placement } => {
                    self.include_at(ticket, placement, ctx);
                    self.view
                        .base_mut()
                        .base_mut()
                        .set_active_with_context(false, ctx);
                }
                AddTicketEvent::Closed => {
                    self.view
                        .base_mut()
                        .base_mut()
                        .set_active_with_context(false, ctx);
                }
            }
        }

        for event in self.table_mut().drain_events() {
            match event {
                DataViewTypedEvent::Activated { row_id } => self
                    .pending
                    .borrow_mut()
                    .push(ComposerAction::SelectTicket(Some(row_id))),
                DataViewTypedEvent::SelectionChanged { selected, .. } => self
                    .pending
                    .borrow_mut()
                    .push(ComposerAction::SetSelectedTickets(selected)),
                _ => {}
            }
        }

        let actions = self.pending.borrow_mut().drain(..).collect::<Vec<_>>();
        let created = actions.iter().any(|action| {
            matches!(
                action,
                ComposerAction::CreateTicket { .. } | ComposerAction::CreateTicketAt { .. }
            )
        });
        let view_mode_changed = actions
            .iter()
            .any(|action| matches!(action, ComposerAction::SetViewMode(_)));
        let mut ticket_action = false;
        let mut persist = false;
        for action in actions {
            if let ComposerAction::ReparentTicket { .. } = action
                && self.request_reparent_confirmation(action.clone(), ctx)
            {
                continue;
            }
            if let ComposerAction::RemoveTicket(id) = &action
                && let Err(error) = self.state.borrow().removal_preview(id)
            {
                self.service
                    .report_notification(tuicore::Notification::error("Remove blocked", error));
                continue;
            }
            ticket_action |= matches!(
                action,
                ComposerAction::MarkTicketDeleted(_)
                    | ComposerAction::RemoveTicket(_)
                    | ComposerAction::RestoreTicket(_)
                    | ComposerAction::ResetTicket(_)
            );
            persist |= action.affects_persistence();
            if let Err(error) = self.state.borrow_mut().dispatch(action) {
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Change blocked",
                        error.to_string(),
                    ));
            }
        }
        if persist && let Some(set) = self.state.borrow().active_set().cloned() {
            self.service.save_change_set(set);
        }
        if created {
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .set_active_with_context(false, ctx);
            Self::focus_tickets(ctx);
        }
        if view_mode_changed {
            Self::focus_tickets(ctx);
        }
        if ticket_action {
            self.view.base_mut().set_active_with_context(false, ctx);
        }
        self.sync();
        self.source.ensure_selected();
        ctx.request_layout();
        self.drain_toolbar_events(ctx);
        ctx.request_redraw();
    }

    fn ticket_list_is_focused(&self) -> bool {
        self.view
            .base()
            .base()
            .base()
            .base()
            .second()
            .first()
            .child()
            .is_focused()
    }

    fn description_reader_is_open(&self) -> bool {
        self.view.is_active()
    }

    fn legal_kinds(&self, placement: &PlacementTarget) -> Vec<TicketKind> {
        let parent = match placement {
            PlacementTarget::Root => None,
            PlacementTarget::ChildOf(id) => Some(id.as_str()),
        };
        self.state.borrow().legal_child_kinds(parent)
    }

    fn sibling_placement(&self) -> PlacementTarget {
        self.state
            .borrow()
            .selected_ticket()
            .and_then(|ticket| ticket.parent_key.clone())
            .map(PlacementTarget::ChildOf)
            .unwrap_or(PlacementTarget::Root)
    }

    fn child_placement(&self) -> Option<PlacementTarget> {
        let state = self.state.borrow();
        let id = state.selected_ticket.as_ref()?.clone();
        (!state.legal_child_kinds(Some(&id)).is_empty()).then_some(PlacementTarget::ChildOf(id))
    }

    fn project_hint(&self) -> Option<String> {
        let state = self.state.borrow();
        let mut projects = state
            .active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter_map(|change| state.ticket_for_change(change))
            .map(|ticket| ticket.project_key.trim())
            .filter(|project| !project.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        projects.sort();
        projects.dedup();
        if projects.len() == 1 {
            return projects.pop();
        }
        let default = self
            .settings
            .read()
            .expect("settings lock poisoned")
            .jira_default_project
            .trim()
            .to_owned();
        (!default.is_empty()).then_some(default)
    }

    fn open_new_ticket(&mut self, placement: PlacementTarget, ctx: &mut EventCtx<()>) {
        let kinds = self.legal_kinds(&placement);
        if kinds.is_empty() {
            return;
        }
        *self.pending_placement.borrow_mut() = placement.clone();
        if let Some(project) = self.project_hint() {
            *self.pending_project.borrow_mut() = Some(project);
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .layer_mut()
                .child_mut()
                .clear();
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .layer_mut()
                .child_mut()
                .set_kinds(kinds);
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .set_active_with_context(true, ctx);
        } else {
            self.view
                .base_mut()
                .base_mut()
                .layer_mut()
                .open_new_project_selector(placement, kinds, ctx);
            self.view
                .base_mut()
                .base_mut()
                .set_active_with_context(true, ctx);
        }
    }

    #[cfg(test)]
    pub(super) fn open_new_ticket_for_test(&mut self, ctx: &mut EventCtx<()>) {
        self.open_new_ticket(PlacementTarget::Root, ctx);
    }

    fn open_existing_ticket(&mut self, placement: PlacementTarget, ctx: &mut EventCtx<()>) {
        let kinds = self.legal_kinds(&placement);
        if kinds.is_empty() {
            return;
        }
        let project_hint = self.project_hint();
        self.view.base_mut().base_mut().layer_mut().open_existing(
            project_hint,
            placement,
            kinds,
            ctx,
        );
        self.view
            .base_mut()
            .base_mut()
            .set_active_with_context(true, ctx);
    }

    fn drain_toolbar_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self
            .toolbar_events
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for event in events {
            match event {
                ToolbarEvent::OpenSibling => {
                    *self.pending_placement.borrow_mut() = self.sibling_placement()
                }
                ToolbarEvent::OpenChild => {
                    if let Some(placement) = self.child_placement() {
                        *self.pending_placement.borrow_mut() = placement;
                    }
                }
                ToolbarEvent::AddSiblingNew | ToolbarEvent::AddChildNew => {
                    let placement = self.pending_placement.borrow().clone();
                    self.open_new_ticket(placement, ctx)
                }
                ToolbarEvent::AddSiblingExisting | ToolbarEvent::AddChildExisting => {
                    let placement = self.pending_placement.borrow().clone();
                    self.open_existing_ticket(placement, ctx)
                }
                ToolbarEvent::Refresh => self.ensure_source(true, ctx),
                ToolbarEvent::Commit => self.open_submit_confirmation(ctx),
            }
        }
    }

    pub(super) fn on_open(&mut self, ctx: &mut EventCtx<()>) {
        self.ensure_source(true, ctx);
        self.pending_focus_tickets = true;
        if self.source.is_loading() {
            self.opening_loading = true;
            ctx.request_layout();
            ctx.request_redraw();
        } else {
            self.opening_loading = false;
            self.pending_focus_tickets = false;
            Self::focus_tickets(ctx);
        }
    }

    pub(super) fn on_open_lifecycle(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.pending_focus_tickets = true;
        if self.state.borrow().remote_queries_allowed() && self.state.borrow().has_remote_tickets()
        {
            self.source.refresh_all();
            if self.source.is_loading() {
                self.opening_loading = true;
                ctx.request_tick();
            }
        }
    }

    pub(super) fn ensure_source(&mut self, force: bool, ctx: &mut EventCtx<()>) {
        if force {
            self.source.refresh_all();
        } else {
            self.source.ensure_selected();
        }
        if self.source.is_loading() {
            ctx.request_tick();
        }
    }

    fn start_submit(&mut self, ctx: &mut EventCtx<()>) {
        if self.submission.is_submitting() {
            return;
        }
        let selected = self.table().selected_ids();
        let changes = match self.state.borrow().commit_changes(&selected) {
            Ok(changes) => changes,
            Err(error) => {
                self.service
                    .report_notification(tuicore::Notification::error("Commit blocked", error));
                return;
            }
        };
        self.submission.start(changes, ctx);
        self.sync();
    }

    fn open_submit_confirmation(&mut self, ctx: &mut EventCtx<()>) {
        if !self.can_submit.get() || self.submission.is_submitting() {
            return;
        }
        self.ticket_dialog_close_requested.set(false);
        self.submit_confirmation_requested.set(false);
        let selected = self.table().selected_ids();
        let changes = match self.state.borrow().commit_changes(&selected) {
            Ok(changes) => changes,
            Err(error) => {
                self.service
                    .report_notification(tuicore::Notification::error("Commit blocked", error));
                return;
            }
        };
        let confirm_submit = Rc::clone(&self.submit_confirmation_requested);
        let cancel_submit = Rc::clone(&self.ticket_dialog_close_requested);
        let keys = self.composer_keys();
        let content = {
            let state = self.state.borrow();
            let mut content = vec![format!("Commit {} ticket changes to Jira:", changes.len())];
            content.extend(changes.into_iter().map(|change| {
                let title = state
                    .changes_for_change(&change)
                    .map(|ticket| ticket.title.as_str())
                    .unwrap_or("Unavailable ticket");
                format!("• {} · {title}", change.id)
            }));
            content
        };
        let dialog = self.view.base_mut().layer_mut();
        dialog.set_top_left("Commit changes");
        dialog.set_actions([
            DialogAction::new("Commit")
                .hotkey(keys.submit_confirm.spec())
                .on_trigger(move || confirm_submit.set(true)),
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || cancel_submit.set(true)),
        ]);
        dialog.set_content(content);
        self.view.base_mut().set_active_with_context(true, ctx);
    }

    fn include_at(
        &mut self,
        ticket: ComposerSearchTicket,
        placement: PlacementTarget,
        ctx: &mut EventCtx<()>,
    ) {
        let presentation = TicketPresentation {
            work_item: ticket.work_item.clone(),
            story_points_configured: ticket.story_points_configured,
            assumed_story_points: ticket.assumed_story_points,
        };
        let ticket = ticket.ticket;
        let existing = {
            let state = self.state.borrow();
            state
                .active_set()
                .and_then(|set| set.tickets.iter().find(|change| change.id == ticket.key))
                .and_then(|change| state.changes_for_change(change))
                .cloned()
        };
        if let Some(existing) = existing
            && existing.parent_key
                != match &placement {
                    PlacementTarget::Root => None,
                    PlacementTarget::ChildOf(id) => Some(id.clone()),
                }
        {
            self.request_reparent_confirmation(
                ComposerAction::ReparentTicket {
                    id: ticket.key.clone(),
                    placement,
                },
                ctx,
            );
        } else {
            self.pending
                .borrow_mut()
                .push(ComposerAction::IncludeTicketAt {
                    ticket: ticket.clone(),
                    placement,
                });
        }
        if let Some(change_set_id) = self.state.borrow().active_change_set.clone() {
            self.pending
                .borrow_mut()
                .push(ComposerAction::SetPresentation {
                    change_set_id,
                    id: ticket.key,
                    presentation,
                });
        }
        self.view
            .base_mut()
            .base_mut()
            .set_active_with_context(false, ctx);
    }

    fn request_reparent_confirmation(
        &mut self,
        action: ComposerAction,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        let ComposerAction::ReparentTicket { id, placement } = action.clone() else {
            return false;
        };
        let requires_confirmation = self
            .state
            .borrow()
            .active_set()
            .and_then(|set| set.tickets.iter().find(|change| change.id == id))
            .is_some_and(|change| change.kind != ChangeKind::Added);
        if !requires_confirmation {
            return false;
        }
        if let Err(error) = self.state.borrow().validate_placement(&id, &placement) {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Change blocked",
                    error.to_string(),
                ));
            return true;
        }
        *self.pending_reparent.borrow_mut() = Some(action);
        self.ticket_dialog_close_requested.set(false);
        self.reparent_confirmation_requested.set(false);
        let confirmed = Rc::clone(&self.reparent_confirmation_requested);
        let cancelled = Rc::clone(&self.ticket_dialog_close_requested);
        let old_parent = {
            let state = self.state.borrow();
            state
                .active_set()
                .and_then(|set| set.tickets.iter().find(|change| change.id == id))
                .and_then(|change| state.changes_for_change(change))
                .and_then(|ticket| ticket.parent_key.clone())
                .unwrap_or_else(|| "Root".into())
        };
        let new_parent = match &placement {
            PlacementTarget::Root => "Root".into(),
            PlacementTarget::ChildOf(parent) => parent.clone(),
        };
        let keys = self.composer_keys();
        let dialog = self.view.base_mut().layer_mut();
        dialog.set_top_left("Change parent");
        dialog.set_content([format!(
            "Move this existing ticket: {old_parent} → {new_parent}?"
        )]);
        dialog.set_actions([
            DialogAction::new("Move")
                .hotkey(keys.reparent_confirm.spec())
                .on_trigger(move || confirmed.set(true)),
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || cancelled.set(true)),
        ]);
        self.view.base_mut().set_active_with_context(true, ctx);
        true
    }

    fn poll_submission(&mut self) -> bool {
        let changed = self.submission.drain_results();
        if changed {
            self.sync();
        }
        changed
    }

    pub(super) fn poll_inactive_submission(&mut self) -> TickResult {
        let changed = self.poll_submission();
        (if self.submission.is_submitting() {
            TickResult::scheduled_after(Duration::from_millis(50))
        } else {
            TickResult::IDLE
        })
        .merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }

    fn composer_keys(&self) -> ComposerKeyBindings {
        self.settings
            .read()
            .expect("settings lock poisoned")
            .composer_keys
            .clone()
    }

    fn handle_toolbar_hotkey(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        if self.view.is_active()
            || self.view.base().is_active()
            || self.view.base().base().is_active()
            || self.view.base().base().base().is_active()
        {
            return None;
        }
        let TuiEvent::Key(key) = event else {
            return None;
        };
        let keys = self.composer_keys();
        if keys.refresh.matches(*key) && self.can_refresh.get() {
            self.toolbar_feedback.request_refresh();
            self.ensure_source(true, ctx);
        } else if keys.commit.matches(*key) && self.can_submit.get() {
            self.open_submit_confirmation(ctx);
        } else {
            return None;
        }
        ctx.request_layout();
        ctx.request_redraw();
        ctx.request_tick();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_ticket_number_jump(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        if self.table().is_searching()
            || (!self.ticket_list_is_focused() && self.table().transform_state().search.is_empty())
        {
            return None;
        }
        let TuiEvent::Key(key) = event else {
            return None;
        };
        if self.number_jump.borrow().cancels(*key) {
            self.number_jump.borrow_mut().clear();
            ctx.request_redraw();
            ctx.stop_propagation();
            return Some(EventOutcome::Handled);
        }
        if self.number_jump.borrow().accepts(*key) {
            let number = self
                .number_jump
                .borrow()
                .query()
                .unwrap_or_default()
                .to_owned();
            let row_id = self.exact_ticket_row_id(&number);
            self.number_jump.borrow_mut().clear();
            if let Some(row_id) = row_id {
                self.jump_to_ticket(row_id, ctx);
            }
            ctx.request_redraw();
            ctx.stop_propagation();
            return Some(EventOutcome::Handled);
        }
        if !self.number_jump.borrow_mut().push(*key) {
            return None;
        }
        let number = self
            .number_jump
            .borrow()
            .query()
            .unwrap_or_default()
            .to_owned();
        let matching_count = self
            .table()
            .rows()
            .iter()
            .filter(|row| {
                crate::components::ticket_number_jump::ticket_number_matches(&row.item.key, &number)
            })
            .count();
        self.table_mut().expand_all();
        if matching_count == 1 {
            if let Some(row_id) = self.exact_ticket_row_id(&number) {
                self.number_jump.borrow_mut().clear();
                self.jump_to_ticket(row_id, ctx);
            }
        }
        ctx.request_redraw();
        ctx.request_tick();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn exact_ticket_row_id(&self, number: &str) -> Option<String> {
        self.table().rows().iter().find_map(|row| {
            exact_ticket_number_matches(&row.item.key, number).then(|| row.item.id.clone())
        })
    }

    fn jump_to_ticket(&mut self, row_id: String, ctx: &mut EventCtx<()>) {
        let table = self.table_mut();
        table.highlight_id(&row_id);
        table.reveal_highlighted_centered();
        self.pending
            .borrow_mut()
            .push(ComposerAction::SelectTicket(Some(row_id)));
        self.drain_outputs(ctx);
    }

    fn open_ticket_action_dialog(&mut self, ctx: &mut EventCtx<()>) -> bool {
        let Some(change) = self.state.borrow().selected_change().cloned() else {
            return false;
        };
        if change.is_submitted() || !self.can_change.get() {
            return false;
        }
        let id = change.id.clone();
        let remove_sink = Rc::clone(&self.pending);
        let keys = self.composer_keys();
        let removal_content = {
            let state = self.state.borrow();
            match state.removal_preview(&id) {
                Ok(changes) => {
                    let mut lines = vec![
                        "Remove these local change-set tickets. Jira tickets stay unchanged:"
                            .into(),
                    ];
                    lines.extend(changes.into_iter().map(|change| {
                        let title = state
                            .changes_for_change(change)
                            .map(|ticket| ticket.title.as_str())
                            .unwrap_or("Unavailable ticket");
                        format!("• {} · {title}", change.id)
                    }));
                    lines
                }
                Err(error) => vec![format!("Remove blocked: {error}")],
            }
        };
        self.ticket_dialog_close_requested.set(false);
        let mut actions = Vec::new();
        if !matches!(change.kind, ChangeKind::Added | ChangeKind::Deleted) {
            let delete_sink = Rc::clone(&self.pending);
            let delete_id = id.clone();
            actions.push(
                DialogAction::new("Delete")
                    .hotkey(keys.delete.spec())
                    .on_trigger(move || {
                        delete_sink
                            .borrow_mut()
                            .push(ComposerAction::MarkTicketDeleted(delete_id.clone()));
                    }),
            );
        }
        actions.push(
            DialogAction::new("Remove")
                .hotkey(keys.remove.spec())
                .on_trigger(move || {
                    remove_sink
                        .borrow_mut()
                        .push(ComposerAction::RemoveTicket(id.clone()));
                }),
        );
        let cancel_ticket_dialog = Rc::clone(&self.ticket_dialog_close_requested);
        actions.push(
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || cancel_ticket_dialog.set(true)),
        );
        let dialog = self.view.base_mut().layer_mut();
        dialog.set_top_left("Ticket action");
        dialog.set_actions(actions);
        dialog.set_content(removal_content);
        self.view.base_mut().set_active_with_context(true, ctx);
        true
    }

    fn open_restore_reset_dialog(&mut self, ctx: &mut EventCtx<()>) -> bool {
        self.ticket_dialog_close_requested.set(false);
        let mut actions = Vec::new();
        let keys = self.composer_keys();
        let change = self.state.borrow().selected_change().cloned();
        let message = if let Some(change) = &change {
            let can_recover = !change.is_submitted() && self.can_change.get();
            if can_recover && change.kind == ChangeKind::Deleted {
                let restore_sink = Rc::clone(&self.pending);
                let restore_id = change.id.clone();
                actions.push(
                    DialogAction::new("Restore")
                        .hotkey(keys.restore.spec())
                        .on_trigger(move || {
                            restore_sink
                                .borrow_mut()
                                .push(ComposerAction::RestoreTicket(restore_id.clone()));
                        }),
                );
            }
            if can_recover && change.kind == ChangeKind::Modified {
                let reset_sink = Rc::clone(&self.pending);
                let reset_id = change.id.clone();
                actions.push(
                    DialogAction::new("Reset")
                        .hotkey(keys.reset.spec())
                        .on_trigger(move || {
                            reset_sink
                                .borrow_mut()
                                .push(ComposerAction::ResetTicket(reset_id.clone()));
                        }),
                );
            }
            if actions.is_empty() {
                "No restore or reset action is available for this ticket."
            } else if change.kind == ChangeKind::Deleted {
                "Restore this ticket to cancel its deletion."
            } else {
                "Reset this ticket to discard its local changes."
            }
        } else {
            "No ticket is selected."
        };
        let cancel_ticket_dialog = Rc::clone(&self.ticket_dialog_close_requested);
        actions.push(
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || cancel_ticket_dialog.set(true)),
        );
        let dialog = self.view.base_mut().layer_mut();
        dialog.set_top_left("Restore or reset");
        dialog.set_actions(actions);
        dialog.set_content([message]);
        self.view.base_mut().set_active_with_context(true, ctx);
        true
    }

    fn handle_ticket_action_key(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        if self.view.is_active()
            || self.view.base().is_active()
            || self.view.base().base().is_active()
            || self.view.base().base().base().is_active()
            || !matches!(event, TuiEvent::Key(key) if self.composer_keys().ticket_action.matches(*key))
        {
            return None;
        }
        self.open_ticket_action_dialog(ctx).then(|| {
            ctx.stop_propagation();
            EventOutcome::Handled
        })
    }

    fn handle_restore_reset_key(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        if self.view.is_active()
            || self.view.base().is_active()
            || self.view.base().base().is_active()
            || self.view.base().base().base().is_active()
            || !matches!(event, TuiEvent::Key(key) if self.composer_keys().restore_reset.matches(*key))
        {
            return None;
        }
        self.open_restore_reset_dialog(ctx).then(|| {
            ctx.stop_propagation();
            EventOutcome::Handled
        })
    }

    pub(super) fn focus_tickets(ctx: &mut EventCtx<()>) {
        ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
    }

    fn handle_exit(
        &mut self,
        event: &TuiEvent,
        create_dialog_was_open: bool,
        add_menu_was_open: bool,
        ticket_dialog_was_open: bool,
        description_reader_was_open: bool,
        ctx: &mut EventCtx<()>,
    ) {
        let leaving_key =
            matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key));
        if !leaving_key || add_menu_was_open || description_reader_was_open {
            return;
        }
        if ticket_dialog_was_open {
            if ctx.propagation() == tuicore::Propagation::Stopped {
                return;
            }
            self.view.base_mut().set_active_with_context(false, ctx);
        } else if create_dialog_was_open {
            if ctx.propagation() == tuicore::Propagation::Stopped {
                return;
            }
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .set_active_with_context(false, ctx);
        } else if !self.ticket_list_is_focused() {
            Self::focus_tickets(ctx);
            ctx.request_redraw();
        } else {
            let _ = self
                .state
                .borrow_mut()
                .dispatch(ComposerAction::CloseChangeSet);
            ctx.request_layout();
            ctx.request_redraw();
        }
        ctx.stop_propagation();
    }
}

fn restore_ticket_selection(table: &mut DataView<TicketRow, String>, selected: Vec<String>) {
    if table.selected_ids() == selected {
        return;
    }
    table.clear_selection();
    for id in selected {
        table.select_id(id);
    }
    table.drain_events();
}

fn ticket_kind_label(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Epic => "Epic",
        TicketKind::Story => "Story",
        TicketKind::Task => "Task",
        TicketKind::Bug => "Bug",
        TicketKind::Subtask => "Sub-task",
    }
}

fn loading_view() -> ScrollContainer<Flex<()>> {
    ScrollContainer::vertical(
        Flex::column()
            .justify(MainAlign::Center)
            .align(CrossAlign::Center)
            .child(
                "loading",
                Flex::row()
                    .gap(1)
                    .align(CrossAlign::Center)
                    .child("spinner", Spinner::new(), FlexItem::fit_content())
                    .child(
                        "message",
                        Paragraph::new("Loading Jira tickets…"),
                        FlexItem::fit_content(),
                    ),
                FlexItem::fit_content(),
            ),
    )
}

fn description_reader(
    description: impl Into<String>,
    actions: PendingDescriptionActions,
    settings: SpeedReaderSettings,
) -> DescriptionReader {
    settings
        .apply(SpeedReader::markdown(description).title("Description"))
        .dialog(move |_| {
            actions
                .borrow_mut()
                .push(DescriptionAction::CloseSpeedReader);
        })
}

impl TuiNode for TicketEditor {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if self.opening_loading {
            self.loading_view.measure(proposal)
        } else {
            self.view.measure(proposal)
        }
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if self.opening_loading {
            ctx.with_focus_fallback(FocusId::new("composer-loading"), area, |ctx| {
                self.loading_view.layout(area, ctx)
            })
        } else {
            let body_height = area.height.saturating_sub(1);
            let ticket_height = (body_height / 3).saturating_sub(4).max(1);
            self.body_mut()
                .set_constraints(Constraint::Length(ticket_height), Constraint::Fill(1));
            self.sync();
            self.view.layout(area, ctx)
        }
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if self.opening_loading {
            self.loading_view.render(frame, area, ctx);
        } else {
            self.view.render(frame, area, ctx);
        }
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.opening_loading {
            let leaving_key =
                matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key));
            if leaving_key {
                self.opening_loading = false;
                let _ = self
                    .state
                    .borrow_mut()
                    .dispatch(ComposerAction::CloseChangeSet);
                ctx.request_layout();
                ctx.request_redraw();
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
            return self.loading_view.event(event, ctx);
        }
        self.poll_submission();
        let create_dialog_open = self.view.base().base().base().is_active();
        let add_menu_open = self.view.base().base().is_active();
        let ticket_dialog_open = self.view.base().is_active();
        let description_reader_open = self.description_reader_is_open();
        if let Some(outcome) = self.handle_ticket_number_jump(event, ctx) {
            return outcome;
        }
        let outcome = self.view.event(event, ctx);
        self.drain_outputs(ctx);
        self.handle_exit(
            event,
            create_dialog_open,
            add_menu_open,
            ticket_dialog_open,
            description_reader_open,
            ctx,
        );
        if outcome == EventOutcome::Handled
            || matches!(ctx.propagation(), tuicore::Propagation::Stopped)
        {
            return outcome;
        }
        if let Some(outcome) = self.handle_toolbar_hotkey(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_ticket_action_key(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_restore_reset_key(event, ctx) {
            return outcome;
        }
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.opening_loading {
            let leaving_key =
                matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key));
            if leaving_key {
                self.opening_loading = false;
                let _ = self
                    .state
                    .borrow_mut()
                    .dispatch(ComposerAction::CloseChangeSet);
                ctx.request_layout();
                ctx.request_redraw();
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
            return self.loading_view.dispatch_event(route, event, ctx);
        }
        self.poll_submission();
        let create_dialog_open = self.view.base().base().base().is_active();
        let add_menu_open = self.view.base().base().is_active();
        let ticket_dialog_open = self.view.base().is_active();
        let description_reader_open = self.description_reader_is_open();
        if let Some(outcome) = self.handle_ticket_number_jump(event, ctx) {
            return outcome;
        }
        let outcome = self.view.dispatch_event(route, event, ctx);
        self.drain_outputs(ctx);
        self.handle_exit(
            event,
            create_dialog_open,
            add_menu_open,
            ticket_dialog_open,
            description_reader_open,
            ctx,
        );
        if outcome == EventOutcome::Handled
            || matches!(ctx.propagation(), tuicore::Propagation::Stopped)
        {
            return outcome;
        }
        if let Some(outcome) = self.handle_toolbar_hotkey(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_ticket_action_key(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_restore_reset_key(event, ctx) {
            return outcome;
        }
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let was_loading = self.source.is_loading() || self.submission.is_submitting();
        let changed = self.poll_submission();
        self.source.ensure_selected();
        let source_changed = self.source.drain();
        if changed || source_changed {
            self.sync();
        }
        if self.opening_loading {
            if !self.source.is_loading() {
                self.opening_loading = false;
                self.pending_focus_tickets = true;
                self.sync();
            }
            let view_result = if self.opening_loading {
                self.loading_view.tick(dt, settings)
            } else {
                self.view.tick(dt, settings)
            };
            let loader_completed = !self.opening_loading;
            return view_result
                .merge(if loader_completed {
                    TickResult {
                        changed: true,
                        layout: true,
                        active: false,
                        next_tick: None,
                    }
                } else if changed || source_changed {
                    TickResult::CHANGED
                } else {
                    TickResult::IDLE
                })
                .merge(
                    if self.opening_loading || was_loading || self.source.is_loading() {
                        TickResult::scheduled_after(Duration::from_millis(50))
                    } else {
                        TickResult::IDLE
                    },
                );
        }
        let number_jump = {
            let mut jump = self.number_jump.borrow_mut();
            if jump.advance(dt) {
                TickResult::CHANGED
            } else {
                jump.remaining()
                    .map_or(TickResult::IDLE, TickResult::scheduled_after)
            }
        };
        self.view
            .tick(dt, settings)
            .merge(if changed || source_changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
            .merge(
                if was_loading || self.submission.is_submitting() || self.source.is_loading() {
                    TickResult::scheduled_after(Duration::from_millis(50))
                } else {
                    TickResult::IDLE
                },
            )
            .merge(number_jump)
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.opening_loading {
            self.loading_view.focus(target, focused, ctx);
        } else {
            self.view.focus(target, focused, ctx);
        }
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.opening_loading {
            self.loading_view.dispatch_focus(target, focused, ctx);
        } else {
            if self.pending_focus_tickets && focused {
                let data_view_id = FocusId::new("data-view");
                if target.id != data_view_id {
                    ctx.focus(FocusRequest::Target(data_view_id));
                    return;
                }
                self.pending_focus_tickets = false;
            }
            self.view.dispatch_focus(target, focused, ctx);
        }
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.init(ctx);
        self.view.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.mount(ctx);
        self.view.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.unmount(ctx);
        self.view.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.destroy(ctx);
        self.view.destroy(ctx);
    }
}
