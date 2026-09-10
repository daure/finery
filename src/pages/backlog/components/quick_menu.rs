use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
};
use tuicore::{
    AnimationSettings, ChildKey, Dropdown, DropdownCommitMode, DropdownLabelPosition,
    DropdownSearchMode, DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    TickResult, TuiEvent, TuiNode, keybindings, line_width,
};

use crate::{
    app_settings::BacklogKeyBindings, components::avatar::initials,
    store::work_items::StatusTransition,
};

const MENU_HOST_WIDTH: u16 = 69;
const MENU_HOST_HEIGHT: u16 = 18;
const MENU_FIELD_WIDTH: u16 = 54;
pub(in crate::pages::backlog) const RELEASE_DROPDOWN_KEY: &str = "releases";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) enum BacklogQuickAction {
    SetStatus(String),
    StatusLoading,
    SetStatusTo(StatusTransition),
    SetStoryPoints(String),
    SetStoryPointsTo(BacklogStoryPoints),
    AssignUser(String),
    AssigneesLoading,
    AssignUserTo(BacklogAssignee),
    SetEpic(String),
    EpicsLoading,
    SetEpicTo(BacklogEpic),
    SetRelease(String),
    ReleasesLoading,
    SetReleaseTo(BacklogRelease),
    ViewDescription,
    MoveToTop,
    MoveToBottom,
    MoveToSection(BacklogDestination),
    MoveToDestinationTop(BacklogDestination),
    MoveToDestinationBottom(BacklogDestination),
}

impl BacklogQuickAction {
    pub(in crate::pages::backlog) fn main_actions(
        status: &str,
        assignee: &str,
        epic: &str,
        release: &str,
        story_points: &str,
    ) -> [Self; 8] {
        [
            Self::AssignUser(assignee.to_owned()),
            Self::SetStatus(status.to_owned()),
            Self::SetStoryPoints(story_points.to_owned()),
            Self::SetEpic(epic.to_owned()),
            Self::SetRelease(release.to_owned()),
            Self::ViewDescription,
            Self::MoveToTop,
            Self::MoveToBottom,
        ]
    }

