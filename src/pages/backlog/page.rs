use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph as RatatuiParagraph, Wrap},
};
use tuicore::{
    AnimationSettings, ChildKey, Column, CrossAlign, DataView, Dialog, DialogBackdrop, DialogHost,
    DialogLayer, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId,
    FocusRequest, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, MainAlign, Paragraph, RenderCtx, ScrollContainer, Spinner, TickResult, TreePath,
    TuiEvent, TuiNode,
};

use crate::{
    app_settings::BacklogRunwaySettings,
    jira::{self, JiraAssignee, JiraOption},
    service::AppService,
    store::work_items::{
        BacklogSnapshot, RankPlan, Sprint, StatusTransition, VelocityReport, VelocitySprint,
        WorkItem, apply_capacity, is_done_status, loaded_story_point_average, rank_plan,
    },
};

use super::components::{
    BacklogAssignee, BacklogDestination, BacklogQuickMenu, BacklogQuickMenuEvent,
    BacklogSectionEvent, BacklogTree, backlog_tree_with_issue_types,
};
use super::velocity_reports::copy_report;

type BacklogQuickMenuLayer = DialogLayer<BacklogTree, BacklogQuickMenu>;
type VelocityDialog = DialogHost<Flex<()>, ()>;
type BacklogView = DialogLayer<BacklogQuickMenuLayer, VelocityDialog>;

#[derive(Clone)]
struct VelocityRow {
    sprint: VelocitySprint,
    alternate_background: bool,
    share_goal: String,
}

enum BacklogResult {
    Loaded {
        generation: u64,
        result: Result<BacklogSnapshot, String>,
    },
    IssueTypesLoaded {
        generation: u64,
        result: Result<Vec<JiraOption>, String>,
    },
    Ranked {
        generation: u64,
        result: Result<(), String>,
    },
    Transferred {
        generation: u64,
        destination: String,
        result: Result<(), String>,
    },
    StatusesLoaded {
        generation: u64,
        keys: Vec<String>,
        result: Result<Vec<(String, Vec<JiraOption>)>, String>,
    },
    StatusSet {
        generation: u64,
        status: StatusTransition,
        result: Result<(), String>,
    },
    AssigneesLoaded {
        generation: u64,
        result: Result<Vec<JiraAssignee>, String>,
    },
    UsersAssigned {
        generation: u64,
        keys: Vec<String>,
        assignee: BacklogAssignee,
        result: Result<(), String>,
    },
    CurrentUserLoaded {
        generation: u64,
        keys: Vec<String>,
        result: Result<JiraAssignee, String>,
    },
}

const RANK_REFRESH_RETRY_DELAY: Duration = Duration::from_secs(1);
pub(super) const MAX_UNCONFIRMED_TRANSFER_REFRESHES: usize = 3;

#[derive(Default)]
pub(super) struct RankRefreshRetry {
    remaining: Option<Duration>,
    preserve_optimistic_view: bool,
}

impl RankRefreshRetry {
    pub(super) fn schedule(&mut self, preserve_optimistic_view: bool) {
        self.remaining = Some(RANK_REFRESH_RETRY_DELAY);
        self.preserve_optimistic_view = preserve_optimistic_view;
    }

    pub(super) fn cancel(&mut self) {
        self.remaining = None;
        self.preserve_optimistic_view = false;
    }

    pub(super) fn pending(&self) -> bool {
        self.remaining.is_some()
    }

