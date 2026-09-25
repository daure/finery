use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc::Sender,
    time::Duration,
};

#[path = "release_row.rs"]
mod release_row;

use ratatui::{
    Frame,
    layout::Constraint,
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tuicore::{
    Button, CellContext, ChildKey, Column, DataViewTransformMode, Dropdown, DropdownLabelPosition,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusRequest,
    FocusTarget, HotkeyEvent, Key, KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, ListControl, ListControlEvent, ListControlKeyBindings,
    MenuButton, MenuItem, RenderCtx, SearchMode, TickResult, Toggle, TreeAdapter, TuiEvent,
    TuiNode,
};

use crate::{
    app_settings::{AppSettings, BacklogKeyBindings, ComposerKeyBinding},
    components::{
        avatar::initials,
        ticket_number_jump::{TicketNumberJump, exact_ticket_number_matches},
        ticket_yank_menu::{TicketYankAction, TicketYankMenu, TicketYankTarget},
        work_item_rows::{
            TicketRowDetails, WorkItemKind, WorkItemRow, status_category_style,
            ticket_summary_text, work_item_title_prefix_width,
        },
    },
    jira::{JiraOption, version_name_cmp},
    store::work_items::{
        BacklogSnapshot, RunwayCapacitySource, RunwayTicket, Sprint, SprintCapacityState,
        StatusCategory, SubtaskProgress, WorkItem, release,
        saved_filter::{
            BacklogFilterCriteria, BacklogFilterOptions, SavedBacklogFilter,
            matches_optional_values,
        },
    },
};

use super::filter_values::SavedFilterField;

const FILTER_POPUP_MAX_WIDTH: u16 = u16::MAX;

#[derive(Clone)]
struct BacklogRow {
    id: String,
    parent_id: Option<String>,
    content: BacklogRowContent,
}

#[derive(Clone)]
enum BacklogRowContent {
    Section {
        title: Text<'static>,
        search_text: String,
    },
    Group {
        title: Text<'static>,
        search_text: String,
    },
    WorkItem(Box<BacklogWorkItem>),
}

#[derive(Clone)]
struct BacklogWorkItem {
    item: WorkItemRow,
    description: String,
    section: String,
    rankable_root: bool,
    runway: Option<RunwayTicket>,
    alternate_background: bool,
    subtask_progress: Option<SubtaskProgress>,
    fix_versions: Vec<String>,
    epic_name: Option<String>,
    comment_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum BacklogSectionEvent {
    Refresh,
    EstimatedChanged(bool),
    IssueTypesChanged(Vec<String>),
    UsersChanged(Vec<String>),
    StatusesChanged(Vec<String>),
    EpicsChanged(Vec<String>),
    LabelsChanged(Vec<String>),
    ReleasesChanged(Vec<String>),
    SavedFilterSelected(u64),
    SavedFilterCleared,
    OpenSavedFilterManager,
    OpenVelocity,
    OpenReports,
    OpenTimeline,
    OpenBoard,
    OpenReleases,
    WebMenuClosed,
    MoveLocked,
    TicketsSyncing {
        keys: Vec<String>,
    },
    OpenTicket {
        key: String,
    },
    OpenCommand {
        key: String,
    },
    OpenDescription {
        key: String,
    },
    YankTicketUrl {
        key: String,
    },
    YankSprintGoal {
        goal: String,
    },
    YankSprintReport {
        sprint_id: u64,
    },
    OpenQuickMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
        section_moves_available: bool,
    },
    OpenStatusMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    OpenStoryPointsMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    OpenAssignMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    OpenEpicMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    OpenReleaseMenu {
        section_id: String,
        keys: Vec<String>,
        source_order: Vec<String>,
    },
    ToggleCurrentUser {
        keys: Vec<String>,
    },
    MoveToEdge {
        section_id: String,
        key: String,
        source_order: Vec<String>,
        to_top: bool,
    },
    Moved {
        section_id: String,
        moved_keys: Vec<String>,
        final_order: Vec<String>,
    },
    Rejected {
        section_id: String,
        message: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum WebMenuItem {
    Board,
    Timeline,
    Releases,
    Reports,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BacklogGroupBy {
    Release,
    Epic,
    User,
}

impl BacklogGroupBy {
    fn id(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Epic => "epic",
            Self::User => "user",
        }
    }
}

fn grouping_icon(grouping: Option<BacklogGroupBy>) -> &'static str {
    match grouping {
        Some(BacklogGroupBy::Release) => "",
        Some(BacklogGroupBy::Epic) => "",
        Some(BacklogGroupBy::User) => "󰀆",
        None => "󰑮",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum GroupByMenuItem {
    Release,
    Epic,
    User,
    Ungroup,
}

#[derive(Clone)]
struct SavedFilterChoice {
    id: String,
    label: String,
}

impl GroupByMenuItem {
    fn grouping(self) -> Option<BacklogGroupBy> {
        match self {
            Self::Release => Some(BacklogGroupBy::Release),
            Self::Epic => Some(BacklogGroupBy::Epic),
            Self::User => Some(BacklogGroupBy::User),
            Self::Ungroup => None,
        }
    }
}

#[cfg(test)]
pub(in crate::pages::backlog) fn backlog_tree(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
) -> BacklogTree {
    backlog_tree_with_issue_types(
        snapshot,
        events,
        move_locked,
        Rc::new(RefCell::new(HashSet::new())),
        Vec::new(),
    )
}

#[cfg(test)]
pub(in crate::pages::backlog) fn backlog_tree_with_issue_types(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    syncing_ticket_keys: Rc<RefCell<HashSet<String>>>,
    issue_types: Vec<JiraOption>,
) -> BacklogTree {
    backlog_tree_with_issue_types_and_settings(
        snapshot,
        events,
        move_locked,
        syncing_ticket_keys,
        issue_types,
        &AppSettings::default(),
    )
}

pub(in crate::pages::backlog) fn backlog_tree_with_issue_types_and_settings(
    snapshot: &BacklogSnapshot,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    syncing_ticket_keys: Rc<RefCell<HashSet<String>>>,
    issue_types: Vec<JiraOption>,
    settings: &AppSettings,
) -> BacklogTree {
    let number_jump = Rc::new(RefCell::new(TicketNumberJump::default()));
    #[cfg(test)]
    let renderer_calls = Rc::new(Cell::new(0));
    let filters = BacklogFilters::default();
    let issue_types = selectable_issue_types(issue_types);
    let mut control = ListControl::new(
        backlog_rows(snapshot, &filters, None, ""),
        |row: &BacklogRow| row.id.clone(),
        |_, _| unreachable!("backlog does not add rows"),
    )
    .headers(false)
    .columns(vec![backlog_column(
        Rc::clone(&number_jump),
        #[cfg(test)]
        Rc::clone(&renderer_calls),
    )])
    .tree(TreeAdapter::mutable_parent_id(
        |row: &BacklogRow| row.parent_id.clone(),
        |row, parent_id| row.parent_id = parent_id,
    ))
    .expanded(initially_expanded_rows(snapshot, None))
    .allow_horizontal_moving(false)
    .max_rows(usize::MAX)
    .panel_visible(false)
    .action_bar(true)
    .search_mode(SearchMode::Contains)
    .keybindings(
        ListControlKeyBindings::default()
            .add([])
            .add_child([])
            .edit([])
            .remove([]),
    )
    .empty_message("No stories");
    control.data_view_mut().set_wrap_geometry_epoch(0);
    let runway_markers_visible = Rc::new(Cell::new(!filters.is_active()));
    let row_marker_visible = Rc::clone(&runway_markers_visible);
    control.data_view_mut().set_row_height_by(|row| {
        if matches!(row.content, BacklogRowContent::Group { .. }) {
            match &row.content {
                BacklogRowContent::Group { search_text, .. } => {
                    1 + u16::from(
                        search_text == "(no user assigned)"
                            || !is_unassigned_group_label(search_text),
                    )
                }
                _ => unreachable!(),
            }
        } else if matches!(row.content, BacklogRowContent::Section { .. }) {
            u16::from(row.id != section_row_id("backlog")) + 1
        } else {
            2
        }
    });
    control.data_view_mut().set_wrap_cells(true);
    control.data_view_mut().set_show_inactive_highlight(true);
    control
        .data_view_mut()
        .set_row_style_by(|row| match &row.content {
            BacklogRowContent::WorkItem(item) if item.alternate_background => {
                Some(Style::default().bg(tuicore::theme().surface_bg()))
            }
            _ => None,
        });
    control
        .data_view_mut()
        .set_left_gutter_marker_by(move |row| match &row.content {
            BacklogRowContent::WorkItem(item) => Some(
                if row_marker_visible.get()
                    && item
                        .runway
                        .as_ref()
                        .is_some_and(|runway| runway.virtual_sprint % 2 == 1)
                {
                    Span::styled("┃", Style::default().fg(tuicore::theme().accent_fg()))
                } else {
                    Span::raw(" ")
                },
            ),
            _ => None,
        });
    let refresh_events = events.clone();
    let velocity_events = events.clone();
    let estimated_events = events.clone();
    let issue_type_events = events.clone();
    let user_events = events.clone();
    let status_events = events.clone();
    let epic_events = events.clone();
    let label_events = events.clone();
    let release_events = events.clone();
    let saved_filter_events = events.clone();
    let issue_type_labels = Rc::new(RefCell::new(issue_type_labels(&issue_types)));
    let selected_issue_type_labels = Rc::clone(&issue_type_labels);
    BacklogTree {
        control,
        refresh: Button::new("󰑓").hotkey("shift+r").on_press(move || {
            let _ = refresh_events.send(BacklogSectionEvent::Refresh);
        }),
        velocity: Button::new("󰓅").hotkey("shift+v").on_press(move || {
            let _ = velocity_events.send(BacklogSectionEvent::OpenVelocity);
        }),
        estimated: Toggle::new("󰑭")
            .checked(true)
            .hotkey("shift+d")
            .preserve_focus_on_hotkey(true)
            .on_change(move |estimated| {
                let _ = estimated_events.send(BacklogSectionEvent::EstimatedChanged(estimated));
            }),
        saved_filter: Dropdown::single(
            saved_filter_choices(&settings.saved_backlog_filters),
            |choice: &SavedFilterChoice| choice.id.clone(),
            |choice| choice.label.clone(),
        )
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("󰈲")
        .no_selection_text("None")
        .field_padding_left(1)
        .hotkey("shift+f")
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let Some(selected) = selected.first() else {
                let _ = saved_filter_events.send(BacklogSectionEvent::SavedFilterCleared);
                return;
            };
            let event = if selected == "manage" {
                BacklogSectionEvent::OpenSavedFilterManager
            } else if let Some(id) = selected
                .strip_prefix("filter:")
                .and_then(|id| id.parse::<u64>().ok())
            {
                BacklogSectionEvent::SavedFilterSelected(id)
            } else {
                return;
            };
            let _ = saved_filter_events.send(event);
        }),
        issue_types: Dropdown::multi(
            issue_types,
            |issue_type: &JiraOption| issue_type.id.clone(),
            |issue_type| issue_type.label.clone(),
        )
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("󰡯")
        .field_padding_left(1)
        .hotkey(SavedFilterField::IssueTypes.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let issue_types = selected
                .into_iter()
                .filter_map(|id| selected_issue_type_labels.borrow().get(&id).cloned())
                .collect();
            let _ = issue_type_events.send(BacklogSectionEvent::IssueTypesChanged(issue_types));
        }),
        users: Dropdown::multi(
            selectable_users(snapshot),
            |user: &String| user.clone(),
            |user| SavedFilterField::Users.label(user),
        )
        .row_style_by(|user| SavedFilterField::Users.style(user))
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("󰀄")
        .field_padding_left(1)
        .selected_label_by(|user| {
            if user == "Unassigned" {
                user.into()
            } else {
                format!("@{}", initials(user))
            }
        })
        .show_multi_labels(true)
        .hotkey(SavedFilterField::Users.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let _ = user_events.send(BacklogSectionEvent::UsersChanged(selected));
        }),
        statuses: Dropdown::multi(
            selectable_statuses(snapshot),
            |status: &String| status.clone(),
            |status| status.clone(),
        )
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("")
        .field_padding_left(1)
        .hotkey(SavedFilterField::Statuses.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let _ = status_events.send(BacklogSectionEvent::StatusesChanged(selected));
        }),
        epics: Dropdown::multi(
            selectable_epics(snapshot),
            |epic: &String| epic.clone(),
            |epic| SavedFilterField::Epics.label(epic),
        )
        .row_style_by(|epic| SavedFilterField::Epics.style(epic))
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("")
        .field_padding_left(1)
        .hotkey(SavedFilterField::Epics.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let _ = epic_events.send(BacklogSectionEvent::EpicsChanged(selected));
        }),
        labels: Dropdown::multi(
            selectable_labels(snapshot),
            |label: &String| label.clone(),
            |label| SavedFilterField::Labels.label(label),
        )
        .row_style_by(|label| SavedFilterField::Labels.style(label))
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("")
        .field_padding_left(1)
        .hotkey(SavedFilterField::Labels.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let _ = label_events.send(BacklogSectionEvent::LabelsChanged(selected));
        }),
        releases: Dropdown::multi(
            selectable_releases(snapshot),
            |release: &String| release.clone(),
            |release| SavedFilterField::Releases.label(release),
        )
        .row_style_by(|release| SavedFilterField::Releases.style(release))
        .label_position(DropdownLabelPosition::Inline)
        .alt_style(true)
        .variant(DropdownVariant::Filled)
        .placeholder("")
        .field_padding_left(1)
        .hotkey(SavedFilterField::Releases.hotkey())
        .max_popup_width(FILTER_POPUP_MAX_WIDTH)
        .on_select(move |selected| {
            let _ = release_events.send(BacklogSectionEvent::ReleasesChanged(selected));
        }),
        group_by: MenuButton::new(
            grouping_icon(None),
            [
                MenuItem::new(GroupByMenuItem::Ungroup, "󰑮 Sprint"),
                MenuItem::new(GroupByMenuItem::Release, " Release"),
                MenuItem::new(GroupByMenuItem::Epic, " Epic"),
                MenuItem::new(GroupByMenuItem::User, "󰀆 User"),
            ],
        )
        .min_popup_width(12)
        .hotkey("shift+p"),
        web: MenuButton::new(
            "󰖟",
            [
                MenuItem::new(WebMenuItem::Board, "Board"),
                MenuItem::new(WebMenuItem::Timeline, "Timeline"),
                MenuItem::new(WebMenuItem::Releases, "Releases"),
                MenuItem::new(WebMenuItem::Reports, "Reports"),
            ],
        )
        .min_popup_width(12)
        .hotkey("shift+w"),
        yank_menu: TicketYankMenu::new(),
        loading: false,
        refresh_area: ratatui::layout::Rect::default(),
        velocity_area: ratatui::layout::Rect::default(),
        estimated_area: ratatui::layout::Rect::default(),
        saved_filter_area: ratatui::layout::Rect::default(),
        issue_types_area: ratatui::layout::Rect::default(),
        users_area: ratatui::layout::Rect::default(),
        statuses_area: ratatui::layout::Rect::default(),
        epics_area: ratatui::layout::Rect::default(),
        labels_area: ratatui::layout::Rect::default(),
        releases_area: ratatui::layout::Rect::default(),
        group_by_area: ratatui::layout::Rect::default(),
        web_area: ratatui::layout::Rect::default(),
        control_area: ratatui::layout::Rect::default(),
        events,
        move_locked,
        syncing_ticket_keys,
        runway_markers_visible,
        filters,
        group_by_selection: None,
        issue_type_labels,
        snapshot: snapshot.clone(),
        number_jump,
        wrap_geometry_epoch: 0,
        #[cfg(test)]
        renderer_calls,
        backlog_keys: settings.backlog_keys.clone(),
        open_command_key: settings.open_command_key.clone(),
    }
}

