use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Text},
};
use tuicore::{
    ActivationMode, AnimationSettings, CellContext, Column, EventCtx, EventOutcome, EventRoute,
    FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, ListControl, ListControlEvent, Panel, RenderCtx, TickResult, TuiEvent, TuiNode,
};

use crate::{
    service::AppService,
    store::composer::{ComposerAction, ComposerState},
};

#[derive(Clone)]
struct ChangeSetRow {
    id: String,
    name: String,
    subtitle: String,
}

pub(super) struct ChangeSetListView {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    control: ListControl<ChangeSetRow, String>,
}

impl ChangeSetListView {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        let rows = rows(&state.borrow());
        let control = ListControl::new(
            rows,
            |row: &ChangeSetRow| row.id.clone(),
            |name, rows| {
                let next = rows
                    .iter()
                    .filter_map(|row| row.id.strip_prefix("CS-")?.parse::<usize>().ok())
                    .max()
                    .unwrap_or(0)
                    + 1;
                ChangeSetRow {
                    id: format!("CS-{next}"),
                    name,
                    subtitle: "0 tickets · ready to compose".into(),
                }
            },
        )
        .column(change_set_column())
        .row_height(2)
        .title("Change sets")
        .panel(Panel::new().top_left("Change sets").one_row(true))
        .activation_mode(ActivationMode::OnActivateKey)
        .confirm_remove("Delete change set?", |row| {
            format!(
                "Delete {} · {}? This removes its local ticket snapshots.",
                row.id, row.name
            )
        });
        Self {
            state,
            service,
            control,
        }
    }

    pub(super) fn sync(&mut self) {
        self.control
            .data_view_mut()
            .set_rows(rows(&self.state.borrow()));
    }

    fn drain_events(&mut self, ctx: &mut EventCtx<()>) {
        for event in self.control.take_events() {
            match event {
                ListControlEvent::Added { row_id } => {
                    if let Some(row) = self.control.items().iter().find(|row| row.id == row_id) {
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::CreateChangeSet {
                                id: row.id.clone(),
                                name: row.name.clone(),
                            });
                        if let Some(set) = self
                            .state
                            .borrow()
                            .change_sets
                            .iter()
                            .find(|set| set.id == row.id)
                            .cloned()
                        {
                            self.service.save_change_set(set);
                        }
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::OpenChangeSet(row_id.clone()));
                        ctx.request_layout();
                        ctx.request_redraw();
                    }
                }
                ListControlEvent::Removed { row_id } => {
                    if self.state.borrow().change_set_is_submitting(&row_id) {
                        self.service
                            .report_notification(tuicore::Notification::error(
                                "Delete blocked",
                                "Cannot delete a change set while its Jira commit is in progress",
                            ));
                        self.sync();
                        ctx.request_layout();
                        ctx.request_redraw();
                        continue;
                    }
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::DeleteChangeSet(row_id.clone()));
                    self.service.delete_change_set(row_id);
                }
                _ => {}
            }
        }
        for event in self.control.data_view_mut().drain_events() {
            if let tuicore::DataViewTypedEvent::Activated { row_id } = event {
                self.state
                    .borrow_mut()
                    .dispatch(ComposerAction::OpenChangeSet(row_id));
                ctx.request_layout();
                ctx.request_redraw();
            }
        }
    }
}

fn rows(state: &ComposerState) -> Vec<ChangeSetRow> {
    let mut rows: Vec<_> = state
        .change_sets
        .iter()
        .map(|set| {
            let submitted = set.submitted_count();
            let state = if set.closed { "closed" } else { "open" };
            ChangeSetRow {
                id: set.id.clone(),
                name: set.name.clone(),
                subtitle: format!("{submitted}/{} submitted · {state}", set.tickets.len()),
            }
        })
        .collect();
    rows.reverse();
    rows
}

fn change_set_column() -> Column<ChangeSetRow, String> {
    Column::multiline(
        "change_set",
        "",
        Constraint::Percentage(100),
        |row: &ChangeSetRow, _: &CellContext<String>| {
            let theme = tuicore::theme();
            Text::from(vec![
                Line::styled(
                    format!("{} · {}", row.id, row.name),
                    Style::default()
                        .fg(theme.text_fg())
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(row.subtitle.clone(), Style::default().fg(theme.subtle_fg())),
            ])
        },
    )
}

impl TuiNode for ChangeSetListView {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.control.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.control.event(event, ctx);
        self.drain_events(ctx);
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.control.dispatch_event(route, event, ctx);
        self.drain_events(ctx);
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.control.tick(dt, settings)
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
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
    }
}