    pub(super) fn elapse(&mut self, dt: Duration) -> Option<bool> {
        let Some(remaining) = self.remaining else {
            return None;
        };
        if dt >= remaining {
            self.remaining = None;
            Some(std::mem::take(&mut self.preserve_optimistic_view))
        } else {
            self.remaining = Some(remaining - dt);
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadCompletion {
    Normal,
    RankRefresh { preserve_optimistic_view: bool },
}

#[derive(Default)]
pub(super) struct RequestGenerations {
    next: u64,
    active_load: Option<u64>,
    active_rank: Option<u64>,
    active_status_load: Option<u64>,
    active_status_sets: HashSet<u64>,
    active_assignees_load: Option<u64>,
    active_users_assignments: HashSet<u64>,
    active_current_user_load: Option<u64>,
    active_issue_types_load: Option<u64>,
    rank_refresh_load: Option<u64>,
    preserve_optimistic_view_load: Option<u64>,
}

impl RequestGenerations {
    fn next(&mut self) -> u64 {
        self.next = self
            .next
            .checked_add(1)
            .expect("backlog request generation overflowed");
        self.next
    }

    pub(super) fn start_load(&mut self, rank_refresh: bool, preserve_optimistic_view: bool) -> u64 {
        let generation = self.next();
        self.active_load = Some(generation);
        self.rank_refresh_load = rank_refresh.then_some(generation);
        self.preserve_optimistic_view_load =
            (rank_refresh && preserve_optimistic_view).then_some(generation);
        generation
    }

    pub(super) fn start_rank(&mut self) -> u64 {
        let generation = self.next();
        self.active_load = None;
        self.rank_refresh_load = None;
        self.preserve_optimistic_view_load = None;
        self.active_rank = Some(generation);
        generation
    }

    fn start_status_load(&mut self) -> u64 {
        let generation = self.next();
        self.active_status_load = Some(generation);
        generation
    }

    fn start_issue_types_load(&mut self) -> u64 {
        let generation = self.next();
        self.active_issue_types_load = Some(generation);
        generation
    }

    fn complete_issue_types_load(&mut self, generation: u64) -> bool {
        if self.active_issue_types_load != Some(generation) {
            return false;
        }
        self.active_issue_types_load = None;
        true
    }

    fn complete_status_load(&mut self, generation: u64) -> bool {
        if self.active_status_load != Some(generation) {
            return false;
        }
        self.active_status_load = None;
        true
    }

    pub(super) fn start_status_set(&mut self) -> u64 {
        let generation = self.next();
        self.active_status_sets.insert(generation);
        generation
    }

    pub(super) fn complete_status_set(&mut self, generation: u64) -> bool {
        self.active_status_sets.remove(&generation)
    }

    fn start_assignees_load(&mut self) -> u64 {
        let generation = self.next();
        self.active_assignees_load = Some(generation);
        generation
    }

    fn complete_assignees_load(&mut self, generation: u64) -> bool {
        if self.active_assignees_load != Some(generation) {
            return false;
        }
        self.active_assignees_load = None;
        true
    }

    fn cancel_assignees_load(&mut self) {
        self.active_assignees_load = None;
    }

    pub(super) fn start_users_assign(&mut self) -> u64 {
        let generation = self.next();
        self.active_users_assignments.insert(generation);
        generation
    }

    pub(super) fn complete_users_assign(&mut self, generation: u64) -> bool {
        self.active_users_assignments.remove(&generation)
    }

    fn start_current_user_load(&mut self) -> u64 {
        let generation = self.next();
        self.active_current_user_load = Some(generation);
        generation
    }

    fn complete_current_user_load(&mut self, generation: u64) -> bool {
        if self.active_current_user_load != Some(generation) {
            return false;
        }
        self.active_current_user_load = None;
        true
    }

    fn cancel_status_load(&mut self) {
        self.active_status_load = None;
    }

    pub(super) fn complete_load(&mut self, generation: u64) -> Option<LoadCompletion> {
        if self.active_load != Some(generation) {
            return None;
        }
        self.active_load = None;
        let completed_rank_refresh = self.rank_refresh_load == Some(generation);
        let preserve_optimistic_view = self.preserve_optimistic_view_load == Some(generation);
        self.rank_refresh_load = None;
        self.preserve_optimistic_view_load = None;
        Some(if completed_rank_refresh {
            LoadCompletion::RankRefresh {
                preserve_optimistic_view,
            }
        } else {
            LoadCompletion::Normal
        })
    }

    pub(super) fn complete_rank(&mut self, generation: u64) -> bool {
        if self.active_rank != Some(generation) {
            return false;
        }
        self.active_rank = None;
        true
    }
}

#[derive(Clone)]
pub(super) struct PendingTransfer {
    pub(super) rollback_snapshot: BacklogSnapshot,
    pub(super) source_section_id: String,
    pub(super) destination_section_id: String,
    pub(super) destination_order: Vec<String>,
    pub(super) keys: Vec<String>,
    pub(super) source_highlight_key: Option<String>,
    pub(super) ambiguous: bool,
    pub(super) unconfirmed_refreshes: usize,
}

#[derive(Clone)]
pub(super) struct PendingRank {
    pub(super) rollback_snapshot: BacklogSnapshot,
    pub(super) section_id: String,
    pub(super) final_order: Vec<String>,
    pub(super) unconfirmed_refreshes: usize,
}

#[derive(Clone)]
struct PendingStatusChange {
    original_items: HashMap<String, WorkItem>,
}

#[derive(Clone)]
struct PendingAssigneeChange {
    original_items: HashMap<String, WorkItem>,
}

#[derive(Default)]
pub(super) struct StatusTransitionCache {
    by_issue: HashMap<String, Vec<JiraOption>>,
}

impl StatusTransitionCache {
    pub(super) fn missing_keys(&self, keys: &[String]) -> Vec<String> {
        keys.iter()
            .filter(|key| !self.by_issue.contains_key(*key))
            .cloned()
            .collect()
    }

    pub(super) fn insert(&mut self, transitions: Vec<(String, Vec<JiraOption>)>) {
        self.by_issue.extend(transitions);
    }

    pub(super) fn common(&self, keys: &[String]) -> Option<Vec<StatusTransition>> {
        let transitions = keys
            .iter()
            .map(|key| {
                self.by_issue
                    .get(key)
                    .cloned()
                    .map(|transitions| (key.clone(), transitions))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(jira::common_status_transitions(transitions))
    }

    pub(super) fn invalidate(&mut self, status: &StatusTransition) {
        for issue in &status.issues {
            self.by_issue.remove(&issue.issue_key);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingTransferReconciliation {
    ConfirmedDestination,
    ConfirmedSourceRollback,
    Unconfirmed,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingRankReconciliation {
    Confirmed,
    Unconfirmed,
    Exhausted,
}

pub(super) fn should_poll(
    loading: bool,
    ranking: bool,
    status_working: bool,
    issue_types_loading: bool,
    retry_pending: bool,
) -> bool {
    loading || ranking || status_working || issue_types_loading || retry_pending
}

pub(crate) fn page(service: AppService) -> BacklogPage {
    BacklogPage::new(service)
}

pub(crate) struct BacklogPage {
    service: AppService,
    sender: Sender<BacklogResult>,
    receiver: Receiver<BacklogResult>,
    section_receiver: Receiver<BacklogSectionEvent>,
    view: BacklogView,
    loading_view: ScrollContainer<Flex<()>>,
    loading: bool,
    ranking: bool,
    status_loading: bool,
    assignees_loading: bool,
    current_user_loading: bool,
    issue_types_loading: bool,
    syncing_ticket_keys: Rc<RefCell<HashSet<String>>>,
    current_user: Option<JiraAssignee>,
    assignees: Option<Vec<BacklogAssignee>>,
    move_locked: Rc<Cell<bool>>,
    generations: RequestGenerations,
    rank_refresh_retry: RankRefreshRetry,
    active_rank_plan: Option<RankPlan>,
    snapshot: Option<BacklogSnapshot>,
    pending_transfer: Option<PendingTransfer>,
    pending_rank: Option<PendingRank>,
    pending_status_changes: HashMap<u64, PendingStatusChange>,
    pending_assignee_changes: HashMap<u64, PendingAssigneeChange>,
    status_transition_cache: StatusTransitionCache,
    focus_backlog_after_load: bool,
    pending_focus: Option<FocusRequest>,
    data_focus_path: TreePath,
    reload_notification_pending: bool,
    settings_revision: u64,
    issue_types_requested: bool,
    velocity_dialog_close_requested: Rc<Cell<bool>>,
}

impl BacklogPage {
    fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (section_sender, section_receiver) = mpsc::channel();
        let move_locked = Rc::new(Cell::new(false));
        let syncing_ticket_keys = Rc::new(RefCell::new(HashSet::new()));
        let velocity_dialog_close_requested = Rc::new(Cell::new(false));
        let settings_revision = service.settings_revision();
        Self {
            service,
            sender,
            receiver,
            section_receiver,
            view: backlog_view(
                &empty_snapshot(),
                section_sender.clone(),
                move_locked.clone(),
                Rc::clone(&syncing_ticket_keys),
                Rc::clone(&velocity_dialog_close_requested),
                Vec::new(),
            ),
            loading_view: loading_view(),
            loading: false,
            ranking: false,
            status_loading: false,
            assignees_loading: false,
            current_user_loading: false,
            issue_types_loading: false,
            syncing_ticket_keys,
            current_user: None,
            assignees: None,
            move_locked,
            generations: RequestGenerations::default(),
            rank_refresh_retry: RankRefreshRetry::default(),
            active_rank_plan: None,
            snapshot: None,
            pending_transfer: None,
            pending_rank: None,
            pending_status_changes: HashMap::new(),
            pending_assignee_changes: HashMap::new(),
            status_transition_cache: StatusTransitionCache::default(),
            focus_backlog_after_load: false,
            pending_focus: None,
            data_focus_path: TreePath::from_keys([
                ChildKey::first(),
                ChildKey::first(),
                ChildKey::new("data"),
            ]),
            reload_notification_pending: false,
            settings_revision,
            issue_types_requested: false,
            velocity_dialog_close_requested,
        }
    }

    #[cfg(test)]
    pub(super) fn with_snapshot_for_test(snapshot: BacklogSnapshot) -> Self {
        Self::with_snapshot_and_service_for_test(snapshot, AppService::for_tests())
    }

    #[cfg(test)]
    pub(super) fn with_snapshot_and_service_for_test(
        snapshot: BacklogSnapshot,
        service: AppService,
    ) -> Self {
        let mut page = Self::new(service);
        page.snapshot = Some(snapshot);
        page.restore_snapshot();
        page
    }

    #[cfg(test)]
    pub(super) fn with_initial_loading_for_test() -> Self {
        let mut page = Self::new(AppService::for_tests());
        page.loading = true;
        page
    }

    #[cfg(test)]
    pub(super) fn with_snapshot_loading_for_test(snapshot: BacklogSnapshot) -> Self {
        let mut page = Self::with_snapshot_for_test(snapshot);
        page.loading = true;
        page
    }

    #[cfg(test)]
    pub(super) fn view_for_test(&mut self) -> &mut BacklogView {
        &mut self.view
    }

    #[cfg(test)]
    pub(super) fn move_is_locked_for_test(&self) -> bool {
        self.move_locked.get()
    }

    #[cfg(test)]
    pub(super) fn is_ranking_for_test(&self) -> bool {
        self.ranking
    }

    #[cfg(test)]
    pub(super) fn is_loading_for_test(&self) -> bool {
        self.loading
    }

    #[cfg(test)]
    pub(super) fn is_status_loading_for_test(&self) -> bool {
        self.status_loading
    }

    #[cfg(test)]
    pub(super) fn begin_rank_result_for_test(
        &mut self,
        plan: RankPlan,
        pending_rank: PendingRank,
    ) -> u64 {
        self.pending_rank = Some(pending_rank);
        self.move_locked.set(true);
        self.ranking = true;
        self.active_rank_plan = Some(plan);
        self.generations.start_rank()
    }

    #[cfg(test)]
    pub(super) fn apply_rank_result_for_test(
        &mut self,
        generation: u64,
        result: Result<(), String>,
    ) -> bool {
        self.apply_rank_result(generation, result)
    }

    #[cfg(test)]
    pub(super) fn has_pending_rank_for_test(&self) -> bool {
        self.pending_rank.is_some()
    }

    #[cfg(test)]
    pub(super) fn has_active_rank_plan_for_test(&self) -> bool {
        self.active_rank_plan.is_some()
    }

    #[cfg(test)]
    pub(super) fn rank_refresh_retry_is_pending_for_test(&self) -> bool {
        self.rank_refresh_retry.pending()
    }

    #[cfg(test)]
    pub(super) fn refresh_snapshot_for_test(&mut self, snapshot: BacklogSnapshot) {
        self.focus_backlog_after_load = true;
        let generation = self.generations.start_load(false, false);
        assert!(self.apply_load_result(generation, Ok(snapshot)));
    }

    fn load(&mut self, rank_refresh: bool, preserve_optimistic_view: bool) {
        if !rank_refresh {
            self.rank_refresh_retry.cancel();
        }
        let generation = self
            .generations
            .start_load(rank_refresh, preserve_optimistic_view);
        self.loading = true;
        self.view.base_mut().base_mut().set_loading(true);
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-backlog".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::Loaded {
                    generation,
                    result: service.jira_backlog(),
                });
            })
        {
            let completion = self.generations.complete_load(generation);
            if let Some(completion) = completion {
                self.loading = false;
                self.view.base_mut().base_mut().set_loading(false);
                let error = format!("Could not load Jira backlog: {error}");
                self.handle_load_failure(completion, error.clone(), error);
                self.reload_notification_pending = false;
                self.finish_requested_backlog_focus();
            }
        }
    }

    fn load_issue_types(&mut self) {
        if self.issue_types_requested {
            return;
        }
        self.issue_types_requested = true;
        let generation = self.generations.start_issue_types_load();
        self.issue_types_loading = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-issue-types".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::IssueTypesLoaded {
                    generation,
                    result: service.jira_project_issue_types(),
                });
            })
            && self.generations.complete_issue_types_load(generation)
        {
            self.issue_types_loading = false;
            self.service
                .report_error(format!("Could not load Jira issue types: {error}"));
        }
    }

    fn shows_initial_loading(&self) -> bool {
        self.loading
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.try_recv() {
            changed |= match result {
                BacklogResult::Loaded { generation, result } => {
                    self.apply_load_result(generation, result)
                }
                BacklogResult::IssueTypesLoaded { generation, result } => {
                    self.apply_issue_types_result(generation, result)
                }
                BacklogResult::Ranked { generation, result } => {
                    self.apply_rank_result(generation, result)
                }
                BacklogResult::Transferred {
                    generation,
                    destination,
                    result,
                } => self.apply_transfer_result(generation, destination, result),
                BacklogResult::StatusesLoaded {
                    generation,
                    keys,
                    result,
                } => self.apply_statuses_result(generation, keys, result),
                BacklogResult::StatusSet {
                    generation,
                    status,
                    result,
                } => self.apply_status_result(generation, status, result),
                BacklogResult::AssigneesLoaded { generation, result } => {
                    self.apply_assignees_result(generation, result)
                }
                BacklogResult::UsersAssigned {
                    generation,
                    keys,
                    assignee,
                    result,
                } => self.apply_users_assigned_result(generation, keys, assignee, result),
                BacklogResult::CurrentUserLoaded {
                    generation,
                    keys,
                    result,
                } => self.apply_current_user_result(generation, keys, result),
            };
        }
        changed
    }

    fn apply_issue_types_result(
        &mut self,
        generation: u64,
        result: Result<Vec<JiraOption>, String>,
    ) -> bool {
        if !self.generations.complete_issue_types_load(generation) {
            return false;
        }
        self.issue_types_loading = false;
        match result {
            Ok(issue_types) => self.view.base_mut().base_mut().set_issue_types(issue_types),
            Err(error) => self
                .service
                .report_error(format!("Could not load Jira issue types: {error}")),
        }
        true
    }

