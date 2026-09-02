use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RenderCtx, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::AppSettings,
    service::AppService,
    store::composer::{ChangeSet, ComposerState},
};

use super::{change_set_list::ChangeSetListView, ticket_editor::TicketEditor};

pub(crate) fn page(
    change_sets: Vec<ChangeSet>,
    service: AppService,
    settings: Arc<RwLock<AppSettings>>,
) -> ComposerPage {
    ComposerPage::new(change_sets, service, settings)
}

pub(crate) struct ComposerPage {
    state: Rc<RefCell<ComposerState>>,
    change_sets: ChangeSetListView,
    editor: TicketEditor,
    service: AppService,
    catalog_revision: i64,
    poll_elapsed: Duration,
    external_reload_needed: bool,
}

impl ComposerPage {
    pub(super) fn new(
        change_sets: Vec<ChangeSet>,
        service: AppService,
        settings: Arc<RwLock<AppSettings>>,
    ) -> Self {
        let composer_state = ComposerState::from_change_sets(change_sets);
        let state = Rc::new(RefCell::new(composer_state));
        let composer_keys = settings
            .read()
            .expect("settings lock poisoned")
            .composer_keys
            .clone();
        Self {
            change_sets: ChangeSetListView::new(Rc::clone(&state), service.clone(), composer_keys),
            editor: TicketEditor::new(Rc::clone(&state), settings, service.clone()),
            state,
            catalog_revision: service.composer_catalog_revision(),
            service,
            poll_elapsed: Duration::ZERO,
            external_reload_needed: false,
        }
    }

    fn in_change_set(&self) -> bool {
        self.state.borrow().active_change_set.is_some()
    }

    fn active(&self) -> &dyn TuiNode<()> {
        if self.in_change_set() {
            &self.editor
        } else {
            &self.change_sets
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        if self.in_change_set() {
            &mut self.editor
        } else {
            &mut self.change_sets
        }
    }

    fn handle_active_view_change(&mut self, was_open: bool, ctx: &mut EventCtx<()>) {
        let is_open = self.in_change_set();
        if was_open == is_open {
            return;
        }
        self.editor.sync();
        if !is_open {
            self.change_sets.sync();
        }
        ctx.request_layout();
        ctx.request_redraw();
        if is_open {
            self.editor.on_open(ctx);
        }
    }

    fn open_selected_ticket(
        &self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        if self.editor.create_dialog_is_open() {
            return None;
        }
        if !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return None;
        }
        let key = self.state.borrow().selected_existing_ticket_key()?;
        self.service.open_jira_issue(&key);
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn poll_external_catalog(&mut self, dt: Duration) -> TickResult {
        self.catalog_revision = self
            .catalog_revision
            .max(self.service.composer_catalog_revision());
        self.poll_elapsed = self.poll_elapsed.saturating_add(dt);
        if self.poll_elapsed >= Duration::from_millis(500) {
            self.poll_elapsed = Duration::ZERO;
            self.service.poll_composer_catalog_revision();
        }

        for alert in self.service.take_composer_alerts() {
            self.service
                .report_notification(tuicore::Notification::warning("Composer refreshed", alert));
        }
        self.external_reload_needed |= self.service.take_composer_reload_required();
        if let Some(result) = self.service.take_composer_catalog_revision() {
            match result {
                Ok(revision) => {
                    self.external_reload_needed |= revision > self.catalog_revision;
                }
                Err(error) => self
                    .service
                    .report_error(format!("composer catalog poll failed: {error}")),
            }
        }

        let mut changed = false;
        if !self.service.composer_writes_pending() && !self.editor.is_submitting() {
            if let Some(result) = self.service.take_loaded_composer_catalog() {
                match result {
                    Ok(loaded) => {
                        let catalog = loaded.catalog;
                        let catalog_is_stale = loaded.requested_catalog_revision
                            < self.catalog_revision
                            || catalog.catalog_revision < self.catalog_revision;
                        if catalog_is_stale || !self.service.accept_composer_catalog(&catalog) {
                            self.catalog_revision = self
                                .catalog_revision
                                .max(self.service.composer_catalog_revision());
                            self.external_reload_needed = true;
                            return TickResult {
                                changed: false,
                                layout: false,
                                active: false,
                                next_tick: Some(Duration::from_millis(50)),
                            };
                        }
                        self.catalog_revision = catalog.catalog_revision;
                        self.state.borrow_mut().replace_change_sets(
                            catalog
                                .change_sets
                                .iter()
                                .map(|set| set.change_set.clone())
                                .collect(),
                        );
                        self.change_sets.sync();
                        self.editor.sync();
                        self.external_reload_needed = false;
                        changed = true;
                    }
                    Err(error) => self
                        .service
                        .report_error(format!("composer catalog reload failed: {error}")),
                }
            } else if self.external_reload_needed {
                self.service.load_composer_catalog();
            }
        }

        let next_tick = if self.service.composer_sync_in_flight() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(500).saturating_sub(self.poll_elapsed)
        };
        TickResult {
            changed,
            layout: changed,
            active: false,
            next_tick: Some(next_tick),
        }
    }

