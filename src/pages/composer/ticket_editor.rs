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
    FocusTarget, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    Panel, PanelHost, RenderCtx, SpeedReader, Split, TextInput, TextInputKeyBindings, TickResult,
    TuiEvent, TuiNode, keybindings,
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
    submission::SubmissionController,
    ticket_rows::{TicketRow, ticket_data_view, ticket_rows},
    ticket_toolbar::{ToolbarEvent, ToolbarEvents, toolbar},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type TicketList = PanelHost<DataView<TicketRow, String>>;
type Body = Split<TicketList, DetailPane>;
type Workspace = Split<Flex<()>, Body>;
type CreateDialog = tuicore::DialogHost<TextInput, ()>;
type CreateLayer = DialogLayer<Workspace, CreateDialog>;
type AddLayer = DialogLayer<CreateLayer, AddTicketMenu>;
type TicketEditorView = DialogLayer<AddLayer, Dialog<()>>;
type DescriptionReader = tuicore::DialogHost<SpeedReader, ()>;
type EditorView = DialogLayer<TicketEditorView, DescriptionReader>;

pub(super) struct TicketEditor {
    state: Rc<RefCell<ComposerState>>,
    pending: PendingActions,
    description_actions: PendingDescriptionActions,
    settings: Arc<RwLock<AppSettings>>,
    service: AppService,
    view: EditorView,
    toolbar_events: ToolbarEvents,
    can_change: Rc<Cell<bool>>,
    can_submit: Rc<Cell<bool>>,
    pending_project: Rc<RefCell<Option<String>>>,
    submission: SubmissionController,
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
        );
        let body = Split::vertical(ticket_list, detail).ratio(1, 2);
        let toolbar_events = Rc::new(RefCell::new(Vec::new()));
        let can_change = Rc::new(Cell::new(true));
        let can_submit = Rc::new(Cell::new(false));
        let workspace = Split::vertical(
            toolbar(
                Rc::clone(&toolbar_events),
                Rc::clone(&can_change),
                Rc::clone(&can_submit),
            ),
            body,
        )
        .constraints(Constraint::Length(1), Constraint::Min(1));

        let create_sink = Rc::clone(&pending);
        let pending_project = Rc::new(RefCell::new(None::<String>));
        let create_project = Rc::clone(&pending_project);
        let create_keys = TextInputKeyBindings::default();
        let create_help = format!(
            "{} create · {} cancel",
            create_keys
                .submit
                .first()
                .map_or_else(|| "Enter".into(), |key| (*key).label()),
            create_keys
                .cancel
                .first()
                .map_or_else(|| "Esc".into(), |key| (*key).label()),
        );
        let create_dialog = Dialog::new()
            .top_left("Create Jira ticket")
            .bottom_left(create_help)
            .host(
                TextInput::new()
                    .keybindings(create_keys)
                    .panel("Title")
                    .placeholder("Ticket title")
                    .on_submit(move |title| {
                        if !title.trim().is_empty()
                            && let Some(project_key) = create_project.borrow().clone()
                        {
                            create_sink.borrow_mut().push(ComposerAction::CreateTicket {
                                title: title.trim().into(),
                                project_key,
                            });
                        }
                    }),
            );
        let create_layer = DialogLayer::new(workspace, create_dialog)
            .active(false)
            .fit_content()
            .fit_content_max(72, 7)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let add_layer = DialogLayer::new(create_layer, AddTicketMenu::new(service.clone()))
            .active(false)
            .fit_content()
            .fit_content_max(120, 12)
            .backdrop(DialogBackdrop::dim().amount(0.55));
        let ticket_action_dialog = Dialog::new()
            .top_left("Ticket action")
            .content(["Choose what should happen to the selected ticket."]);
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

        Self {
            state,
            pending,
            description_actions,
            settings,
            service,
            view,
            toolbar_events,
            can_change,
            can_submit,
            pending_project,
            submission,
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

    pub(super) fn sync(&mut self) {
        let (breadcrumb, rows, selected, submitted, is_open) = {
            let state = self.state.borrow();
            let breadcrumb = state.active_set().map_or_else(
                || "Change sets".into(),
                |set| format!("Change sets > {}", set.name),
            );
            let submitted = state
                .active_set()
                .into_iter()
                .flat_map(|set| &set.tickets)
                .filter(|change| change.is_submitted())
                .map(|change| change.id.clone())
                .collect::<Vec<_>>();
            (
                breadcrumb,
                ticket_rows(&state),
                state.selected_ticket.clone(),
                submitted,
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
        if let Some(selected) = selected {
            table.highlight_id(&selected);
        }
        for id in submitted {
            if self.table().is_selected(&id) {
                self.table_mut().toggle_selected(id);
            }
        }
        self.can_change
            .set(is_open && !self.submission.is_submitting());
        self.can_submit.set(
            is_open && !self.submission.is_submitting() && !self.table().selected_ids().is_empty(),
        );
    }

    fn drain_description_actions(&mut self, ctx: &mut EventCtx<()>) {
        let actions = self
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for action in actions {
            match action {
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
                        .set_value("");
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
                DataViewTypedEvent::HighlightChanged { row_id } => self
                    .pending
                    .borrow_mut()
                    .push(ComposerAction::SelectTicket(row_id)),
                DataViewTypedEvent::SelectionChanged { .. } => {}
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
            .filter_map(|change| change.visible_ticket(true))
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
                .set_value("");
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
                ToolbarEvent::Submit => self.start_submit(),
            }
        }
    }

    fn start_submit(&mut self) {
        if self.submission.is_submitting() {
            return;
        }
        let selected = self.table().selected_ids();
        let changes = self
            .state
            .borrow()
            .active_set()
            .into_iter()
            .flat_map(|set| &set.tickets)
            .filter(|change| selected.contains(&change.id) && !change.is_submitted())
            .cloned()
            .collect::<Vec<_>>();
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

    fn open_ticket_action_dialog(&mut self, ctx: &mut EventCtx<()>) -> bool {
        let Some(change) = self.state.borrow().selected_change().cloned() else {
            return false;
        };
        if change.is_submitted() || !self.can_change.get() {
            return false;
        }
        let id = change.id.clone();
        let remove_sink = Rc::clone(&self.pending);
        let mut actions = Vec::new();
        if change.kind != ChangeKind::Added {
            let delete_sink = Rc::clone(&self.pending);
            let delete_id = id.clone();
            actions.push(
                DialogAction::new("Mark for deletion")
                    .hotkey(KeySpec::plain('d'))
                    .on_trigger(move || {
                        delete_sink
                            .borrow_mut()
                            .push(ComposerAction::MarkTicketDeleted(delete_id.clone()));
                    }),
            );
        }
        actions.push(
            DialogAction::new("Remove from change set")
                .hotkey(KeySpec::plain('r'))
                .on_trigger(move || {
                    remove_sink
                        .borrow_mut()
                        .push(ComposerAction::RemoveTicket(id.clone()));
                }),
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
            || !matches!(event, TuiEvent::Key(key) if KeySpec::plain('-').matches(*key))
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
        self.sync();
        self.view.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.view.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.poll_submission(ctx);
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
        if changed {
            self.sync();
        }
        self.view
            .tick(dt, settings)
            .merge(if changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
            .merge(if self.submission.is_submitting() {
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