    fn apply_load_result(
        &mut self,
        generation: u64,
        result: Result<BacklogSnapshot, String>,
    ) -> bool {
        let Some(completion) = self.generations.complete_load(generation) else {
            return false;
        };
        self.loading = false;
        self.view.base_mut().base_mut().set_loading(false);
        let reload_notification_pending = std::mem::take(&mut self.reload_notification_pending);
        match result {
            Ok(snapshot) => {
                let preserves_optimistic_view = matches!(
                    completion,
                    LoadCompletion::RankRefresh {
                        preserve_optimistic_view: true
                    }
                );
                let highlight = if preserves_optimistic_view && self.pending_transfer.is_some() {
                    let transfer = self
                        .pending_transfer
                        .as_ref()
                        .expect("optimistic transfer exists")
                        .clone();
                    let reconciliation = reconcile_pending_transfer(
                        self.snapshot
                            .as_mut()
                            .expect("optimistic transfer has a snapshot"),
                        &mut self.pending_transfer,
                        snapshot,
                    );
                    match reconciliation {
                        PendingTransferReconciliation::Unconfirmed => {
                            self.rank_refresh_retry.schedule(true);
                            return true;
                        }
                        PendingTransferReconciliation::Exhausted => {
                            self.move_locked.set(false);
                            self.rank_refresh_retry.cancel();
                            self.service.report_error(
                                "Jira did not verify the ticket move after refresh retries".into(),
                            );
                            None
                        }
                        PendingTransferReconciliation::ConfirmedDestination
                        | PendingTransferReconciliation::ConfirmedSourceRollback => {
                            self.move_locked.set(false);
                            transfer_reconciliation_highlight(reconciliation, &transfer)
                        }
                    }
                } else if preserves_optimistic_view && self.pending_rank.is_some() {
                    let reconciliation = reconcile_pending_rank(
                        self.snapshot
                            .as_mut()
                            .expect("optimistic rank has a snapshot"),
                        &mut self.pending_rank,
                        snapshot,
                    );
                    match reconciliation {
                        PendingRankReconciliation::Unconfirmed => {
                            self.rank_refresh_retry.schedule(true);
                            return true;
                        }
                        PendingRankReconciliation::Exhausted => {
                            self.move_locked.set(false);
                            self.rank_refresh_retry.cancel();
                            self.service.report_error(
                                "Jira did not verify the ticket rank after refresh retries".into(),
                            );
                            None
                        }
                        PendingRankReconciliation::Confirmed => {
                            self.move_locked.set(false);
                            None
                        }
                    }
                } else {
                    self.snapshot = Some(snapshot.clone());
                    if matches!(completion, LoadCompletion::RankRefresh { .. }) {
                        self.move_locked.set(false);
                    }
                    None
                };
                self.rank_refresh_retry.cancel();
                let snapshot = self
                    .snapshot
                    .as_ref()
                    .expect("successful backlog load has a snapshot");
                for warning in &snapshot.warnings {
                    self.service
                        .report_notification(tuicore::Notification::warning(
                            "Jira backlog warning",
                            warning.clone(),
                        ));
                }
                if !preserves_optimistic_view
                    || (self.pending_transfer.is_none() && self.pending_rank.is_none())
                {
                    self.view.base_mut().base_mut().set_snapshot(snapshot);
                    if let Some((_, row_id)) = highlight {
                        self.view.base_mut().base_mut().highlight(&row_id);
                    }
                }
                if reload_notification_pending {
                    self.service
                        .report_notification(tuicore::Notification::success(
                            "Backlog reloaded",
                            "Reloaded the backlog and sprints from Jira",
                        ));
                }
            }
            Err(error) => {
                self.handle_load_failure(
                    completion,
                    format!("Could not load Jira backlog: {error}"),
                    error,
                );
            }
        }
        self.finish_requested_backlog_focus();
        true
    }

    fn reload(&mut self) {
        self.focus_backlog_after_load = true;
        self.reload_notification_pending = true;
        self.load(false, false);
    }

    fn finish_requested_backlog_focus(&mut self) {
        if self.focus_backlog_after_load {
            self.focus_backlog_after_load = false;
            self.queue_backlog_data_focus();
        }
    }

    fn queue_backlog_data_focus(&mut self) {
        self.pending_focus = Some(FocusRequest::Target(FocusId::new("data-view")));
    }

    fn focus_backlog_data(&mut self, ctx: &mut EventCtx<()>) {
        ctx.focus(FocusRequest::TargetAt {
            path: self.data_focus_path.clone(),
            id: FocusId::new("data-view"),
        });
    }

