use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect, style::Style};
use tuicore::{
    AnimationSettings, Dropdown, DropdownSearchMode, DropdownVariant, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal,
    LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, ScrollContainer, ScrollbarConfig,
    TickResult, TuiEvent, TuiNode, theme,
};

use crate::{
    app_settings::{ComposerKeyBindings, ComposerSequenceBinding},
    jira::{JiraAssignee, JiraFieldOptions, JiraFixVersion, JiraOption},
    service::AppService,
    store::composer::{
        ComposerAction, ComposerState, ComposerViewMode, PlacementTarget, Ticket, TicketKind,
    },
};

use super::fields::{BoundLabelsInput, BoundTicketPropertyInput, TicketPropertyText};
use super::issue_links::BoundIssueLinks;
use super::web_links::BoundWebLinks;

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type PropertyDropdown = Dropdown<JiraOption, String>;
type FixVersionsDropdown = Dropdown<JiraFixVersion, String>;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

fn jira_project_key(ticket: &Ticket) -> String {
    (!ticket.key.starts_with("NEW-"))
        .then(|| {
            ticket
                .key
                .split_once('-')
                .map(|(project_key, _)| project_key)
        })
        .flatten()
        .unwrap_or(ticket.project_key.as_str())
        .to_owned()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PropertyKind {
    IssueType,
    Parent,
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
    root: ScrollContainer<Flex<()>>,
    sender: Sender<(u64, String, Result<JiraFieldOptions, String>)>,
    receiver: Receiver<(u64, String, Result<JiraFieldOptions, String>)>,
    generation: u64,
}

impl PropertyFields {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        keys: ComposerKeyBindings,
    ) -> Self {
        let shared = Rc::new(RefCell::new(SharedOptions::default()));
        let fields = Flex::column()
            .child(
                "kind",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::IssueType,
                    keys.issue_type.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "parent",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::Parent,
                    keys.parent.clone(),
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
                    keys.status.clone(),
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
                    keys.priority.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "assignee",
                BoundPropertyDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    Rc::clone(&shared),
                    service.clone(),
                    PropertyKind::Assignee,
                    keys.assignee.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "story-points",
                BoundTicketPropertyInput::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    TicketPropertyText::StoryPoints,
                    keys.story_points.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "fix-versions",
                BoundFixVersionsDropdown::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.fix_versions.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "labels",
                BoundLabelsInput::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.labels.clone(),
                ),
                FlexItem::fixed(3),
            )
            .child(
                "web-links",
                BoundWebLinks::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.web_links.clone(),
                ),
                FlexItem::fixed(6),
            )
            .child(
                "issue-links",
                BoundIssueLinks::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.issue_links.clone(),
                ),
                FlexItem::fixed(11),
            );
        let root = ScrollContainer::vertical(fields)
            .scrollbars(ScrollbarConfig::default())
            .focus_reveal(true);
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

pub(super) struct BoundFixVersionsDropdown {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    control: FixVersionsDropdown,
    sender: Sender<(u64, Result<Vec<JiraFixVersion>, String>)>,
    receiver: Receiver<(u64, Result<Vec<JiraFixVersion>, String>)>,
    generation: u64,
    ticket_id: Option<String>,
    versions: Vec<JiraFixVersion>,
    selected: Vec<String>,
    synced_diff: Option<(Vec<String>, Vec<String>)>,
}