    #[cfg(test)]
    pub(super) fn create_ticket(&mut self, title: &str) {
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::CreateTicket {
                title: title.into(),
                project_key: "FIN".into(),
            });
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn open_change_set_for_test(&mut self, id: &str) {
        let _ = self.state.borrow_mut().dispatch(
            crate::store::composer::ComposerAction::OpenChangeSet(id.into()),
        );
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn create_child_ticket(&mut self, title: &str) {
        let parent = self.state.borrow().selected_ticket.clone().unwrap();
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::CreateTicketAt {
                title: title.into(),
                project_key: "FIN".into(),
                kind: crate::store::composer::TicketKind::Subtask,
                placement: crate::store::composer::PlacementTarget::ChildOf(parent),
            })
            .unwrap();
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn open_new_ticket(&mut self, ctx: &mut EventCtx<()>) {
        self.editor.open_new_ticket_for_test(ctx);
    }

    #[cfg(test)]
    pub(super) fn mark_selected_deleted(&mut self) {
        let selected = self.state.borrow().selected_ticket.clone().unwrap();
        self.state.borrow_mut().dispatch(
            crate::store::composer::ComposerAction::MarkTicketDeleted(selected),
        );
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn set_view_mode(&mut self, mode: crate::store::composer::ComposerViewMode) {
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::SetViewMode(mode));
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn submit_selected_locally(&mut self) {
        let change = self.state.borrow().selected_change().cloned().unwrap();
        let change_set_id = self.state.borrow().active_change_set.clone().unwrap();
        let original = change.original.clone();
        let updated = change.updated.clone().or_else(|| original.clone());
        self.state.borrow_mut().dispatch(
            crate::store::composer::ComposerAction::CompleteSubmission {
                change_set_id,
                id: change.id,
                snapshot: crate::store::composer::SubmissionSnapshot { original, updated },
            },
        );
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn set_selected_source(&mut self, ticket: crate::store::composer::Ticket) {
        let id = self.state.borrow().selected_ticket.clone().unwrap();
        let change_set_id = self.state.borrow().active_change_set.clone().unwrap();
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::SetSource {
                change_set_id,
                id,
                ticket,
            });
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn update_selected_kind(&mut self, kind: crate::store::composer::TicketKind) {
        self.state
            .borrow_mut()
            .dispatch(crate::store::composer::ComposerAction::UpdateKind(kind));
        self.editor.sync();
    }

    #[cfg(test)]
    pub(super) fn selected_changes(&self) -> crate::store::composer::Ticket {
        self.state.borrow().selected_changes().cloned().unwrap()
    }

    #[cfg(test)]
    pub(super) fn detail_panel_areas(&self) -> (Rect, Rect) {
        self.editor.detail_panel_areas()
    }

    #[cfg(test)]
    pub(super) fn ticket_detail_areas(&self) -> (Rect, Rect) {
        self.editor.ticket_detail_areas()
    }

    #[cfg(test)]
    pub(super) fn narrow_border_style(&self) -> tuicore::TabsBodyBorderStyle {
        self.editor.narrow_border_style()
    }

    #[cfg(test)]
    pub(super) fn create_kind_menu_is_open(&self) -> bool {
        self.editor.create_kind_menu_is_open()
    }

    #[cfg(test)]
    pub(super) fn create_dialog_is_open(&self) -> bool {
        self.editor.create_dialog_is_open()
    }

    #[cfg(test)]
    pub(super) fn active_change_set_name(&self) -> Option<String> {
        self.state.borrow().active_set().map(|set| set.name.clone())
    }

    #[cfg(test)]
    pub(super) fn overview_highlighted_change_set(&self) -> Option<String> {
        self.change_sets.highlighted_change_set()
    }
}

impl TuiNode for ComposerPage {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.active().measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.editor.sync();
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active().render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let was_open = self.in_change_set();
        let outcome = self.active_mut().event(event, ctx);
        self.handle_active_view_change(was_open, ctx);
        (outcome == EventOutcome::Ignored)
            .then(|| self.open_selected_ticket(event, ctx))
            .flatten()
            .unwrap_or(outcome)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let was_open = self.in_change_set();
        let outcome = self.active_mut().dispatch_event(route, event, ctx);
        self.handle_active_view_change(was_open, ctx);
        (outcome == EventOutcome::Ignored)
            .then(|| self.open_selected_ticket(event, ctx))
            .flatten()
            .unwrap_or(outcome)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let was_open = self.in_change_set();
        let outcome = if was_open {
            self.editor.tick(dt, settings)
        } else {
            self.change_sets
                .tick(dt, settings)
                .merge(self.editor.poll_inactive_submission())
        };
        let outcome = outcome.merge(self.poll_external_catalog(dt));
        if was_open && !self.in_change_set() {
            self.change_sets.sync();
            outcome.merge(TickResult {
                changed: true,
                layout: true,
                active: false,
                next_tick: None,
            })
        } else {
            outcome
        }
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.change_sets.init(ctx);
        self.editor.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.change_sets.mount(ctx);
        self.editor.mount(ctx);
        if self.in_change_set() {
            self.editor.on_open_lifecycle(ctx);
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.editor.unmount(ctx);
        self.change_sets.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.editor.destroy(ctx);
        self.change_sets.destroy(ctx);
    }
}