    fn handle_load_failure(
        &mut self,
        completion: LoadCompletion,
        reported_error: String,
        view_error: String,
    ) {
        self.service.report_error(reported_error);
        if let LoadCompletion::RankRefresh {
            preserve_optimistic_view,
        } = completion
        {
            self.rank_refresh_retry.schedule(preserve_optimistic_view);
            return;
        }
        let _ = view_error;
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.view.base_mut().base_mut().set_snapshot(snapshot);
        }
    }

    fn apply_rank_result(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if !self.generations.complete_rank(generation) {
            return false;
        }
        self.ranking = false;
        match result {
            Ok(()) => {
                let plan = self
                    .active_rank_plan
                    .as_ref()
                    .expect("completed Jira rank has an active plan");
                self.report_rank_success(plan);
                self.active_rank_plan = None;
                self.pending_rank = None;
                self.move_locked.set(false);
                self.view.base_mut().base_mut().set_loading(false);
            }
            Err(error) => {
                self.active_rank_plan = None;
                self.restore_rank_snapshot();
                self.service
                    .report_error(format!("Could not rank Jira backlog: {error}"));
                self.load(true, false);
            }
        }
        true
    }

    fn drain_section_events(&mut self, ctx: &mut EventCtx<()>) -> bool {
        let mut changed = false;
        while let Ok(event) = self.section_receiver.try_recv() {
            match event {
                BacklogSectionEvent::Refresh => {
                    if !self.loading
                        && !self.ranking
                        && self.syncing_ticket_keys.borrow().is_empty()
                    {
                        self.reload();
                    }
                }
                BacklogSectionEvent::EstimatedChanged(estimated) => {
                    self.view.base_mut().base_mut().set_estimated(estimated);
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::IssueTypesChanged(issue_types) => {
                    self.view
                        .base_mut()
                        .base_mut()
                        .set_issue_types_filter(issue_types);
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::UsersChanged(users) => {
                    self.view.base_mut().base_mut().set_users_filter(users);
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::OpenVelocity => self.open_velocity_dialog(ctx),
                BacklogSectionEvent::OpenReports => {
                    self.service.open_jira_board_page(Some("reports"));
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::OpenTimeline => {
                    self.service.open_jira_board_page(Some("timeline"));
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::OpenBoard => {
                    self.service.open_jira_board_page(None);
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::OpenReleases => {
                    self.service.open_jira_releases();
                    self.focus_backlog_data(ctx);
                }
                BacklogSectionEvent::WebMenuClosed => self.focus_backlog_data(ctx),
                BacklogSectionEvent::MoveLocked => self.report_move_locked(),
                BacklogSectionEvent::TicketsSyncing { keys } => self.report_ticket_syncing(&keys),
                BacklogSectionEvent::OpenTicket { key } => self.service.open_jira_issue(&key),
                BacklogSectionEvent::YankTicketUrl { key } => {
                    self.copy_jira_url(&key, ctx);
                }
                BacklogSectionEvent::YankSprintGoal { goal } => {
                    ctx.copy_to_clipboard(goal);
                }
                BacklogSectionEvent::YankSprintReport { sprint_id } => {
                    let Some(sprint) = self.snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .sprints
                            .iter()
                            .find(|sprint| sprint.id == sprint_id)
                    }) else {
                        continue;
                    };
                    copy_report(
                        &self.service,
                        VelocitySprint {
                            id: sprint.id,
                            name: sprint.name.clone(),
                            goal: sprint.goal.clone(),
                            completed: 0.0,
                            work_items: None,
                        },
                    );
                }
                BacklogSectionEvent::OpenQuickMenu {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                        continue;
                    }
                    let (status, assignee) = quick_menu_labels(self.snapshot.as_ref(), &keys);
                    if !self.view.base_mut().layer_mut().open(
                        section_id.clone(),
                        keys,
                        source_order,
                        status,
                        assignee,
                        transfer_destinations(self.snapshot.as_ref(), &section_id),
                        ctx,
                    ) {
                        self.report_move_locked();
                        continue;
                    }
                    self.view.base_mut().set_active_with_context(true, ctx);
                }
                BacklogSectionEvent::OpenStatusMenu {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if !self.view.base_mut().layer_mut().open_status_menu(
                        section_id.clone(),
                        keys,
                        source_order,
                        ctx,
                    ) {
                        self.report_move_locked();
                        continue;
                    }
                    self.view.base_mut().set_active_with_context(true, ctx);
                }
                BacklogSectionEvent::OpenAssignMenu {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if !self.view.base_mut().layer_mut().open_assign_menu(
                        section_id,
                        keys,
                        source_order,
                        ctx,
                    ) {
                        self.report_move_locked();
                        continue;
                    }
                    self.view.base_mut().set_active_with_context(true, ctx);
                }
                BacklogSectionEvent::ToggleCurrentUser { keys } => self.assign_current_user(keys),
                BacklogSectionEvent::Moved {
                    section_id,
                    moved_keys,
                    final_order,
                } => self.rank(section_id, moved_keys, final_order),
                BacklogSectionEvent::Rejected {
                    section_id,
                    message,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                        continue;
                    }
                    self.service
                        .report_error(format!("Could not rank {section_id}: {message}"));
                    self.restore_snapshot();
                    self.load(false, false);
                }
            }
            changed = true;
        }
        changed
    }

    fn drain_quick_menu_events(&mut self, ctx: &mut EventCtx<()>) -> bool {
        let events = self.view.base_mut().layer_mut().take_events();
        let changed = !events.is_empty();
        for event in events {
            match event {
                BacklogQuickMenuEvent::LoadStatuses { keys } => self.load_statuses(keys),
                BacklogQuickMenuEvent::SetStatus { status } => {
                    self.set_status(status);
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::LoadAssignees => self.load_assignees(),
                BacklogQuickMenuEvent::AssignUser { keys, assignee } => {
                    self.assign_users(keys, assignee);
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveToTop {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                    } else {
                        self.move_from_menu(section_id, keys, source_order, true);
                    }
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveToBottom {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                    } else {
                        self.move_from_menu(section_id, keys, source_order, false);
                    }
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveToSection {
                    source_section_id,
                    destination,
                    keys,
                    to_top,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                    } else {
                        self.transfer_to_section(source_section_id, destination, keys, to_top);
                    }
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveLocked => {
                    self.report_move_locked();
                    self.view.base_mut().set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::Closed => self.dismiss_quick_menu(ctx),
            }
        }
        changed
    }

    fn drain_events(&mut self, ctx: &mut EventCtx<()>) -> bool {
        let mut changed = self.drain_quick_menu_events(ctx);
        changed |= self.drain_section_events(ctx);
        changed |= self.drain_quick_menu_events(ctx);
        changed
    }

    fn load_statuses(&mut self, keys: Vec<String>) {
        if self.status_loading {
            return;
        }
        let missing_keys = self.status_transition_cache.missing_keys(&keys);
        if missing_keys.is_empty() {
            self.show_statuses(
                self.status_transition_cache
                    .common(&keys)
                    .unwrap_or_default(),
            );
            return;
        }
        let generation = self.generations.start_status_load();
        self.status_loading = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-statuses".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::StatusesLoaded {
                    generation,
                    keys,
                    result: service.jira_status_transitions_by_issue(&missing_keys),
                });
            })
            && self.generations.complete_status_load(generation)
        {
            self.status_loading = false;
            self.view.base_mut().set_active(false);
            self.queue_backlog_data_focus();
            self.service
                .report_error(format!("Could not load Jira status transitions: {error}"));
        }
    }

    fn apply_statuses_result(
        &mut self,
        generation: u64,
        keys: Vec<String>,
        result: Result<Vec<(String, Vec<JiraOption>)>, String>,
    ) -> bool {
        if !self.generations.complete_status_load(generation) {
            return false;
        }
        self.status_loading = false;
        match result {
            Ok(transitions) => {
                self.status_transition_cache.insert(transitions);
                self.show_statuses(
                    self.status_transition_cache
                        .common(&keys)
                        .unwrap_or_default(),
                );
            }
            Err(error) => {
                self.view.base_mut().set_active(false);
                self.queue_backlog_data_focus();
                self.service
                    .report_error(format!("Could not load Jira status transitions: {error}"));
            }
        }
        true
    }

    fn show_statuses(&mut self, statuses: Vec<StatusTransition>) {
        if statuses.is_empty() {
            self.view.base_mut().set_active(false);
            self.queue_backlog_data_focus();
            self.service.report_error(
                "Could not set status: the selected tickets have no common Jira transition".into(),
            );
        } else {
            self.view.base_mut().layer_mut().set_statuses(statuses);
        }
    }

    fn load_assignees(&mut self) {
        if let Some(assignees) = self.assignees.as_ref() {
            self.view
                .base_mut()
                .layer_mut()
                .set_assignees(assignees.clone());
            return;
        }
        if self.assignees_loading {
            return;
        }
        let generation = self.generations.start_assignees_load();
        self.assignees_loading = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-users".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::AssigneesLoaded {
                    generation,
                    result: service.jira_default_project_assignees(),
                });
            })
            && self.generations.complete_assignees_load(generation)
        {
            self.assignees_loading = false;
            self.view.base_mut().set_active(false);
            self.queue_backlog_data_focus();
            self.service
                .report_error(format!("Could not load Jira users: {error}"));
        }
    }

    fn assign_current_user(&mut self, keys: Vec<String>) {
        if keys.is_empty() || self.current_user_loading || self.tickets_are_syncing(&keys) {
            self.report_ticket_syncing(&keys);
            return;
        }
        if let Some(user) = self.current_user.clone() {
            self.assign_users(
                keys.clone(),
                current_user_assignment(self.snapshot.as_ref(), &keys, &user),
            );
            return;
        }
        let generation = self.generations.start_current_user_load();
        self.current_user_loading = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-current-user".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::CurrentUserLoaded {
                    generation,
                    keys,
                    result: service.jira_current_user(),
                });
            })
            && self.generations.complete_current_user_load(generation)
        {
            self.current_user_loading = false;
            self.service
                .report_error(format!("Could not load the current Jira user: {error}"));
        }
    }

    fn apply_current_user_result(
        &mut self,
        generation: u64,
        keys: Vec<String>,
        result: Result<JiraAssignee, String>,
    ) -> bool {
        if !self.generations.complete_current_user_load(generation) {
            return false;
        }
        self.current_user_loading = false;
        match result {
            Ok(user) => {
                let assignee = current_user_assignment(self.snapshot.as_ref(), &keys, &user);
                self.current_user = Some(user);
                self.assign_users(keys, assignee);
            }
            Err(error) => self
                .service
                .report_error(format!("Could not load the current Jira user: {error}")),
        }
        true
    }

    fn dismiss_quick_menu(&mut self, ctx: &mut EventCtx<()>) {
        self.generations.cancel_status_load();
        self.generations.cancel_assignees_load();
        self.status_loading = false;
        self.assignees_loading = false;
        self.view.base_mut().set_active_with_context(false, ctx);
        self.queue_backlog_data_focus();
    }

    fn apply_assignees_result(
        &mut self,
        generation: u64,
        result: Result<Vec<JiraAssignee>, String>,
    ) -> bool {
        if !self.generations.complete_assignees_load(generation) {
            return false;
        }
        self.assignees_loading = false;
        match result {
            Ok(users) => {
                let mut assignees = Vec::with_capacity(users.len() + 1);
                assignees.push(BacklogAssignee {
                    account_id: String::new(),
                    display_name: "Unassigned".into(),
                });
                assignees.extend(users.into_iter().map(|user| BacklogAssignee {
                    account_id: user.account_id,
                    display_name: user.display_name,
                }));
                self.assignees = Some(assignees.clone());
                self.view.base_mut().layer_mut().set_assignees(assignees);
            }
            Err(error) => {
                self.view.base_mut().set_active(false);
                self.queue_backlog_data_focus();
                self.service
                    .report_error(format!("Could not load Jira users: {error}"));
            }
        }
        true
    }

    fn assign_users(&mut self, keys: Vec<String>, assignee: BacklogAssignee) {
        if keys.is_empty() {
            return;
        }
        if self.tickets_are_syncing(&keys) {
            self.report_ticket_syncing(&keys);
            return;
        }
        let Some(pending_change) = self.show_optimistic_assignee(&keys, &assignee.display_name)
        else {
            self.service
                .report_error("Could not assign users: selected tickets are unavailable".into());
            return;
        };
        let generation = self.generations.start_users_assign();
        self.pending_assignee_changes
            .insert(generation, pending_change);
        self.begin_ticket_sync(&keys);
        let sync_keys = keys.clone();
        let service = self.service.clone();
        let sender = self.sender.clone();
        let result_assignee = assignee.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-assign-users".into())
            .spawn(move || {
                let account_id =
                    (!assignee.account_id.is_empty()).then_some(assignee.account_id.clone());
                let result = service.jira_assign_users(&keys, account_id.as_deref());
                let _ = sender.send(BacklogResult::UsersAssigned {
                    generation,
                    keys,
                    assignee,
                    result,
                });
            })
            && self.generations.complete_users_assign(generation)
        {
            self.restore_assignee_change(generation);
            self.finish_ticket_sync(&sync_keys);
            self.service.report_error(format!(
                "Could not start Jira assignment to {}: {error}",
                result_assignee.display_name
            ));
        }
    }

    fn apply_users_assigned_result(
        &mut self,
        generation: u64,
        keys: Vec<String>,
        assignee: BacklogAssignee,
        result: Result<(), String>,
    ) -> bool {
        if !self.generations.complete_users_assign(generation) {
            return false;
        }
        self.finish_ticket_sync(&keys);
        match result {
            Ok(()) => {
                self.pending_assignee_changes.remove(&generation);
                let message = match keys.as_slice() {
                    [key] => format!("{key} assigned to {}", assignee.display_name),
                    _ => format!(
                        "{} tickets assigned to {}",
                        keys.len(),
                        assignee.display_name
                    ),
                };
                self.service
                    .report_notification(tuicore::Notification::success(
                        "Jira assignee updated",
                        message,
                    ));
            }
            Err(error) => {
                self.restore_assignee_change(generation);
                self.service
                    .report_error(format!("Could not assign Jira users: {error}"));
            }
        }
        true
    }

    fn set_status(&mut self, status: StatusTransition) {
        let keys = status
            .issues
            .iter()
            .map(|issue| issue.issue_key.clone())
            .collect::<Vec<_>>();
        if self.tickets_are_syncing(&keys) {
            self.report_ticket_syncing(&keys);
            return;
        }
        let Some(pending_change) = self.show_optimistic_status(&status) else {
            self.service
                .report_error("Could not set status: selected tickets are unavailable".into());
            return;
        };
        let generation = self.generations.start_status_set();
        self.pending_status_changes
            .insert(generation, pending_change);
        self.begin_ticket_sync(&keys);
        let service = self.service.clone();
        let sender = self.sender.clone();
        let result_status = status.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-set-status".into())
            .spawn(move || {
                let result = service.jira_set_status(&status);
                let _ = sender.send(BacklogResult::StatusSet {
                    generation,
                    status,
                    result,
                });
            })
            && self.generations.complete_status_set(generation)
        {
            self.restore_status_change(generation);
            self.finish_ticket_sync(&keys);
            self.service.report_error(format!(
                "Could not start Jira transition to {}: {error}",
                result_status.label
            ));
        }
    }

    fn apply_status_result(
        &mut self,
        generation: u64,
        status: StatusTransition,
        result: Result<(), String>,
    ) -> bool {
        if !self.generations.complete_status_set(generation) {
            return false;
        }
        let keys = status
            .issues
            .iter()
            .map(|issue| issue.issue_key.clone())
            .collect::<Vec<_>>();
        self.finish_ticket_sync(&keys);
        self.status_transition_cache.invalidate(&status);
        match result {
            Ok(()) => {
                self.pending_status_changes.remove(&generation);
                let message = match status.issues.as_slice() {
                    [issue] => format!("{} changed to {}", issue.issue_key, status.label),
                    issues => format!("{} tickets changed to {}", issues.len(), status.label),
                };
                self.service
                    .report_notification(tuicore::Notification::success(
                        "Jira status updated",
                        message,
                    ));
            }
            Err(error) => {
                self.restore_status_change(generation);
                self.service
                    .report_error(format!("Could not set Jira status: {error}"));
                if self.syncing_ticket_keys.borrow().is_empty() {
                    self.load(false, false);
                }
            }
        }
        true
    }

    fn show_optimistic_status(&mut self, status: &StatusTransition) -> Option<PendingStatusChange> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return None;
        };
        let keys = status
            .issues
            .iter()
            .map(|issue| issue.issue_key.clone())
            .collect::<Vec<_>>();
        let original_items = ticket_items_by_key(snapshot, &keys)?;
        let mut optimistic = snapshot.clone();
        if !apply_status_to_snapshot(&mut optimistic, &keys, &status.label) {
            return None;
        }
        if let Ok(settings) = self.service.settings().read() {
            recalculate_capacity(&mut optimistic, &settings.backlog_runway);
        }
        self.snapshot = Some(optimistic.clone());
        self.view.base_mut().base_mut().set_snapshot(&optimistic);
        Some(PendingStatusChange { original_items })
    }

    fn restore_status_change(&mut self, generation: u64) {
        let Some(pending) = self.pending_status_changes.remove(&generation) else {
            return;
        };
        self.restore_ticket_items(pending.original_items);
    }

    fn show_optimistic_assignee(
        &mut self,
        keys: &[String],
        assignee: &str,
    ) -> Option<PendingAssigneeChange> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return None;
        };
        let original_items = ticket_items_by_key(snapshot, keys)?;
        let mut optimistic = snapshot.clone();
        if !apply_assignee_to_snapshot(&mut optimistic, keys, assignee) {
            return None;
        }
        self.snapshot = Some(optimistic.clone());
        self.view.base_mut().base_mut().set_snapshot(&optimistic);
        Some(PendingAssigneeChange { original_items })
    }

    fn restore_assignee_change(&mut self, generation: u64) {
        let Some(pending) = self.pending_assignee_changes.remove(&generation) else {
            return;
        };
        self.restore_ticket_items(pending.original_items);
    }

    fn restore_ticket_items(&mut self, originals: HashMap<String, WorkItem>) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        for item in snapshot
            .sprints
            .iter_mut()
            .flat_map(|sprint| &mut sprint.work_items)
            .chain(&mut snapshot.work_items)
        {
            if let Some(original) = originals.get(&item.key) {
                *item = original.clone();
            }
        }
        self.restore_snapshot();
    }

    fn tickets_are_syncing(&self, keys: &[String]) -> bool {
        let syncing = self.syncing_ticket_keys.borrow();
        keys.iter().any(|key| syncing.contains(key))
    }

    fn begin_ticket_sync(&mut self, keys: &[String]) {
        self.syncing_ticket_keys
            .borrow_mut()
            .extend(keys.iter().cloned());
        self.refresh_ticket_sync_indicators();
    }

    fn finish_ticket_sync(&mut self, keys: &[String]) {
        let mut syncing = self.syncing_ticket_keys.borrow_mut();
        for key in keys {
            syncing.remove(key);
        }
        drop(syncing);
        self.refresh_ticket_sync_indicators();
    }

    fn refresh_ticket_sync_indicators(&mut self) {
        self.view.base_mut().base_mut().refresh_syncing_tickets();
    }

    fn report_ticket_syncing(&self, keys: &[String]) {
        let syncing = self.syncing_ticket_keys.borrow();
        let ticket = keys.iter().find(|key| syncing.contains(*key));
        let message = ticket.map_or_else(
            || "Could not update tickets: a selected ticket is still syncing with Jira".into(),
            |key| format!("Could not update {key}: it is still syncing with Jira"),
        );
        self.service.report_error(message);
    }

    fn open_velocity_dialog(&mut self, ctx: &mut EventCtx<()>) {
        let settings = self.service.settings();
        let settings = settings.read().expect("settings lock poisoned");
        self.velocity_dialog_close_requested.set(false);
        self.view.replace_layer(
            velocity_dialog(
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.velocity.as_ref()),
                &settings.backlog_runway,
                self.snapshot.as_ref().and_then(loaded_story_point_average),
                Rc::clone(&self.velocity_dialog_close_requested),
                Some(self.service.clone()),
            ),
            ctx,
        );
        self.view.set_active_with_context(true, ctx);
    }

    fn copy_jira_url(&self, key: &str, ctx: &mut EventCtx<()>) {
        let url = self
            .service
            .settings()
            .read()
            .ok()
            .and_then(|settings| settings.jira_issue_url(key));
        if let Some(url) = url {
            ctx.copy_to_clipboard(url);
        } else {
            self.service
                .report_error("Could not copy Jira URL: Jira URL is not configured".into());
        }
    }

    fn close_velocity_dialog(&mut self, ctx: &mut EventCtx<()>) {
        if self.velocity_dialog_close_requested.replace(false) {
            self.view.set_active_with_context(false, ctx);
            self.focus_backlog_data(ctx);
        }
    }

    pub(super) fn move_from_menu(
        &mut self,
        section_id: String,
        keys: Vec<String>,
        mut final_order: Vec<String>,
        to_top: bool,
    ) {
        if self.move_locked.get() {
            self.report_move_locked();
            return;
        }
        if self.tickets_are_syncing(&keys) {
            self.report_ticket_syncing(&keys);
            return;
        }
        if keys.is_empty() || !keys.iter().all(|key| final_order.contains(key)) {
            return;
        }
        final_order.retain(|item| !keys.contains(item));
        if to_top {
            final_order.splice(0..0, keys.iter().cloned());
        } else {
            final_order.extend(keys.iter().cloned());
        }
        if final_order == source_order(self.snapshot.as_ref(), &section_id) {
            self.restore_snapshot();
            return;
        }
        self.rank(section_id, keys, final_order);
    }

    fn transfer_to_section(
        &mut self,
        source_section_id: String,
        destination: BacklogDestination,
        keys: Vec<String>,
        to_top: bool,
    ) {
        if self.move_locked.get() {
            self.report_move_locked();
            return;
        }
        if self.tickets_are_syncing(&keys) {
            self.report_ticket_syncing(&keys);
            return;
        }
        if keys.is_empty() {
            return;
        }
        if keys.len() > crate::store::work_items::MAX_RANK_ISSUES {
            self.service.report_error(format!(
                "Could not move tickets: Jira can move at most {} issues at once",
                crate::store::work_items::MAX_RANK_ISSUES
            ));
            return;
        }
        let mut destination_order = source_order(self.snapshot.as_ref(), &destination.section_id);
        if to_top {
            destination_order.splice(0..0, keys.iter().cloned());
        } else {
            destination_order.extend(keys.iter().cloned());
        }
        let placement_plan = match rank_plan(keys.clone(), &destination_order) {
            Ok(plan) => plan,
            Err(error) => {
                self.service
                    .report_error(format!("Could not rank {}: {error}", destination.label));
                return;
            }
        };
        if !self.show_optimistic_transfer(
            &source_section_id,
            &destination.section_id,
            &keys,
            to_top,
            destination_order,
        ) {
            return;
        }
        self.move_locked.set(true);
        let generation = self.generations.start_rank();
        self.loading = false;
        self.view.base_mut().base_mut().set_loading(true);
        self.rank_refresh_retry.cancel();
        self.ranking = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        let destination_label = destination.label.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-transfer".into())
            .spawn(move || {
                let sprint_id = (destination.section_id != "backlog")
                    .then(|| {
                        destination
                            .section_id
                            .strip_prefix("sprint-")
                            .ok_or_else(|| "Unknown sprint destination".to_string())
                            .and_then(|id| {
                                id.parse::<u64>()
                                    .map_err(|_| "Invalid sprint destination".to_string())
                            })
                    })
                    .transpose();
                let result = sprint_id.and_then(|sprint_id| {
                    service.jira_transfer(sprint_id, &keys, placement_plan.as_ref())
                });
                let _ = sender.send(BacklogResult::Transferred {
                    generation,
                    destination: destination_label,
                    result,
                });
            })
        {
            if self.generations.complete_rank(generation) {
                self.ranking = false;
                self.view.base_mut().base_mut().set_loading(false);
                self.move_locked.set(false);
                self.restore_transfer_snapshot();
                self.service
                    .report_error(format!("Could not start Jira ticket move: {error}"));
            }
        }
    }

    fn apply_transfer_result(
        &mut self,
        generation: u64,
        destination: String,
        result: Result<(), String>,
    ) -> bool {
        if !self.generations.complete_rank(generation) {
            return false;
        }
        self.ranking = false;
        self.view.base_mut().base_mut().set_loading(false);
        self.move_locked.set(false);
        match result {
            Ok(()) => {
                self.pending_transfer = None;
                self.service
                    .report_notification(tuicore::Notification::success(
                        "Jira tickets moved",
                        format!("Moved tickets to {destination}"),
                    ));
            }
            Err(error) => {
                self.restore_transfer_snapshot();
                self.service
                    .report_error(format!("Could not move Jira tickets: {error}"));
            }
        }
        true
    }

    fn show_optimistic_transfer(
        &mut self,
        source_section_id: &str,
        destination_section_id: &str,
        keys: &[String],
        to_top: bool,
        destination_order: Vec<String>,
    ) -> bool {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let source_highlight_key =
            source_transfer_highlight_key(&source_order(Some(snapshot), source_section_id), keys);
        let (_, highlighted_row_id) =
            source_transfer_highlight(source_section_id, source_highlight_key.as_deref());
        let mut optimistic = snapshot.clone();
        if !move_work_items_to_edge(
            &mut optimistic,
            source_section_id,
            destination_section_id,
            keys,
            to_top,
        ) {
            return false;
        }
        if let Ok(settings) = self.service.settings().read() {
            recalculate_capacity(&mut optimistic, &settings.backlog_runway);
        }
        self.pending_transfer = Some(PendingTransfer {
            rollback_snapshot: snapshot.clone(),
            source_section_id: source_section_id.into(),
            destination_section_id: destination_section_id.into(),
            destination_order,
            keys: keys.to_vec(),
            source_highlight_key,
            ambiguous: false,
            unconfirmed_refreshes: 0,
        });
        self.snapshot = Some(optimistic.clone());
        self.view.base_mut().base_mut().set_snapshot(&optimistic);
        self.view
            .base_mut()
            .base_mut()
            .highlight(&highlighted_row_id);
        true
    }

    fn restore_transfer_snapshot(&mut self) {
        let Some(pending_transfer) = self.pending_transfer.take() else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        if !restore_transfer_sections(snapshot, &pending_transfer) {
            return;
        }
        if let Ok(settings) = self.service.settings().read() {
            recalculate_capacity(snapshot, &settings.backlog_runway);
        }
        self.restore_snapshot();
    }

    fn show_optimistic_order(
        &mut self,
        section_id: &str,
        order: &[String],
    ) -> Option<BacklogSnapshot> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return None;
        };
        let rollback_snapshot = snapshot.clone();
        let mut optimistic = rollback_snapshot.clone();
        if section_id == "backlog" {
            sort_work_items(&mut optimistic.work_items, order);
        } else if let Some(sprint) = optimistic
            .sprints
            .iter_mut()
            .find(|sprint| format!("sprint-{}", sprint.id) == section_id)
        {
            sort_work_items(&mut sprint.work_items, order);
        }
        self.snapshot = Some(optimistic.clone());
        self.view.base_mut().base_mut().set_snapshot(&optimistic);
        Some(rollback_snapshot)
    }

    fn rank(&mut self, section_id: String, moved_keys: Vec<String>, final_order: Vec<String>) {
        if self.move_locked.get() {
            self.report_move_locked();
            return;
        }
        if self.tickets_are_syncing(&moved_keys) {
            self.report_ticket_syncing(&moved_keys);
            return;
        }
        let plan = match rank_plan(moved_keys, &final_order) {
            Ok(Some(plan)) => plan,
            Ok(None) => return,
            Err(error) => {
                self.service
                    .report_error(format!("Could not rank {section_id}: {error}"));
                self.restore_snapshot();
                self.load(false, false);
                return;
            }
        };
        let Some(rollback_snapshot) = self.show_optimistic_order(&section_id, &final_order) else {
            return;
        };
        self.pending_rank = Some(PendingRank {
            rollback_snapshot,
            section_id,
            final_order,
            unconfirmed_refreshes: 0,
        });
        self.move_locked.set(true);
        self.start_rank(plan);
    }

    fn start_rank(&mut self, plan: RankPlan) {
        let generation = self.generations.start_rank();
        self.loading = false;
        self.view.base_mut().base_mut().set_loading(true);
        self.rank_refresh_retry.cancel();
        self.ranking = true;
        self.active_rank_plan = Some(plan.clone());
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-rank".into())
            .spawn(move || {
                let _ = sender.send(BacklogResult::Ranked {
                    generation,
                    result: service.jira_rank(&plan),
                });
            })
        {
            if self.generations.complete_rank(generation) {
                self.ranking = false;
                self.view.base_mut().base_mut().set_loading(false);
                self.active_rank_plan = None;
                self.restore_rank_snapshot();
                self.move_locked.set(false);
                self.service
                    .report_error(format!("Could not start Jira rank: {error}"));
            }
        }
    }

    fn restore_snapshot(&mut self) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.view.base_mut().base_mut().set_snapshot(snapshot);
        }
    }

    fn restore_rank_snapshot(&mut self) {
        let Some(pending_rank) = self.pending_rank.take() else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let order = source_order(
            Some(&pending_rank.rollback_snapshot),
            &pending_rank.section_id,
        );
        let Some(work_items) = work_items_mut(snapshot, &pending_rank.section_id) else {
            return;
        };
        sort_work_items(work_items, &order);
        self.restore_snapshot();
    }

    fn report_rank_success(&self, plan: &RankPlan) {
        let message = match plan.issues.as_slice() {
            [key] => format!("{key} moved"),
            issues => format!("{} tickets moved", issues.len()),
        };
        self.service
            .report_notification(tuicore::Notification::success(
                "Jira backlog ranked",
                message,
            ));
    }

    fn report_move_locked(&self) {
        self.service.report_error(
            "Could not move tickets: another backlog move is still syncing with Jira".into(),
        );
    }

    fn retry_rank_refresh(&mut self, dt: Duration) -> bool {
        let Some(preserve_optimistic_view) = self.rank_refresh_retry.elapse(dt) else {
            return false;
        };
        self.load(true, preserve_optimistic_view);
        true
    }

    fn refresh_for_settings_change(&mut self) -> bool {
        let settings_revision = self.service.settings_revision();
        if settings_revision == self.settings_revision
            || self.loading
            || self.ranking
            || !self.syncing_ticket_keys.borrow().is_empty()
            || self.current_user_loading
        {
            return false;
        }
        self.settings_revision = settings_revision;
        self.assignees = None;
        self.load_assignees();
        self.load(false, false);
        true
    }
}