pub(in crate::pages::backlog) struct BacklogTree {
    control: ListControl<BacklogRow, String>,
    refresh: Button<()>,
    velocity: Button<()>,
    estimated: Toggle<()>,
    saved_filter: Dropdown<SavedFilterChoice, String>,
    issue_types: Dropdown<JiraOption, String>,
    users: Dropdown<String, String>,
    statuses: Dropdown<String, String>,
    epics: Dropdown<String, String>,
    labels: Dropdown<String, String>,
    releases: Dropdown<String, String>,
    group_by: MenuButton<GroupByMenuItem>,
    web: MenuButton<WebMenuItem>,
    yank_menu: TicketYankMenu,
    loading: bool,
    refresh_area: ratatui::layout::Rect,
    velocity_area: ratatui::layout::Rect,
    estimated_area: ratatui::layout::Rect,
    saved_filter_area: ratatui::layout::Rect,
    issue_types_area: ratatui::layout::Rect,
    users_area: ratatui::layout::Rect,
    statuses_area: ratatui::layout::Rect,
    epics_area: ratatui::layout::Rect,
    labels_area: ratatui::layout::Rect,
    releases_area: ratatui::layout::Rect,
    group_by_area: ratatui::layout::Rect,
    web_area: ratatui::layout::Rect,
    control_area: ratatui::layout::Rect,
    events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    syncing_ticket_keys: Rc<RefCell<HashSet<String>>>,
    runway_markers_visible: Rc<Cell<bool>>,
    filters: BacklogFilters,
    group_by_selection: Option<BacklogGroupBy>,
    issue_type_labels: Rc<RefCell<HashMap<String, String>>>,
    snapshot: BacklogSnapshot,
    number_jump: Rc<RefCell<TicketNumberJump>>,
    wrap_geometry_epoch: u64,
    #[cfg(test)]
    renderer_calls: Rc<Cell<usize>>,
    backlog_keys: BacklogKeyBindings,
    open_command_key: ComposerKeyBinding,
}

impl BacklogTree {
    pub(in crate::pages::backlog) fn set_snapshot(&mut self, snapshot: &BacklogSnapshot) {
        self.snapshot = snapshot.clone();
        self.set_issue_types(issue_types_in_snapshot(snapshot));
        self.users.set_rows(selectable_users(snapshot));
        self.statuses.set_rows(selectable_statuses(snapshot));
        self.epics.set_rows(selectable_epics(snapshot));
        self.labels.set_rows(selectable_labels(snapshot));
        self.releases.set_rows(selectable_releases(snapshot));
        let highlighted = self.control.data_view().highlighted_id();
        let expanded = self.control.data_view().tree_expansion_snapshot();
        let highlighted_parent = highlighted.as_ref().and_then(|id| {
            self.control
                .items()
                .iter()
                .find(|row| &row.id == id)
                .and_then(|row| row.parent_id.clone())
        });
        let search = self.control.data_view().transform_state().search.clone();
        self.control.set_rows(backlog_rows(
            snapshot,
            &self.filters,
            self.group_by_selection,
            &search,
        ));
        self.advance_wrap_geometry_epoch();
        self.sync_search_results();
        let mut expanded = expanded
            .into_iter()
            .filter(|id| self.is_expandable(id))
            .collect::<HashSet<_>>();
        if !self.filters.issue_types.is_empty()
            || !self.filters.statuses.is_empty()
            || !self.filters.epics.is_empty()
            || !self.filters.labels.is_empty()
            || !self.filters.releases.is_empty()
        {
            expanded.extend(self.control.items().iter().filter_map(|row| {
                row.parent_id
                    .as_deref()
                    .filter(|id| id.starts_with("ticket:"))
                    .map(str::to_owned)
            }));
        }
        self.control
            .data_view_mut()
            .restore_tree_expansion(expanded);
        if highlighted
            .as_ref()
            .is_some_and(|id| !self.control.items().iter().any(|row| &row.id == id))
        {
            let fallback = highlighted_parent
                .filter(|id| self.control.items().iter().any(|row| &row.id == id))
                .unwrap_or_else(|| {
                    self.control
                        .items()
                        .iter()
                        .find(|row| row.parent_id.is_none())
                        .map(|row| row.id.clone())
                        .unwrap_or_else(|| section_row_id("backlog"))
                });
            self.highlight(&fallback);
        }
        let parents = self
            .control
            .transient_selected_ids()
            .into_iter()
            .filter_map(|id| self.control.items().iter().find(|row| row.id == id))
            .filter_map(|row| row.parent_id.clone())
            .collect::<std::collections::HashSet<_>>();
        if parents.len() > 1 {
            self.control.clear_transient_selection();
        }
    }

    pub(in crate::pages::backlog) fn set_estimated(&mut self, estimated: bool) {
        self.saved_filter.clear_selection();
        self.filters.estimated = estimated;
        self.estimated.set_value(estimated);
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_issue_types_filter(&mut self, issue_types: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.issue_types = issue_types;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_users_filter(&mut self, users: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.users = SavedFilterField::Users.selection(users);
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_statuses_filter(&mut self, statuses: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.statuses = statuses;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_epics_filter(&mut self, epics: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.epics = epics;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_labels_filter(&mut self, labels: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.labels = labels;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn set_releases_filter(&mut self, releases: Vec<String>) {
        self.saved_filter.clear_selection();
        self.filters.releases = releases;
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn reset_to_home(&mut self) {
        self.filters = BacklogFilters::default();
        self.saved_filter.close();
        self.saved_filter.clear_selection();
        self.estimated.set_value(true);
        self.issue_types.close();
        self.issue_types.clear_selection();
        self.users.close();
        self.users.clear_selection();
        self.statuses.close();
        self.statuses.clear_selection();
        self.epics.close();
        self.epics.clear_selection();
        self.labels.close();
        self.labels.clear_selection();
        self.releases.close();
        self.releases.clear_selection();
        self.group_by_selection = None;
        self.group_by.set_disabled(true);
        self.group_by.set_disabled(false);
        self.group_by.set_label(grouping_icon(None));
        self.web.set_disabled(true);
        self.web.set_disabled(false);
        self.number_jump.borrow_mut().clear();
        self.advance_wrap_geometry_epoch();

        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
        self.control.data_view_mut().clear_search();
        self.sync_search_results();
        self.control
            .data_view_mut()
            .restore_tree_expansion(HashSet::from([section_row_id("backlog")]));
        self.control.clear_transient_selection();
        self.highlight(&section_row_id("backlog"));
        self.control.data_view_mut().reveal_highlighted_centered();
        self.runway_markers_visible.set(true);
    }

    pub(in crate::pages::backlog) fn set_saved_filters(
        &mut self,
        saved_filters: &[SavedBacklogFilter],
    ) {
        self.saved_filter
            .set_rows(saved_filter_choices(saved_filters));
    }

    pub(in crate::pages::backlog) fn set_selected_saved_filter(&mut self, id: Option<u64>) {
        if let Some(id) = id {
            self.saved_filter.set_selected_one(format!("filter:{id}"));
        } else {
            self.saved_filter.clear_selection();
        }
    }

    pub(in crate::pages::backlog) fn apply_saved_filter(&mut self, filter: &SavedBacklogFilter) {
        self.set_selected_saved_filter(Some(filter.id));
        self.apply_filter_criteria(&filter.criteria);
    }

    pub(in crate::pages::backlog) fn clear_saved_filter(&mut self) {
        self.set_selected_saved_filter(None);
        self.apply_filter_criteria(&BacklogFilterCriteria::unfiltered());
    }

    fn apply_filter_criteria(&mut self, criteria: &BacklogFilterCriteria) {
        self.filters = BacklogFilters::from(criteria);
        self.estimated.set_value(criteria.estimated);
        let issue_type_ids = self
            .issue_type_labels
            .borrow()
            .iter()
            .filter(|(_, label)| {
                criteria
                    .issue_types
                    .iter()
                    .any(|selected| selected.eq_ignore_ascii_case(label))
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        self.issue_types.set_selected(issue_type_ids);
        self.users.set_selected(self.filters.users.clone());
        self.statuses.set_selected(criteria.statuses.clone());
        self.epics.set_selected(self.filters.epics.clone());
        self.labels.set_selected(self.filters.labels.clone());
        self.releases.set_selected(self.filters.releases.clone());
        self.runway_markers_visible.set(self.show_runway_bands());
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
    }

    pub(in crate::pages::backlog) fn filter_options(&self) -> BacklogFilterOptions {
        let mut issue_types = self
            .issue_type_labels
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        issue_types.sort_unstable_by_key(|value| value.to_ascii_lowercase());
        BacklogFilterOptions {
            issue_types,
            users: selectable_users(&self.snapshot),
            statuses: selectable_statuses(&self.snapshot),
            epics: selectable_epics(&self.snapshot),
            labels: selectable_labels(&self.snapshot),
            releases: selectable_releases(&self.snapshot),
        }
    }

    pub(in crate::pages::backlog) fn set_issue_types(&mut self, issue_types: Vec<JiraOption>) {
        let issue_types = selectable_issue_types(issue_types);
        *self.issue_type_labels.borrow_mut() = issue_type_labels(&issue_types);
        self.issue_types.set_rows(issue_types);
    }

    pub(in crate::pages::backlog) fn set_backlog_keys(&mut self, backlog_keys: BacklogKeyBindings) {
        self.backlog_keys = backlog_keys;
    }

    pub(in crate::pages::backlog) fn set_open_command_key(&mut self, key: ComposerKeyBinding) {
        self.open_command_key = key;
    }

    pub(in crate::pages::backlog) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        self.refresh.set_disabled(loading);
        self.velocity.set_disabled(loading);
        self.estimated.set_disabled(loading);
        self.saved_filter.set_disabled(loading);
        self.issue_types.set_disabled(loading);
        self.users.set_disabled(loading);
        self.statuses.set_disabled(loading);
        self.epics.set_disabled(loading);
        self.labels.set_disabled(loading);
        self.releases.set_disabled(loading);
        self.group_by.set_disabled(loading);
    }

    pub(in crate::pages::backlog) fn highlight(&mut self, row_id: &str) {
        self.control
            .data_view_mut()
            .highlight_id(&row_id.to_owned());
    }

    pub(in crate::pages::backlog) fn clear_selection_and_highlight_ticket(&mut self, key: &str) {
        let row_id = self
            .control
            .items()
            .iter()
            .find_map(|row| match &row.content {
                BacklogRowContent::WorkItem(item) if item.item.key == key => Some(row.id.clone()),
                BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. } => None,
                BacklogRowContent::WorkItem(_) => None,
            });
        self.control.clear_transient_selection();
        if let Some(row_id) = row_id {
            self.highlight(&row_id);
        }
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn highlighted_id_for_test(&self) -> Option<String> {
        self.control.data_view().highlighted_id()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn take_renderer_calls_for_test(&self) -> usize {
        self.renderer_calls.replace(0)
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn is_reordering_for_test(&self) -> bool {
        self.control.is_reordering()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn group_by_release_for_test(&mut self) {
        self.set_group_by_selection(Some(BacklogGroupBy::Release));
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn group_by_epic_for_test(&mut self) {
        self.set_group_by_selection(Some(BacklogGroupBy::Epic));
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn group_by_user_for_test(&mut self) {
        self.set_group_by_selection(Some(BacklogGroupBy::User));
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn expansion_snapshot_for_test(&self) -> HashSet<String> {
        self.control.data_view().tree_expansion_snapshot()
    }

    #[cfg(test)]
    pub(in crate::pages::backlog) fn runway_markers_visible_for_test(&self) -> bool {
        self.runway_markers_visible.get()
    }

    fn is_expandable(&self, id: &str) -> bool {
        self.control.items().iter().any(|row| {
            row.id == id
                && (matches!(
                    row.content,
                    BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. }
                ) || self
                    .control
                    .items()
                    .iter()
                    .any(|child| child.parent_id.as_deref() == Some(id)))
        })
    }

    fn is_section(&self, id: &str) -> bool {
        self.control
            .items()
            .iter()
            .any(|row| row.id == id && matches!(row.content, BacklogRowContent::Section { .. }))
    }

    fn highlighted_section(&self) -> bool {
        self.control
            .data_view()
            .highlighted_id()
            .is_some_and(|id| self.is_section(&id))
    }

    fn highlighted_subtask(&self) -> bool {
        self.control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
            .and_then(|row| row.parent_id.as_deref())
            .is_some_and(|parent_id| parent_id.starts_with("ticket:"))
    }

    fn selected_issue_keys(&self) -> Option<(String, Vec<String>, Vec<String>)> {
        let selected = self.control.transient_selected_ids();
        let ids = if selected.is_empty() {
            self.control
                .data_view()
                .highlighted_id()
                .into_iter()
                .collect()
        } else {
            selected
        };
        let mut section = None;
        let mut keys = Vec::new();
        for id in ids {
            let row = self.control.items().iter().find(|row| row.id == id)?;
            let BacklogRowContent::WorkItem(item) = &row.content else {
                return None;
            };
            if section
                .as_ref()
                .is_some_and(|current| current != &item.section)
            {
                return None;
            }
            section = Some(item.section.clone());
            keys.push(item.item.key.clone());
        }
        let section = section?;
        let order = self.issue_keys_in_section(&section);
        keys.sort_by_key(|key| order.iter().position(|candidate| candidate == key));
        Some((section, keys, order))
    }

    fn tickets_are_syncing(&self, keys: &[String]) -> bool {
        let syncing = self.syncing_ticket_keys.borrow();
        keys.iter().any(|key| syncing.contains(key))
    }

    fn issue_keys_in_section(&self, section: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        self.control
            .items()
            .iter()
            .filter_map(|row| match &row.content {
                BacklogRowContent::WorkItem(item)
                    if item.section == section
                        && item.rankable_root
                        && seen.insert(item.item.key.clone()) =>
                {
                    Some(item.item.key.clone())
                }
                BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. } => None,
                BacklogRowContent::WorkItem(_) => None,
            })
            .collect()
    }

    fn blocks_locked_gesture(&self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        let modifiers = key.modifiers;
        let selection_navigation = (modifiers == KeyModifiers::SHIFT
            || modifiers == KeyModifiers::CONTROL)
            && (tuicore::keybindings().line_up_matches(*key)
                || tuicore::keybindings().line_down_matches(*key));
        KeySpec::key_with_modifiers(Key::Char('m'), KeyModifiers::CONTROL).matches(*key)
            || KeySpec::plain('<').matches(*key)
            || KeySpec::plain('>').matches(*key)
            || KeySpec::plain(' ').matches(*key)
            || selection_navigation
    }

    fn blocks_section_gesture(&self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        ((key.modifiers == KeyModifiers::SHIFT || key.modifiers == KeyModifiers::CONTROL)
            && (tuicore::keybindings().line_up_matches(*key)
                || tuicore::keybindings().line_down_matches(*key)))
            || KeySpec::key_with_modifiers(Key::Char('m'), KeyModifiers::CONTROL).matches(*key)
    }

    fn open_quick_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            '.',
            true,
            |section_id, keys, source_order| BacklogSectionEvent::OpenQuickMenu {
                section_id,
                keys,
                source_order,
                section_moves_available: self.group_by_selection.is_none(),
            },
            ctx,
        )
    }

    fn open_status_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            's',
            false,
            |section_id, keys, source_order| BacklogSectionEvent::OpenStatusMenu {
                section_id,
                keys,
                source_order,
            },
            ctx,
        )
    }

    fn open_assign_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            'a',
            false,
            |section_id, keys, source_order| BacklogSectionEvent::OpenAssignMenu {
                section_id,
                keys,
                source_order,
            },
            ctx,
        )
    }

    fn open_story_points_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            'p',
            false,
            |section_id, keys, source_order| BacklogSectionEvent::OpenStoryPointsMenu {
                section_id,
                keys,
                source_order,
            },
            ctx,
        )
    }

    fn open_epic_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            'e',
            false,
            |section_id, keys, source_order| BacklogSectionEvent::OpenEpicMenu {
                section_id,
                keys,
                source_order,
            },
            ctx,
        )
    }

    fn open_release_menu(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            'r',
            false,
            |section_id, keys, source_order| BacklogSectionEvent::OpenReleaseMenu {
                section_id,
                keys,
                source_order,
            },
            ctx,
        )
    }

    fn toggle_current_user(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        self.open_item_menu(
            event,
            'i',
            false,
            |_, keys, _| BacklogSectionEvent::ToggleCurrentUser { keys },
            ctx,
        )
    }

    fn open_item_menu(
        &self,
        event: &TuiEvent,
        key: char,
        requires_move_unlocked: bool,
        event_for_selection: impl FnOnce(String, Vec<String>, Vec<String>) -> BacklogSectionEvent,
        ctx: &mut EventCtx<()>,
    ) -> bool {
        if self.control.data_view().is_searching() {
            return false;
        }
        if !matches!(event, TuiEvent::Key(pressed) if KeySpec::plain(key).matches(*pressed)) {
            return false;
        }
        if requires_move_unlocked && self.move_locked.get() {
            let _ = self.events.send(BacklogSectionEvent::MoveLocked);
        } else if !self.control.is_reordering()
            && let Some((section_id, keys, source_order)) = self.selected_issue_keys()
        {
            let event = if self.tickets_are_syncing(&keys) {
                BacklogSectionEvent::TicketsSyncing { keys }
            } else {
                event_for_selection(section_id, keys, source_order)
            };
            let _ = self.events.send(event);
        }
        ctx.stop_propagation();
        true
    }

    fn open_highlighted_ticket(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let custom = matches!(event, TuiEvent::Key(key) if self.open_command_key.matches(*key));
        if !custom
            && !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return false;
        }
        let Some(row) = self
            .control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
        else {
            return false;
        };
        let BacklogRowContent::WorkItem(item) = &row.content else {
            return false;
        };
        let key = item.item.key.clone();
        let _ = self.events.send(if custom {
            BacklogSectionEvent::OpenCommand { key }
        } else {
            BacklogSectionEvent::OpenTicket { key }
        });
        ctx.stop_propagation();
        true
    }