impl BoundFixVersionsDropdown {
    fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        hotkey: ComposerSequenceBinding,
    ) -> Self {
        let sink = Rc::clone(&pending);
        let control = Dropdown::multi(
            [],
            |version: &JiraFixVersion| version.name.clone(),
            |version| version.name.clone(),
        )
        .variant(DropdownVariant::Bordered)
        .label("Fix versions")
        .show_multi_labels(true)
        .hotkey(hotkey.sequence())
        .on_select(move |names| {
            sink.borrow_mut()
                .push(ComposerAction::UpdateFixVersions(names));
        });
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            control,
            sender,
            receiver,
            generation: 0,
            ticket_id: None,
            versions: Vec::new(),
            selected: Vec::new(),
            synced_diff: None,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        versions: Vec<JiraFixVersion>,
    ) -> Self {
        let mut dropdown = Self::new(
            state,
            pending,
            service,
            ComposerKeyBindings::default().fix_versions,
        );
        dropdown.versions = versions;
        dropdown.control.set_rows(dropdown.versions.clone());
        dropdown
    }

    fn sync(&mut self) -> bool {
        let (ticket_id, project_key, selected, editable, remote_queries_allowed, diff) = {
            let state = self.state.borrow();
            let ticket = state.selected_ticket();
            let selected = ticket
                .map(|ticket| ticket.fix_versions.clone())
                .unwrap_or_default();
            let diff = (state.view_mode == ComposerViewMode::Diff).then(|| {
                let previous = state
                    .selected_change()
                    .and_then(|change| change.original.as_ref())
                    .map(|ticket| ticket.fix_versions.as_slice())
                    .unwrap_or_default();
                let added = selected
                    .iter()
                    .filter(|version| !previous.contains(version))
                    .cloned()
                    .collect::<Vec<_>>();
                let removed = previous
                    .iter()
                    .filter(|version| !selected.contains(version))
                    .cloned()
                    .collect::<Vec<_>>();
                (added, removed)
            });
            (
                ticket.map(|ticket| ticket.key.clone()),
                ticket.map(jira_project_key),
                selected,
                state.selected_is_editable(),
                state.remote_queries_allowed(),
                diff,
            )
        };
        let mut changed = false;
        if self.ticket_id != ticket_id {
            self.ticket_id = ticket_id;
            self.generation = self.generation.saturating_add(1);
            self.versions.clear();
            if remote_queries_allowed {
                if let Some(project_key) = project_key {
                    self.fetch_versions(project_key);
                }
            }
            changed = true;
        }
        if self.control.is_disabled() == editable {
            self.control.set_disabled(!editable);
            changed = true;
        }
        if self.selected != selected {
            self.control.set_selected(selected.clone());
            self.selected = selected;
            changed = true;
        }
        if self.synced_diff != diff {
            if let Some((added, removed)) = &diff {
                let unchanged = added.is_empty() && removed.is_empty();
                let added = added.clone();
                self.control.set_selected_style_by(move |version| {
                    added.contains(version).then(|| {
                        let theme = theme();
                        Style::default()
                            .fg(theme.diff_added_fg())
                            .bg(theme.diff_added_bg())
                    })
                });
                if unchanged {
                    self.control.set_bottom_left("(unchanged)");
                    self.control.clear_bottom_left_style();
                } else if removed.is_empty() {
                    self.control.clear_bottom_left();
                    self.control.clear_bottom_left_style();
                } else {
                    self.control.set_bottom_left(removed.join(", "));
                    let theme = theme();
                    self.control.set_bottom_left_style(
                        Style::default()
                            .fg(theme.diff_removed_fg())
                            .bg(theme.diff_removed_bg()),
                    );
                }
            } else {
                self.control.clear_selected_style_by();
                self.control.clear_bottom_left();
                self.control.clear_bottom_left_style();
            }
            self.synced_diff = diff;
            changed = true;
        }
        changed
    }

    fn fetch_versions(&self, project_key: String) {
        let generation = self.generation;
        let sender = self.sender.clone();
        let service = self.service.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-fix-versions-{generation}"))
            .spawn(move || {
                let _ = sender.send((
                    generation,
                    service.search_jira_fix_versions(&project_key, ""),
                ));
            })
        {
            self.service
                .report_error(format!("could not fetch Jira fix versions: {error}"));
        }
    }

    fn drain_versions(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, result)) = self.receiver.try_recv() {
            if generation != self.generation {
                continue;
            }
            match result {
                Ok(versions) => {
                    self.versions = versions;
                    self.control.set_rows(self.versions.clone());
                    self.control.set_selected(self.selected.clone());
                    changed = true;
                }
                Err(error) => self
                    .service
                    .report_error(format!("Jira fix-version lookup failed: {error}")),
            }
        }
        changed
    }
}