pub(super) fn recalculate_capacity(
    snapshot: &mut BacklogSnapshot,
    settings: &BacklogRunwaySettings,
) {
    let Some((capacity, source)) = snapshot
        .runway
        .as_ref()
        .map(|runway| (runway.capacity, runway.source))
    else {
        return;
    };
    let assumed_ticket_size = if settings.use_average_ticket_size {
        loaded_story_point_average(snapshot).map(|size| (size, true))
    } else {
        Some((settings.fixed_ticket_size, false))
    };
    apply_capacity(
        snapshot,
        capacity,
        assumed_ticket_size,
        source,
        settings.sprint_tolerance_percent,
    );
}

pub(super) fn apply_status_to_snapshot(
    snapshot: &mut BacklogSnapshot,
    keys: &[String],
    status: &str,
) -> bool {
    let keys = keys.iter().collect::<std::collections::HashSet<_>>();
    if keys.is_empty()
        || !keys.iter().all(|key| {
            snapshot.work_items.iter().any(|item| &item.key == *key)
                || snapshot
                    .sprints
                    .iter()
                    .flat_map(|sprint| &sprint.work_items)
                    .any(|item| &item.key == *key)
        })
    {
        return false;
    }
    let now = chrono::Utc::now();
    for item in snapshot
        .work_items
        .iter_mut()
        .chain(
            snapshot
                .sprints
                .iter_mut()
                .flat_map(|sprint| &mut sprint.work_items),
        )
        .filter(|item| keys.contains(&item.key))
    {
        item.status = status.to_owned();
        item.done = is_done_status(status);
        item.status_changed_at = Some(now);
    }
    true
}

