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
    AnimationSettings, DataView, DataViewTypedEvent, Dialog, DialogAction, DialogBackdrop,
    DialogLayer, EventCtx, EventOutcome, EventRoute, Flex, FocusCtx, FocusId, FocusRequest,
    FocusTarget, Key, KeyEvent, KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, Panel, PanelHost, RenderCtx, SpeedReader, Split, TextInput,
    TextInputKeyBindings, TickResult, TuiEvent, TuiNode, keybindings,
};

use crate::{
    app_settings::AppSettings,
    service::AppService,
    speed_reader_settings::SpeedReaderSettings,
    store::composer::{ChangeKind, ComposerAction, ComposerState},
};

use super::{
    add_ticket_menu::{AddTicketEvent, AddTicketMenu},
    detail::DetailPane,
    fields::{DescriptionAction, PendingDescriptionActions},
    source::SourceController,
    submission::SubmissionController,
    ticket_rows::{TicketRow, set_active_ticket_style, ticket_data_view, ticket_rows},
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
type EditorView = DialogLayer<TicketEditorView, DescriptionReader>;

struct CreateTicketForm {
    input: TextInput<()>,
    feedback: TitleFeedback,
    on_ctrl_enter: Box<dyn Fn(String)>,
    input_area: Rect,
    feedback_area: Rect,
}

impl CreateTicketForm {
    fn new(
        input: TextInput<()>,
        feedback: TitleFeedback,
        on_ctrl_enter: impl Fn(String) + 'static,
    ) -> Self {
        let mut input = input;
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        Self {
            input,
            feedback,
            on_ctrl_enter: Box::new(on_ctrl_enter),
            input_area: Rect::default(),
            feedback_area: Rect::default(),
        }
    }

    fn clear(&mut self) {
        self.input.set_value("");
        self.feedback.clear();
    }

    fn submit_on_ctrl_enter(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }) = event
        else {
            return false;
        };
        if !self.input.insert_mode() {
            return false;
        }
        (self.on_ctrl_enter)(self.input.current_value().to_owned());
        ctx.request_redraw();
        ctx.stop_propagation();
        true
    }
}

