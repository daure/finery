use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownSearchMode, DropdownVariant, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal,
    LayoutResult, LayoutSizeHint, LifecycleCtx, Panel, PanelHost, RenderCtx, TickResult, TuiEvent,
    TuiNode,
};

use crate::{
    jira::{JiraAssignee, JiraFieldOptions, JiraOption},
    service::AppService,
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, Ticket, TicketKind},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type PropertyDropdown = PanelHost<Dropdown<JiraOption, String>>;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq)]
enum PropertyKind {
    IssueType,
    Status,
    Priority,
    Assignee,
}

#[derive(Default)]
struct SharedOptions {
    ticket_id: Option<String>,
    values: Option<JiraFieldOptions>,
}

pub(super) struct PropertyFields {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    shared: Rc<RefCell<SharedOptions>>,
    root: Flex<()>,
    sender: Sender<(u64, String, Result<JiraFieldOptions, String>)>,
    receiver: Receiver<(u64, String, Result<JiraFieldOptions, String>)>,
    generation: u64,
}

impl PropertyFields {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
    ) -> Self {
        let shared = Rc::new(RefCell::new(SharedOptions::default()));
        let root = Flex::column()
            .child(
                "kind",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::IssueType,
                ),
                FlexItem::fixed(3),
            )
            .child(
                "status",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::Status,
                ),
                FlexItem::fixed(3),
            )
            .child(
                "priority",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::Priority,
                ),
                FlexItem::fixed(3),
            )
            .child(
                "assignee",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    pending,
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::Assignee,
                ),
                FlexItem::fixed(3),
            );
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            shared,
            root,
            sender,
            receiver,
            generation: 0,
        }
    }

    fn ensure_options(&mut self) {
        if !self.state.borrow().remote_queries_allowed() {
            if self.shared.borrow().ticket_id.is_some() {
                self.generation = self.generation.saturating_add(1);
            }
            *self.shared.borrow_mut() = SharedOptions::default();
            return;
        }
        let target = self.state.borrow().selected_changes().cloned();
        let Some(ticket) = target else {
            self.shared.borrow_mut().ticket_id = None;
            return;
        };
        let id = ticket.key.clone();
        if self.shared.borrow().ticket_id.as_deref() == Some(id.as_str()) {
            return;
        }
        {
            let mut shared = self.shared.borrow_mut();
            shared.ticket_id = Some(id.clone());
            shared.values = None;
        }
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-fields-{generation}"))
            .spawn(move || {
                let _ = sender.send((generation, id, service.jira_field_options(&ticket)));
            })
        {
            self.service
                .report_error(format!("could not fetch Jira field options: {error}"));
        }
    }

    fn drain_options(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, id, result)) = self.receiver.try_recv() {
            if generation != self.generation
                || self.shared.borrow().ticket_id.as_deref() != Some(id.as_str())
            {
                continue;
            }
            match result {
                Ok(values) => self.shared.borrow_mut().values = Some(values),
                Err(error) => self
                    .service
                    .report_error(format!("Jira field option fetch failed: {error}")),
            }
            changed = true;
        }
        changed
    }
}

struct BoundPropertyDropdown {
    state: Rc<RefCell<ComposerState>>,
    shared: Rc<RefCell<SharedOptions>>,
    service: AppService,
    kind: PropertyKind,
    control: PropertyDropdown,
    labels: Rc<RefCell<HashMap<String, String>>>,
    disabled: bool,
    assignee_sender: Sender<(u64, Result<Vec<JiraAssignee>, String>)>,
    assignee_receiver: Receiver<(u64, Result<Vec<JiraAssignee>, String>)>,
    assignee_generation: u64,
    assignee_ticket: Option<String>,
    assignee_query: String,
    pending_search: Option<Duration>,
}