    fn open_highlighted_description(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if self.control.data_view().is_searching()
            || self.control.is_reordering()
            || !matches!(event, TuiEvent::Key(key) if self.backlog_keys.view_description.matches(*key))
        {
            return false;
        }
        let Some(row) = self
            .control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
        else {
            return false;
        };
        let BacklogRowContent::WorkItem(item) = &row.content else {
            return false;
        };
        let _ = self.events.send(BacklogSectionEvent::OpenDescription {
            key: item.item.key.clone(),
        });
        ctx.stop_propagation();
        true
    }

    fn move_highlighted_ticket_to_edge(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if self.control.data_view().is_searching() || self.control.is_reordering() {
            return false;
        }
        let TuiEvent::Key(pressed) = event else {
            return false;
        };
        let to_top = if self.backlog_keys.move_to_top.matches(*pressed) {
            true
        } else if self.backlog_keys.move_to_bottom.matches(*pressed) {
            false
        } else {
            return false;
        };
        let Some(row) = self
            .control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
        else {
            return false;
        };
        let BacklogRowContent::WorkItem(item) = &row.content else {
            return false;
        };
        if !item.rankable_root || matches!(item.item.kind, WorkItemKind::Subtask) {
            return false;
        }
        let key = item.item.key.clone();
        if self.move_locked.get() {
            let _ = self.events.send(BacklogSectionEvent::MoveLocked);
        } else if self.tickets_are_syncing(std::slice::from_ref(&key)) {
            let _ = self
                .events
                .send(BacklogSectionEvent::TicketsSyncing { keys: vec![key] });
        } else {
            let _ = self.events.send(BacklogSectionEvent::MoveToEdge {
                source_order: self.issue_keys_in_section(&item.section),
                section_id: item.section.clone(),
                key,
                to_top,
            });
        }
        ctx.stop_propagation();
        true
    }

