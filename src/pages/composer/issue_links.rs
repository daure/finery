use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    text::Text,
};
use tuicore::{
    AnimationSettings, Column, DropdownPopupDirection, DropdownSearchMode, EventCtx, EventOutcome,
    EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, ListControl, ListControlEvent, ListControlField, RenderCtx,
    TickResult, TuiEvent, TuiNode, theme,
};

use crate::{
    app_settings::ComposerSequenceBinding,
    components::work_item_rows::{
        TicketRowDetails, WorkItemKind, WorkItemRow, ticket_summary_text,
    },
    service::{AppService, ComposerSearchTicket},
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, TicketIssueLink},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type IssueLinkControl = ListControl<IssueLinkRow, String>;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

const RELATIONSHIPS: [(&str, &str, bool); 7] = [
    ("blocks", "blocks", true),
    ("is blocked by", "is blocked by", false),
    ("duplicates", "duplicates", true),
    ("is duplicated by", "is duplicated by", false),
    ("clones", "clones", true),
    ("is cloned by", "is cloned by", false),
    ("relates to", "relates to", true),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum IssueLinkDiff {
    None,
    Added,
    Removed,
}

#[derive(Clone)]
struct IssueLinkRow {
    row_id: String,
    link: TicketIssueLink,
    target: Option<ComposerSearchTicket>,
    diff: IssueLinkDiff,
}

impl PartialEq for IssueLinkRow {
    fn eq(&self, other: &Self) -> bool {
        self.row_id == other.row_id && self.link == other.link && self.diff == other.diff
    }
}

impl Eq for IssueLinkRow {}

pub(super) struct BoundIssueLinks {
    state: Rc<RefCell<ComposerState>>,
    pending: PendingActions,
    service: AppService,
    control: IssueLinkControl,
    targets: Rc<RefCell<HashMap<String, ComposerSearchTicket>>>,
    search_sender: Sender<(u64, Result<Vec<ComposerSearchTicket>, String>)>,
    search_receiver: Receiver<(u64, Result<Vec<ComposerSearchTicket>, String>)>,
    search_generation: u64,
    search_query: String,
    pending_search: Option<Duration>,
    synced_rows: Vec<IssueLinkRow>,
    disabled: bool,
}

impl BoundIssueLinks {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        hotkey: ComposerSequenceBinding,
    ) -> Self {
        let targets = Rc::new(RefCell::new(HashMap::new()));
        let creator_targets = Rc::clone(&targets);
        let editor_targets = Rc::clone(&targets);
        let mut control = ListControl::new_fields(
            [],
            |row: &IssueLinkRow| row.row_id.clone(),
            [
                ListControlField::dropdown_options(
                    "Relationship",
                    RELATIONSHIPS.map(|(id, label, _)| (id, label)),
                ),
                ListControlField::dropdown_options_rich(
                    "Ticket",
                    Vec::<(String, String)>::new(),
                    {
                        let targets = Rc::clone(&targets);
                        move |key, title, query, mode| {
                            targets
                                .borrow()
                                .get(key)
                                .map(ticket_summary_for_search_result)
                                .unwrap_or_else(|| issue_link_ticket_text(key, title, query, mode))
                        }
                    },
                ),
            ],
            move |values, _| IssueLinkRow {
                row_id: format!("new:{}:{}", values[0], values[1]),
                link: issue_link(&values, &creator_targets.borrow()),
                target: creator_targets.borrow().get(&values[1]).cloned(),
                diff: IssueLinkDiff::None,
            },
        )
        .editable(
            |row| vec![row.link.relationship.clone(), row.link.target_key.clone()],
            move |row, values| {
                row.link = issue_link(&values, &editor_targets.borrow());
                row.target = editor_targets.borrow().get(&values[1]).cloned();
            },
        )
        .column(Column::multiline(
            "issue-link",
            "",
            Constraint::Fill(1),
            |row, _| issue_link_summary_text(row),
        ))
        .headers(false)
        .focus_id("issue-links-data-view")
        .row_height(3)
        .max_rows(3)
        .filter_controls(false)
        .action_bar(true)
        .title("Issue links")
        .hotkey(hotkey.sequence())
        .empty_message("No issue links");
        control.set_dropdown_popup_direction(0, DropdownPopupDirection::Up);
        control.set_dropdown_popup_direction(1, DropdownPopupDirection::Up);
        control.set_dropdown_search_mode(1, DropdownSearchMode::External);
        control.set_dropdown_external_loading(1, true);
        control
            .data_view_mut()
            .set_row_style_by(|row| match row.diff {
                IssueLinkDiff::None => None,
                IssueLinkDiff::Added => Some(
                    Style::default()
                        .fg(theme().diff_added_fg())
                        .bg(theme().diff_added_bg()),
                ),
                IssueLinkDiff::Removed => Some(
                    Style::default()
                        .fg(theme().diff_removed_fg())
                        .bg(theme().diff_removed_bg()),
                ),
            });
        let (search_sender, search_receiver) = mpsc::channel();
        let mut bound = Self {
            state,
            pending,
            service,
            control,
            targets,
            search_sender,
            search_receiver,
            search_generation: 0,
            search_query: String::new(),
            pending_search: Some(Duration::ZERO),
            synced_rows: Vec::new(),
            disabled: false,
        };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let rows = issue_link_rows(&state, &self.targets.borrow());
        let disabled = !state.selected_is_editable();
        let changed = rows != self.synced_rows || disabled != self.disabled;
        if rows != self.synced_rows {
            self.control.set_rows(rows.clone());
            self.synced_rows = rows;
        }
        if disabled != self.disabled {
            self.control.set_disabled(disabled);
            self.disabled = disabled;
        }
        changed
    }

    fn drain_events(&mut self) {
        for event in self.control.take_events() {
            let action = match event {
                ListControlEvent::Added { row_id } => self
                    .control
                    .items()
                    .iter()
                    .find(|row| row.row_id == row_id)
                    .map(|row| ComposerAction::AddIssueLink {
                        relationship: row.link.relationship.clone(),
                        target_key: row.link.target_key.clone(),
                        target_title: row.link.target_title.clone(),
                        outward: row.link.outward,
                    }),
                ListControlEvent::Edited { row_id } => self
                    .control
                    .items()
                    .iter()
                    .find(|row| row.row_id == row_id)
                    .map(|row| ComposerAction::UpdateIssueLink {
                        id: row.link.id.clone(),
                        relationship: row.link.relationship.clone(),
                        target_key: row.link.target_key.clone(),
                        target_title: row.link.target_title.clone(),
                        outward: row.link.outward,
                    }),
                ListControlEvent::Removed { row_id } => self
                    .synced_rows
                    .iter()
                    .find(|row| row.row_id == row_id)
                    .map(|row| ComposerAction::RemoveIssueLink(row.link.id.clone())),
                _ => None,
            };
            if let Some(action) = action {
                self.pending.borrow_mut().push(action);
            }
        }
    }

    fn sync_ticket_search(&mut self, dt: Duration) -> bool {
        let query = self
            .control
            .dropdown_search_query(1)
            .unwrap_or_default()
            .to_owned();
        if query != self.search_query {
            self.search_query = query;
            self.search_generation = self.search_generation.saturating_add(1);
            self.pending_search = Some(Duration::ZERO);
            self.control.set_dropdown_external_loading(1, true);
        }
        if let Some(elapsed) = &mut self.pending_search {
            *elapsed += dt;
            if *elapsed >= SEARCH_DEBOUNCE {
                self.pending_search = None;
                self.start_ticket_search();
            }
        }
        let mut changed = false;
        while let Ok((generation, result)) = self.search_receiver.try_recv() {
            if generation != self.search_generation {
                continue;
            }
            self.control.set_dropdown_external_loading(1, false);
            match result {
                Ok(tickets) => {
                    let selected_key = self
                        .state
                        .borrow()
                        .selected_ticket()
                        .map(|ticket| ticket.key.clone());
                    let tickets = tickets
                        .into_iter()
                        .filter(|ticket| Some(&ticket.ticket.key) != selected_key.as_ref())
                        .collect::<Vec<_>>();
                    *self.targets.borrow_mut() = tickets
                        .iter()
                        .map(|ticket| (ticket.ticket.key.clone(), ticket.clone()))
                        .collect();
                    self.control.set_dropdown_rows(
                        1,
                        tickets
                            .into_iter()
                            .map(|ticket| (ticket.ticket.key, ticket.ticket.title)),
                    );
                }
                Err(error) => self
                    .service
                    .report_error(format!("Jira ticket search failed: {error}")),
            }
            changed = true;
        }
        changed
    }

    fn start_ticket_search(&mut self) {
        self.search_generation = self.search_generation.saturating_add(1);
        let generation = self.search_generation;
        let query = self.search_query.clone();
        let service = self.service.clone();
        let sender = self.search_sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("finery-jira-issue-link-search-{generation}"))
            .spawn(move || {
                let _ = sender.send((generation, service.search_jira_for_composer(&query)));
            })
        {
            self.control.set_dropdown_external_loading(1, false);
            self.service
                .report_error(format!("could not search Jira tickets: {error}"));
        }
    }
}

