use std::{
    cell::Cell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, CrossAlign, DialogBackdrop, DialogLayer, EventCtx, EventOutcome, EventRoute,
    Flex, FlexItem, FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, MainAlign, Paragraph, RenderCtx, ScrollContainer, Spinner,
    TickResult, TuiEvent, TuiNode,
};

use crate::{
    service::AppService,
    store::work_items::{BacklogSnapshot, RankPlan, rank_plan},
};

use super::components::{
    BacklogDestination, BacklogQuickMenu, BacklogQuickMenuEvent, BacklogSectionEvent, BacklogTree,
    backlog_tree,
};

type BacklogView = DialogLayer<BacklogTree, BacklogQuickMenu>;

enum BacklogResult {
    Loaded {
        generation: u64,
        result: Result<BacklogSnapshot, String>,
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

pub(super) fn should_poll(loading: bool, ranking: bool, retry_pending: bool) -> bool {
    loading || ranking || retry_pending
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
    move_locked: Rc<Cell<bool>>,
    generations: RequestGenerations,
    rank_refresh_retry: RankRefreshRetry,
    active_rank_plan: Option<RankPlan>,
    snapshot: Option<BacklogSnapshot>,
    pending_transfer: Option<PendingTransfer>,
    pending_rank: Option<PendingRank>,
}

impl BacklogPage {
    fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (section_sender, section_receiver) = mpsc::channel();
        let move_locked = Rc::new(Cell::new(false));
        Self {
            service,
            sender,
            receiver,
            section_receiver,
            view: backlog_view(
                &empty_snapshot(),
                section_sender.clone(),
                move_locked.clone(),
            ),
            loading_view: loading_view(),
            loading: false,
            ranking: false,
            move_locked,
            generations: RequestGenerations::default(),
            rank_refresh_retry: RankRefreshRetry::default(),
            active_rank_plan: None,
            snapshot: None,
            pending_transfer: None,
            pending_rank: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_snapshot_for_test(snapshot: BacklogSnapshot) -> Self {
        let mut page = Self::new(AppService::for_tests());
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
    pub(super) fn view_for_test(&mut self) -> &mut BacklogView {
        &mut self.view
    }

    #[cfg(test)]
    pub(super) fn take_section_event_for_test(&self) -> Option<BacklogSectionEvent> {
        self.section_receiver.try_recv().ok()
    }

    #[cfg(test)]
    pub(super) fn move_is_locked_for_test(&self) -> bool {
        self.move_locked.get()
    }

    #[cfg(test)]
    pub(super) fn is_ranking_for_test(&self) -> bool {
        self.ranking
    }

    fn load(&mut self, rank_refresh: bool, preserve_optimistic_view: bool) {
        if !rank_refresh {
            self.rank_refresh_retry.cancel();
        }
        let generation = self
            .generations
            .start_load(rank_refresh, preserve_optimistic_view);
        self.loading = true;
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
                let error = format!("Could not load Jira backlog: {error}");
                self.handle_load_failure(completion, error.clone(), error);
            }
        }
    }

    fn shows_initial_loading(&self) -> bool {
        self.loading && self.snapshot.is_none()
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.try_recv() {
            changed |= match result {
                BacklogResult::Loaded { generation, result } => {
                    self.apply_load_result(generation, result)
                }
                BacklogResult::Ranked { generation, result } => {
                    self.apply_rank_result(generation, result)
                }
                BacklogResult::Transferred {
                    generation,
                    destination,
                    result,
                } => self.apply_transfer_result(generation, destination, result),
            };
        }
        changed
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
                            "Jira story points unavailable",
                            warning.clone(),
                        ));
                }
                if !preserves_optimistic_view
                    || (self.pending_transfer.is_none() && self.pending_rank.is_none())
                {
                    self.view.base_mut().set_snapshot(snapshot);
                    if let Some((_, row_id)) = highlight {
                        self.view.base_mut().highlight(&row_id);
                    }
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
        true
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
            self.view.base_mut().set_snapshot(snapshot);
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
                self.load(true, true);
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
                BacklogSectionEvent::MoveLocked => self.report_move_locked(),
                BacklogSectionEvent::OpenQuickMenu {
                    section_id,
                    keys,
                    source_order,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                        continue;
                    }
                    if !self.view.layer_mut().open(
                        section_id.clone(),
                        keys,
                        source_order,
                        transfer_destinations(self.snapshot.as_ref(), &section_id),
                        ctx,
                    ) {
                        self.report_move_locked();
                        continue;
                    }
                    self.view.set_active_with_context(true, ctx);
                }
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
        let events = self.view.layer_mut().take_events();
        let changed = !events.is_empty();
        for event in events {
            match event {
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
                    self.view.set_active_with_context(false, ctx);
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
                    self.view.set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveToSection {
                    source_section_id,
                    destination,
                    keys,
                } => {
                    if self.move_locked.get() {
                        self.report_move_locked();
                    } else {
                        self.transfer_to_section(source_section_id, destination, keys);
                    }
                    self.view.set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::MoveLocked => {
                    self.report_move_locked();
                    self.view.set_active_with_context(false, ctx);
                }
                BacklogQuickMenuEvent::Closed => self.view.set_active_with_context(false, ctx),
            }
        }
        changed
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
    ) {
        if self.move_locked.get() {
            self.report_move_locked();
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
        if !self.show_optimistic_transfer(&source_section_id, &destination.section_id, &keys) {
            return;
        }
        self.move_locked.set(true);
        let generation = self.generations.start_rank();
        self.loading = false;
        self.rank_refresh_retry.cancel();
        self.ranking = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        let destination_label = destination.label.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-transfer".into())
            .spawn(move || {
                let result = if destination.section_id == "backlog" {
                    service.jira_move_to_backlog(&keys)
                } else {
                    let sprint_id = destination
                        .section_id
                        .strip_prefix("sprint-")
                        .ok_or_else(|| "Unknown sprint destination".to_string())
                        .and_then(|id| {
                            id.parse::<u64>()
                                .map_err(|_| "Invalid sprint destination".to_string())
                        });
                    sprint_id.and_then(|sprint_id| service.jira_move_to_sprint(sprint_id, &keys))
                };
                let _ = sender.send(BacklogResult::Transferred {
                    generation,
                    destination: destination_label,
                    result,
                });
            })
        {
            if self.generations.complete_rank(generation) {
                self.ranking = false;
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
        match result {
            Ok(()) => {
                self.service
                    .report_notification(tuicore::Notification::success(
                        "Jira tickets moved",
                        format!("Moved tickets to {destination}"),
                    ));
                self.load(true, true);
            }
            Err(error) => {
                if let Some(transfer) = self.pending_transfer.as_mut() {
                    transfer.ambiguous = true;
                }
                self.service
                    .report_error(format!("Could not move Jira tickets: {error}"));
                self.load(true, true);
            }
        }
        true
    }

    fn show_optimistic_transfer(
        &mut self,
        source_section_id: &str,
        destination_section_id: &str,
        keys: &[String],
    ) -> bool {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let source_highlight_key =
            source_transfer_highlight_key(&source_order(Some(snapshot), source_section_id), keys);
        let (_, highlighted_row_id) =
            source_transfer_highlight(source_section_id, source_highlight_key.as_deref());
        let mut optimistic = snapshot.clone();
        if !move_work_items(
            &mut optimistic,
            source_section_id,
            destination_section_id,
            keys,
        ) {
            return false;
        }
        self.pending_transfer = Some(PendingTransfer {
            rollback_snapshot: snapshot.clone(),
            source_section_id: source_section_id.into(),
            destination_section_id: destination_section_id.into(),
            keys: keys.to_vec(),
            source_highlight_key,
            ambiguous: false,
            unconfirmed_refreshes: 0,
        });
        self.snapshot = Some(optimistic.clone());
        self.view.base_mut().set_snapshot(&optimistic);
        self.view.base_mut().highlight(&highlighted_row_id);
        true
    }

    fn restore_transfer_snapshot(&mut self) {
        let Some(pending_transfer) = self.pending_transfer.take() else {
            return;
        };
        self.snapshot = Some(pending_transfer.rollback_snapshot);
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
        self.view.base_mut().set_snapshot(&optimistic);
        Some(rollback_snapshot)
    }

    fn rank(&mut self, section_id: String, moved_keys: Vec<String>, final_order: Vec<String>) {
        if self.move_locked.get() {
            self.report_move_locked();
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
            self.view.base_mut().set_snapshot(snapshot);
        }
    }

    fn restore_rank_snapshot(&mut self) {
        let Some(pending_rank) = self.pending_rank.take() else {
            return;
        };
        self.snapshot = Some(pending_rank.rollback_snapshot);
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
            "Could not move tickets: a ticket move is still syncing with Jira".into(),
        );
    }

    fn retry_rank_refresh(&mut self, dt: Duration) -> bool {
        let Some(preserve_optimistic_view) = self.rank_refresh_retry.elapse(dt) else {
            return false;
        };
        self.load(true, preserve_optimistic_view);
        true
    }
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
    let destination_confirmed = transfer.keys.iter().all(|key| {
        section_contains(&refreshed_snapshot, &transfer.destination_section_id, key)
            && !section_contains(&refreshed_snapshot, &transfer.source_section_id, key)
    });
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

pub(super) fn move_work_items(
    snapshot: &mut BacklogSnapshot,
    source_section_id: &str,
    destination_section_id: &str,
    keys: &[String],
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
    destination.extend(moved);
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

#[cfg(test)]
pub(super) fn snapshot_view(snapshot: &BacklogSnapshot) -> BacklogTree {
    let (sender, _) = mpsc::channel();
    backlog_tree(snapshot, sender, Rc::new(Cell::new(false)))
}

fn backlog_view(
    snapshot: &BacklogSnapshot,
    section_sender: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
) -> BacklogView {
    DialogLayer::new(
        backlog_tree(snapshot, section_sender, move_locked.clone()),
        BacklogQuickMenu::new(move_locked),
    )
    .active(false)
    .fit_content()
    .fit_content_max(46, 10)
    .backdrop(DialogBackdrop::dim().amount(0.55))
}

fn empty_snapshot() -> BacklogSnapshot {
    BacklogSnapshot {
        board_name: "Backlog".into(),
        sprints: Vec::new(),
        work_items: Vec::new(),
        warnings: Vec::new(),
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
        if self.drain_quick_menu_events(ctx) || self.drain_section_events(ctx) {
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
        if self.drain_quick_menu_events(ctx) || self.drain_section_events(ctx) {
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
        let changed = result_changed || retry_started;
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
}