impl TuiNode for BoundFixVersionsDropdown {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <FixVersionsDropdown as TuiNode<()>>::measure(&self.control, proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        <FixVersionsDropdown as TuiNode<()>>::layout(&mut self.control, area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.control.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.control.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync() || self.drain_versions();
        <FixVersionsDropdown as TuiNode<()>>::tick(&mut self.control, dt, settings).merge(
            if changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            },
        )
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

pub(super) struct BoundPropertyDropdown {
    state: Rc<RefCell<ComposerState>>,
    shared: Rc<RefCell<SharedOptions>>,
    service: AppService,
    kind: PropertyKind,
    control: PropertyDropdown,
    labels: Rc<RefCell<HashMap<String, String>>>,
    assignee_sender: Sender<(u64, Result<Vec<JiraAssignee>, String>)>,
    assignee_receiver: Receiver<(u64, Result<Vec<JiraAssignee>, String>)>,
    assignee_generation: u64,
    assignee_ticket: Option<String>,
    assignee_query: String,
    pending_search: Option<Duration>,
    synced_rows: Vec<JiraOption>,
    synced_selected_value: Option<String>,
    synced_previous_value: Option<String>,
}

impl BoundPropertyDropdown {
    fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        shared: Rc<RefCell<SharedOptions>>,
        service: AppService,
        kind: PropertyKind,
        hotkey: ComposerSequenceBinding,
    ) -> Self {
        let labels = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let callback_labels = Rc::clone(&labels);
        let sink = Rc::clone(&pending);
        let callback_state = Rc::clone(&state);
        let control = Dropdown::single(
            [],
            |option: &JiraOption| option.id.clone(),
            |option| option.label.clone(),
        )
        .variant(DropdownVariant::Bordered)
        .label(kind.label())
        .search_mode(if kind == PropertyKind::Assignee {
            DropdownSearchMode::External
        } else {
            DropdownSearchMode::Fuzzy
        })
        .external_loading_message("Fetching users")
        .hotkey(hotkey.sequence())
        .on_select(move |ids| {
            let Some(id) = ids.first() else {
                return;
            };
            let label = callback_labels
                .borrow()
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.clone());
            if kind == PropertyKind::Parent {
                let Some(ticket_id) = callback_state.borrow().selected_ticket.clone() else {
                    return;
                };
                let placement = if id == "__root" {
                    PlacementTarget::Root
                } else {
                    PlacementTarget::ChildOf(id.clone())
                };
                sink.borrow_mut().push(ComposerAction::ReparentTicket {
                    id: ticket_id,
                    placement,
                });
            } else {
                sink.borrow_mut().push(kind.action(id.clone(), label));
            }
        });
        let (assignee_sender, assignee_receiver) = mpsc::channel();
        Self {
            state,
            shared,
            service,
            kind,
            control,
            labels,
            assignee_sender,
            assignee_receiver,
            assignee_generation: 0,
            assignee_ticket: None,
            assignee_query: String::new(),
            pending_search: None,
            synced_rows: Vec::new(),
            synced_selected_value: None,
            synced_previous_value: None,
        }
    }

    #[cfg(test)]
    pub(super) fn priority_for_test(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        priorities: Vec<JiraOption>,
    ) -> Self {
        let shared = Rc::new(RefCell::new(SharedOptions {
            ticket_id: None,
            values: Some(JiraFieldOptions {
                issue_types: Vec::new(),
                statuses: Vec::new(),
                priorities,
            }),
        }));
        Self::new(
            state,
            pending,
            shared,
            service,
            PropertyKind::Priority,
            ComposerKeyBindings::default().priority,
        )
    }

    #[cfg(test)]
    pub(super) fn assignee_for_test(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        assignees: Vec<JiraOption>,
    ) -> Self {
        let mut control = Self::new(
            state,
            pending,
            Rc::new(RefCell::new(SharedOptions::default())),
            service,
            PropertyKind::Assignee,
            ComposerKeyBindings::default().assignee,
        );
        control.synced_rows = assignees;
        control.control.set_rows(control.synced_rows.clone());
        control
    }