pub(super) fn apply_assignee_to_snapshot(
    snapshot: &mut BacklogSnapshot,
    keys: &[String],
    assignee: &str,
) -> bool {
    let keys = keys.iter().collect::<std::collections::HashSet<_>>();
    if keys.is_empty()
        || !keys.iter().all(|key| {
            snapshot.work_items.iter().any(|item| &item.key == *key)
                || snapshot
                    .sprints
                    .iter()
                    .flat_map(|sprint| &sprint.work_items)
                    .any(|item| &item.key == *key)
        })
    {
        return false;
    }
    for item in snapshot
        .work_items
        .iter_mut()
        .chain(
            snapshot
                .sprints
                .iter_mut()
                .flat_map(|sprint| &mut sprint.work_items),
        )
        .filter(|item| keys.contains(&item.key))
    {
        item.assignee = assignee.to_owned();
    }
    true
}

fn ticket_items_by_key(
    snapshot: &BacklogSnapshot,
    keys: &[String],
) -> Option<HashMap<String, WorkItem>> {
    let items = snapshot
        .work_items
        .iter()
        .chain(
            snapshot
                .sprints
                .iter()
                .flat_map(|sprint| &sprint.work_items),
        )
        .map(|item| (item.key.clone(), item.clone()))
        .collect::<HashMap<_, _>>();
    keys.iter()
        .map(|key| items.get(key).cloned().map(|item| (key.clone(), item)))
        .collect()
}

fn source_order(snapshot: Option<&BacklogSnapshot>, section_id: &str) -> Vec<String> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let work_items = if section_id == "backlog" {
        &snapshot.work_items
    } else if let Some(sprint) = snapshot
        .sprints
        .iter()
        .find(|sprint| format!("sprint-{}", sprint.id) == section_id)
    {
        &sprint.work_items
    } else {
        return Vec::new();
    };
    work_items.iter().map(|item| item.key.clone()).collect()
}

pub(super) fn reconcile_pending_transfer(
    optimistic_snapshot: &mut BacklogSnapshot,
    pending_transfer: &mut Option<PendingTransfer>,
    refreshed_snapshot: BacklogSnapshot,
) -> PendingTransferReconciliation {
    let Some(transfer) = pending_transfer.as_mut() else {
        return PendingTransferReconciliation::Unconfirmed;
    };
    let destination_confirmed =
        transfer.keys.iter().all(|key| {
            section_contains(&refreshed_snapshot, &transfer.destination_section_id, key)
                && !section_contains(&refreshed_snapshot, &transfer.source_section_id, key)
        }) && source_order(Some(&refreshed_snapshot), &transfer.destination_section_id)
            == transfer.destination_order;
    if destination_confirmed {
        *optimistic_snapshot = refreshed_snapshot;
        pending_transfer.take();
        return PendingTransferReconciliation::ConfirmedDestination;
    }
    let source_confirmed = transfer.ambiguous
        && transfer.keys.iter().all(|key| {
            section_contains(&refreshed_snapshot, &transfer.source_section_id, key)
                && !section_contains(&refreshed_snapshot, &transfer.destination_section_id, key)
        });
    if source_confirmed {
        *optimistic_snapshot = refreshed_snapshot;
        pending_transfer.take();
        return PendingTransferReconciliation::ConfirmedSourceRollback;
    }
    transfer.unconfirmed_refreshes += 1;
    if transfer.unconfirmed_refreshes >= MAX_UNCONFIRMED_TRANSFER_REFRESHES {
        *optimistic_snapshot = refreshed_snapshot;
        pending_transfer.take();
        PendingTransferReconciliation::Exhausted
    } else {
        PendingTransferReconciliation::Unconfirmed
    }
}

pub(super) fn reconcile_pending_rank(
    optimistic_snapshot: &mut BacklogSnapshot,
    pending_rank: &mut Option<PendingRank>,
    refreshed_snapshot: BacklogSnapshot,
) -> PendingRankReconciliation {
    let Some(rank) = pending_rank.as_mut() else {
        return PendingRankReconciliation::Unconfirmed;
    };
    if source_order(Some(&refreshed_snapshot), &rank.section_id) == rank.final_order {
        *optimistic_snapshot = refreshed_snapshot;
        pending_rank.take();
        return PendingRankReconciliation::Confirmed;
    }
    rank.unconfirmed_refreshes += 1;
    if rank.unconfirmed_refreshes >= MAX_UNCONFIRMED_TRANSFER_REFRESHES {
        *optimistic_snapshot = refreshed_snapshot;
        pending_rank.take();
        PendingRankReconciliation::Exhausted
    } else {
        PendingRankReconciliation::Unconfirmed
    }
}

pub(super) fn source_transfer_highlight_key(
    source_order: &[String],
    moved_keys: &[String],
) -> Option<String> {
    let last_moved_index = source_order
        .iter()
        .enumerate()
        .filter_map(|(index, key)| moved_keys.contains(key).then_some(index))
        .last()?;
    source_order[last_moved_index + 1..]
        .iter()
        .find(|key| !moved_keys.contains(key))
        .or_else(|| {
            source_order[..last_moved_index]
                .iter()
                .rev()
                .find(|key| !moved_keys.contains(key))
        })
        .cloned()
}

pub(super) fn transfer_reconciliation_highlight(
    reconciliation: PendingTransferReconciliation,
    transfer: &PendingTransfer,
) -> Option<(String, String)> {
    match reconciliation {
        PendingTransferReconciliation::ConfirmedDestination
        | PendingTransferReconciliation::ConfirmedSourceRollback => {}
        PendingTransferReconciliation::Unconfirmed | PendingTransferReconciliation::Exhausted => {
            return None;
        }
    }
    Some(source_transfer_highlight(
        &transfer.source_section_id,
        transfer.source_highlight_key.as_deref(),
    ))
}

