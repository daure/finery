use std::{
    cell::Cell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, CrossAlign, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    MainAlign, Panel, Paragraph, RenderCtx, ScrollContainer, Spinner, TickResult, TuiEvent,
    TuiNode,
};

use crate::{
    service::AppService,
    store::work_items::{BacklogSnapshot, RankPlan, rank_plan},
};

use super::components::{BacklogSectionEvent, backlog_section};

enum BacklogResult {
    Loaded {
        generation: u64,
        result: Result<BacklogSnapshot, String>,
    },
    Ranked {
        generation: u64,
        result: Result<(), String>,
    },
}

const RANK_REFRESH_RETRY_DELAY: Duration = Duration::from_secs(1);

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

    pub(super) fn complete_load(
        &mut self,
        generation: u64,
        succeeded: bool,
        move_locked: &Cell<bool>,
    ) -> Option<LoadCompletion> {
        if self.active_load != Some(generation) {
            return None;
        }
        self.active_load = None;
        let completed_rank_refresh = self.rank_refresh_load == Some(generation);
        let preserve_optimistic_view = self.preserve_optimistic_view_load == Some(generation);
        self.rank_refresh_load = None;
        self.preserve_optimistic_view_load = None;
        if completed_rank_refresh && succeeded {
            move_locked.set(false);
        }
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
    section_sender: Sender<BacklogSectionEvent>,
    section_receiver: Receiver<BacklogSectionEvent>,
    view: ScrollContainer<Flex<()>>,
    loading: bool,
    ranking: bool,
    move_locked: Rc<Cell<bool>>,
    generations: RequestGenerations,
    rank_refresh_retry: RankRefreshRetry,
    active_rank_plan: Option<RankPlan>,
    snapshot: Option<BacklogSnapshot>,
}

impl BacklogPage {
    fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (section_sender, section_receiver) = mpsc::channel();
        Self {
            service,
            sender,
            receiver,
            section_sender,
            section_receiver,
            view: loading_view(),
            loading: false,
            ranking: false,
            move_locked: Rc::new(Cell::new(false)),
            generations: RequestGenerations::default(),
            rank_refresh_retry: RankRefreshRetry::default(),
            active_rank_plan: None,
            snapshot: None,
        }
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
            let completion = self
                .generations
                .complete_load(generation, false, &self.move_locked);
            if let Some(completion) = completion {
                self.loading = false;
                let error = format!("Could not load Jira backlog: {error}");
                self.handle_load_failure(completion, error.clone(), error);
            }
        }
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
            };
        }
        changed
    }

    fn apply_load_result(
        &mut self,
        generation: u64,
        result: Result<BacklogSnapshot, String>,
    ) -> bool {
        let Some(completion) =
            self.generations
                .complete_load(generation, result.is_ok(), &self.move_locked)
        else {
            return false;
        };
        self.loading = false;
        match result {
            Ok(snapshot) => {
                if matches!(completion, LoadCompletion::RankRefresh { .. }) {
                    self.rank_refresh_retry.cancel();
                }
                self.snapshot = Some(snapshot.clone());
                for warning in &snapshot.warnings {
                    self.service
                        .report_notification(tuicore::Notification::warning(
                            "Jira story points unavailable",
                            warning.clone(),
                        ));
                }
                if !matches!(
                    completion,
                    LoadCompletion::RankRefresh {
                        preserve_optimistic_view: true
                    }
                ) {
                    self.view = snapshot_view_with_events(
                        &snapshot,
                        self.section_sender.clone(),
                        self.move_locked.clone(),
                    );
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
        self.view = self.snapshot.as_ref().map_or_else(
            || status_view("Backlog", view_error),
            |snapshot| {
                snapshot_view_with_events(
                    snapshot,
                    self.section_sender.clone(),
                    self.move_locked.clone(),
                )
            },
        );
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
                    .take()
                    .expect("completed Jira rank has an active plan");
                self.report_rank_success(&plan);
                self.load(true, true);
            }
            Err(error) => {
                self.active_rank_plan = None;
                self.restore_snapshot();
                self.service
                    .report_error(format!("Could not rank Jira backlog: {error}"));
                self.load(true, false);
            }
        }
        true
    }

    fn drain_section_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.section_receiver.try_recv() {
            match event {
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

    fn rank(&mut self, section_id: String, moved_keys: Vec<String>, final_order: Vec<String>) {
        if self.move_locked.get() {
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
                self.restore_snapshot();
                self.move_locked.set(false);
                self.service
                    .report_error(format!("Could not start Jira rank: {error}"));
            }
        }
    }

    fn restore_snapshot(&mut self) {
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.view = snapshot_view_with_events(
                snapshot,
                self.section_sender.clone(),
                self.move_locked.clone(),
            );
        }
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

    fn retry_rank_refresh(&mut self, dt: Duration) -> bool {
        let Some(preserve_optimistic_view) = self.rank_refresh_retry.elapse(dt) else {
            return false;
        };
        self.load(true, preserve_optimistic_view);
        true
    }
}

#[cfg(test)]
pub(super) fn snapshot_view(snapshot: &BacklogSnapshot) -> ScrollContainer<Flex<()>> {
    let (sender, _) = mpsc::channel();
    snapshot_view_with_events(snapshot, sender, Rc::new(Cell::new(false)))
}

fn snapshot_view_with_events(
    snapshot: &BacklogSnapshot,
    section_events: Sender<BacklogSectionEvent>,
    move_locked: Rc<Cell<bool>>,
) -> ScrollContainer<Flex<()>> {
    let mut view = Flex::column();
    for sprint in &snapshot.sprints {
        view = view.child(
            format!("sprint-{}", sprint.id),
            backlog_section(
                format!("sprint-{}", sprint.id),
                format!(
                    "{} · {} ({})",
                    snapshot.board_name, sprint.name, sprint.state
                ),
                &sprint.work_items,
                None,
                false,
                section_events.clone(),
                move_locked.clone(),
            ),
            FlexItem::fit_content(),
        );
    }
    view = view.child(
        "backlog",
        backlog_section(
            "backlog",
            format!("{} · Backlog", snapshot.board_name),
            &snapshot.work_items,
            Some("shift+b"),
            true,
            section_events,
            move_locked,
        ),
        FlexItem::fit_content(),
    );
    ScrollContainer::vertical(view)
}

fn status_view(title: impl Into<String>, message: impl Into<String>) -> ScrollContainer<Flex<()>> {
    ScrollContainer::vertical(
        Flex::column().child(
            "backlog-status",
            Panel::new()
                .one_row(true)
                .top_left(title)
                .content([message.into()]),
            FlexItem::fit_content(),
        ),
    )
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
        self.view.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.view.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.view.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.view.event(event, ctx);
        if self.drain_section_events() {
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
        let outcome = self.view.dispatch_event(route, event, ctx);
        if self.drain_section_events() {
            ctx.request_redraw();
            ctx.request_tick();
        }
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let result = self.view.tick(dt, settings);
        let section_changed = self.drain_section_events();
        let result_changed = self.drain_results();
        let retry_started = if result_changed {
            false
        } else {
            self.retry_rank_refresh(dt)
        };
        let changed = section_changed || result_changed || retry_started;
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
        self.load(false, false);
        ctx.request_tick();
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.destroy(ctx);
    }
}