    fn handle_yank(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if self.yank_menu.is_open() {
            self.yank_menu.event(event, ctx);
            self.finish_yank(ctx);
            if !self.yank_menu.is_open() {
                ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
            }
            ctx.stop_propagation();
            return true;
        }
        let open_requested = matches!(event, TuiEvent::Yank)
            || matches!(event, TuiEvent::Key(key) if key.code == Key::Char('y') && key.modifiers == KeyModifiers::NONE)
            || matches!(event, TuiEvent::Hotkey(HotkeyEvent::Pending(sequence)) if sequence == "y");
        if open_requested {
            if let Some(target) = self.selected_yank_target() {
                self.yank_menu.open(target, ctx);
                ctx.stop_propagation();
                return true;
            }
            if matches!(event, TuiEvent::Yank) {
                ctx.stop_propagation();
                return true;
            }
            return false;
        }
        let TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) = event else {
            return false;
        };
        let Some(id) = self.control.data_view().highlighted_id() else {
            return false;
        };
        let event = match sequence.as_str() {
            "yg" => self.sprint_for_section(&id).and_then(|sprint| {
                sprint
                    .goal
                    .clone()
                    .filter(|goal| !goal.trim().is_empty())
                    .map(|goal| BacklogSectionEvent::YankSprintGoal { goal })
            }),
            "yv" => {
                self.sprint_for_section(&id)
                    .map(|sprint| BacklogSectionEvent::YankSprintReport {
                        sprint_id: sprint.id,
                    })
            }
            _ => None,
        };
        let Some(event) = event else {
            return false;
        };
        let _ = self.events.send(event);
        ctx.stop_propagation();
        true
    }

    fn selected_yank_target(&self) -> Option<TicketYankTarget> {
        self.control
            .data_view()
            .highlighted_id()
            .and_then(|id| self.control.items().iter().find(|row| row.id == id))
            .and_then(|row| match &row.content {
                BacklogRowContent::WorkItem(item) => Some(TicketYankTarget {
                    key: item.item.key.clone(),
                    title: item.item.title.clone(),
                    description: item.description.clone(),
                }),
                BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. } => None,
            })
    }

    fn finish_yank(&mut self, ctx: &mut EventCtx<()>) {
        let Some((action, target)) = self.yank_menu.take_selection() else {
            return;
        };
        if action == TicketYankAction::Url {
            let _ = self
                .events
                .send(BacklogSectionEvent::YankTicketUrl { key: target.key });
        } else if let Some(value) = action.text(&target) {
            ctx.copy_to_clipboard(value);
        }
    }

    fn sprint_for_section(&self, id: &str) -> Option<&Sprint> {
        let sprint_id = id.strip_prefix("section:sprint-")?.parse::<u64>().ok()?;
        self.snapshot
            .sprints
            .iter()
            .find(|sprint| sprint.id == sprint_id)
    }

    fn handle_ticket_number_jump(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if self.control.data_view().is_searching() {
            return false;
        }
        if self.number_jump.borrow().cancels(*key) {
            self.number_jump.borrow_mut().clear();
            self.advance_wrap_geometry_epoch();
            ctx.request_redraw();
            ctx.stop_propagation();
            return true;
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
            self.advance_wrap_geometry_epoch();
            if let Some(row_id) = row_id {
                self.jump_to_ticket(&row_id);
            }
            ctx.request_redraw();
            ctx.stop_propagation();
            return true;
        }
        if !self.number_jump.borrow_mut().push(*key) {
            return false;
        }
        self.advance_wrap_geometry_epoch();
        let number = self
            .number_jump
            .borrow()
            .query()
            .unwrap_or_default()
            .to_owned();
        self.control.data_view_mut().expand_all();
        let matching_count = self
            .control
            .items()
            .iter()
            .filter(|row| matches!(&row.content, BacklogRowContent::WorkItem(item) if crate::components::ticket_number_jump::ticket_number_matches(&item.item.key, &number)))
            .count();
        if matching_count == 1
            && let Some(row_id) = self.exact_ticket_row_id(&number)
        {
            self.number_jump.borrow_mut().clear();
            self.advance_wrap_geometry_epoch();
            self.jump_to_ticket(&row_id);
        }
        ctx.request_redraw();
        ctx.request_tick();
        ctx.stop_propagation();
        true
    }

    fn exact_ticket_row_id(&self, number: &str) -> Option<String> {
        self.control
            .items()
            .iter()
            .find_map(|row| match &row.content {
                BacklogRowContent::WorkItem(item)
                    if exact_ticket_number_matches(&item.item.key, number) =>
                {
                    Some(row.id.clone())
                }
                _ => None,
            })
    }

    fn jump_to_ticket(&mut self, row_id: &str) {
        let view = self.control.data_view_mut();
        view.highlight_id(&row_id.to_owned());
        view.reveal_highlighted_centered();
    }

    fn advance_wrap_geometry_epoch(&mut self) {
        self.wrap_geometry_epoch = self.wrap_geometry_epoch.wrapping_add(1);
        self.control
            .data_view_mut()
            .set_wrap_geometry_epoch(self.wrap_geometry_epoch);
    }

    fn drain_events(&mut self, source_parents: HashMap<String, String>) {
        for event in self.control.take_events() {
            let (ids, parent) = match event {
                ListControlEvent::TreeMoved {
                    row_id, parent_id, ..
                } => (vec![row_id], parent_id),
                ListControlEvent::TreeBlockMoved {
                    row_ids, parent_id, ..
                } => (row_ids, parent_id),
                _ => continue,
            };
            let Some(section_id) = ids
                .first()
                .and_then(|id| source_parents.get(id))
                .and_then(|id| id.strip_prefix("section:"))
                .map(str::to_owned)
            else {
                continue;
            };
            let source_parent = section_row_id(&section_id);
            let valid = !ids.is_empty()
                && ids
                    .iter()
                    .all(|id| source_parents.get(id) == Some(&source_parent))
                && parent.as_deref() == Some(source_parent.as_str())
                && ids.iter().all(|id| !self.is_section(id));
            if !valid {
                let _ = self.events.send(BacklogSectionEvent::Rejected {
                    section_id,
                    message: "Backlog tickets must remain in their section".into(),
                });
                continue;
            }
            let moved_keys = self.issue_keys_for_ids(&ids, &source_parents);
            if moved_keys.len() > crate::store::work_items::MAX_RANK_ISSUES {
                let _ = self.events.send(BacklogSectionEvent::Rejected {
                    section_id,
                    message: format!(
                        "Jira can rank at most {} issues at once",
                        crate::store::work_items::MAX_RANK_ISSUES
                    ),
                });
            } else {
                let final_order = self.issue_keys_in_section(&section_id);
                let _ = self.events.send(BacklogSectionEvent::Moved {
                    section_id,
                    moved_keys,
                    final_order,
                });
            }
        }
    }

    fn issue_keys_for_ids(
        &self,
        ids: &[String],
        source_parents: &HashMap<String, String>,
    ) -> Vec<String> {
        self.control
            .items()
            .iter()
            .filter_map(|row| {
                (ids.contains(&row.id) && source_parents.contains_key(&row.id)).then(
                    || match &row.content {
                        BacklogRowContent::WorkItem(item) => Some(item.item.key.clone()),
                        BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. } => None,
                    },
                )?
            })
            .collect()
    }

    fn handle_event(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
        dispatch: impl FnOnce(&mut ListControl<BacklogRow, String>, &mut EventCtx<()>) -> EventOutcome,
    ) -> EventOutcome {
        if self.handle_yank(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.handle_ticket_number_jump(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_highlighted_ticket(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_highlighted_description(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_quick_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_status_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_story_points_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_assign_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_epic_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_release_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.toggle_current_user(event, ctx) {
            return EventOutcome::Handled;
        }
        let selected_syncing = self
            .selected_issue_keys()
            .is_some_and(|(_, keys, _)| self.tickets_are_syncing(&keys));
        if ((self.move_locked.get() || self.group_by_selection.is_some() || selected_syncing)
            && self.blocks_locked_gesture(event))
            || ((self.highlighted_section() || self.highlighted_subtask())
                && self.blocks_section_gesture(event))
        {
            if selected_syncing && let Some((_, keys, _)) = self.selected_issue_keys() {
                let _ = self
                    .events
                    .send(BacklogSectionEvent::TicketsSyncing { keys });
            }
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let source_parents = self
            .control
            .items()
            .iter()
            .map(|row| (row.id.clone(), row.parent_id.clone().unwrap_or_default()))
            .collect();
        let search = self.control.data_view().transform_state().search.clone();
        let outcome = dispatch(&mut self.control, ctx);
        if self.control.data_view().transform_state().search != search {
            if self.group_by_selection == Some(BacklogGroupBy::Release) {
                let snapshot = self.snapshot.clone();
                self.set_snapshot(&snapshot);
            } else {
                self.sync_search_results();
            }
        }
        let show_runway_bands = self.show_runway_bands();
        if self.runway_markers_visible.replace(show_runway_bands) != show_runway_bands {
            ctx.request_redraw();
        }
        self.drain_events(source_parents);
        outcome
    }

    fn show_runway_bands(&self) -> bool {
        self.group_by_selection.is_none()
            && !self.filters.is_active()
            && self
                .control
                .data_view()
                .transform_state()
                .search
                .trim()
                .is_empty()
    }

    fn sync_search_results(&mut self) {
        let search = self
            .control
            .data_view()
            .transform_state()
            .search
            .trim()
            .to_owned();
        if search.is_empty() {
            self.control
                .data_view_mut()
                .set_transform_mode(DataViewTransformMode::Local);
            self.control.data_view_mut().clear_visible_row_ids();
            return;
        }

        self.control
            .data_view_mut()
            .set_transform_mode(DataViewTransformMode::External);

        let rows = self.control.items();
        let matching_ticket_row_id = rows.iter().find_map(|row| {
            matches!(row.content, BacklogRowContent::WorkItem(_))
                .then(|| {
                    tuicore::search_match(&search, &backlog_search_text(row), SearchMode::Contains)
                        .is_some()
                })
                .filter(|matches| *matches)
                .map(|_| row.id.clone())
        });
        let highlighted_matches = self.control.data_view().highlighted_id().is_some_and(|id| {
            rows.iter().any(|row| {
                row.id == id
                    && tuicore::search_match(
                        &search,
                        &backlog_search_text(row),
                        SearchMode::Contains,
                    )
                    .is_some()
            })
        });
        let visible_ids = search_visible_row_ids(rows, &search);
        let visible_row_ids = rows
            .iter()
            .filter(|row| visible_ids.contains(row.id.as_str()))
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        self.control
            .data_view_mut()
            .set_visible_row_ids(visible_row_ids);
        if !highlighted_matches && let Some(row_id) = matching_ticket_row_id {
            self.control.data_view_mut().highlight_id(&row_id);
        }
    }

    fn drain_web_menu(&mut self, was_open: bool) {
        let activated = self.web.take_activated();
        for item in activated.iter().copied() {
            let event = match item {
                WebMenuItem::Board => BacklogSectionEvent::OpenBoard,
                WebMenuItem::Timeline => BacklogSectionEvent::OpenTimeline,
                WebMenuItem::Releases => BacklogSectionEvent::OpenReleases,
                WebMenuItem::Reports => BacklogSectionEvent::OpenReports,
            };
            let _ = self.events.send(event);
        }
        if was_open && activated.is_empty() && !self.web.is_open() {
            let _ = self.events.send(BacklogSectionEvent::WebMenuClosed);
        }
    }

    fn drain_group_by_menu(&mut self, ctx: &mut EventCtx<()>) {
        let Some(selection) = self.group_by.take_activated().into_iter().last() else {
            return;
        };
        if self.set_group_by_selection(selection.grouping()) {
            ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
        }
    }

    fn set_group_by_selection(&mut self, grouping: Option<BacklogGroupBy>) -> bool {
        if self.group_by_selection == grouping {
            return false;
        }
        self.group_by_selection = grouping;
        self.runway_markers_visible.set(self.show_runway_bands());
        self.group_by.set_label(grouping_icon(grouping));
        let snapshot = self.snapshot.clone();
        self.set_snapshot(&snapshot);
        self.control
            .data_view_mut()
            .restore_tree_expansion(HashSet::new());
        true
    }

    fn refocus_data_view_after_unfocus(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        if !tuicore::keybindings().focus().unfocus_matches(*key) {
            return false;
        }
        ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
        ctx.stop_propagation();
        true
    }
}

impl TuiNode for BacklogTree {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: ratatui::layout::Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let compact_toolbar = area.width < 160;
        let header_height = if area.is_empty() { 0 } else { 1 };
        let button_width = |button: &Button<()>| {
            button
                .measure(LayoutProposal::at_most(area.width, 1))
                .preferred
                .width
        };
        let dropdown_width = |proposal_width, dropdown: &Dropdown<JiraOption, String>| {
            <Dropdown<JiraOption, String> as TuiNode<()>>::measure(
                dropdown,
                LayoutProposal::at_most(proposal_width, 1),
            )
            .preferred
            .width
            .min(proposal_width)
        };
        let users_width = |proposal_width, dropdown: &Dropdown<String, String>| {
            <Dropdown<String, String> as TuiNode<()>>::measure(
                dropdown,
                LayoutProposal::at_most(proposal_width, 1),
            )
            .preferred
            .width
            .min(proposal_width)
        };
        let saved_filter_width =
            |proposal_width, dropdown: &Dropdown<SavedFilterChoice, String>| {
                <Dropdown<SavedFilterChoice, String> as TuiNode<()>>::measure(
                    dropdown,
                    LayoutProposal::at_most(proposal_width, 1),
                )
                .preferred
                .width
                .min(proposal_width)
            };
        let menu_width = |proposal_width, menu: &MenuButton<GroupByMenuItem>| {
            menu.measure(LayoutProposal::at_most(proposal_width, 1))
                .preferred
                .width
                .min(proposal_width)
        };

        if compact_toolbar {
            let mut remaining_width = area.width;
            let refresh_width = button_width(&self.refresh).min(remaining_width);
            remaining_width = remaining_width.saturating_sub(refresh_width + 1);
            let velocity_width = button_width(&self.velocity).min(remaining_width);
            remaining_width = remaining_width.saturating_sub(velocity_width + 1);
            let group_by_width = menu_width(remaining_width, &self.group_by);
            remaining_width = remaining_width.saturating_sub(group_by_width + 1);
            let web_width = self
                .web
                .measure(LayoutProposal::at_most(remaining_width, 1))
                .preferred
                .width
                .min(remaining_width);
            self.web_area = ratatui::layout::Rect::new(area.x, area.y, web_width, 1);
            self.group_by_area = ratatui::layout::Rect::new(
                self.web_area
                    .x
                    .saturating_add(web_width)
                    .saturating_add(u16::from(web_width > 0)),
                area.y,
                group_by_width,
                1,
            );
            self.refresh_area = ratatui::layout::Rect::new(
                area.x
                    .saturating_add(area.width.saturating_sub(refresh_width)),
                area.y,
                refresh_width,
                1,
            );
            self.velocity_area = ratatui::layout::Rect::new(
                self.refresh_area
                    .x
                    .saturating_sub(velocity_width)
                    .saturating_sub(u16::from(velocity_width > 0)),
                area.y,
                velocity_width,
                1,
            );

            let row_y = area.y.saturating_add(header_height);
            let releases_width = users_width(area.width, &self.releases).min(6);
            let labels_width = users_width(
                area.width
                    .saturating_sub(releases_width)
                    .saturating_sub(u16::from(releases_width > 0)),
                &self.labels,
            );
            let labels_width = labels_width.min(6);
            let epics_width = users_width(
                area.width
                    .saturating_sub(releases_width)
                    .saturating_sub(labels_width)
                    .saturating_sub(u16::from(releases_width > 0))
                    .saturating_sub(u16::from(labels_width > 0)),
                &self.epics,
            );
            let epics_width = epics_width.min(6);
            let statuses_width = users_width(
                area.width
                    .saturating_sub(releases_width)
                    .saturating_sub(labels_width)
                    .saturating_sub(epics_width)
                    .saturating_sub(u16::from(releases_width > 0))
                    .saturating_sub(u16::from(labels_width > 0))
                    .saturating_sub(u16::from(epics_width > 0)),
                &self.statuses,
            );
            let statuses_width = statuses_width.min(6);
            let issue_types_width = dropdown_width(
                area.width
                    .saturating_sub(releases_width)
                    .saturating_sub(labels_width)
                    .saturating_sub(epics_width)
                    .saturating_sub(statuses_width)
                    .saturating_sub(u16::from(releases_width > 0))
                    .saturating_sub(u16::from(labels_width > 0))
                    .saturating_sub(u16::from(epics_width > 0))
                    .saturating_sub(u16::from(statuses_width > 0)),
                &self.issue_types,
            );
            let issue_types_width = issue_types_width.min(6);
            let users_width = users_width(
                area.width
                    .saturating_sub(releases_width)
                    .saturating_sub(labels_width)
                    .saturating_sub(epics_width)
                    .saturating_sub(statuses_width)
                    .saturating_sub(issue_types_width)
                    .saturating_sub(u16::from(releases_width > 0))
                    .saturating_sub(u16::from(labels_width > 0))
                    .saturating_sub(u16::from(epics_width > 0))
                    .saturating_sub(u16::from(statuses_width > 0))
                    .saturating_sub(u16::from(issue_types_width > 0)),
                &self.users,
            );
            let users_width = users_width.min(6);
            let occupied_filter_width = issue_types_width
                .saturating_add(users_width)
                .saturating_add(statuses_width)
                .saturating_add(epics_width)
                .saturating_add(labels_width)
                .saturating_add(releases_width)
                .saturating_add(u16::from(issue_types_width > 0))
                .saturating_add(u16::from(users_width > 0))
                .saturating_add(u16::from(statuses_width > 0))
                .saturating_add(u16::from(epics_width > 0))
                .saturating_add(u16::from(labels_width > 0))
                .saturating_add(u16::from(releases_width > 0));
            let saved_filter_width = saved_filter_width(
                area.width.saturating_sub(occupied_filter_width),
                &self.saved_filter,
            )
            .min(18);
            let estimated_width = <Toggle<()> as TuiNode<()>>::measure(
                &self.estimated,
                LayoutProposal::at_most(
                    area.width
                        .saturating_sub(occupied_filter_width)
                        .saturating_sub(saved_filter_width)
                        .saturating_sub(u16::from(saved_filter_width > 0)),
                    1,
                ),
            )
            .preferred
            .width
            .min(
                area.width
                    .saturating_sub(occupied_filter_width)
                    .saturating_sub(saved_filter_width)
                    .saturating_sub(u16::from(saved_filter_width > 0)),
            );
            self.estimated_area = ratatui::layout::Rect::new(
                area.x.saturating_add(
                    area.width
                        .saturating_sub(estimated_width)
                        .saturating_sub(saved_filter_width)
                        .saturating_sub(users_width)
                        .saturating_sub(issue_types_width)
                        .saturating_sub(statuses_width)
                        .saturating_sub(epics_width)
                        .saturating_sub(labels_width)
                        .saturating_sub(releases_width)
                        .saturating_sub(u16::from(users_width > 0))
                        .saturating_sub(u16::from(saved_filter_width > 0))
                        .saturating_sub(u16::from(issue_types_width > 0))
                        .saturating_sub(u16::from(statuses_width > 0))
                        .saturating_sub(u16::from(epics_width > 0))
                        .saturating_sub(u16::from(labels_width > 0))
                        .saturating_sub(u16::from(releases_width > 0)),
                ),
                row_y,
                estimated_width,
                1,
            );
            self.saved_filter_area = ratatui::layout::Rect::new(
                self.estimated_area
                    .x
                    .saturating_add(estimated_width)
                    .saturating_add(u16::from(estimated_width > 0)),
                row_y,
                saved_filter_width,
                1,
            );
            self.users_area = ratatui::layout::Rect::new(
                self.saved_filter_area
                    .x
                    .saturating_add(saved_filter_width)
                    .saturating_add(u16::from(saved_filter_width > 0)),
                row_y,
                users_width,
                1,
            );
            self.issue_types_area = ratatui::layout::Rect::new(
                self.users_area
                    .x
                    .saturating_add(users_width)
                    .saturating_add(u16::from(users_width > 0)),
                row_y,
                issue_types_width,
                1,
            );
            self.statuses_area = ratatui::layout::Rect::new(
                self.issue_types_area
                    .x
                    .saturating_add(issue_types_width)
                    .saturating_add(u16::from(issue_types_width > 0)),
                row_y,
                statuses_width,
                1,
            );
            self.epics_area = ratatui::layout::Rect::new(
                self.statuses_area
                    .x
                    .saturating_add(statuses_width)
                    .saturating_add(u16::from(statuses_width > 0)),
                row_y,
                epics_width,
                1,
            );
            self.labels_area = ratatui::layout::Rect::new(
                self.epics_area
                    .x
                    .saturating_add(epics_width)
                    .saturating_add(u16::from(epics_width > 0)),
                row_y,
                labels_width,
                1,
            );
            self.releases_area = ratatui::layout::Rect::new(
                self.labels_area
                    .x
                    .saturating_add(labels_width)
                    .saturating_add(u16::from(labels_width > 0)),
                row_y,
                releases_width,
                1,
            );
        } else {
            let mut remaining_width = area.width;
            let refresh_width = button_width(&self.refresh).min(remaining_width);
            remaining_width = remaining_width.saturating_sub(refresh_width + 1);
            let velocity_width = button_width(&self.velocity).min(remaining_width);
            remaining_width = remaining_width.saturating_sub(velocity_width + 1);
            let releases_width = users_width(remaining_width, &self.releases);
            remaining_width = remaining_width.saturating_sub(releases_width + 1);
            let labels_width = users_width(remaining_width, &self.labels);
            remaining_width = remaining_width.saturating_sub(labels_width + 1);
            let epics_width = users_width(remaining_width, &self.epics);
            remaining_width = remaining_width.saturating_sub(epics_width + 1);
            let statuses_width = users_width(remaining_width, &self.statuses);
            remaining_width = remaining_width.saturating_sub(statuses_width + 1);
            let issue_types_width = dropdown_width(remaining_width, &self.issue_types);
            remaining_width = remaining_width.saturating_sub(issue_types_width + 1);
            let users_width = users_width(remaining_width, &self.users);
            remaining_width = remaining_width.saturating_sub(users_width + 1);
            let saved_filter_width =
                saved_filter_width(remaining_width, &self.saved_filter).min(24);
            remaining_width = remaining_width.saturating_sub(saved_filter_width + 1);
            let estimated_width = <Toggle<()> as TuiNode<()>>::measure(
                &self.estimated,
                LayoutProposal::at_most(remaining_width, 1),
            )
            .preferred
            .width
            .min(remaining_width);
            remaining_width = remaining_width.saturating_sub(estimated_width + 1);
            let group_by_width = menu_width(remaining_width, &self.group_by);
            remaining_width = remaining_width.saturating_sub(group_by_width + 1);
            let web_width = self
                .web
                .measure(LayoutProposal::at_most(remaining_width, 1))
                .preferred
                .width
                .min(remaining_width);
            self.web_area = ratatui::layout::Rect::new(area.x, area.y, web_width, 1);
            self.refresh_area = ratatui::layout::Rect::new(
                area.x
                    .saturating_add(area.width.saturating_sub(refresh_width)),
                area.y,
                refresh_width,
                1,
            );
            self.velocity_area = ratatui::layout::Rect::new(
                self.refresh_area
                    .x
                    .saturating_sub(velocity_width)
                    .saturating_sub(u16::from(velocity_width > 0)),
                area.y,
                velocity_width,
                1,
            );
            self.statuses_area = ratatui::layout::Rect::new(
                self.velocity_area
                    .x
                    .saturating_sub(releases_width)
                    .saturating_sub(u16::from(releases_width > 0))
                    .saturating_sub(labels_width)
                    .saturating_sub(u16::from(labels_width > 0))
                    .saturating_sub(epics_width)
                    .saturating_sub(u16::from(epics_width > 0))
                    .saturating_sub(statuses_width)
                    .saturating_sub(u16::from(statuses_width > 0)),
                area.y,
                statuses_width,
                1,
            );
            self.epics_area = ratatui::layout::Rect::new(
                self.statuses_area
                    .x
                    .saturating_add(statuses_width)
                    .saturating_add(u16::from(statuses_width > 0)),
                area.y,
                epics_width,
                1,
            );
            self.labels_area = ratatui::layout::Rect::new(
                self.epics_area
                    .x
                    .saturating_add(epics_width)
                    .saturating_add(u16::from(epics_width > 0)),
                area.y,
                labels_width,
                1,
            );
            self.releases_area = ratatui::layout::Rect::new(
                self.labels_area
                    .x
                    .saturating_add(labels_width)
                    .saturating_add(u16::from(labels_width > 0)),
                area.y,
                releases_width,
                1,
            );
            self.issue_types_area = ratatui::layout::Rect::new(
                self.statuses_area
                    .x
                    .saturating_sub(issue_types_width)
                    .saturating_sub(u16::from(issue_types_width > 0)),
                area.y,
                issue_types_width,
                1,
            );
            self.users_area = ratatui::layout::Rect::new(
                self.issue_types_area
                    .x
                    .saturating_sub(users_width)
                    .saturating_sub(u16::from(users_width > 0)),
                area.y,
                users_width,
                1,
            );
            self.saved_filter_area = ratatui::layout::Rect::new(
                self.users_area
                    .x
                    .saturating_sub(saved_filter_width)
                    .saturating_sub(u16::from(saved_filter_width > 0)),
                area.y,
                saved_filter_width,
                1,
            );
            self.estimated_area = ratatui::layout::Rect::new(
                self.saved_filter_area
                    .x
                    .saturating_sub(estimated_width)
                    .saturating_sub(u16::from(estimated_width > 0)),
                area.y,
                estimated_width,
                1,
            );
            self.group_by_area = ratatui::layout::Rect::new(
                self.web_area
                    .x
                    .saturating_add(web_width)
                    .saturating_add(u16::from(web_width > 0)),
                area.y,
                group_by_width,
                1,
            );
        }
        self.control_area = ratatui::layout::Rect::new(
            area.x,
            area.y.saturating_add(header_height),
            area.width,
            area.height.saturating_sub(header_height),
        );
        ctx.push_slot(ChildKey::new("web"), self.web_area, |ctx| {
            self.web.layout(self.web_area, ctx)
        });
        ctx.push_slot(ChildKey::new("group-by"), self.group_by_area, |ctx| {
            self.group_by.layout(self.group_by_area, ctx)
        });
        if compact_toolbar {
            ctx.push_slot(ChildKey::new("velocity"), self.velocity_area, |ctx| {
                self.velocity.layout(self.velocity_area, ctx)
            });
            ctx.push_slot(ChildKey::new("refresh"), self.refresh_area, |ctx| {
                self.refresh.layout(self.refresh_area, ctx)
            });
        }
        ctx.push_slot(ChildKey::new("estimated"), self.estimated_area, |ctx| {
            <Toggle<()> as TuiNode<()>>::layout(&mut self.estimated, self.estimated_area, ctx)
        });
        ctx.push_slot(
            ChildKey::new("saved-filter"),
            self.saved_filter_area,
            |ctx| {
                <Dropdown<SavedFilterChoice, String> as TuiNode<()>>::layout(
                    &mut self.saved_filter,
                    self.saved_filter_area,
                    ctx,
                )
            },
        );
        ctx.push_slot(ChildKey::new("users"), self.users_area, |ctx| {
            <Dropdown<String, String> as TuiNode<()>>::layout(&mut self.users, self.users_area, ctx)
        });
        ctx.push_slot(ChildKey::new("issue-types"), self.issue_types_area, |ctx| {
            <Dropdown<JiraOption, String> as TuiNode<()>>::layout(
                &mut self.issue_types,
                self.issue_types_area,
                ctx,
            )
        });
        ctx.push_slot(ChildKey::new("statuses"), self.statuses_area, |ctx| {
            <Dropdown<String, String> as TuiNode<()>>::layout(
                &mut self.statuses,
                self.statuses_area,
                ctx,
            )
        });
        ctx.push_slot(ChildKey::new("epics"), self.epics_area, |ctx| {
            <Dropdown<String, String> as TuiNode<()>>::layout(&mut self.epics, self.epics_area, ctx)
        });
        ctx.push_slot(ChildKey::new("labels"), self.labels_area, |ctx| {
            <Dropdown<String, String> as TuiNode<()>>::layout(
                &mut self.labels,
                self.labels_area,
                ctx,
            )
        });
        ctx.push_slot(ChildKey::new("releases"), self.releases_area, |ctx| {
            <Dropdown<String, String> as TuiNode<()>>::layout(
                &mut self.releases,
                self.releases_area,
                ctx,
            )
        });
        if !compact_toolbar {
            ctx.push_slot(ChildKey::new("velocity"), self.velocity_area, |ctx| {
                self.velocity.layout(self.velocity_area, ctx)
            });
            ctx.push_slot(ChildKey::new("refresh"), self.refresh_area, |ctx| {
                self.refresh.layout(self.refresh_area, ctx)
            });
        }
        let (result, _) = ctx.with_focus_fallback_hotkey_sequences_status(
            FocusId::new("data-view"),
            self.control_area,
            [
                "yu".to_owned(),
                "yt".to_owned(),
                "yd".to_owned(),
                "yk".to_owned(),
                "yf".to_owned(),
                "ys".to_owned(),
                "yg".to_owned(),
                "yv".to_owned(),
                self.backlog_keys.view_description.sequence().to_owned(),
            ],
            |ctx| self.control.layout(self.control_area, ctx),
        );
        if self.yank_menu.is_open() {
            ctx.push_slot(ChildKey::new("yank-menu"), area, |ctx| {
                self.yank_menu.layout(area, ctx)
            });
        }
        result
    }
    fn render<'a>(
        &'a self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        ctx: &mut RenderCtx<'a>,
    ) {
        self.control.render(frame, self.control_area, ctx);
        self.web.render(frame, self.web_area, ctx);
        self.group_by.render(frame, self.group_by_area, ctx);
        self.estimated.render(frame, self.estimated_area);
        self.saved_filter.render(frame, self.saved_filter_area, ctx);
        self.users.render(frame, self.users_area, ctx);
        self.issue_types.render(frame, self.issue_types_area, ctx);
        self.statuses.render(frame, self.statuses_area, ctx);
        self.epics.render(frame, self.epics_area, ctx);
        self.labels.render(frame, self.labels_area, ctx);
        self.releases.render(frame, self.releases_area, ctx);
        self.velocity.render(frame, self.velocity_area);
        self.refresh.render(frame, self.refresh_area);
        if self.yank_menu.is_open() {
            self.yank_menu.render(frame, area, ctx);
        }
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.yank_menu.is_open() && self.handle_yank(event, ctx) {
            return EventOutcome::Handled;
        }
        let web_was_open = self.web.is_open();
        if matches!(event, TuiEvent::Mouse(_))
            && (self.refresh.event(event, ctx) == EventOutcome::Handled
                || self.velocity.event(event, ctx) == EventOutcome::Handled
                || self.estimated.event(event, ctx) == EventOutcome::Handled
                || self.saved_filter.event(event, ctx) == EventOutcome::Handled
                || self.issue_types.event(event, ctx) == EventOutcome::Handled
                || self.users.event(event, ctx) == EventOutcome::Handled
                || self.statuses.event(event, ctx) == EventOutcome::Handled
                || self.epics.event(event, ctx) == EventOutcome::Handled
                || self.labels.event(event, ctx) == EventOutcome::Handled
                || self.releases.event(event, ctx) == EventOutcome::Handled
                || self.group_by.event(event, ctx) == EventOutcome::Handled
                || self.web.event(event, ctx) == EventOutcome::Handled)
        {
            self.drain_group_by_menu(ctx);
            self.drain_web_menu(web_was_open);
            return EventOutcome::Handled;
        }
        let outcome = self.handle_event(event, ctx, |control, ctx| control.event(event, ctx));
        self.drain_group_by_menu(ctx);
        self.drain_web_menu(web_was_open);
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.yank_menu.is_open() && self.handle_yank(event, ctx) {
            return EventOutcome::Handled;
        }
        if let Some(refresh_path) = route.path.without_first_if(&ChildKey::new("refresh")) {
            let outcome = self
                .refresh
                .dispatch_event(&EventRoute::new(refresh_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(velocity_path) = route.path.without_first_if(&ChildKey::new("velocity")) {
            let outcome = self
                .velocity
                .dispatch_event(&EventRoute::new(velocity_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(estimated_path) = route.path.without_first_if(&ChildKey::new("estimated")) {
            let outcome =
                self.estimated
                    .dispatch_event(&EventRoute::new(estimated_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(saved_filter_path) = route.path.without_first_if(&ChildKey::new("saved-filter"))
        {
            let outcome =
                self.saved_filter
                    .dispatch_event(&EventRoute::new(saved_filter_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(issue_types_path) = route.path.without_first_if(&ChildKey::new("issue-types")) {
            let outcome =
                self.issue_types
                    .dispatch_event(&EventRoute::new(issue_types_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(users_path) = route.path.without_first_if(&ChildKey::new("users")) {
            let outcome = self
                .users
                .dispatch_event(&EventRoute::new(users_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(statuses_path) = route.path.without_first_if(&ChildKey::new("statuses")) {
            let outcome = self
                .statuses
                .dispatch_event(&EventRoute::new(statuses_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(epics_path) = route.path.without_first_if(&ChildKey::new("epics")) {
            let outcome = self
                .epics
                .dispatch_event(&EventRoute::new(epics_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(labels_path) = route.path.without_first_if(&ChildKey::new("labels")) {
            let outcome = self
                .labels
                .dispatch_event(&EventRoute::new(labels_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(releases_path) = route.path.without_first_if(&ChildKey::new("releases")) {
            let outcome = self
                .releases
                .dispatch_event(&EventRoute::new(releases_path), event, ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(group_by_path) = route.path.without_first_if(&ChildKey::new("group-by")) {
            let outcome = self
                .group_by
                .dispatch_event(&EventRoute::new(group_by_path), event, ctx);
            self.drain_group_by_menu(ctx);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if let Some(web_path) = route.path.without_first_if(&ChildKey::new("web")) {
            let web_was_open = self.web.is_open();
            let outcome = self
                .web
                .dispatch_event(&EventRoute::new(web_path), event, ctx);
            self.drain_web_menu(web_was_open);
            return if self.refocus_data_view_after_unfocus(event, ctx) {
                EventOutcome::Handled
            } else {
                outcome
            };
        }
        if route
            .path
            .without_first_if(&ChildKey::new("data"))
            .is_some()
            && self.move_highlighted_ticket_to_edge(event, ctx)
        {
            return EventOutcome::Handled;
        }
        let keeps_data_focus = route
            .path
            .without_first_if(&ChildKey::new("data"))
            .is_some()
            && matches!(event, TuiEvent::Key(key) if tuicore::keybindings().focus().unfocus_matches(*key));
        let outcome = self.handle_event(event, ctx, |control, ctx| {
            control.dispatch_event(route, event, ctx)
        });
        if keeps_data_focus && outcome == EventOutcome::Ignored {
            ctx.stop_propagation();
            EventOutcome::Handled
        } else {
            outcome
        }
    }
    fn tick(&mut self, dt: Duration, settings: tuicore::AnimationSettings) -> TickResult {
        let (number_jump, number_jump_changed) = {
            let mut jump = self.number_jump.borrow_mut();
            if jump.advance(dt) {
                (TickResult::CHANGED, true)
            } else {
                (
                    jump.remaining()
                        .map_or(TickResult::IDLE, TickResult::scheduled_after),
                    false,
                )
            }
        };
        if number_jump_changed {
            self.advance_wrap_geometry_epoch();
        }
        self.control
            .tick(dt, settings)
            .merge(<Button<()> as TuiNode<()>>::tick(
                &mut self.refresh,
                dt,
                settings,
            ))
            .merge(<Button<()> as TuiNode<()>>::tick(
                &mut self.velocity,
                dt,
                settings,
            ))
            .merge(<Toggle<()> as TuiNode<()>>::tick(
                &mut self.estimated,
                dt,
                settings,
            ))
            .merge(<Dropdown<SavedFilterChoice, String> as TuiNode<()>>::tick(
                &mut self.saved_filter,
                dt,
                settings,
            ))
            .merge(<Dropdown<JiraOption, String> as TuiNode<()>>::tick(
                &mut self.issue_types,
                dt,
                settings,
            ))
            .merge(<Dropdown<String, String> as TuiNode<()>>::tick(
                &mut self.users,
                dt,
                settings,
            ))
            .merge(<Dropdown<String, String> as TuiNode<()>>::tick(
                &mut self.statuses,
                dt,
                settings,
            ))
            .merge(<Dropdown<String, String> as TuiNode<()>>::tick(
                &mut self.epics,
                dt,
                settings,
            ))
            .merge(<Dropdown<String, String> as TuiNode<()>>::tick(
                &mut self.labels,
                dt,
                settings,
            ))
            .merge(<Dropdown<String, String> as TuiNode<()>>::tick(
                &mut self.releases,
                dt,
                settings,
            ))
            .merge(self.group_by.tick(dt, settings))
            .merge(self.web.tick(dt, settings))
            .merge(self.yank_menu.tick(dt, settings))
            .merge(number_jump)
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.yank_menu.is_open() {
            self.yank_menu.focus(target, focused, ctx);
        } else {
            self.control.focus(target, focused, ctx);
        }
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(yank_target) = target.for_child(&ChildKey::new("yank-menu")) {
            self.yank_menu.dispatch_focus(&yank_target, focused, ctx);
            return;
        }
        if let Some(refresh_target) = target.for_child(&ChildKey::new("refresh")) {
            self.refresh.dispatch_focus(&refresh_target, focused, ctx);
            return;
        }
        if let Some(velocity_target) = target.for_child(&ChildKey::new("velocity")) {
            self.velocity.dispatch_focus(&velocity_target, focused, ctx);
            return;
        }
        if let Some(estimated_target) = target.for_child(&ChildKey::new("estimated")) {
            self.estimated
                .dispatch_focus(&estimated_target, focused, ctx);
            return;
        }
        if let Some(saved_filter_target) = target.for_child(&ChildKey::new("saved-filter")) {
            self.saved_filter
                .dispatch_focus(&saved_filter_target, focused, ctx);
            return;
        }
        if let Some(issue_types_target) = target.for_child(&ChildKey::new("issue-types")) {
            self.issue_types
                .dispatch_focus(&issue_types_target, focused, ctx);
            return;
        }
        if let Some(users_target) = target.for_child(&ChildKey::new("users")) {
            self.users.dispatch_focus(&users_target, focused, ctx);
            return;
        }
        if let Some(statuses_target) = target.for_child(&ChildKey::new("statuses")) {
            self.statuses.dispatch_focus(&statuses_target, focused, ctx);
            return;
        }
        if let Some(epics_target) = target.for_child(&ChildKey::new("epics")) {
            self.epics.dispatch_focus(&epics_target, focused, ctx);
            return;
        }
        if let Some(labels_target) = target.for_child(&ChildKey::new("labels")) {
            self.labels.dispatch_focus(&labels_target, focused, ctx);
            return;
        }
        if let Some(releases_target) = target.for_child(&ChildKey::new("releases")) {
            self.releases.dispatch_focus(&releases_target, focused, ctx);
            return;
        }
        if let Some(group_by_target) = target.for_child(&ChildKey::new("group-by")) {
            self.group_by.dispatch_focus(&group_by_target, focused, ctx);
            return;
        }
        if let Some(web_target) = target.for_child(&ChildKey::new("web")) {
            self.web.dispatch_focus(&web_target, focused, ctx);
            return;
        }
        self.control.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.init(ctx);
        self.estimated.init(ctx);
        self.saved_filter.init(ctx);
        self.issue_types.init(ctx);
        self.users.init(ctx);
        self.statuses.init(ctx);
        self.epics.init(ctx);
        self.labels.init(ctx);
        self.releases.init(ctx);
        self.group_by.init(ctx);
        self.yank_menu.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.mount(ctx);
        self.estimated.mount(ctx);
        self.saved_filter.mount(ctx);
        self.issue_types.mount(ctx);
        self.users.mount(ctx);
        self.statuses.mount(ctx);
        self.epics.mount(ctx);
        self.labels.mount(ctx);
        self.releases.mount(ctx);
        self.group_by.mount(ctx);
        self.yank_menu.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
        self.estimated.unmount(ctx);
        self.saved_filter.unmount(ctx);
        self.issue_types.unmount(ctx);
        self.users.unmount(ctx);
        self.statuses.unmount(ctx);
        self.epics.unmount(ctx);
        self.labels.unmount(ctx);
        self.releases.unmount(ctx);
        self.group_by.unmount(ctx);
        self.yank_menu.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
        self.estimated.destroy(ctx);
        self.saved_filter.destroy(ctx);
        self.issue_types.destroy(ctx);
        self.users.destroy(ctx);
        self.statuses.destroy(ctx);
        self.epics.destroy(ctx);
        self.labels.destroy(ctx);
        self.releases.destroy(ctx);
        self.group_by.destroy(ctx);
        self.yank_menu.destroy(ctx);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct BacklogFilters {
    estimated: bool,
    issue_types: Vec<String>,
    users: Vec<String>,
    statuses: Vec<String>,
    epics: Vec<String>,
    labels: Vec<String>,
    releases: Vec<String>,
}

impl Default for BacklogFilters {
    fn default() -> Self {
        Self {
            estimated: true,
            issue_types: Vec::new(),
            users: Vec::new(),
            statuses: Vec::new(),
            epics: Vec::new(),
            labels: Vec::new(),
            releases: Vec::new(),
        }
    }
}

impl BacklogFilters {
    fn is_active(&self) -> bool {
        !self.estimated
            || !self.issue_types.is_empty()
            || !self.users.is_empty()
            || !self.statuses.is_empty()
            || !self.epics.is_empty()
            || !self.labels.is_empty()
            || !self.releases.is_empty()
    }
}

impl From<&BacklogFilterCriteria> for BacklogFilters {
    fn from(criteria: &BacklogFilterCriteria) -> Self {
        Self {
            estimated: criteria.estimated,
            issue_types: criteria.issue_types.clone(),
            users: SavedFilterField::Users.selection(criteria.users.clone()),
            statuses: criteria.statuses.clone(),
            epics: SavedFilterField::Epics.selection(criteria.epics.clone()),
            labels: SavedFilterField::Labels.selection(criteria.labels.clone()),
            releases: SavedFilterField::Releases.selection(criteria.releases.clone()),
        }
    }
}

fn saved_filter_choices(filters: &[SavedBacklogFilter]) -> Vec<SavedFilterChoice> {
    filters
        .iter()
        .map(|filter| SavedFilterChoice {
            id: format!("filter:{}", filter.id),
            label: filter.name.clone(),
        })
        .chain(std::iter::once(SavedFilterChoice {
            id: "manage".into(),
            label: "Manage filters…".into(),
        }))
        .collect()
}

pub(in crate::pages::backlog) fn selectable_issue_types(
    issue_types: Vec<JiraOption>,
) -> Vec<JiraOption> {
    issue_types
        .into_iter()
        .filter(|issue_type| {
            let normalized = issue_type
                .label
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .flat_map(char::to_lowercase)
                .collect::<String>();
            !matches!(normalized.as_str(), "subtask" | "subtasks" | "epic")
        })
        .collect()
}

pub(in crate::pages::backlog) fn issue_types_in_snapshot(
    snapshot: &BacklogSnapshot,
) -> Vec<JiraOption> {
    let mut issue_types = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .map(|item| item.kind.trim())
        .filter(|kind| !kind.is_empty())
        .map(|kind| JiraOption {
            id: kind.to_owned(),
            label: kind.to_owned(),
        })
        .collect::<Vec<_>>();
    issue_types.sort_unstable_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.label.cmp(&right.label))
    });
    issue_types.dedup_by(|left, right| left.label.eq_ignore_ascii_case(&right.label));
    selectable_issue_types(issue_types)
}

fn issue_type_labels(issue_types: &[JiraOption]) -> HashMap<String, String> {
    issue_types
        .iter()
        .map(|issue_type| (issue_type.id.clone(), issue_type.label.clone()))
        .collect()
}

fn selectable_users(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut users = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .map(|item| item.assignee.trim())
        .filter(|user| !user.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    users.sort_unstable_by_key(|user| user.to_ascii_lowercase());
    users.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    SavedFilterField::Users.options(users)
}

fn selectable_statuses(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut statuses = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .map(|item| item.status.trim())
        .filter(|status| !status.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    statuses.sort_unstable_by_key(|status| status.to_ascii_lowercase());
    statuses.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    statuses
}

fn selectable_epics(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut epics = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .filter_map(|item| item.epic_name.as_deref())
        .map(str::trim)
        .filter(|epic| !epic.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    epics.sort_unstable_by_key(|epic| epic.to_ascii_lowercase());
    epics.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    SavedFilterField::Epics.options(epics)
}

fn selectable_labels(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut labels = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .flat_map(|item| &item.labels)
        .map(|label| label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    labels.sort_unstable_by_key(|label| label.to_ascii_lowercase());
    labels.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    SavedFilterField::Labels.options(labels)
}

fn selectable_releases(snapshot: &BacklogSnapshot) -> Vec<String> {
    let mut releases = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .flat_map(|item| &item.fix_versions)
        .map(|release| release.trim())
        .filter(|release| !release.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    releases.sort_unstable_by(|left, right| version_name_cmp(left, right));
    releases.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    SavedFilterField::Releases.options(releases)
}

fn backlog_rows(
    snapshot: &BacklogSnapshot,
    filters: &BacklogFilters,
    group_by: Option<BacklogGroupBy>,
    search: &str,
) -> Vec<BacklogRow> {
    if let Some(group_by) = group_by {
        return grouped_backlog_rows(snapshot, filters, group_by, search);
    }

    let mut rows = Vec::new();
    for sprint in &snapshot.sprints {
        let section = format!("sprint-{}", sprint.id);
        rows.push(sprint_section_row(&section, sprint));
        let visible_items = visible_work_items(&sprint.work_items, filters);
        let item_keys = visible_items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<std::collections::HashSet<_>>();
        let is_active_sprint = sprint.state == "active";
        rows.extend(visible_items.into_iter().enumerate().map(|(index, item)| {
            work_item_row(
                item,
                &section,
                &section_row_id(&section),
                None,
                is_active_sprint,
                &item_keys,
                snapshot.story_points_configured,
                snapshot
                    .ticket_comments
                    .get(&item.key)
                    .map(|comments| comments.total),
                None,
                sprint.capacity.as_ref().map(|capacity| {
                    (
                        capacity.assumed_ticket_size,
                        capacity.assumed_ticket_size_from_average,
                    )
                }),
                index % 2 == 0,
            )
        }));
    }
    rows.push(backlog_section_row(snapshot));
    let visible_items = visible_work_items(&snapshot.work_items, filters);
    let item_keys = visible_items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<HashSet<_>>();
    rows.extend(visible_items.into_iter().enumerate().map(|(index, item)| {
        work_item_row(
            item,
            "backlog",
            &section_row_id("backlog"),
            None,
            false,
            &item_keys,
            snapshot.story_points_configured,
            snapshot
                .ticket_comments
                .get(&item.key)
                .map(|comments| comments.total),
            backlog_runway_ticket(snapshot, item),
            None,
            index % 2 == 0,
        )
    }));
    rows
}

fn grouped_backlog_rows(
    snapshot: &BacklogSnapshot,
    filters: &BacklogFilters,
    group_by: BacklogGroupBy,
    search: &str,
) -> Vec<BacklogRow> {
    let visible_items = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| visible_work_items(&sprint.work_items, filters))
        .chain(visible_work_items(&snapshot.work_items, filters))
        .collect::<Vec<_>>();
    let full_release_groups = if group_by == BacklogGroupBy::Release && filters.is_active() {
        let unfiltered = BacklogFilters::default();
        let items = snapshot
            .sprints
            .iter()
            .flat_map(|sprint| visible_work_items(&sprint.work_items, &unfiltered))
            .chain(visible_work_items(&snapshot.work_items, &unfiltered))
            .collect::<Vec<_>>();
        grouped_work_items(snapshot, &items, group_by)
    } else {
        Vec::new()
    };

    grouped_work_items(snapshot, &visible_items, group_by)
        .into_iter()
        .flat_map(|group| {
            let group_id = group_row_id(group_by, &group.label);
            let item_keys = group
                .items
                .iter()
                .map(|item| item.key.as_str())
                .collect::<HashSet<_>>();
            let full_group = full_release_groups
                .iter()
                .find(|full| full.label == group.label && !is_unassigned_group_label(&full.label))
                .unwrap_or(&group);
            let hides_tickets = full_group.items.len() > group.items.len();
            let mut rows = vec![group_row(
                snapshot,
                group_by,
                &group_id,
                full_group,
                hides_tickets,
            )];
            rows.extend(group.items.iter().enumerate().map(|(index, item)| {
                let source = work_item_source(snapshot, item);
                work_item_row(
                    item,
                    &source.section,
                    &group_id,
                    Some(&group_id),
                    source.is_active_sprint,
                    &item_keys,
                    snapshot.story_points_configured,
                    snapshot
                        .ticket_comments
                        .get(&item.key)
                        .map(|comments| comments.total),
                    source
                        .is_backlog
                        .then(|| backlog_runway_ticket(snapshot, item))
                        .flatten(),
                    source.assumed_ticket_size,
                    index % 2 == 0,
                )
            }));
            if group_by == BacklogGroupBy::Release && !hides_tickets && !search.trim().is_empty() {
                let visible_ids = search_visible_row_ids(&rows, search.trim());
                if rows
                    .iter()
                    .skip(1)
                    .any(|row| !visible_ids.contains(row.id.as_str()))
                {
                    rows[0] = group_row(snapshot, group_by, &group_id, full_group, true);
                }
            }
            rows
        })
        .collect()
}

struct WorkItemSource {
    section: String,
    is_active_sprint: bool,
    is_backlog: bool,
    assumed_ticket_size: Option<(f64, bool)>,
}

fn work_item_source(snapshot: &BacklogSnapshot, item: &WorkItem) -> WorkItemSource {
    snapshot
        .sprints
        .iter()
        .find(|sprint| {
            sprint
                .work_items
                .iter()
                .any(|candidate| candidate.key == item.key)
        })
        .map(|sprint| WorkItemSource {
            section: format!("sprint-{}", sprint.id),
            is_active_sprint: sprint.state == "active",
            is_backlog: false,
            assumed_ticket_size: sprint.capacity.as_ref().map(|capacity| {
                (
                    capacity.assumed_ticket_size,
                    capacity.assumed_ticket_size_from_average,
                )
            }),
        })
        .unwrap_or_else(|| WorkItemSource {
            section: "backlog".into(),
            is_active_sprint: false,
            is_backlog: true,
            assumed_ticket_size: None,
        })
}

struct WorkItemGroup<'a> {
    label: String,
    items: Vec<&'a WorkItem>,
    root_count: usize,
}

fn grouped_work_items<'a>(
    snapshot: &BacklogSnapshot,
    items: &[&'a WorkItem],
    group_by: BacklogGroupBy,
) -> Vec<WorkItemGroup<'a>> {
    let items_by_key = items
        .iter()
        .map(|item| (item.key.as_str(), *item))
        .collect::<HashMap<_, _>>();
    let mut groups = Vec::<(String, HashSet<&str>)>::new();
    for item in items {
        if root_work_item_key(item, &items_by_key) != item.key {
            continue;
        }
        for label in group_labels(item, group_by) {
            let index = groups
                .iter_mut()
                .position(|(existing, _)| existing == &label)
                .unwrap_or_else(|| {
                    groups.push((label, HashSet::new()));
                    groups.len() - 1
                });
            groups[index].1.insert(item.key.as_str());
        }
    }
    let mut groups = groups
        .into_iter()
        .map(|(label, roots)| WorkItemGroup {
            label,
            items: items
                .iter()
                .copied()
                .filter(|item| roots.contains(root_work_item_key(item, &items_by_key)))
                .collect(),
            root_count: roots.len(),
        })
        .collect::<Vec<_>>();
    groups.sort_unstable_by(|left, right| {
        group_label_cmp(snapshot, group_by, &left.label, &right.label)
    });
    groups
}

fn group_label_cmp(
    snapshot: &BacklogSnapshot,
    group_by: BacklogGroupBy,
    left: &str,
    right: &str,
) -> std::cmp::Ordering {
    match (
        is_unassigned_group_label(left),
        is_unassigned_group_label(right),
    ) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => match group_by {
            BacklogGroupBy::Release => match (
                release::scheduled_dates(snapshot, left),
                release::scheduled_dates(snapshot, right),
            ) {
                (Some(left_dates), Some(right_dates)) => left_dates.cmp(&right_dates),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then_with(|| version_name_cmp(left, right)),
            BacklogGroupBy::Epic | BacklogGroupBy::User => {
                left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase())
            }
        },
    }
}

fn is_unassigned_group_label(label: &str) -> bool {
    matches!(
        label,
        "(no release version)" | "(no epic assigned)" | "(no user assigned)"
    )
}

fn root_work_item_key<'a>(
    item: &'a WorkItem,
    items_by_key: &HashMap<&'a str, &'a WorkItem>,
) -> &'a str {
    let mut current = item;
    let mut seen = HashSet::from([item.key.as_str()]);
    while let Some(parent_key) = current.parent_key.as_deref() {
        let Some(parent) = items_by_key.get(parent_key).copied() else {
            break;
        };
        if !seen.insert(parent.key.as_str()) {
            break;
        }
        current = parent;
    }
    current.key.as_str()
}

fn group_labels(item: &WorkItem, group_by: BacklogGroupBy) -> Vec<String> {
    let mut labels = match group_by {
        BacklogGroupBy::Release => item
            .fix_versions
            .iter()
            .map(|version| version.trim())
            .filter(|version| !version.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        BacklogGroupBy::Epic => item
            .epic_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .into_iter()
            .collect(),
        BacklogGroupBy::User => {
            let assignee = item.assignee.trim();
            if assignee.is_empty()
                || assignee == "--"
                || assignee.eq_ignore_ascii_case("unassigned")
            {
                Vec::new()
            } else {
                vec![assignee.to_owned()]
            }
        }
    };
    labels.sort_unstable();
    labels.dedup();
    if labels.is_empty() {
        labels.push(match group_by {
            BacklogGroupBy::Release => "(no release version)".into(),
            BacklogGroupBy::Epic => "(no epic assigned)".into(),
            BacklogGroupBy::User => "(no user assigned)".into(),
        });
    }
    labels
}

fn backlog_runway_ticket(snapshot: &BacklogSnapshot, item: &WorkItem) -> Option<RunwayTicket> {
    snapshot
        .runway
        .as_ref()
        .and_then(|runway| runway.tickets.iter().find(|ticket| ticket.key == item.key))
        .cloned()
}

fn visible_work_items<'a>(items: &'a [WorkItem], filters: &BacklogFilters) -> Vec<&'a WorkItem> {
    let items_by_key = items
        .iter()
        .map(|item| (item.key.as_str(), item))
        .collect::<std::collections::HashMap<_, _>>();
    items
        .iter()
        .filter(|item| {
            (!matches!(work_item_kind(&item.kind), WorkItemKind::Subtask)
                || item
                    .parent_key
                    .as_deref()
                    .is_some_and(|parent| items_by_key.contains_key(parent)))
                && matches_filters(item, filters)
                && ancestors_match_filters(item, &items_by_key, filters)
        })
        .collect()
}

fn ancestors_match_filters(
    item: &WorkItem,
    items_by_key: &std::collections::HashMap<&str, &WorkItem>,
    filters: &BacklogFilters,
) -> bool {
    let mut ancestor = item;
    let mut seen = std::collections::HashSet::from([item.key.as_str()]);
    while let Some(parent_key) = ancestor.parent_key.as_deref() {
        let Some(parent) = items_by_key.get(parent_key).copied() else {
            return true;
        };
        if !seen.insert(parent.key.as_str()) || !matches_filters(parent, filters) {
            return false;
        }
        ancestor = parent;
    }
    true
}

fn matches_filters(item: &WorkItem, filters: &BacklogFilters) -> bool {
    (filters.estimated || (estimation_eligible(item) && item.story_points.is_none()))
        && (filters.issue_types.is_empty()
            || matches!(work_item_kind(&item.kind), WorkItemKind::Subtask)
            || filters
                .issue_types
                .iter()
                .any(|issue_type| item.kind.eq_ignore_ascii_case(issue_type)))
        && matches_optional_values(
            &filters.users,
            std::iter::once(item.assignee.as_str())
                .filter(|user| !user.trim().eq_ignore_ascii_case("Unassigned")),
        )
        && (filters.statuses.is_empty()
            || filters
                .statuses
                .iter()
                .any(|status| item.status.eq_ignore_ascii_case(status)))
        && matches_optional_values(&filters.epics, item.epic_name.as_deref())
        && matches_optional_values(&filters.labels, item.labels.iter().map(String::as_str))
        && matches_optional_values(
            &filters.releases,
            item.fix_versions.iter().map(String::as_str),
        )
}

fn initially_expanded_rows(
    snapshot: &BacklogSnapshot,
    group_by: Option<BacklogGroupBy>,
) -> Vec<String> {
    if let Some(group_by) = group_by {
        let items = snapshot
            .sprints
            .iter()
            .flat_map(|sprint| &sprint.work_items)
            .chain(&snapshot.work_items)
            .collect::<Vec<_>>();
        return grouped_work_items(snapshot, &items, group_by)
            .into_iter()
            .flat_map(|group| {
                let group_id = group_row_id(group_by, &group.label);
                std::iter::once(group_id.clone())
                    .chain(group_parent_row_ids(&group.items, &group_id))
            })
            .collect();
    }

    let mut expanded = vec![section_row_id("backlog")];
    for sprint in &snapshot.sprints {
        expanded.extend(parent_row_ids(&sprint.work_items));
    }
    expanded.extend(parent_row_ids(&snapshot.work_items));
    expanded
}

fn parent_row_ids(items: &[WorkItem]) -> Vec<String> {
    let item_keys = items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<std::collections::HashSet<_>>();
    items
        .iter()
        .filter_map(|item| {
            item.parent_key
                .as_deref()
                .filter(|parent| item_keys.contains(parent))
                .map(|parent| format!("ticket:{parent}"))
        })
        .collect()
}

fn group_parent_row_ids(items: &[&WorkItem], group_id: &str) -> Vec<String> {
    let item_keys = items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<HashSet<_>>();
    items
        .iter()
        .filter_map(|item| {
            item.parent_key
                .as_deref()
                .filter(|parent| item_keys.contains(parent))
                .map(|parent| ticket_row_id(parent, Some(group_id)))
        })
        .collect()
}

fn section_row(section: &str, title: Text<'static>, search_text: String) -> BacklogRow {
    BacklogRow {
        id: section_row_id(section),
        parent_id: None,
        content: BacklogRowContent::Section { title, search_text },
    }
}

fn group_row_id(group_by: BacklogGroupBy, label: &str) -> String {
    format!("group:{}:{label}", group_by.id())
}

fn group_row(
    snapshot: &BacklogSnapshot,
    group_by: BacklogGroupBy,
    id: &str,
    group: &WorkItemGroup<'_>,
    filtered: bool,
) -> BacklogRow {
    if group_by == BacklogGroupBy::Release && !is_unassigned_group_label(&group.label) {
        return BacklogRow {
            id: id.into(),
            parent_id: None,
            content: BacklogRowContent::Group {
                title: release_row::title(
                    snapshot,
                    group,
                    chrono::Utc::now().date_naive(),
                    filtered,
                ),
                search_text: group.label.clone(),
            },
        };
    }
    if group_by == BacklogGroupBy::User {
        return BacklogRow {
            id: id.into(),
            parent_id: None,
            content: BacklogRowContent::Group {
                title: user_group_title(group),
                search_text: group.label.clone(),
            },
        };
    }
    let theme = tuicore::theme();
    let no_assignment = is_unassigned_group_label(&group.label);
    let mut heading = vec![
        Span::styled(
            grouping_icon(Some(group_by)),
            Style::default().fg(theme.accent_fg()),
        ),
        Span::raw(" "),
        Span::styled(
            group.label.clone(),
            if no_assignment {
                Style::default().fg(theme.muted_fg())
            } else {
                Style::default()
                    .fg(theme.text_fg())
                    .add_modifier(Modifier::BOLD)
            },
        ),
    ];
    if no_assignment {
        heading.extend([
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!("{} items", group.root_count),
                Style::default().fg(theme.muted_fg()),
            ),
        ]);
    }
    let mut title = vec![Line::from(heading)];
    if !no_assignment {
        let (completed_points, total_points) = group_points(&group.items);
        let (coverage, coverage_style) = estimation_coverage(&group.items);
        let (completed_items, total_items) = root_item_counts_refs(&group.items);
        title.push(Line::from(vec![
            Span::styled(format!("{coverage} est"), coverage_style),
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!("{} open", total_items - completed_items),
                Style::default().fg(theme.muted_fg()),
            ),
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!(
                    "{} pts remaining",
                    points_label((total_points - completed_points).max(0.0))
                ),
                Style::default().fg(theme.text_fg()),
            ),
        ]));
    }
    BacklogRow {
        id: id.into(),
        parent_id: None,
        content: BacklogRowContent::Group {
            title: Text::from(title),
            search_text: group.label.clone(),
        },
    }
}

fn user_group_title(group: &WorkItemGroup<'_>) -> Text<'static> {
    let theme = tuicore::theme();
    let muted = Style::default().fg(theme.muted_fg());
    let no_assignment = is_unassigned_group_label(&group.label);
    let (coverage, coverage_style) = estimation_coverage(&group.items);
    let heading = Line::from(vec![
        Span::styled(
            "󰀆",
            if no_assignment {
                muted
            } else {
                Style::default().fg(theme.accent_fg())
            },
        ),
        Span::raw(" "),
        Span::styled(
            group.label.clone(),
            if no_assignment {
                muted
            } else {
                Style::default()
                    .fg(theme.text_fg())
                    .add_modifier(Modifier::BOLD)
            },
        ),
        Span::styled(" • ", muted),
        Span::styled(format!("{coverage} est"), coverage_style),
        Span::styled(" • ", muted),
        Span::styled(format!("{} items", group.items.len()), muted),
    ]);
    let mut status = Vec::new();
    for summary in status_summaries(&group.items) {
        if !status.is_empty() {
            status.push(Span::styled(" • ", muted));
        }
        status.push(Span::styled(
            format!("{} {}", summary.count, summary.label),
            status_category_style(summary.category),
        ));
    }
    Text::from(vec![heading, Line::from(status)])
}

struct StatusSummary {
    category: StatusCategory,
    label: String,
    count: usize,
}

fn status_summaries(items: &[&WorkItem]) -> Vec<StatusSummary> {
    let mut summaries = Vec::<StatusSummary>::new();
    for item in items {
        let category = if item.status_category == StatusCategory::Unknown && item.done {
            StatusCategory::Done
        } else {
            item.status_category
        };
        let label = match item.status.trim() {
            "" => match category {
                StatusCategory::Todo => "To Do",
                StatusCategory::InProgress => "In Progress",
                StatusCategory::Done => "Done",
                StatusCategory::Unknown => "Unknown",
            },
            label => label,
        };
        if let Some(summary) = summaries.iter_mut().find(|summary| {
            summary.category == category && summary.label.eq_ignore_ascii_case(label)
        }) {
            summary.count += 1;
        } else {
            summaries.push(StatusSummary {
                category,
                label: label.to_owned(),
                count: 1,
            });
        }
    }
    summaries.sort_unstable_by(|left, right| {
        status_category_order(left.category)
            .cmp(&status_category_order(right.category))
            .then_with(|| {
                status_name_order(left.category, &left.label)
                    .cmp(&status_name_order(right.category, &right.label))
            })
            .then_with(|| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
            })
            .then_with(|| left.label.cmp(&right.label))
    });
    summaries
}

fn status_category_order(category: StatusCategory) -> u8 {
    match category {
        StatusCategory::Todo => 0,
        StatusCategory::InProgress => 1,
        StatusCategory::Unknown => 2,
        StatusCategory::Done => 3,
    }
}

fn status_name_order(category: StatusCategory, label: &str) -> u8 {
    let label = label.to_ascii_lowercase();
    match category {
        StatusCategory::Todo if label.contains("backlog") => 0,
        StatusCategory::Todo if label.contains("to do") => 1,
        StatusCategory::Todo if label.contains("todo") => 2,
        StatusCategory::Todo => 3,
        StatusCategory::InProgress if label.contains("progress") => 0,
        StatusCategory::InProgress if label.contains("review") => 1,
        StatusCategory::InProgress => 2,
        StatusCategory::Done if label.contains("done") => 1,
        StatusCategory::Done | StatusCategory::Unknown => 0,
    }
}

fn sprint_section_row(section: &str, sprint: &Sprint) -> BacklogRow {
    let theme = tuicore::theme();
    let icon = sprint_icon(&sprint.state);
    let mut title = vec![
        Span::styled(icon, Style::default().fg(theme.accent_fg())),
        Span::raw(" "),
        Span::styled(
            sprint.name.clone(),
            Style::default()
                .fg(theme.text_fg())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(date_range) = sprint_date_range(sprint) {
        title.extend([
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(date_range, Style::default().fg(theme.text_fg())),
        ]);
    }
    let search_text = sprint_title(sprint);
    let is_active = sprint.state == "active";
    let Some(capacity) = sprint.capacity.as_ref() else {
        title.extend([
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                sprint_item_summary(sprint),
                Style::default().fg(theme.muted_fg()),
            ),
        ]);
        return section_row(section, Text::from(Line::from(title)), search_text);
    };
    let (coverage, coverage_style) = sprint_estimation_coverage(sprint);
    section_row(
        section,
        Text::from(vec![
            Line::from(title),
            Line::from(vec![
                Span::styled(format!("{coverage} est"), coverage_style),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(
                    sprint_item_summary(sprint),
                    Style::default().fg(theme.muted_fg()),
                ),
                Span::styled(" • ", Style::default().fg(theme.muted_fg())),
                Span::styled(
                    sprint_capacity_icon(capacity.state),
                    capacity_indicator_style(capacity.state),
                ),
                Span::raw(" "),
                Span::styled(
                    capacity_load_label(capacity, is_active),
                    Style::default().fg(theme.text_fg()),
                ),
            ]),
        ]),
        search_text,
    )
}

fn backlog_section_row(snapshot: &BacklogSnapshot) -> BacklogRow {
    let theme = tuicore::theme();
    section_row(
        "backlog",
        Text::from(Line::from(vec![
            Span::styled("", Style::default().fg(theme.accent_fg())),
            Span::raw(" "),
            Span::styled(
                "Backlog",
                Style::default()
                    .fg(theme.text_fg())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" • ", Style::default().fg(theme.muted_fg())),
            Span::styled(
                format!("{} items", root_item_count_label(&snapshot.work_items)),
                Style::default().fg(theme.muted_fg()),
            ),
        ])),
        backlog_title(snapshot),
    )
}
fn section_row_id(section: &str) -> String {
    format!("section:{section}")
}
#[expect(
    clippy::too_many_arguments,
    reason = "Row construction combines section geometry and ticket presentation inputs"
)]
fn work_item_row(
    item: &WorkItem,
    section: &str,
    root_parent_id: &str,
    row_namespace: Option<&str>,
    is_active_sprint: bool,
    item_keys: &std::collections::HashSet<&str>,
    show_story_points: bool,
    comment_count: Option<usize>,
    runway: Option<RunwayTicket>,
    assumed_ticket_size: Option<(f64, bool)>,
    alternate_background: bool,
) -> BacklogRow {
    let assumed_ticket_size = matches!(
        work_item_kind(&item.kind),
        WorkItemKind::Story | WorkItemKind::Task
    )
    .then(|| {
        runway
            .as_ref()
            .filter(|runway| runway.assumed)
            .map(|runway| (runway.effective_points, runway.assumed_from_average))
            .or(assumed_ticket_size)
    })
    .flatten();
    BacklogRow {
        id: ticket_row_id(&item.key, row_namespace),
        parent_id: item
            .parent_key
            .as_deref()
            .filter(|parent| item_keys.contains(parent))
            .map(|parent| ticket_row_id(parent, row_namespace))
            .or_else(|| Some(root_parent_id.into())),
        content: BacklogRowContent::WorkItem(Box::new(BacklogWorkItem {
            item: WorkItemRow {
                id: item.key.clone(),
                key: item.key.clone(),
                title: item.title.clone(),
                kind: work_item_kind(&item.kind),
                priority: item.priority.clone(),
                status: item.status.clone(),
                status_category: item.status_category,
                done: item.done,
                assignee: item.assignee.clone(),
                labels: item.labels.clone(),
                story_points: item
                    .story_points
                    .or_else(|| assumed_ticket_size.map(|(points, _)| points)),
                show_story_points: show_story_points || assumed_ticket_size.is_some(),
                story_points_estimated: item.story_points.is_none()
                    && assumed_ticket_size.is_some(),
                story_points_from_average: item.story_points.is_none()
                    && assumed_ticket_size.is_some_and(|(_, from_average)| from_average),
                change_badge: None,
                submitted: false,
                status_changed_at: item.status_changed_at,
                show_time_in_status: is_active_sprint,
            },
            description: item.description.clone(),
            section: section.into(),
            rankable_root: item
                .parent_key
                .as_deref()
                .is_none_or(|parent| !item_keys.contains(parent)),
            runway,
            alternate_background,
            subtask_progress: item.subtask_progress.clone(),
            fix_versions: item.fix_versions.clone(),
            epic_name: item.epic_name.clone(),
            comment_count,
        })),
    }
}

fn ticket_row_id(key: &str, row_namespace: Option<&str>) -> String {
    row_namespace
        .map(|namespace| format!("ticket:{namespace}:{key}"))
        .unwrap_or_else(|| format!("ticket:{key}"))
}
fn backlog_column(
    number_jump: Rc<RefCell<TicketNumberJump>>,
    #[cfg(test)] renderer_calls: Rc<Cell<usize>>,
) -> Column<BacklogRow, String> {
    Column::multiline(
        "backlog",
        "",
        Constraint::Percentage(100),
        move |row: &BacklogRow, _: &CellContext<String>| {
            #[cfg(test)]
            renderer_calls.set(renderer_calls.get().saturating_add(1));
            match &row.content {
                BacklogRowContent::Section { title, .. }
                | BacklogRowContent::Group { title, .. } => title.clone(),
                BacklogRowContent::WorkItem(item) => {
                    backlog_work_item_text(item, number_jump.borrow().query())
                }
            }
        },
    )
    .constrained()
    .wrap_continuation_indent_by(|row| match &row.content {
        BacklogRowContent::Section { .. } | BacklogRowContent::Group { .. } => 0,
        BacklogRowContent::WorkItem(item) => tuicore::preset()
            .data_view()
            .tree_indent_width()
            .saturating_add(1)
            .saturating_add(2)
            .saturating_add(work_item_title_prefix_width(&item.item)),
    })
    .search_key(backlog_search_text)
}

fn backlog_search_text(row: &BacklogRow) -> String {
    match &row.content {
        BacklogRowContent::Section { search_text, .. }
        | BacklogRowContent::Group { search_text, .. } => search_text.clone(),
        BacklogRowContent::WorkItem(item) => format!(
            "{} {} {}",
            item.item.key,
            item.item.title,
            item.epic_name.as_deref().unwrap_or_default(),
        ),
    }
}

fn descendant_row_ids<'a>(id: &'a str, rows: &'a [BacklogRow]) -> Vec<&'a str> {
    let mut descendants = Vec::new();
    let mut parents = vec![id];
    while let Some(parent) = parents.pop() {
        for child in rows
            .iter()
            .filter(|row| row.parent_id.as_deref() == Some(parent))
        {
            descendants.push(child.id.as_str());
            parents.push(child.id.as_str());
        }
    }
    descendants
}

fn search_visible_row_ids<'a>(rows: &'a [BacklogRow], search: &str) -> HashSet<&'a str> {
    let parent_ids = rows
        .iter()
        .map(|row| (row.id.as_str(), row.parent_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut visible_ids = HashSet::new();
    for row in rows.iter().filter(|row| {
        tuicore::search_match(search, &backlog_search_text(row), SearchMode::Contains).is_some()
    }) {
        visible_ids.insert(row.id.as_str());
        visible_ids.extend(descendant_row_ids(row.id.as_str(), rows));
        let mut parent_id = row.parent_id.as_deref();
        while let Some(id) = parent_id {
            if !visible_ids.insert(id) {
                break;
            }
            parent_id = parent_ids.get(id).copied().flatten();
        }
    }
    visible_ids
}

fn backlog_work_item_text(row: &BacklogWorkItem, number_query: Option<&str>) -> Text<'static> {
    ticket_summary_text(
        &row.item,
        number_query,
        None,
        TicketRowDetails {
            subtask_progress: row
                .subtask_progress
                .as_ref()
                .map(|progress| (progress.completed, progress.total)),
            fix_versions: &row.fix_versions,
            epic_name: row.epic_name.as_deref(),
            annotation: None,
            comment_count: row.comment_count,
        },
    )
}

fn sprint_title(sprint: &Sprint) -> String {
    let icon = sprint_icon(&sprint.state);
    let date_range = sprint_date_range(sprint)
        .map(|range| format!(" • {range}"))
        .unwrap_or_default();
    let is_active = sprint.state == "active";
    let Some(capacity) = sprint.capacity.as_ref() else {
        return format!("{icon} {}{date_range}", sprint.name);
    };
    format!(
        "{icon} {}{date_range}\n{} est • {} • {} {}",
        sprint.name,
        sprint_estimation_coverage(sprint).0,
        sprint_item_summary(sprint),
        sprint_capacity_icon(capacity.state),
        capacity_load_label(capacity, is_active),
    )
}

fn sprint_estimation_coverage(sprint: &Sprint) -> (String, Style) {
    let items = sprint.work_items.iter().collect::<Vec<_>>();
    estimation_coverage(&items)
}

fn estimation_coverage(items: &[&WorkItem]) -> (String, Style) {
    let theme = tuicore::theme();
    let eligible_items = items
        .iter()
        .filter(|item| estimation_eligible(item))
        .count();
    let estimated_items = items
        .iter()
        .filter(|item| estimation_eligible(item) && item.story_points.is_some())
        .count();
    if estimated_items == eligible_items {
        (
            format!("✓ {eligible_items}/{eligible_items}"),
            Style::default().fg(theme.success_fg()),
        )
    } else {
        (
            format!("󰄰 {estimated_items}/{eligible_items}"),
            Style::default().fg(theme.warning_fg()),
        )
    }
}

fn group_points(items: &[&WorkItem]) -> (f64, f64) {
    items.iter().fold((0.0, 0.0), |(completed, total), item| {
        let points = item.story_points.filter(|_| estimation_eligible(item));
        (
            completed
                + points
                    .filter(|_| item.done || crate::store::work_items::is_done_status(&item.status))
                    .unwrap_or_default(),
            total + points.unwrap_or_default(),
        )
    })
}

fn estimation_eligible(item: &WorkItem) -> bool {
    matches!(item.kind.to_ascii_lowercase().as_str(), "task" | "story")
}

fn capacity_load_label(
    capacity: &crate::store::work_items::SprintCapacity,
    is_active: bool,
) -> String {
    let prefix = matches!(capacity.source, RunwayCapacitySource::JiraVelocity)
        .then_some("~")
        .unwrap_or("");
    let source_suffix = match capacity.source {
        RunwayCapacitySource::JiraVelocity => "v",
        RunwayCapacitySource::Fixed | RunwayCapacitySource::FixedFallback => "c",
    };
    let (numerator, denominator) = if is_active {
        (capacity.completed_points, capacity.effective_points)
    } else {
        (capacity.effective_points, capacity.capacity)
    };
    format!(
        "{prefix}{}/{} pts ({}{source_suffix})",
        points_label(numerator),
        points_label(denominator),
        points_label(capacity.capacity)
    )
}

fn backlog_title(snapshot: &BacklogSnapshot) -> String {
    format!(
        " Backlog • {} items",
        root_item_count_label(&snapshot.work_items)
    )
}

fn sprint_item_summary(sprint: &Sprint) -> String {
    let (completed, total) = root_item_counts(&sprint.work_items);
    if sprint.state == "future" {
        format!("{total} planned")
    } else {
        format!("{completed}/{total} done")
    }
}

fn root_item_count_label(items: &[WorkItem]) -> String {
    let (_, total) = root_item_counts(items);
    format!("{total}")
}

fn root_item_counts(items: &[WorkItem]) -> (usize, usize) {
    let items = items.iter().collect::<Vec<_>>();
    root_item_counts_refs(&items)
}

fn root_item_counts_refs(items: &[&WorkItem]) -> (usize, usize) {
    let keys = items
        .iter()
        .map(|item| item.key.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut completed = 0;
    let mut total = 0;
    for item in items {
        if !is_subtask(item)
            && item
                .parent_key
                .as_deref()
                .is_none_or(|parent| !keys.contains(parent))
        {
            total += 1;
            if item.done || crate::store::work_items::is_done_status(&item.status) {
                completed += 1;
            }
        }
    }
    (completed, total)
}

fn is_subtask(item: &WorkItem) -> bool {
    matches!(
        item.kind.to_ascii_lowercase().as_str(),
        "sub-task" | "subtask"
    )
}

fn sprint_icon(state: &str) -> &'static str {
    match state {
        "active" => "",
        "future" => "",
        _ => "•",
    }
}

fn sprint_capacity_icon(state: SprintCapacityState) -> &'static str {
    match state {
        SprintCapacityState::OnTarget => "󱩿",
        SprintCapacityState::OverCommitted => "󰸁",
        SprintCapacityState::UnderCommitted => "󰸂",
    }
}

fn capacity_indicator_style(state: SprintCapacityState) -> Style {
    let theme = tuicore::theme();
    Style::default().fg(match state {
        SprintCapacityState::UnderCommitted => theme.warning_fg(),
        SprintCapacityState::OnTarget => theme.success_fg(),
        SprintCapacityState::OverCommitted => theme.error_fg(),
    })
}

fn sprint_date_range(sprint: &Sprint) -> Option<String> {
    Some(format!(
        "{} – {}",
        sprint_date_label(sprint.start_date.as_deref()?)?,
        sprint_date_label(sprint.end_date.as_deref()?)?,
    ))
}

fn sprint_date_label(date: &str) -> Option<String> {
    let mut parts = date.get(..10)?.split('-');
    let year = parts.next()?.parse::<u16>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    let month = match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => return None,
    };
    (year > 0 && (1..=31).contains(&day) && parts.next().is_none())
        .then(|| format!("{day} {month}"))
}

fn points_label(points: f64) -> String {
    if points.fract().abs() < f64::EPSILON {
        format!("{points:.0}")
    } else {
        format!("{points:.1}")
    }
}
fn work_item_kind(kind: &str) -> WorkItemKind {
    match kind.to_ascii_lowercase().as_str() {
        "epic" => WorkItemKind::Epic,
        "story" => WorkItemKind::Story,
        "task" => WorkItemKind::Task,
        "bug" => WorkItemKind::Bug,
        "subtask" | "sub-task" => WorkItemKind::Subtask,
        _ => WorkItemKind::Other,
    }
}