impl TuiNode for CreateTicketForm {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let input = self.input.measure(proposal);
        let feedback = self.feedback.measure(proposal);
        LayoutSizeHint::content(
            input.preferred.width.max(feedback.preferred.width).max(80),
            input
                .preferred
                .height
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
        let feedback_y = area.y.saturating_add(input_height.saturating_add(1));
        self.feedback_area = Rect::new(
            area.x,
            feedback_y,
            area.width,
            area.bottom().saturating_sub(feedback_y),
        );
        self.input.layout(self.input_area, ctx);
        self.feedback.layout(self.feedback_area, ctx);
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        TuiNode::render(&self.input, frame, self.input_area, ctx);
        self.feedback.render(frame, self.feedback_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.submit_on_ctrl_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        self.input.event(event, ctx)
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
        self.input.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
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
    can_refresh: Rc<Cell<bool>>,
    can_submit: Rc<Cell<bool>>,
    pending_project: Rc<RefCell<Option<String>>>,
    create_dialog_close_requested: Rc<Cell<bool>>,
    ticket_dialog_close_requested: Rc<Cell<bool>>,
    submission: SubmissionController,
    source: SourceController,
}

impl TicketEditor {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        settings: Arc<RwLock<AppSettings>>,
        service: AppService,
    ) -> Self {
        let pending = Rc::new(RefCell::new(Vec::new()));
        let description_actions = Rc::new(RefCell::new(Vec::new()));
        let ticket_list = Panel::new()
            .top_left("Change sets")
            .one_row(true)
            .host(ticket_data_view(&state.borrow()));

        let detail = DetailPane::new(
            Rc::clone(&state),
            Rc::clone(&pending),
            Rc::clone(&description_actions),
            service.clone(),
        );
        let body = Split::vertical(ticket_list, detail).ratio(1, 2);
        let toolbar_events = Rc::new(RefCell::new(Vec::new()));
        let toolbar_feedback = ToolbarFeedback::new();
        let can_change = Rc::new(Cell::new(true));
        let can_refresh = Rc::new(Cell::new(false));
        let can_submit = Rc::new(Cell::new(false));
        let workspace = Split::vertical(
            toolbar(
                Rc::clone(&toolbar_events),
                toolbar_feedback.clone(),
                Rc::clone(&can_change),
                Rc::clone(&can_refresh),
                Rc::clone(&can_submit),
            ),
            body,
        )
        .constraints(Constraint::Length(1), Constraint::Min(1));

        let create_sink = Rc::clone(&pending);
        let pending_project = Rc::new(RefCell::new(None::<String>));
        let create_project = Rc::clone(&pending_project);
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
                    .push(ComposerAction::CreateTicket { title, project_key });
            }
        });
        let input_submit = Rc::clone(&submit_ticket);
        let ok_submit = Rc::clone(&submit_ticket);
        let ok_title = Rc::clone(&title);
        let cancel_action = Rc::clone(&create_dialog_close_requested);
        let close_action = Rc::clone(&create_dialog_close_requested);
        let create_dialog = Dialog::new()
            .top_left("Create ticket")
            .actions([
                DialogAction::new("OK")
                    .hotkey(KeySpec::plain('o'))
                    .on_trigger(move || ok_submit(ok_title.borrow().clone())),
                DialogAction::new("Cancel")
                    .hotkey(KeySpec::plain('c'))
                    .on_trigger(move || cancel_action.set(true)),
            ])
            .close_on_unfocus_from_descendants(true)
            .on_close(move |_| close_action.set(true))
            .host(CreateTicketForm::new(
                TextInput::new()
                    .keybindings(create_keys)
                    .placeholder("Ticket title")
                    .focused(true)
                    .on_change(move |value| *title_input.borrow_mut() = value)
                    .on_submit(move |title| input_submit(title)),
                title_feedback,
                move |title| submit_ticket(title),
            ));
        let create_layer = DialogLayer::new(workspace, create_dialog)
            .active(false)
            .layer_percent(60)
            .layer_cross_percent(50)
            .fit_content()
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let add_layer = DialogLayer::new(create_layer, AddTicketMenu::new(service.clone()))
            .active(false)
            .fit_content()
            .fit_content_max(120, 12)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let ticket_dialog_close_requested = Rc::new(Cell::new(false));
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
            can_refresh,
            can_submit,
            pending_project,
            create_dialog_close_requested,
            ticket_dialog_close_requested,
            submission,
            source,
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
            let breadcrumb = state.active_set().map_or_else(
                || "Change sets".into(),
                |set| format!("Change sets > {}", set.name),
            );
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
        set_active_ticket_style(table, selected.clone());
        restore_ticket_selection(table, selected_for_submission);
        if let Some(selected) = selected {
            table.highlight_id(&selected);
        }
        self.can_change
            .set(is_open && !self.submission.is_submitting());
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

    fn drain_description_actions(&mut self, ctx: &mut EventCtx<()>) {
        let actions = self
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for action in actions {
            match action {
                DescriptionAction::ShowChanges => {
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::SetViewMode(
                            crate::store::composer::ComposerViewMode::Changes,
                        ))
                }
                DescriptionAction::Focus { edit } => {
                    self.detail_mut().focus_description(edit, ctx);
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
        if self.ticket_dialog_close_requested.replace(false) {
            self.view.base_mut().set_active_with_context(false, ctx);
        }
        let add_events = self.view.base_mut().base_mut().layer_mut().take_events();
        for event in add_events {
            match event {
                AddTicketEvent::CreateNew(project_key) => {
                    *self.pending_project.borrow_mut() = Some(project_key);
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
                    self.view
                        .base_mut()
                        .base_mut()
                        .base_mut()
                        .set_active_with_context(true, ctx);
                }
                AddTicketEvent::Include(ticket) => {
                    self.pending
                        .borrow_mut()
                        .push(ComposerAction::IncludeTicket(ticket));
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
        let created = actions
            .iter()
            .any(|action| matches!(action, ComposerAction::CreateTicket { .. }));
        let ticket_action = actions.iter().any(|action| {
            matches!(
                action,
                ComposerAction::MarkTicketDeleted(_) | ComposerAction::RemoveTicket(_)
            )
        });
        let persist = actions.iter().any(ComposerAction::affects_persistence);
        for action in actions {
            self.state.borrow_mut().dispatch(action);
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
        }
        if ticket_action {
            self.view.base_mut().set_active_with_context(false, ctx);
        }
        self.sync();
        self.source.ensure_selected();
        ctx.request_layout();
        self.drain_toolbar_events(ctx);
        self.submission.drain_notices(ctx);
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

    fn open_add_menu(&mut self, ctx: &mut EventCtx<()>) {
        let project_hint = self.project_hint();
        self.view
            .base_mut()
            .base_mut()
            .layer_mut()
            .open(project_hint);
        self.view
            .base_mut()
            .base_mut()
            .set_active_with_context(true, ctx);
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

    fn open_new_ticket(&mut self, ctx: &mut EventCtx<()>) {
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
                .set_active_with_context(true, ctx);
        } else {
            self.view
                .base_mut()
                .base_mut()
                .layer_mut()
                .open_projects(ctx);
            self.view
                .base_mut()
                .base_mut()
                .set_active_with_context(true, ctx);
        }
    }

    fn drain_toolbar_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self
            .toolbar_events
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for event in events {
            match event {
                ToolbarEvent::NewTicket => self.open_new_ticket(ctx),
                ToolbarEvent::Refresh => self.ensure_source(true, ctx),
                ToolbarEvent::Submit => self.start_submit(),
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

    fn start_submit(&mut self) {
        if self.submission.is_submitting() {
            return;
        }
        let selected = self.table().selected_ids();
        let changes = {
            let state = self.state.borrow();
            state
                .active_set()
                .into_iter()
                .flat_map(|set| &set.tickets)
                .filter(|change| selected.contains(&change.id) && !change.is_submitted())
                .cloned()
                .filter_map(|mut change| {
                    if change.kind != ChangeKind::Added {
                        change.original = Some(state.sources.get(&change.id)?.clone());
                    }
                    Some(change)
                })
                .collect::<Vec<_>>()
        };
        self.submission.start(changes);
        self.sync();
    }

    fn poll_submission(&mut self, ctx: &mut EventCtx<()>) {
        if self.submission.drain_results() {
            self.sync();
        }
        self.submission.drain_notices(ctx);
    }

    fn handle_add_key(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> Option<EventOutcome> {
        if self.view.is_active()
            || self.view.base().is_active()
            || self.view.base().base().is_active()
            || self.view.base().base().base().is_active()
            || !self.can_change.get()
            || !self.ticket_list_is_focused()
            || !matches!(event, TuiEvent::Key(key) if KeySpec::plain('+').matches(*key))
        {
            return None;
        }
        self.open_add_menu(ctx);
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
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
        if KeySpec::shifted('r').matches(*key) && self.can_refresh.get() {
            self.toolbar_feedback.request_refresh();
            self.ensure_source(true, ctx);
        } else if KeySpec::shifted('s').matches(*key) && self.can_submit.get() {
            self.toolbar_feedback.request_submit();
            self.start_submit();
        } else {
            return None;
        }
        ctx.request_layout();
        ctx.request_redraw();
        ctx.request_tick();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
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
        self.ticket_dialog_close_requested.set(false);
        let mut actions = Vec::new();
        if change.kind != ChangeKind::Added {
            let delete_sink = Rc::clone(&self.pending);
            let delete_id = id.clone();
            actions.push(
                DialogAction::new("Delete")
                    .hotkey(KeySpec::plain('d'))
                    .on_trigger(move || {
                        delete_sink
                            .borrow_mut()
                            .push(ComposerAction::MarkTicketDeleted(delete_id.clone()));
                    }),
            );
        }
        actions.push(
            DialogAction::new("Remove")
                .hotkey(KeySpec::plain('r'))
                .on_trigger(move || {
                    remove_sink
                        .borrow_mut()
                        .push(ComposerAction::RemoveTicket(id.clone()));
                }),
        );
        let cancel_ticket_dialog = Rc::clone(&self.ticket_dialog_close_requested);
        actions.push(
            DialogAction::new("Cancel")
                .hotkey(KeySpec::plain('c'))
                .on_trigger(move || cancel_ticket_dialog.set(true)),
        );
        self.view.base_mut().layer_mut().set_actions(actions);
        self.view
            .base_mut()
            .layer_mut()
            .set_content([if change.kind == ChangeKind::Added {
            "This ticket does not exist in Jira and can only be removed from the change set."
        } else {
            "Mark the Jira ticket for deletion, or remove it from this change set without changing Jira."
        }]);
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
            || !self.ticket_list_is_focused()
            || !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Char('x'), KeyModifiers::CONTROL).matches(*key))
        {
            return None;
        }
        self.open_ticket_action_dialog(ctx).then(|| {
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
            self.view.base_mut().set_active_with_context(false, ctx);
        } else if create_dialog_was_open {
            self.view
                .base_mut()
                .base_mut()
                .base_mut()
                .set_active_with_context(false, ctx);
        } else if !self.ticket_list_is_focused() {
            Self::focus_tickets(ctx);
            ctx.request_redraw();
        } else {
            self.state
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
        self.view.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let body_height = area.height.saturating_sub(1);
        let ticket_height = (body_height / 3).saturating_sub(4).max(1);
        self.body_mut()
            .set_constraints(Constraint::Length(ticket_height), Constraint::Fill(1));
        self.sync();
        self.view.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.view.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.poll_submission(ctx);
        if let Some(outcome) = self.handle_toolbar_hotkey(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_add_key(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_ticket_action_key(event, ctx) {
            return outcome;
        }
        let create_dialog_open = self.view.base().base().base().is_active();
        let add_menu_open = self.view.base().base().is_active();
        let ticket_dialog_open = self.view.base().is_active();
        let description_reader_open = self.description_reader_is_open();
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
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.poll_submission(ctx);
        if let Some(outcome) = self.handle_toolbar_hotkey(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_add_key(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_ticket_action_key(event, ctx) {
            return outcome;
        }
        let create_dialog_open = self.view.base().base().base().is_active();
        let add_menu_open = self.view.base().base().is_active();
        let ticket_dialog_open = self.view.base().is_active();
        let description_reader_open = self.description_reader_is_open();
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
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.submission.drain_results();
        self.source.ensure_selected();
        let source_changed = self.source.drain();
        if changed || source_changed {
            self.sync();
        }
        self.view
            .tick(dt, settings)
            .merge(if changed || source_changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
            .merge(if self.submission.is_submitting() {
                TickResult::scheduled_after(Duration::from_millis(50))
            } else {
                TickResult::IDLE
            })
            .merge(if self.source.is_loading() {
                TickResult::scheduled_after(Duration::from_millis(50))
            } else {
                TickResult::IDLE
            })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.view.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.view.dispatch_focus(target, focused, ctx);
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