pub(super) fn source_transfer_highlight(
    source_section_id: &str,
    source_highlight_key: Option<&str>,
) -> (String, String) {
    let section_id = source_section_id.to_owned();
    let row_id = source_highlight_key
        .map(|key| format!("ticket:{key}"))
        .unwrap_or_else(|| format!("section:{section_id}"));
    (section_id, row_id)
}

fn section_contains(snapshot: &BacklogSnapshot, section_id: &str, key: &str) -> bool {
    if section_id == "backlog" {
        snapshot.work_items.iter().any(|item| item.key == key)
    } else {
        section_id
            .strip_prefix("sprint-")
            .and_then(|id| id.parse::<u64>().ok())
            .and_then(|sprint_id| {
                snapshot
                    .sprints
                    .iter()
                    .find(|sprint| sprint.id == sprint_id)
            })
            .is_some_and(|sprint| sprint.work_items.iter().any(|item| item.key == key))
    }
}

pub(super) fn quick_menu_labels(
    snapshot: Option<&BacklogSnapshot>,
    keys: &[String],
) -> (String, String) {
    let Some(key) = keys.first() else {
        return (String::new(), "Unassigned".into());
    };
    let item = snapshot.and_then(|snapshot| {
        snapshot
            .work_items
            .iter()
            .chain(
                snapshot
                    .sprints
                    .iter()
                    .flat_map(|sprint| &sprint.work_items),
            )
            .find(|item| item.key == *key)
    });
    item.map_or_else(
        || (String::new(), "Unassigned".into()),
        |item| (item.status.clone(), item.assignee.clone()),
    )
}

pub(super) fn current_user_assignment(
    snapshot: Option<&BacklogSnapshot>,
    keys: &[String],
    current_user: &JiraAssignee,
) -> BacklogAssignee {
    let (_, assignee) = quick_menu_labels(snapshot, keys);
    if assignee.eq_ignore_ascii_case(&current_user.display_name) {
        BacklogAssignee {
            account_id: String::new(),
            display_name: "Unassigned".into(),
        }
    } else {
        BacklogAssignee {
            account_id: current_user.account_id.clone(),
            display_name: current_user.display_name.clone(),
        }
    }
}

pub(super) fn transfer_destinations(
    snapshot: Option<&BacklogSnapshot>,
    current_section_id: &str,
) -> Vec<BacklogDestination> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let mut destinations = Vec::new();
    if current_section_id != "backlog" {
        destinations.push(BacklogDestination {
            section_id: "backlog".into(),
            label: "backlog".into(),
        });
    }
    destinations.extend(
        snapshot
            .sprints
            .iter()
            .filter(|sprint| format!("sprint-{}", sprint.id) != current_section_id)
            .map(|sprint| BacklogDestination {
                section_id: format!("sprint-{}", sprint.id),
                label: format!("{} ({})", sprint.name, sprint.state),
            }),
    );
    destinations
}

fn sort_work_items(work_items: &mut [crate::store::work_items::WorkItem], order: &[String]) {
    work_items.sort_by_key(|item| {
        order
            .iter()
            .position(|key| key == &item.key)
            .unwrap_or(usize::MAX)
    });
}

pub(super) fn move_work_items_to_edge(
    snapshot: &mut BacklogSnapshot,
    source_section_id: &str,
    destination_section_id: &str,
    keys: &[String],
    to_top: bool,
) -> bool {
    if source_section_id == destination_section_id || keys.is_empty() {
        return false;
    }
    let Some(source) = work_items_mut(snapshot, source_section_id) else {
        return false;
    };
    let moved = source
        .iter()
        .filter(|item| keys.contains(&item.key))
        .cloned()
        .collect::<Vec<_>>();
    if moved.len() != keys.len() {
        return false;
    }
    source.retain(|item| !keys.contains(&item.key));
    let Some(destination) = work_items_mut(snapshot, destination_section_id) else {
        return false;
    };
    if to_top {
        destination.splice(0..0, moved);
    } else {
        destination.extend(moved);
    }
    true
}

fn work_items_mut<'a>(
    snapshot: &'a mut BacklogSnapshot,
    section_id: &str,
) -> Option<&'a mut Vec<crate::store::work_items::WorkItem>> {
    if section_id == "backlog" {
        Some(&mut snapshot.work_items)
    } else {
        let sprint_id = section_id.strip_prefix("sprint-")?.parse::<u64>().ok()?;
        snapshot
            .sprints
            .iter_mut()
            .find(|sprint| sprint.id == sprint_id)
            .map(|sprint| &mut sprint.work_items)
    }
}

fn work_items<'a>(snapshot: &'a BacklogSnapshot, section_id: &str) -> Option<&'a [WorkItem]> {
    if section_id == "backlog" {
        Some(&snapshot.work_items)
    } else {
        let sprint_id = section_id.strip_prefix("sprint-")?.parse::<u64>().ok()?;
        snapshot
            .sprints
            .iter()
            .find(|sprint| sprint.id == sprint_id)
            .map(|sprint| sprint.work_items.as_slice())
    }
}

fn restore_transfer_sections(snapshot: &mut BacklogSnapshot, transfer: &PendingTransfer) -> bool {
    let Some(original_source) =
        work_items(&transfer.rollback_snapshot, &transfer.source_section_id)
    else {
        return false;
    };
    let Some(original_destination) = work_items(
        &transfer.rollback_snapshot,
        &transfer.destination_section_id,
    ) else {
        return false;
    };
    let Some(current_source) = work_items(snapshot, &transfer.source_section_id) else {
        return false;
    };
    let Some(current_destination) = work_items(snapshot, &transfer.destination_section_id) else {
        return false;
    };
    let mut items_by_key = current_source
        .iter()
        .chain(current_destination)
        .map(|item| (item.key.clone(), item.clone()))
        .collect::<HashMap<_, _>>();
    let source = original_source
        .iter()
        .map(|item| {
            items_by_key
                .remove(&item.key)
                .unwrap_or_else(|| item.clone())
        })
        .collect();
    let destination = original_destination
        .iter()
        .map(|item| {
            items_by_key
                .remove(&item.key)
                .unwrap_or_else(|| item.clone())
        })
        .collect();
    let Some(source_items) = work_items_mut(snapshot, &transfer.source_section_id) else {
        return false;
    };
    *source_items = source;
    let Some(destination_items) = work_items_mut(snapshot, &transfer.destination_section_id) else {
        return false;
    };
    *destination_items = destination;
    true
}

fn backlog_view(
    snapshot: &BacklogSnapshot,
    section_sender: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
    syncing_ticket_keys: Rc<RefCell<HashSet<String>>>,
    velocity_dialog_close_requested: Rc<Cell<bool>>,
    issue_types: Vec<JiraOption>,
) -> BacklogView {
    let quick_menu = DialogLayer::new(
        backlog_tree_with_issue_types(
            snapshot,
            section_sender,
            move_locked.clone(),
            syncing_ticket_keys,
            issue_types,
        ),
        BacklogQuickMenu::new(move_locked),
    )
    .active(false)
    .fit_content()
    .fit_content_max(46, 10)
    .backdrop(DialogBackdrop::dim().amount(0.55));
    DialogLayer::new(
        quick_menu,
        velocity_dialog(
            None,
            &BacklogRunwaySettings::default(),
            None,
            velocity_dialog_close_requested,
            None,
        ),
    )
    .active(false)
    .fit_content()
    .fit_content_max(96, 26)
    .backdrop(DialogBackdrop::dim().amount(0.55))
}

pub(super) fn velocity_dialog(
    report: Option<&VelocityReport>,
    settings: &BacklogRunwaySettings,
    dynamic_ticket_size: Option<f64>,
    close_requested: Rc<Cell<bool>>,
    service: Option<AppService>,
) -> VelocityDialog {
    let latest_sprints = report.map_or(settings.jira_velocity_sprints, |report| {
        report.configured_sprints
    });
    let dynamic_value = report
        .and_then(|report| report.dynamic_capacity)
        .map(|value| format!("~{value:.1}"))
        .unwrap_or_else(|| "unavailable".into());
    let status = velocity_status(
        settings,
        &dynamic_value,
        latest_sprints,
        dynamic_ticket_size,
    );
    let rows = report.map_or_else(Vec::new, |report| {
        report
            .sprints
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, sprint)| VelocityRow {
                share_goal: sprint.goal.clone().unwrap_or_default(),
                sprint,
                alternate_background: index % 2 == 0,
            })
            .collect()
    });
    let mut table = DataView::new(rows, |row: &VelocityRow| row.sprint.id)
        .columns(vec![
            Column::multiline(
                "sprint",
                "Sprint",
                Constraint::Percentage(75),
                |row: &VelocityRow, _| velocity_sprint_text(row),
            )
            .constrained(),
            Column::text(
                "completed",
                "Completed",
                Constraint::Percentage(25),
                |row: &VelocityRow| format!("{:.1}", row.sprint.completed),
            ),
        ])
        .headers(true)
        .row_height(2)
        .wrap_cells()
        .copy_hotkey("yg", |row| Some(row.share_goal.clone()))
        .copy_hotkey("yv", move |row| {
            if let Some(service) = &service {
                copy_report(service, row.sprint.clone());
            }
            None
        })
        .focused(true);
    table.set_row_style_by(|row| {
        row.alternate_background
            .then(|| Style::default().bg(tuicore::theme().background_bg()))
    });
    let content = Flex::column()
        .child(
            "status",
            VelocityStatus::new(status),
            FlexItem::fit_content(),
        )
        .child("padding", Paragraph::new(""), FlexItem::fixed(1))
        .child("table", table, FlexItem::fixed(17));
    Dialog::new()
        .top_left("Velocity")
        .on_close(move |_| close_requested.set(true))
        .host(content)
}

pub(super) fn velocity_share_report(
    sprint: &VelocitySprint,
    snapshot: Option<&BacklogSnapshot>,
    jira_base_url: Option<&str>,
) -> String {
    if let Some(sprint) = snapshot.and_then(|snapshot| {
        snapshot
            .sprints
            .iter()
            .find(|candidate| candidate.id == sprint.id)
    }) {
        return sprint_report(sprint, jira_base_url);
    }
    if let Some(work_items) = &sprint.work_items {
        return sprint_report(
            &Sprint {
                id: sprint.id,
                name: sprint.name.clone(),
                state: "closed".into(),
                goal: sprint.goal.clone(),
                start_date: None,
                end_date: None,
                work_items: work_items.clone(),
                capacity: None,
            },
            jira_base_url,
        );
    }
    velocity_sprint_report(sprint)
}