fn issue_link(
    values: &[String],
    targets: &HashMap<String, ComposerSearchTicket>,
) -> TicketIssueLink {
    let (relationship, _, outward) = RELATIONSHIPS
        .iter()
        .find(|(id, _, _)| *id == values[0])
        .copied()
        .unwrap_or(RELATIONSHIPS[0]);
    TicketIssueLink {
        id: String::new(),
        relationship: relationship.into(),
        target_key: values[1].clone(),
        target_title: targets
            .get(&values[1])
            .map(|ticket| ticket.ticket.title.clone())
            .unwrap_or_else(|| values[1].clone()),
        outward,
    }
}

fn issue_link_rows(
    state: &ComposerState,
    targets: &HashMap<String, ComposerSearchTicket>,
) -> Vec<IssueLinkRow> {
    let current = state
        .selected_ticket()
        .map(|ticket| ticket.issue_links.clone())
        .unwrap_or_default();
    if state.view_mode != ComposerViewMode::Diff {
        return current
            .into_iter()
            .map(|link| IssueLinkRow {
                row_id: link.id.clone(),
                target: targets.get(&link.target_key).cloned(),
                link,
                diff: IssueLinkDiff::None,
            })
            .collect();
    }
    let source = state
        .selected_source()
        .map(|ticket| ticket.issue_links.clone())
        .unwrap_or_default();
    let current_by_id = current
        .iter()
        .map(|link| (link.id.as_str(), link))
        .collect::<HashMap<_, _>>();
    let mut rows = Vec::new();
    for link in &source {
        match current_by_id.get(link.id.as_str()) {
            Some(current) if *current == link => rows.push(IssueLinkRow {
                row_id: link.id.clone(),
                target: targets.get(&link.target_key).cloned(),
                link: link.clone(),
                diff: IssueLinkDiff::None,
            }),
            Some(current) => {
                rows.push(IssueLinkRow {
                    row_id: format!("diff-removed:{}", link.id),
                    target: targets.get(&link.target_key).cloned(),
                    link: link.clone(),
                    diff: IssueLinkDiff::Removed,
                });
                rows.push(IssueLinkRow {
                    row_id: format!("diff-added:{}", link.id),
                    target: targets.get(&current.target_key).cloned(),
                    link: (*current).clone(),
                    diff: IssueLinkDiff::Added,
                });
            }
            None => rows.push(IssueLinkRow {
                row_id: format!("diff-removed:{}", link.id),
                target: targets.get(&link.target_key).cloned(),
                link: link.clone(),
                diff: IssueLinkDiff::Removed,
            }),
        }
    }
    rows.extend(
        current
            .into_iter()
            .filter(|link| !source.iter().any(|source| source.id == link.id))
            .map(|link| IssueLinkRow {
                row_id: format!("diff-added:{}", link.id),
                target: targets.get(&link.target_key).cloned(),
                link,
                diff: IssueLinkDiff::Added,
            }),
    );
    rows
}