impl BoundPropertyDropdown {
    fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        shared: Rc<RefCell<SharedOptions>>,
        service: AppService,
        kind: PropertyKind,
    ) -> Self {
        let labels = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let callback_labels = Rc::clone(&labels);
        let sink = Rc::clone(&pending);
        let control = Panel::new().top_left(kind.label()).host(
            Dropdown::single(
                [],
                |option: &JiraOption| option.id.clone(),
                |option| option.label.clone(),
            )
            .variant(DropdownVariant::Filled)
            .search_mode(if kind == PropertyKind::Assignee {
                DropdownSearchMode::External
            } else {
                DropdownSearchMode::Fuzzy
            })
            .external_loading_message("Fetching users")
            .hotkey(kind.hotkey())
            .on_select(move |ids| {
                let Some(id) = ids.first() else {
                    return;
                };
                let label = callback_labels
                    .borrow()
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.clone());
                sink.borrow_mut().push(kind.action(id.clone(), label));
            }),
        );
        let (assignee_sender, assignee_receiver) = mpsc::channel();
        Self {
            state,
            shared,
            service,
            kind,
            control,
            labels,
            disabled: false,
            assignee_sender,
            assignee_receiver,
            assignee_generation: 0,
            assignee_ticket: None,
            assignee_query: String::new(),
            pending_search: None,
        }
    }

    fn sync(&mut self) -> bool {
        let (value, footer, disabled) = {
            let state = self.state.borrow();
            let displayed = state.selected_ticket();
            let opposite = if state.view_mode == ComposerViewMode::Source {
                state.selected_changes()
            } else {
                state.selected_source()
            };
            let value = displayed
                .map_or("", |ticket| self.kind.value(ticket))
                .to_owned();
            let opposite_value = opposite.map_or("", |ticket| self.kind.value(ticket));
            let footer = if state.view_mode == ComposerViewMode::Source {
                format!("Changes: {opposite_value}")
            } else {
                format!("Source: {opposite_value}")
            };
            (value, footer, !state.selected_is_editable())
        };
        self.control.panel_mut().set_bottom_left(footer);
        self.disabled = disabled;
        if self.kind != PropertyKind::Assignee {
            let mut options = self.options();
            if !value.is_empty() && !options.iter().any(|option| option.label == value) {
                options.push(JiraOption {
                    id: value.clone(),
                    label: value.clone(),
                });
            }
            self.set_options(options, &value);
        }
        false
    }

    fn options(&self) -> Vec<JiraOption> {
        let shared = self.shared.borrow();
        let Some(values) = shared.values.as_ref() else {
            return Vec::new();
        };
        match self.kind {
            PropertyKind::IssueType => values.issue_types.clone(),
            PropertyKind::Status => values.statuses.clone(),
            PropertyKind::Priority => values.priorities.clone(),
            PropertyKind::Assignee => Vec::new(),
        }
        .into_iter()
        .map(|option| JiraOption {
            id: option.label.clone(),
            label: option.label,
        })
        .collect()
    }

    fn set_options(&mut self, options: Vec<JiraOption>, selected_label: &str) {
        *self.labels.borrow_mut() = options
            .iter()
            .map(|option| (option.id.clone(), option.label.clone()))
            .collect();
        let selected = options
            .iter()
            .find(|option| option.label == selected_label)
            .map(|option| option.id.clone());
        self.control.child_mut().set_rows(options);
        if let Some(selected) = selected {
            self.control.child_mut().set_selected_one(selected);
        }
    }

    fn sync_assignee_search(&mut self, dt: Duration) -> bool {
        if self.kind != PropertyKind::Assignee {
            return false;
        }
        if !self.state.borrow().remote_queries_allowed() {
            if self.assignee_ticket.take().is_some() || self.pending_search.take().is_some() {
                self.assignee_generation = self.assignee_generation.saturating_add(1);
            }
            self.control.child_mut().set_external_loading(false);
            return false;
        }
        let ticket = self.state.borrow().selected_changes().cloned();
        let Some(ticket) = ticket else {
            return false;
        };
        let query = self.control.child().search_query().to_owned();
        if self.assignee_ticket.as_deref() != Some(ticket.key.as_str())
            || query != self.assignee_query
        {
            self.assignee_ticket = Some(ticket.key.clone());
            self.assignee_query = query;
            self.pending_search = Some(Duration::ZERO);
            self.control.child_mut().set_external_loading(true);
        }
        if let Some(elapsed) = &mut self.pending_search {
            *elapsed += dt;
            if *elapsed >= SEARCH_DEBOUNCE {
                self.pending_search = None;
                self.start_assignee_search(ticket);
            }
        }
        let mut changed = false;
        while let Ok((generation, result)) = self.assignee_receiver.try_recv() {
            if generation != self.assignee_generation {
                continue;
            }
            self.control.child_mut().set_external_loading(false);
            match result {
                Ok(users) => {
                    let selected_ticket = self.state.borrow().selected_ticket().cloned();
                    let selected = selected_ticket
                        .as_ref()
                        .map(|ticket| ticket.assignee.clone())
                        .unwrap_or_default();
                    let mut options = users
                        .into_iter()
                        .map(|user| JiraOption {
                            id: user.account_id,
                            label: user.display_name,
                        })
                        .collect::<Vec<_>>();
                    options.insert(
                        0,
                        JiraOption {
                            id: String::new(),
                            label: "Unassigned".into(),
                        },
                    );
                    if let Some(ticket) = selected_ticket
                        && !options.iter().any(|option| option.label == ticket.assignee)
                    {
                        options.push(JiraOption {
                            id: ticket.assignee_account_id,
                            label: ticket.assignee,
                        });
                    }
                    self.set_options(options, &selected);
                }
                Err(error) => self
                    .service
                    .report_error(format!("Jira user search failed: {error}")),
            }
            changed = true;
        }
        changed
    }

    fn start_assignee_search(&mut self, ticket: Ticket) {
        self.assignee_generation = self.assignee_generation.saturating_add(1);
        let generation = self.assignee_generation;
        let query = self.assignee_query.clone();
        let sender = self.assignee_sender.clone();
        let service = self.service.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-users-{generation}"))
            .spawn(move || {
                let result = service.search_jira_assignees(&ticket.project_key, &query);
                let _ = sender.send((generation, result));
            })
        {
            self.control.child_mut().set_external_loading(false);
            self.service
                .report_error(format!("could not search Jira users: {error}"));
        }
    }
}