    #[cfg(test)]
    pub(super) fn parent_for_test(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
    ) -> Self {
        Self::new(
            state,
            pending,
            Rc::new(RefCell::new(SharedOptions::default())),
            service,
            PropertyKind::Parent,
            ComposerKeyBindings::default().parent,
        )
    }

    #[cfg(test)]
    pub(super) fn set_priorities_for_test(&mut self, priorities: Vec<JiraOption>) {
        self.shared.borrow_mut().values.as_mut().unwrap().priorities = priorities;
    }

    #[cfg(test)]
    pub(super) fn is_open_for_test(&self) -> bool {
        self.control.is_open()
    }

    #[cfg(test)]
    pub(super) fn selected_for_test(&self) -> Option<String> {
        self.control.selected_id()
    }

    #[cfg(test)]
    pub(super) fn sync_for_test(&mut self) {
        self.sync();
    }

    fn sync(&mut self) -> bool {
        let (value, disabled, previous, diff_styles) = {
            let state = self.state.borrow();
            let displayed = state.selected_ticket();
            let value = if self.kind == PropertyKind::Parent {
                state
                    .selected_parent_id()
                    .unwrap_or_else(|| "__root".into())
            } else if self.kind == PropertyKind::Assignee {
                displayed.map_or_else(String::new, |ticket| ticket.assignee_account_id.clone())
            } else {
                displayed.map_or_else(String::new, |ticket| self.kind.value(ticket))
            };
            let comparison_value = if self.kind == PropertyKind::Parent {
                displayed
                    .and_then(|ticket| ticket.parent_key.clone())
                    .unwrap_or_default()
            } else {
                value.clone()
            };
            let previous = self.previous_value(&state, &comparison_value);
            let diff_styles = self.diff_value_styles(&state, &comparison_value);
            (value, !state.selected_is_editable(), previous, diff_styles)
        };
        let mut changed = false;
        if self.control.is_disabled() != disabled {
            self.control.set_disabled(disabled);
            changed = true;
        }
        if self.synced_previous_value != previous {
            if let Some(previous) = &previous {
                self.control.set_bottom_left(previous.clone());
            } else {
                self.control.clear_bottom_left();
            }
            self.synced_previous_value = previous;
            changed = true;
        }
        if let Some((current_style, previous_style)) = diff_styles {
            self.control.set_field_text_style(current_style);
            self.control.set_bottom_left_style(previous_style);
        } else {
            self.control.clear_field_text_style();
            self.control.clear_bottom_left_style();
        }
        if self.kind != PropertyKind::Assignee {
            let mut options = self.options();
            if !value.is_empty() && !options.iter().any(|option| option.id == value) {
                options.push(JiraOption {
                    id: value.clone(),
                    label: if self.kind == PropertyKind::Parent {
                        format!("{value} (unavailable)")
                    } else {
                        value.clone()
                    },
                });
            }
            changed |= self.set_options(options, &value);
        } else if !self.synced_rows.is_empty() {
            changed |= self.set_options(self.synced_rows.clone(), &value);
        }
        changed
    }

    fn previous_value(&self, state: &ComposerState, current: &str) -> Option<String> {
        if state.view_mode != crate::store::composer::ComposerViewMode::Diff {
            return None;
        }
        let previous = state
            .selected_change()
            .and_then(|change| change.original.as_ref())
            .and_then(|ticket| self.kind.previous_value(ticket));
        match previous {
            Some((value, _)) if value == current => Some("(unchanged)".into()),
            Some((_, label)) => Some(label),
            None => Some("(none)".into()),
        }
    }

    fn diff_value_styles(&self, state: &ComposerState, current: &str) -> Option<(Style, Style)> {
        let previous = state
            .selected_change()
            .and_then(|change| change.original.as_ref())
            .and_then(|ticket| self.kind.previous_value(ticket))?;
        if state.view_mode != crate::store::composer::ComposerViewMode::Diff
            || previous.0 == current
            || current.is_empty()
        {
            return None;
        }
        let theme = theme();
        Some((
            Style::default()
                .fg(theme.diff_added_fg())
                .bg(theme.diff_added_bg()),
            Style::default()
                .fg(theme.diff_removed_fg())
                .bg(theme.diff_removed_bg()),
        ))
    }