    pub(in crate::pages::backlog) fn label(&self) -> String {
        match self {
            Self::SetStatus(status) if status.is_empty() => "Set status".into(),
            Self::SetStatus(status) => format!("Set status ({status})"),
            Self::StatusLoading => "Loading statuses…".into(),
            Self::SetStatusTo(status) => status.label.clone(),
            Self::SetStoryPoints(story_points) if story_points.is_empty() => {
                "Set story points".into()
            }
            Self::SetStoryPoints(story_points) => format!("Set story points ({story_points})"),
            Self::SetStoryPointsTo(story_points) => story_points.label().into(),
            Self::AssignUser(assignee) => format!("Assign user (@{})", initials(assignee)),
            Self::AssigneesLoading => "Loading users…".into(),
            Self::AssignUserTo(assignee) if assignee.account_id.is_empty() => "None".into(),
            Self::AssignUserTo(assignee) => {
                format!(
                    "{} (@{})",
                    assignee.display_name,
                    initials(&assignee.display_name)
                )
            }
            Self::SetEpic(epic) if epic.is_empty() => "Set epic".into(),
            Self::SetEpic(epic) => format!("Set epic ({epic})"),
            Self::EpicsLoading => "Loading epics…".into(),
            Self::SetEpicTo(epic) if epic.key.is_empty() => "None".into(),
            Self::SetEpicTo(epic) => epic.title.clone(),
            Self::SetRelease(release) if release.is_empty() => "Set release".into(),
            Self::SetRelease(release) => format!("Set release ({release})"),
            Self::ReleasesLoading => "Loading releases…".into(),
            Self::SetReleaseTo(release) => release.name.clone(),
            Self::ViewDescription => "View description".into(),
            Self::MoveToTop => "Move to top".into(),
            Self::MoveToBottom => "Move to bottom".into(),
            Self::MoveToSection(destination) => format!("Move to {}", destination.label),
            Self::MoveToDestinationTop(destination) => format!("Top of {}", destination.label),
            Self::MoveToDestinationBottom(destination) => {
                format!("Bottom of {}", destination.label)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) enum BacklogStoryPoints {
    None,
    One,
    Two,
    Three,
    Five,
    Eight,
    Thirteen,
    Twenty,
}

impl BacklogStoryPoints {
    const ALL: [Self; 8] = [
        Self::None,
        Self::One,
        Self::Two,
        Self::Three,
        Self::Five,
        Self::Eight,
        Self::Thirteen,
        Self::Twenty,
    ];

    fn value(self) -> Option<f64> {
        match self {
            Self::None => None,
            Self::One => Some(1.0),
            Self::Two => Some(2.0),
            Self::Three => Some(3.0),
            Self::Five => Some(5.0),
            Self::Eight => Some(8.0),
            Self::Thirteen => Some(13.0),
            Self::Twenty => Some(20.0),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::One => "1",
            Self::Two => "2",
            Self::Three => "3",
            Self::Five => "5",
            Self::Eight => "8",
            Self::Thirteen => "13",
            Self::Twenty => "20",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BacklogAssignee {
    pub account_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) struct BacklogEpic {
    pub key: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) struct BacklogRelease {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::pages::backlog) struct BacklogDestination {
    pub section_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages::backlog) enum BacklogQuickMenuEvent {
    LoadStatuses {
        keys: Vec<String>,
    },
    SetStatus {
        status: StatusTransition,
    },
    SetStoryPoints {
        keys: Vec<String>,
        story_points: Option<f64>,
    },
    LoadAssignees,
    AssignUser {
        keys: Vec<String>,
        assignee: BacklogAssignee,
    },
    LoadEpics,
    SetEpic {
        keys: Vec<String>,
        epic: BacklogEpic,
    },
    LoadReleases,
    SetReleases {
        keys: Vec<String>,
        releases: Vec<BacklogRelease>,
    },
    ViewDescription {
        key: String,
    },
    MoveToTop {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    MoveToBottom {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    MoveToSection {
        source_section_id: String,
        destination: BacklogDestination,
        keys: Vec<String>,
        to_top: bool,
    },
    MoveLocked,
    Closed,
}

pub(in crate::pages::backlog) struct BacklogQuickMenu {
    dropdown: Dropdown<BacklogQuickAction, BacklogQuickAction>,
    release_dropdown: Dropdown<BacklogQuickAction, BacklogQuickAction>,
    selected: Rc<RefCell<Vec<BacklogQuickAction>>>,
    selected_releases: Rc<RefCell<Option<Vec<BacklogQuickAction>>>>,
    keys: Vec<String>,
    section_id: Option<String>,
    source_order: Vec<String>,
    events: Vec<BacklogQuickMenuEvent>,
    field_area: Rect,
    move_locked: Rc<Cell<bool>>,
    selecting_releases: bool,
    current_release_names: Vec<String>,
    backlog_keys: Rc<RefCell<BacklogKeyBindings>>,
}

impl BacklogQuickMenu {
    #[cfg(test)]
    pub(in crate::pages::backlog) fn main_action_labels(
        status: &str,
        assignee: &str,
        epic: &str,
        release: &str,
    ) -> Vec<String> {
        BacklogQuickAction::main_actions(status, assignee, epic, release, "")
            .iter()
            .map(BacklogQuickAction::label)
            .collect()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn assignee_label(assignee: BacklogAssignee) -> String {
        BacklogQuickAction::AssignUserTo(assignee).label()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn new(move_locked: Rc<Cell<bool>>) -> Self {
        Self::new_with_keys(move_locked, BacklogKeyBindings::default())
    }

    pub(in crate::pages::backlog) fn new_with_keys(
        move_locked: Rc<Cell<bool>>,
        backlog_keys: BacklogKeyBindings,
    ) -> Self {
        let selected = Rc::new(RefCell::new(Vec::new()));
        let selected_releases = Rc::new(RefCell::new(None));
        let selected_actions = Rc::clone(&selected);
        let action_keys = Rc::new(RefCell::new(backlog_keys));
        let action_keys_for_renderer = Rc::clone(&action_keys);
        let dropdown = Dropdown::single_rich(
            BacklogQuickAction::main_actions("", "Unassigned", "", "", ""),
            |action| action.clone(),
            |action| action.label(),
            move |action, query, mode| {
                quick_action_text(action, query, mode, &action_keys_for_renderer.borrow())
            },
        )
        .variant(DropdownVariant::Filled)
        .label("Backlog actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(16)
        .on_select(move |actions| {
            if let Some(action) = actions.first() {
                selected_actions.borrow_mut().push(action.clone());
            }
        });
        let release_actions = Rc::clone(&selected_releases);
        let release_dropdown = Dropdown::multi(
            [BacklogQuickAction::ReleasesLoading],
            |action| action.clone(),
            |action| action.label(),
        )
        .variant(DropdownVariant::Filled)
        .label("Backlog actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(16)
        .external_loading_message("Loading releases…")
        .on_select(move |actions| *release_actions.borrow_mut() = Some(actions));
        Self {
            dropdown,
            release_dropdown,
            selected,
            selected_releases,
            keys: Vec::new(),
            section_id: None,
            source_order: Vec::new(),
            events: Vec::new(),
            field_area: Rect::default(),
            move_locked,
            selecting_releases: false,
            current_release_names: Vec::new(),
            backlog_keys: action_keys,
        }
    }

    pub(in crate::pages::backlog) fn set_backlog_keys(&mut self, backlog_keys: BacklogKeyBindings) {
        *self.backlog_keys.borrow_mut() = backlog_keys;
    }

    pub(in crate::pages::backlog) fn open(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        status: String,
        assignee: String,
        story_points: String,
        epic: String,
        release: String,
        destinations: Vec<BacklogDestination>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        self.selecting_releases = false;
        if !self.prepare_open(section_id, keys, source_order, true) {
            return false;
        }
        self.dropdown.set_rows(
            BacklogQuickAction::main_actions(&status, &assignee, &epic, &release, &story_points)
                .into_iter()
                .chain(
                    destinations
                        .into_iter()
                        .map(BacklogQuickAction::MoveToSection),
                ),
        );
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_status_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_statuses(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_assign_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_assignees(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_story_points_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_story_points(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_epic_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_epics(ctx);
        true
    }

    pub(in crate::pages::backlog) fn open_release_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if !self.prepare_open(section_id, keys, source_order, false) {
            return false;
        }
        self.open_releases(ctx);
        true
    }

    fn prepare_open(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        requires_move_unlocked: bool,
    ) -> bool {
        if requires_move_unlocked && self.move_locked.get() {
            self.events.push(BacklogQuickMenuEvent::MoveLocked);
            return false;
        }
        self.section_id = Some(section_id);
        self.keys = keys;
        self.source_order = source_order;
        self.selected.borrow_mut().clear();
        self.selected_releases.borrow_mut().take();
        self.dropdown.clear_selection();
        true
    }

    pub(in crate::pages::backlog) fn set_current_release_names(&mut self, names: Vec<String>) {
        self.current_release_names = names;
    }

    fn active_dropdown(&self) -> &Dropdown<BacklogQuickAction, BacklogQuickAction> {
        if self.selecting_releases {
            &self.release_dropdown
        } else {
            &self.dropdown
        }
    }

    fn active_dropdown_mut(&mut self) -> &mut Dropdown<BacklogQuickAction, BacklogQuickAction> {
        if self.selecting_releases {
            &mut self.release_dropdown
        } else {
            &mut self.dropdown
        }
    }

    pub(in crate::pages::backlog) fn set_statuses(&mut self, statuses: Vec<StatusTransition>) {
        self.selecting_releases = false;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows(statuses.into_iter().map(BacklogQuickAction::SetStatusTo));
        self.dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn set_assignees(&mut self, assignees: Vec<BacklogAssignee>) {
        self.selecting_releases = false;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows(assignees.into_iter().map(BacklogQuickAction::AssignUserTo));
        self.dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn set_epics(&mut self, epics: Vec<BacklogEpic>) {
        self.selecting_releases = false;
        self.selected.borrow_mut().clear();
        self.dropdown.clear_selection();
        self.dropdown.set_rows(
            std::iter::once(BacklogQuickAction::SetEpicTo(BacklogEpic {
                key: String::new(),
                title: "No epic".into(),
            }))
            .chain(epics.into_iter().map(BacklogQuickAction::SetEpicTo)),
        );
        self.dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn set_releases(&mut self, releases: Vec<BacklogRelease>) {
        self.selecting_releases = true;
        self.selected.borrow_mut().clear();
        self.release_dropdown.set_external_loading(false);
        self.release_dropdown
            .set_search_mode(DropdownSearchMode::Fuzzy);
        self.release_dropdown.clear_selection();
        let selected = releases
            .iter()
            .filter(|release| self.current_release_names.contains(&release.name))
            .cloned()
            .map(BacklogQuickAction::SetReleaseTo)
            .collect::<Vec<_>>();
        self.release_dropdown
            .set_rows(releases.into_iter().map(BacklogQuickAction::SetReleaseTo));
        self.release_dropdown.set_selected(selected);
        self.release_dropdown.set_search_query("");
    }

    pub(in crate::pages::backlog) fn take_events(&mut self) -> Vec<BacklogQuickMenuEvent> {
        std::mem::take(&mut self.events)
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn is_open_for_test(&self) -> bool {
        self.active_dropdown().is_open()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn selected_release_ids_for_test(&self) -> Vec<String> {
        self.release_dropdown
            .selected_ids()
            .into_iter()
            .filter_map(|action| match action {
                BacklogQuickAction::SetReleaseTo(release) => Some(release.id),
                _ => None,
            })
            .collect()
    }

    fn centered_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let hint = <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::measure(
            self.active_dropdown(),
            LayoutProposal::at_most(width, area.height),
        );
        let height = hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    fn finish_event(
        &mut self,
        was_open: bool,
        outcome: EventOutcome,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let selected = self.selected.borrow_mut().drain(..).collect::<Vec<_>>();
        if self.selecting_releases
            && let Some(selected) = self.selected_releases.borrow_mut().take()
        {
            let releases = selected
                .into_iter()
                .filter_map(|action| match action {
                    BacklogQuickAction::SetReleaseTo(release) => Some(release),
                    _ => None,
                })
                .collect::<Vec<_>>();
            self.events.push(BacklogQuickMenuEvent::SetReleases {
                keys: self.keys.clone(),
                releases,
            });
            return outcome;
        }
        for action in selected {
            let Some(section_id) = self.section_id.clone() else {
                continue;
            };
            let event = match action {
                BacklogQuickAction::SetStatus(_) => {
                    self.open_statuses(ctx);
                    continue;
                }
                BacklogQuickAction::StatusLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::SetStatusTo(status) => {
                    BacklogQuickMenuEvent::SetStatus { status }
                }
                BacklogQuickAction::SetStoryPoints(_) => {
                    self.open_story_points(ctx);
                    continue;
                }
                BacklogQuickAction::SetStoryPointsTo(story_points) => {
                    BacklogQuickMenuEvent::SetStoryPoints {
                        keys: self.keys.clone(),
                        story_points: story_points.value(),
                    }
                }
                BacklogQuickAction::AssignUser(_) => {
                    self.open_assignees(ctx);
                    continue;
                }
                BacklogQuickAction::AssigneesLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::AssignUserTo(assignee) => BacklogQuickMenuEvent::AssignUser {
                    keys: self.keys.clone(),
                    assignee,
                },
                BacklogQuickAction::SetEpic(_) => {
                    self.open_epics(ctx);
                    continue;
                }
                BacklogQuickAction::EpicsLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::SetEpicTo(epic) => BacklogQuickMenuEvent::SetEpic {
                    keys: self.keys.clone(),
                    epic,
                },
                BacklogQuickAction::SetRelease(_) => {
                    self.open_releases(ctx);
                    continue;
                }
                BacklogQuickAction::ReleasesLoading => {
                    self.dropdown.clear_selection();
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::SetReleaseTo(_) => continue,
                BacklogQuickAction::ViewDescription => {
                    let Some(key) = self.keys.first().cloned() else {
                        continue;
                    };
                    BacklogQuickMenuEvent::ViewDescription { key }
                }
                BacklogQuickAction::MoveToTop => BacklogQuickMenuEvent::MoveToTop {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToBottom => BacklogQuickMenuEvent::MoveToBottom {
                    section_id,
                    keys: self.keys.clone(),
                    source_order: self.source_order.clone(),
                },
                BacklogQuickAction::MoveToSection(destination) => {
                    self.dropdown.clear_selection();
                    self.dropdown.set_rows([
                        BacklogQuickAction::MoveToDestinationTop(destination.clone()),
                        BacklogQuickAction::MoveToDestinationBottom(destination),
                    ]);
                    self.dropdown.set_search_query("");
                    self.dropdown.open_with_context(ctx);
                    continue;
                }
                BacklogQuickAction::MoveToDestinationTop(destination) => {
                    BacklogQuickMenuEvent::MoveToSection {
                        source_section_id: section_id,
                        destination,
                        keys: self.keys.clone(),
                        to_top: true,
                    }
                }
                BacklogQuickAction::MoveToDestinationBottom(destination) => {
                    BacklogQuickMenuEvent::MoveToSection {
                        source_section_id: section_id,
                        destination,
                        keys: self.keys.clone(),
                        to_top: false,
                    }
                }
            };
            self.events.push(event);
        }
        if was_open && !self.active_dropdown().is_open() && self.events.is_empty() {
            self.events.push(BacklogQuickMenuEvent::Closed);
        }
        outcome
    }

    fn open_statuses(&mut self, ctx: &mut EventCtx<()>) {
        self.selecting_releases = false;
        self.dropdown.clear_selection();
        self.dropdown.set_rows([BacklogQuickAction::StatusLoading]);
        self.events.push(BacklogQuickMenuEvent::LoadStatuses {
            keys: self.keys.clone(),
        });
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn open_assignees(&mut self, ctx: &mut EventCtx<()>) {
        self.selecting_releases = false;
        self.dropdown.clear_selection();
        self.dropdown
            .set_rows([BacklogQuickAction::AssigneesLoading]);
        self.events.push(BacklogQuickMenuEvent::LoadAssignees);
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn open_story_points(&mut self, ctx: &mut EventCtx<()>) {
        self.selecting_releases = false;
        self.dropdown.clear_selection();
        self.dropdown.set_rows(
            BacklogStoryPoints::ALL
                .into_iter()
                .map(BacklogQuickAction::SetStoryPointsTo),
        );
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn open_epics(&mut self, ctx: &mut EventCtx<()>) {
        self.selecting_releases = false;
        self.dropdown.clear_selection();
        self.dropdown.set_rows([BacklogQuickAction::EpicsLoading]);
        self.events.push(BacklogQuickMenuEvent::LoadEpics);
        self.dropdown.set_search_query("");
        self.dropdown.open_with_context(ctx);
    }

    fn open_releases(&mut self, ctx: &mut EventCtx<()>) {
        self.selecting_releases = true;
        self.release_dropdown.clear_selection();
        self.release_dropdown
            .set_rows([BacklogQuickAction::ReleasesLoading]);
        self.release_dropdown
            .set_search_mode(DropdownSearchMode::External);
        self.release_dropdown.set_external_loading(true);
        self.events.push(BacklogQuickMenuEvent::LoadReleases);
        self.release_dropdown.set_search_query("");
        self.release_dropdown.open_with_context(ctx);
    }

    fn close(&mut self, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.active_dropdown_mut().close();
        self.selected.borrow_mut().clear();
        self.selected_releases.borrow_mut().take();
        self.events.push(BacklogQuickMenuEvent::Closed);
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        EventOutcome::Handled
    }
}

fn quick_action_text(
    action: &BacklogQuickAction,
    _query: &str,
    _mode: DropdownSearchMode,
    backlog_keys: &BacklogKeyBindings,
) -> Text<'static> {
    let label = action.label();
    let Some(hotkey) = quick_action_hotkey(action, backlog_keys) else {
        return quick_action_detail_text(action);
    };
    let spacing = usize::from(MENU_FIELD_WIDTH)
        .saturating_sub(line_width(&Line::from(label.as_str())))
        .saturating_sub(line_width(&Line::from(hotkey.as_str())));
    Text::from(Line::from(vec![
        Span::raw(label),
        Span::raw(" ".repeat(spacing)),
        Span::styled(hotkey, Style::default().fg(tuicore::theme().muted_fg())),
    ]))
}

fn quick_action_hotkey(
    action: &BacklogQuickAction,
    backlog_keys: &BacklogKeyBindings,
) -> Option<String> {
    match action {
        BacklogQuickAction::AssignUser(_) => Some(tuicore::KeySpec::plain('a').label()),
        BacklogQuickAction::SetStatus(_) => Some(tuicore::KeySpec::plain('s').label()),
        BacklogQuickAction::SetStoryPoints(_) => Some(tuicore::KeySpec::plain('p').label()),
        BacklogQuickAction::SetEpic(_) => Some(tuicore::KeySpec::plain('e').label()),
        BacklogQuickAction::SetRelease(_) => Some(tuicore::KeySpec::plain('r').label()),
        BacklogQuickAction::ViewDescription => Some(backlog_keys.view_description.label()),
        BacklogQuickAction::MoveToTop => Some(backlog_keys.move_to_top.label()),
        BacklogQuickAction::MoveToBottom => Some(backlog_keys.move_to_bottom.label()),
        BacklogQuickAction::StatusLoading
        | BacklogQuickAction::SetStatusTo(_)
        | BacklogQuickAction::SetStoryPointsTo(_)
        | BacklogQuickAction::AssigneesLoading
        | BacklogQuickAction::AssignUserTo(_)
        | BacklogQuickAction::EpicsLoading
        | BacklogQuickAction::SetEpicTo(_)
        | BacklogQuickAction::ReleasesLoading
        | BacklogQuickAction::SetReleaseTo(_)
        | BacklogQuickAction::MoveToSection(_)
        | BacklogQuickAction::MoveToDestinationTop(_)
        | BacklogQuickAction::MoveToDestinationBottom(_) => None,
    }
}

fn quick_action_detail_text(action: &BacklogQuickAction) -> Text<'static> {
    match action {
        BacklogQuickAction::SetStoryPointsTo(BacklogStoryPoints::None) => Text::from(Line::from(
            Span::styled("None", Style::default().fg(tuicore::theme().muted_fg())),
        )),
        BacklogQuickAction::AssignUserTo(BacklogAssignee { account_id, .. })
        | BacklogQuickAction::SetEpicTo(BacklogEpic {
            key: account_id, ..
        }) if account_id.is_empty() => Text::from(Line::from(Span::styled(
            "None",
            Style::default().fg(tuicore::theme().muted_fg()),
        ))),
        _ => Text::raw(action.label()),
    }
}

impl TuiNode for BacklogQuickMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.field_area = self.centered_field_area(area);
        let field_area = self.field_area;
        if self.selecting_releases {
            ctx.push_slot(ChildKey::new(RELEASE_DROPDOWN_KEY), field_area, |ctx| {
                <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::layout(
                    &mut self.release_dropdown,
                    field_area,
                    ctx,
                )
            });
        } else {
            <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::layout(
                &mut self.dropdown,
                field_area,
                ctx,
            );
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active_dropdown().render(frame, self.field_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            return self.close(ctx);
        }
        let was_open = self.active_dropdown().is_open();
        let outcome = self.active_dropdown_mut().event(event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            return self.close(ctx);
        }
        let was_open = self.active_dropdown().is_open();
        let outcome = if self.selecting_releases {
            route
                .path
                .without_first_if(&ChildKey::new(RELEASE_DROPDOWN_KEY))
                .map(EventRoute::new)
                .map(|route| self.release_dropdown.dispatch_event(&route, event, ctx))
                .unwrap_or(EventOutcome::Ignored)
        } else {
            self.dropdown.dispatch_event(route, event, ctx)
        };
        self.finish_event(was_open, outcome, ctx)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_dropdown_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(target) = target.for_child(&ChildKey::new(RELEASE_DROPDOWN_KEY)) {
            self.release_dropdown.dispatch_focus(&target, focused, ctx);
        } else {
            self.dropdown.dispatch_focus(target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<BacklogQuickAction, BacklogQuickAction> as TuiNode<()>>::tick(
            self.active_dropdown_mut(),
            dt,
            settings,
        )
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.init(ctx);
        self.release_dropdown.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.mount(ctx);
        self.release_dropdown.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.unmount(ctx);
        self.release_dropdown.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.destroy(ctx);
        self.release_dropdown.destroy(ctx);
    }
}