impl PropertyKind {
    fn label(self) -> &'static str {
        match self {
            Self::IssueType => "Issue type",
            Self::Status => "Status",
            Self::Priority => "Priority",
            Self::Assignee => "Assignee",
        }
    }

    fn hotkey(self) -> &'static str {
        match self {
            Self::IssueType => "it",
            Self::Status => "st",
            Self::Priority => "pri",
            Self::Assignee => "as",
        }
    }

    fn value(self, ticket: &Ticket) -> &str {
        match self {
            Self::IssueType => match ticket.kind {
                TicketKind::Epic => "Epic",
                TicketKind::Story => "Story",
                TicketKind::Task => "Task",
                TicketKind::Bug => "Bug",
                TicketKind::Subtask => "Sub-task",
            },
            Self::Status => &ticket.status,
            Self::Priority => &ticket.priority,
            Self::Assignee => &ticket.assignee,
        }
    }

    fn action(self, id: String, label: String) -> ComposerAction {
        match self {
            Self::IssueType => {
                ComposerAction::UpdateKind(match label.to_ascii_lowercase().as_str() {
                    "epic" => TicketKind::Epic,
                    "story" => TicketKind::Story,
                    "bug" => TicketKind::Bug,
                    "subtask" | "sub-task" => TicketKind::Subtask,
                    _ => TicketKind::Task,
                })
            }
            Self::Status => ComposerAction::UpdateStatus(label),
            Self::Priority => ComposerAction::UpdatePriority(label),
            Self::Assignee => ComposerAction::UpdateAssignee {
                name: label,
                account_id: id,
            },
        }
    }
}

impl TuiNode for BoundPropertyDropdown {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        let previous = ctx.focus_disabled();
        ctx.set_focus_disabled(previous || self.disabled);
        let result = self.control.layout(area, ctx);
        ctx.set_focus_disabled(previous);
        result
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.disabled {
            EventOutcome::Ignored
        } else {
            self.control.event(event, ctx)
        }
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.disabled {
            EventOutcome::Ignored
        } else {
            self.control.dispatch_event(route, event, ctx)
        }
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.sync();
        let changed = self.sync_assignee_search(dt);
        self.control.tick(dt, settings).merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.mount(ctx);
        ctx.request_tick();
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
    }
}

impl TuiNode for PropertyFields {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.root.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.root.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.ensure_options();
        let changed = self.drain_options();
        self.root.tick(dt, settings).merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::scheduled_after(Duration::from_millis(50))
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
        ctx.request_tick();
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}
