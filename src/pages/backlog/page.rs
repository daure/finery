use std::{
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

use crate::{service::AppService, store::work_items::BacklogSnapshot};

use super::components::backlog_data_view;

type BacklogResult = Result<BacklogSnapshot, String>;

pub(crate) fn page(service: AppService) -> BacklogPage {
    BacklogPage::new(service)
}

pub(crate) struct BacklogPage {
    service: AppService,
    sender: Sender<BacklogResult>,
    receiver: Receiver<BacklogResult>,
    view: ScrollContainer<Flex<()>>,
    loading: bool,
}

impl BacklogPage {
    fn new(service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            service,
            sender,
            receiver,
            view: loading_view(),
            loading: false,
        }
    }

    fn load(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-backlog".into())
            .spawn(move || {
                let _ = sender.send(service.jira_backlog());
            })
        {
            self.loading = false;
            self.view = status_view("Backlog", format!("Could not load Jira backlog: {error}"));
        }
    }

    fn drain_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.try_recv() {
            self.loading = false;
            self.view = match result {
                Ok(snapshot) => snapshot_view(&snapshot),
                Err(error) => status_view("Backlog", error),
            };
            changed = true;
        }
        changed
    }
}

pub(super) fn snapshot_view(snapshot: &BacklogSnapshot) -> ScrollContainer<Flex<()>> {
    let mut view = Flex::column();
    for sprint in &snapshot.sprints {
        view = view.child(
            format!("sprint-{}", sprint.id),
            backlog_data_view(
                format!("sprint-{}", sprint.id),
                format!(
                    "{} · {} ({})",
                    snapshot.board_name, sprint.name, sprint.state
                ),
                &sprint.work_items,
                None,
                false,
            ),
            FlexItem::fit_content(),
        );
    }
    view = view.child(
        "backlog",
        backlog_data_view(
            "backlog",
            format!("{} · Backlog", snapshot.board_name),
            &snapshot.work_items,
            Some("shift+b"),
            true,
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
        self.view.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.view.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let result = self.view.tick(dt, settings);
        if self.drain_results() {
            result.merge(TickResult {
                changed: true,
                layout: true,
                active: false,
                next_tick: None,
            })
        } else if self.loading {
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
        self.load();
        ctx.request_tick();
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.destroy(ctx);
    }
}