fn velocity_sprint_report(sprint: &VelocitySprint) -> String {
    let mut lines = vec![sprint.name.clone()];
    let has_goal = if let Some(goal) = sprint
        .goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        lines.push(String::new());
        lines.push(format!("Goal: {goal}"));
        true
    } else {
        false
    };
    if !has_goal {
        lines.push(String::new());
    }
    lines.push(format!(
        "Completed: {} real points",
        points_label(sprint.completed)
    ));
    lines.join("\n")
}

fn velocity_sprint_text(row: &VelocityRow) -> Text<'static> {
    let theme = tuicore::theme();
    Text::from(vec![
        Line::from(Span::styled(
            row.sprint.name.clone(),
            Style::default()
                .fg(theme.text_fg())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            row.sprint
                .goal
                .clone()
                .unwrap_or_else(|| "(no sprint goal)".into()),
            Style::default().fg(theme.muted_fg()),
        )),
    ])
}

pub(super) fn sprint_report(sprint: &Sprint, base_url: Option<&str>) -> String {
    let mut lines = vec![sprint.name.clone()];
    let has_goal = if let Some(goal) = sprint
        .goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        lines.push(String::new());
        lines.push(format!("Goal: {goal}"));
        true
    } else {
        false
    };
    let report_items = sprint
        .work_items
        .iter()
        .filter(|item| !is_subtask(item))
        .collect::<Vec<_>>();
    let completed = report_items
        .iter()
        .copied()
        .filter(|item| crate::store::work_items::is_done_status(&item.status))
        .collect::<Vec<_>>();
    let total_points = report_items
        .iter()
        .copied()
        .filter(|item| is_estimate_eligible(item))
        .filter_map(|item| item.story_points)
        .sum::<f64>();
    let completed_points = completed
        .iter()
        .filter(|item| is_estimate_eligible(item))
        .filter_map(|item| item.story_points)
        .sum::<f64>();
    let estimated_items = report_items
        .iter()
        .filter(|item| is_estimate_eligible(item) && item.story_points.is_some())
        .count();
    let completed_estimated_items = completed
        .iter()
        .filter(|item| is_estimate_eligible(item) && item.story_points.is_some())
        .count();
    if !has_goal {
        lines.push(String::new());
    }
    lines.extend([
        format!(
            "Points: {}/{} pts completed",
            points_label(completed_points),
            points_label(total_points)
        ),
        format!("Tickets: {}/{} done", completed.len(), report_items.len()),
        format!("Estimated stories/tasks: {completed_estimated_items}/{estimated_items}"),
        String::new(),
        "Tickets:".into(),
    ]);
    lines.extend(report_items.iter().copied().map(|item| {
        let completion = sprint_ticket_marker(item);
        let reference = base_url
            .map(|url| format!("{url}/browse/{}", item.key))
            .unwrap_or_else(|| item.key.clone());
        let marker = ticket_type_marker(item)
            .map(|marker| format!(" {marker}"))
            .unwrap_or_default();
        let points = is_estimate_eligible(item).then(|| {
            item.story_points
                .map(points_label)
                .map(|points| format!("{points}pts"))
                .unwrap_or_else(|| "?pts".into())
        });
        let mut details = vec![item.title.clone()];
        if let Some(points) = points {
            details.push(points);
        }
        if !item.status.trim().is_empty() {
            details.push(item.status.clone());
        }
        details.push(reference);
        format!("{completion}{marker} {}", details.join(" - "))
    }));
    lines.join("\n")
}

fn points_label(points: f64) -> String {
    if points.abs() < f64::EPSILON {
        return "0".into();
    }
    if points.fract().abs() < f64::EPSILON {
        format!("{points:.0}")
    } else {
        format!("{points:.1}")
    }
}

fn is_estimate_eligible(item: &WorkItem) -> bool {
    matches!(item.kind.to_ascii_lowercase().as_str(), "story" | "task")
}

fn is_subtask(item: &WorkItem) -> bool {
    matches!(
        item.kind.to_ascii_lowercase().as_str(),
        "sub-task" | "subtask"
    )
}

fn sprint_ticket_marker(item: &WorkItem) -> &'static str {
    if crate::store::work_items::is_done_status(&item.status) {
        "✓"
    } else if matches!(
        item.status.to_ascii_lowercase().as_str(),
        "to do" | "selected for development"
    ) {
        "·"
    } else {
        "~"
    }
}

fn ticket_type_marker(item: &WorkItem) -> Option<&'static str> {
    match item.kind.to_ascii_lowercase().as_str() {
        "story" => Some("[S]"),
        "task" => Some("[T]"),
        "bug" => Some("[B]"),
        _ => None,
    }
}

fn velocity_status(
    settings: &BacklogRunwaySettings,
    dynamic_value: &str,
    latest_sprints: usize,
    dynamic_ticket_size: Option<f64>,
) -> Line<'static> {
    let underlined = Style::default().add_modifier(Modifier::UNDERLINED);
    let source = if settings.use_jira_velocity {
        "dynamic"
    } else {
        "fixed"
    };
    let value = if settings.use_jira_velocity {
        dynamic_value.to_owned()
    } else {
        format!("{:.1}", settings.fixed_sprint_capacity)
    };
    let mut spans = vec![
        Span::raw("Velocity is set as "),
        Span::styled(source, underlined),
        Span::raw(" with a value of "),
        Span::styled(value, underlined),
    ];
    if settings.use_jira_velocity {
        spans.extend([
            Span::raw(" using the latest "),
            Span::styled(latest_sprints.to_string(), underlined),
            Span::raw(" completed sprints."),
        ]);
    } else {
        spans.push(Span::raw("."));
    }
    let ticket_size_source = if settings.use_average_ticket_size {
        "dynamic"
    } else {
        "fixed"
    };
    let ticket_size = if settings.use_average_ticket_size {
        dynamic_ticket_size
            .map(|value| format!("~{value:.1}"))
            .unwrap_or_else(|| "unavailable".into())
    } else {
        format!("{:.1}", settings.fixed_ticket_size)
    };
    spans.extend([
        Span::raw(" Stories without story points use a "),
        Span::styled(ticket_size_source, underlined),
        Span::raw(" allocation of "),
        Span::styled(ticket_size, underlined),
        Span::raw(" points each."),
    ]);
    Line::from(spans)
}

struct VelocityStatus {
    line: Line<'static>,
    measurement: Paragraph,
}

impl VelocityStatus {
    fn new(line: Line<'static>) -> Self {
        Self {
            measurement: Paragraph::new(line.to_string()),
            line,
        }
    }
}

impl TuiNode for VelocityStatus {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <Paragraph as TuiNode<()>>::measure(&self.measurement, proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult { area }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        frame.render_widget(
            RatatuiParagraph::new(self.line.clone()).wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn empty_snapshot() -> BacklogSnapshot {
    BacklogSnapshot {
        board_name: "Backlog".into(),
        story_points_configured: false,
        sprints: Vec::new(),
        work_items: Vec::new(),
        top_level_backlog_keys: Vec::new(),
        warnings: Vec::new(),
        runway: None,
        velocity: None,
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
                        Paragraph::new("Loading Jira backlog…"),
                        FlexItem::fit_content(),
                    ),
                FlexItem::fit_content(),
            ),
    )
}

impl TuiNode for BacklogPage {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if self.shows_initial_loading() {
            self.loading_view.measure(proposal)
        } else {
            self.view.measure(proposal)
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if self.shows_initial_loading() {
            ctx.with_focus_fallback(FocusId::new("backlog-loading"), area, |ctx| {
                self.loading_view.layout(area, ctx)
            })
        } else {
            self.data_focus_path = ctx
                .current_path()
                .child(ChildKey::first())
                .child(ChildKey::first())
                .child(ChildKey::new("data"));
            self.view.layout(area, ctx)
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if self.shows_initial_loading() {
            self.loading_view.render(frame, area, ctx);
        } else {
            self.view.render(frame, area, ctx);
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.shows_initial_loading() {
            return self.loading_view.event(event, ctx);
        }
        let outcome = self.view.event(event, ctx);
        self.close_velocity_dialog(ctx);
        if self.drain_events(ctx) {
            ctx.request_redraw();
            ctx.request_tick();
        }
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.shows_initial_loading() {
            return self.loading_view.dispatch_event(route, event, ctx);
        }
        let outcome = self.view.dispatch_event(route, event, ctx);
        self.close_velocity_dialog(ctx);
        if self.drain_events(ctx) {
            ctx.request_redraw();
            ctx.request_tick();
        }
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let result = if self.shows_initial_loading() {
            self.loading_view.tick(dt, settings)
        } else {
            self.view.tick(dt, settings)
        };
        let result_changed = self.drain_results();
        let retry_started = if result_changed {
            false
        } else {
            self.retry_rank_refresh(dt)
        };
        let settings_refresh_started = !result_changed && self.refresh_for_settings_change();
        let changed = result_changed || retry_started || settings_refresh_started;
        let result = if changed {
            result.merge(TickResult {
                changed: true,
                layout: true,
                active: false,
                next_tick: None,
            })
        } else {
            result
        };
        if should_poll(
            self.loading,
            self.ranking,
            self.status_loading
                || self.assignees_loading
                || self.current_user_loading
                || !self.syncing_ticket_keys.borrow().is_empty(),
            self.issue_types_loading,
            self.rank_refresh_retry.pending(),
        ) {
            result.merge(TickResult::scheduled_after(Duration::from_millis(50)))
        } else {
            result
        }
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.shows_initial_loading() {
            self.loading_view.focus(target, focused, ctx);
        } else {
            self.view.focus(target, focused, ctx);
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.shows_initial_loading() {
            self.loading_view.dispatch_focus(target, focused, ctx);
        } else {
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
        self.focus_backlog_after_load = true;
        self.load_issue_types();
        self.load_assignees();
        self.load(false, false);
        ctx.request_tick();
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.unmount(ctx);
        self.view.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.loading_view.destroy(ctx);
        self.view.destroy(ctx);
    }

    fn take_pending_focus_request(&mut self) -> Option<FocusRequest> {
        self.pending_focus.take()
    }
}