    fn options(&self) -> Vec<JiraOption> {
        if self.kind == PropertyKind::Parent {
            return self.parent_options();
        }
        let shared = self.shared.borrow();
        let Some(values) = shared.values.as_ref() else {
            return if self.kind == PropertyKind::IssueType {
                self.kind_options()
            } else {
                Vec::new()
            };
        };
        match self.kind {
            PropertyKind::IssueType => values
                .issue_types
                .iter()
                .filter(|option| {
                    self.kind_options()
                        .iter()
                        .any(|legal| legal.label == option.label)
                })
                .cloned()
                .collect(),
            PropertyKind::Status => values.statuses.clone(),
            PropertyKind::Priority => values.priorities.clone(),
            PropertyKind::Assignee => Vec::new(),
            PropertyKind::Parent => Vec::new(),
        }
        .into_iter()
        .map(|option| JiraOption {
            id: option.label.clone(),
            label: option.label,
        })
        .collect()
    }

    fn kind_options(&self) -> Vec<JiraOption> {
        self.state
            .borrow()
            .legal_kinds_for_selected()
            .into_iter()
            .map(|kind| JiraOption {
                id: self.kind_name(kind).into(),
                label: self.kind_name(kind).into(),
            })
            .collect()
    }

    fn parent_options(&self) -> Vec<JiraOption> {
        let state = self.state.borrow();
        let Some(ticket) = state.selected_ticket() else {
            return Vec::new();
        };
        let selected_id = state.selected_ticket.clone();
        let mut options = Vec::new();
        if state.legal_child_kinds(None).contains(&ticket.kind) {
            options.push(JiraOption {
                id: "__root".into(),
                label: "Root".into(),
            });
        }
        let mut parent_ids = HashSet::new();
        options.extend(
            state
                .parent_candidates(ticket.kind)
                .into_iter()
                .filter(|candidate| Some(&candidate.id) != selected_id.as_ref())
                .filter_map(|candidate| {
                    state
                        .changes_for_change(candidate)
                        .map(|parent| JiraOption {
                            id: candidate.id.clone(),
                            label: format!("{} · {}", parent.key, parent.title),
                        })
                })
                .filter(|option| {
                    selected_id.as_deref().is_some_and(|id| {
                        state
                            .validate_placement(id, &PlacementTarget::ChildOf(option.id.clone()))
                            .is_ok()
                    })
                })
                .filter(|option| parent_ids.insert(option.id.clone())),
        );
        options
    }