fn issue_link_ticket_text(
    key: &str,
    title: &str,
    _query: &str,
    _mode: DropdownSearchMode,
) -> Text<'static> {
    ticket_summary_text(
        &issue_link_work_item(key, title),
        None,
        None,
        TicketRowDetails {
            subtask_progress: None,
            fix_versions: &[],
            epic_name: None,
            annotation: None,
        },
    )
}

fn ticket_summary_for_search_result(ticket: &ComposerSearchTicket) -> Text<'static> {
    ticket_summary_text(
        &WorkItemRow {
            id: ticket.work_item.key.clone(),
            key: ticket.work_item.key.clone(),
            title: ticket.work_item.title.clone(),
            kind: match ticket.ticket.kind {
                crate::store::composer::TicketKind::Epic => WorkItemKind::Epic,
                crate::store::composer::TicketKind::Story => WorkItemKind::Story,
                crate::store::composer::TicketKind::Task => WorkItemKind::Task,
                crate::store::composer::TicketKind::Bug => WorkItemKind::Bug,
                crate::store::composer::TicketKind::Subtask => WorkItemKind::Subtask,
            },
            priority: ticket.work_item.priority.clone(),
            status: ticket.work_item.status.clone(),
            done: ticket.work_item.done,
            assignee: ticket.work_item.assignee.clone(),
            labels: ticket.work_item.labels.clone(),
            story_points: ticket.work_item.story_points,
            show_story_points: ticket.story_points_configured,
            story_points_estimated: false,
            story_points_from_average: false,
            change_badge: None,
            submitted: false,
        },
        None,
        None,
        TicketRowDetails {
            subtask_progress: ticket
                .work_item
                .subtask_progress
                .as_ref()
                .map(|progress| (progress.completed, progress.total)),
            fix_versions: &ticket.work_item.fix_versions,
            epic_name: ticket.work_item.epic_name.as_deref(),
            annotation: None,
        },
    )
}