    fn kind_name(&self, kind: TicketKind) -> &'static str {
        match kind {
            TicketKind::Epic => "Epic",
            TicketKind::Story => "Story",
            TicketKind::Task => "Task",
            TicketKind::Bug => "Bug",
            TicketKind::Subtask => "Sub-task",
        }
    }

    fn set_options(&mut self, options: Vec<JiraOption>, selected_label: &str) -> bool {
        let rows_changed = self.synced_rows != options;
        let selected_changed = self.synced_selected_value.as_deref() != Some(selected_label);
        if !rows_changed && !selected_changed {
            return false;
        }

        if selected_changed {
            self.control.close();
        }
        if rows_changed {
            self.control.set_rows(options.clone());
            *self.labels.borrow_mut() = options
                .iter()
                .map(|option| (option.id.clone(), option.label.clone()))
                .collect();
            self.synced_rows = options;
        }
        if selected_changed {
            if !selected_label.is_empty()
                && self
                    .synced_rows
                    .iter()
                    .any(|option| option.id == selected_label)
            {
                self.control.set_selected_one(selected_label.to_owned());
            } else {
                self.control.clear_selection();
            }
            self.synced_selected_value = Some(selected_label.to_owned());
        }
        true
    }

    fn sync_assignee_search(&mut self, dt: Duration) -> bool {
        if self.kind != PropertyKind::Assignee {
            return false;
        }
        if !self.state.borrow().remote_queries_allowed() {
            if self.assignee_ticket.take().is_some() || self.pending_search.take().is_some() {
                self.assignee_generation = self.assignee_generation.saturating_add(1);
            }
            self.control.set_external_loading(false);
            return false;
        }
        let ticket = self.state.borrow().selected_changes().cloned();
        let query = self.control.search_query().to_owned();
        if self.assignee_ticket.as_deref() != ticket.as_ref().map(|ticket| ticket.key.as_str())
            || query != self.assignee_query
        {
            self.assignee_generation = self.assignee_generation.saturating_add(1);
            self.assignee_ticket = ticket.as_ref().map(|ticket| ticket.key.clone());
            self.assignee_query = query;
            self.pending_search = ticket.as_ref().map(|_| Duration::ZERO);
            self.control.set_external_loading(ticket.is_some());
        }
        let Some(ticket) = ticket else {
            return false;
        };
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
            self.control.set_external_loading(false);
            match result {
                Ok(users) => {
                    let selected_ticket = self.state.borrow().selected_ticket().cloned();
                    let selected = selected_ticket
                        .as_ref()
                        .map(|ticket| ticket.assignee_account_id.clone())
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
                        && !options
                            .iter()
                            .any(|option| option.id == ticket.assignee_account_id)
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
        let generation = self.assignee_generation;
        let query = self.assignee_query.clone();
        let sender = self.assignee_sender.clone();
        let service = self.service.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-users-{generation}"))
            .spawn(move || {
                let result = service.search_jira_assignees(&jira_project_key(&ticket), &query);
                let _ = sender.send((generation, result));
            })
        {
            self.control.set_external_loading(false);
            self.service
                .report_error(format!("could not search Jira users: {error}"));
        }
    }
}

impl PropertyKind {
    fn label(self) -> &'static str {
        match self {
            Self::IssueType => "Issue type",
            Self::Parent => "Parent",
            Self::Status => "Status",
            Self::Priority => "Priority",
            Self::Assignee => "Assignee",
        }
    }

    fn value(self, ticket: &Ticket) -> String {
        match self {
            Self::IssueType => match ticket.kind {
                TicketKind::Epic => "Epic".into(),
                TicketKind::Story => "Story".into(),
                TicketKind::Task => "Task".into(),
                TicketKind::Bug => "Bug".into(),
                TicketKind::Subtask => "Sub-task".into(),
            },
            Self::Parent => ticket.parent_key.clone().unwrap_or_else(|| "Root".into()),
            Self::Status => ticket.status.clone(),
            Self::Priority => ticket.priority.clone(),
            Self::Assignee => ticket.assignee.clone(),
        }
    }

    fn previous_value(self, ticket: &Ticket) -> Option<(String, String)> {
        match self {
            Self::IssueType | Self::Status | Self::Priority => {
                let value = self.value(ticket);
                (!value.is_empty()).then(|| (value.clone(), value))
            }
            Self::Parent => ticket
                .parent_key
                .as_ref()
                .map(|parent| (parent.clone(), parent.clone())),
            Self::Assignee => (!ticket.assignee_account_id.is_empty())
                .then(|| (ticket.assignee_account_id.clone(), ticket.assignee.clone())),
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
            Self::Parent => unreachable!("parent selections are handled by the dropdown"),
        }
    }
}

impl TuiNode for BoundPropertyDropdown {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <PropertyDropdown as TuiNode<()>>::measure(&self.control, proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        <PropertyDropdown as TuiNode<()>>::layout(&mut self.control, area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.control.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.control.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let sync_changed = self.sync();
        let changed = self.sync_assignee_search(dt) || sync_changed;
        <PropertyDropdown as TuiNode<()>>::tick(&mut self.control, dt, settings).merge(if changed {
            TickResult {
                changed: true,
                layout: true,
                ..TickResult::IDLE
            }
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