fn issue_link_summary_text(row: &IssueLinkRow) -> Text<'static> {
    let marker = match row.diff {
        IssueLinkDiff::None => "",
        IssueLinkDiff::Added => "+ ",
        IssueLinkDiff::Removed => "- ",
    };
    let mut lines = vec![ratatui::text::Line::raw(format!(
        "{marker}{}",
        row.link.relationship
    ))];
    lines.extend(
        row.target
            .as_ref()
            .map(ticket_summary_for_search_result)
            .unwrap_or_else(|| {
                issue_link_ticket_text(
                    &row.link.target_key,
                    &row.link.target_title,
                    "",
                    DropdownSearchMode::Fuzzy,
                )
            })
            .lines,
    );
    Text::from(lines)
}

fn issue_link_work_item(key: &str, title: &str) -> WorkItemRow {
    WorkItemRow {
        id: key.into(),
        key: key.into(),
        title: title.into(),
        kind: WorkItemKind::Other,
        priority: String::new(),
        status: String::new(),
        done: false,
        assignee: String::new(),
        labels: Vec::new(),
        story_points: None,
        show_story_points: false,
        story_points_estimated: false,
        story_points_from_average: false,
        change_badge: None,
        submitted: false,
    }
}

impl TuiNode for BoundIssueLinks {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.control.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.control.event(event, ctx);
        self.drain_events();
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.control.dispatch_event(route, event, ctx);
        self.drain_events();
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync_ticket_search(dt) || self.sync();
        self.control
            .tick(dt, settings)
            .merge(if changed {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            })
            .merge(TickResult::scheduled_after(Duration::from_millis(50)))
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
