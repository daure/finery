use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dialog, DialogAction, DialogBackdrop, DialogLayer, EventCtx, EventOutcome,
    EventRoute, FocusCtx, FocusId, FocusRequest, FocusTarget, HotkeyEvent, InputChrome, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::ComposerKeyBindings,
    service::{AppService, composer_service::CloneChangeSetResponse},
    store::composer::{ArchiveOutcome, ChangeSet, ComposerAction, ComposerState},
};

use super::{
    change_set_list::{ChangeSetDialog, ChangeSetListView, WideTextInput},
    change_set_quick_menu::{
        ChangeSetQuickAction, ChangeSetQuickMenu, MENU_HOST_HEIGHT, MENU_HOST_WIDTH,
    },
};

type OverviewList = DialogLayer<ChangeSetListView, ChangeSetQuickMenu>;

pub(super) struct ChangeSetOverview {
    view: DialogLayer<OverviewList, ChangeSetDialog>,
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    keys: ComposerKeyBindings,
    confirmed: Rc<RefCell<Option<ArchiveOutcome>>>,
    dismissed: Rc<RefCell<bool>>,
    target: Option<ChangeSet>,
    menu_target: Option<String>,
    clone_requested: Rc<RefCell<Option<String>>>,
    clone_dismissed: Rc<RefCell<bool>>,
    clone_target: Option<ChangeSet>,
    clone_sender: Sender<Result<CloneChangeSetResponse, String>>,
    clone_receiver: Receiver<Result<CloneChangeSetResponse, String>>,
    clone_in_progress: bool,
    rename_requested: Rc<RefCell<Option<String>>>,
    rename_dismissed: Rc<RefCell<bool>>,
    rename_target: Option<String>,
}

impl ChangeSetOverview {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        service: AppService,
        keys: ComposerKeyBindings,
    ) -> Self {
        let clone_requested = Rc::new(RefCell::new(None));
        let clone_dismissed = Rc::new(RefCell::new(false));
        let rename_requested = Rc::new(RefCell::new(None));
        let rename_dismissed = Rc::new(RefCell::new(false));
        let (clone_sender, clone_receiver) = mpsc::channel();
        Self {
            view: DialogLayer::new(
                DialogLayer::new(
                    ChangeSetListView::new(Rc::clone(&state), service.clone(), keys.clone()),
                    ChangeSetQuickMenu::new(keys.clone()),
                )
                .active(false)
                .fit_content()
                .fit_content_max(MENU_HOST_WIDTH, MENU_HOST_HEIGHT)
                .backdrop(DialogBackdrop::dim().amount(0.55)),
                clone_dialog(
                    &keys,
                    Rc::clone(&clone_requested),
                    Rc::clone(&clone_dismissed),
                    "",
                ),
            )
            .active(false)
            .fit_content()
            .child_overlays_use_base_bounds(true)
            .backdrop(DialogBackdrop::dim().amount(0.55)),
            state,
            service,
            keys,
            confirmed: Rc::new(RefCell::new(None)),
            dismissed: Rc::new(RefCell::new(false)),
            target: None,
            menu_target: None,
            clone_requested,
            clone_dismissed,
            clone_target: None,
            clone_sender,
            clone_receiver,
            clone_in_progress: false,
            rename_requested,
            rename_dismissed,
            rename_target: None,
        }
    }

    pub(super) fn sync(&mut self) {
        self.view.base_mut().base_mut().sync();
    }

    pub(super) fn reset_to_home(&mut self, ctx: &mut EventCtx<()>) {
        self.view.set_active_with_context(false, ctx);
        self.view.base_mut().set_active_with_context(false, ctx);
        self.view.base_mut().base_mut().reset_to_home(ctx);
        self.confirmed.borrow_mut().take();
        self.dismissed.replace(false);
        self.target = None;
        self.menu_target = None;
        self.clone_requested.borrow_mut().take();
        self.clone_dismissed.replace(false);
        self.clone_target = None;
        self.rename_requested.borrow_mut().take();
        self.rename_dismissed.replace(false);
        self.rename_target = None;
    }

    #[cfg(test)]
    pub(super) fn highlighted_change_set(&self) -> Option<String> {
        self.view.base().base().highlighted_change_set()
    }

    fn open_archive(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let matches = match event {
            TuiEvent::Key(key) => self.keys.archive.matches(*key),
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) => {
                sequence == self.keys.archive.sequence()
            }
            _ => false,
        };
        if !matches || !self.overview_actions_available() {
            return false;
        }
        ctx.stop_propagation();
        let Some(id) = self.view.base().base().highlighted_change_set() else {
            return true;
        };
        self.show_archive(&id, ctx)
    }

    fn open_rename(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let matches = match event {
            TuiEvent::Key(key) => self.keys.rename_change_set.matches(*key),
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) => {
                sequence == self.keys.rename_change_set.sequence()
            }
            _ => false,
        };
        if !matches || !self.overview_actions_available() {
            return false;
        }
        ctx.stop_propagation();
        let Some(id) = self.view.base().base().highlighted_change_set() else {
            return true;
        };
        self.show_rename(&id, ctx);
        true
    }

    fn open_clone(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let matches = match event {
            TuiEvent::Key(key) => self.keys.clone_change_set.matches(*key),
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) => {
                sequence == self.keys.clone_change_set.sequence()
            }
            _ => false,
        };
        if !matches || !self.overview_actions_available() {
            return false;
        }
        ctx.stop_propagation();
        let Some(id) = self.view.base().base().highlighted_change_set() else {
            return true;
        };
        self.show_clone(&id, ctx);
        true
    }

    fn show_archive(&mut self, id: &str, ctx: &mut EventCtx<()>) -> bool {
        if let Err(error) = self.state.borrow().validate_archive(id) {
            self.service
                .report_notification(tuicore::Notification::warning("Cannot archive", error));
            return true;
        }
        let Some(set) = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .cloned()
        else {
            return true;
        };
        let done = Rc::clone(&self.confirmed);
        let reject = Rc::clone(&self.confirmed);
        let cancel = Rc::clone(&self.dismissed);
        let close = Rc::clone(&self.dismissed);
        let dialog = Dialog::new()
            .top_left("Complete change set?")
            .actions([
                DialogAction::new("Done")
                    .hotkey(self.keys.archive_done.spec())
                    .on_trigger(move || *done.borrow_mut() = Some(ArchiveOutcome::Concluded)),
                DialogAction::new("Reject")
                    .hotkey(self.keys.archive_reject.spec())
                    .on_trigger(move || *reject.borrow_mut() = Some(ArchiveOutcome::Cancelled)),
                DialogAction::new("Cancel")
                    .hotkey(self.keys.dialog_cancel.spec())
                    .on_trigger(move || *cancel.borrow_mut() = true),
            ])
            .on_close(move |_| *close.borrow_mut() = true)
            .host(WideTextInput::prose_only(
                "Choose Done to complete or Reject to close.",
            ));
        self.target = Some(set);
        self.view.replace_layer(dialog, ctx);
        self.view.set_active_with_context(true, ctx);
        true
    }

    fn drain_outcome(&mut self, ctx: &mut EventCtx<()>) {
        let confirmed = self.confirmed.borrow_mut().take();
        let dismissed = self.dismissed.replace(false);
        if confirmed.is_none() && !dismissed {
            return;
        }
        self.view.set_active_with_context(false, ctx);
        let Some(expected) = self.target.take() else {
            return;
        };
        let Some(outcome) = confirmed else {
            return;
        };
        let result = {
            let mut state = self.state.borrow_mut();
            if state.change_sets.iter().find(|set| set.id == expected.id) != Some(&expected) {
                Err("The change set changed. Review it and choose an outcome again.".to_owned())
            } else {
                state.validate_archive(&expected.id).and_then(|()| {
                    state
                        .dispatch(ComposerAction::ArchiveChangeSet {
                            id: expected.id.clone(),
                            outcome,
                        })
                        .map_err(|error| error.to_string())?;
                    Ok(state
                        .change_sets
                        .iter()
                        .find(|set| set.id == expected.id)
                        .cloned())
                })
            }
        };
        match result {
            Ok(Some(set)) => self.service.save_change_set(set),
            Ok(None) => {}
            Err(error) => self
                .service
                .report_notification(tuicore::Notification::warning("Cannot archive", error)),
        }
        self.sync();
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn overview_actions_available(&self) -> bool {
        !self.view.is_active()
            && !self.view.base().is_active()
            && !self.clone_in_progress
            && self.view.base().base().archive_available()
    }

    fn open_quick_menu(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let matches = match event {
            TuiEvent::Key(key) => self.keys.change_set_actions.matches(*key),
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) => {
                sequence == self.keys.change_set_actions.sequence()
            }
            _ => false,
        };
        if !matches || !self.overview_actions_available() {
            return false;
        }
        ctx.stop_propagation();
        let Some(id) = self.view.base().base().highlighted_change_set() else {
            return true;
        };
        let clone_available = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .is_some_and(|set| set.closed);
        self.menu_target = Some(id);
        self.view.base_mut().layer_mut().open(clone_available, ctx);
        self.view.base_mut().set_active_with_context(true, ctx);
        true
    }

    fn drain_quick_menu(&mut self, ctx: &mut EventCtx<()>) {
        if !self.view.base().is_active() {
            return;
        }
        let action = self.view.base_mut().layer_mut().take_action();
        if action.is_none() && self.view.base().layer().is_open() {
            return;
        }
        self.view.base_mut().set_active_with_context(false, ctx);
        let Some(id) = self.menu_target.take() else {
            return;
        };
        match action {
            Some(ChangeSetQuickAction::Clone) => self.show_clone(&id, ctx),
            Some(ChangeSetQuickAction::Rename) => self.show_rename(&id, ctx),
            Some(ChangeSetQuickAction::Delete) => {
                self.view.base_mut().base_mut().request_delete(&id, ctx)
            }
            Some(ChangeSetQuickAction::Archive) => {
                self.show_archive(&id, ctx);
            }
            None => {}
        }
    }

    fn show_rename(&mut self, id: &str, ctx: &mut EventCtx<()>) {
        let Some(name) = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .map(|set| set.name.clone())
        else {
            return;
        };
        self.rename_requested.borrow_mut().take();
        self.rename_dismissed.replace(false);
        self.rename_target = Some(id.into());
        self.view.replace_layer(
            rename_dialog(
                &self.keys,
                Rc::clone(&self.rename_requested),
                Rc::clone(&self.rename_dismissed),
                &name,
            ),
            ctx,
        );
        self.view.set_active_with_context(true, ctx);
        ctx.focus(FocusRequest::Target(FocusId::new("input")));
        self.view.layer_mut().child_mut().enter_insert_mode();
    }

    fn drain_rename_dialog(&mut self, ctx: &mut EventCtx<()>) {
        let requested = self.rename_requested.borrow_mut().take();
        let dismissed = self.rename_dismissed.replace(false);
        if requested.is_none() && !dismissed {
            return;
        }
        if requested.is_none() {
            self.rename_target.take();
            self.view.set_active_with_context(false, ctx);
            return;
        }
        let name = requested.unwrap_or_default();
        if name.is_empty() {
            return;
        }
        let Some(id) = self.rename_target.take() else {
            return;
        };
        let result = {
            let mut state = self.state.borrow_mut();
            state
                .dispatch(ComposerAction::RenameChangeSetById {
                    id: id.clone(),
                    name,
                })
                .map_err(|error| error.to_string())
                .map(|()| state.change_sets.iter().find(|set| set.id == id).cloned())
        };
        self.view.set_active_with_context(false, ctx);
        match result {
            Ok(Some(set)) => self.service.save_change_set(set),
            Ok(None) => {}
            Err(error) => self
                .service
                .report_notification(tuicore::Notification::warning("Cannot rename", error)),
        }
        self.sync();
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn submit_rename_on_ctrl_enter(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if self.rename_target.is_none()
            || !self.view.is_active()
            || !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return false;
        }
        *self.rename_requested.borrow_mut() =
            Some(self.view.layer().child().current_value().trim().into());
        self.drain_rename_dialog(ctx);
        ctx.stop_propagation();
        true
    }

    fn show_clone(&mut self, id: &str, ctx: &mut EventCtx<()>) {
        let Some(source) = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .cloned()
        else {
            return;
        };
        if !source.closed {
            self.service
                .report_notification(tuicore::Notification::warning(
                    "Cannot clone",
                    "Only archived change sets can be cloned",
                ));
            return;
        }
        self.clone_requested.borrow_mut().take();
        self.clone_dismissed.replace(false);
        let dialog = clone_dialog(
            &self.keys,
            Rc::clone(&self.clone_requested),
            Rc::clone(&self.clone_dismissed),
            &format!("Clone of {}", source.name),
        );
        self.clone_target = Some(source);
        self.view.replace_layer(dialog, ctx);
        self.view.set_active_with_context(true, ctx);
        ctx.focus(FocusRequest::Target(FocusId::new("input")));
        self.view.layer_mut().child_mut().enter_insert_mode();
    }

    fn drain_clone_dialog(&mut self, ctx: &mut EventCtx<()>) {
        let requested = self.clone_requested.borrow_mut().take();
        if requested.is_none() && !self.clone_dismissed.replace(false) {
            return;
        }
        if requested.is_none() {
            self.clone_target.take();
            self.view.set_active_with_context(false, ctx);
            return;
        }
        let name = requested.unwrap_or_default();
        if name.is_empty() {
            return;
        }
        let Some(source) = self.clone_target.take() else {
            return;
        };
        self.view.set_active_with_context(false, ctx);
        self.clone_in_progress = true;
        let service = self.service.clone();
        let sender = self.clone_sender.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("finery-clone-{}", source.id))
            .spawn(move || {
                let result = service
                    .composer_service()
                    .clone_change_set(&source.id, name)
                    .map_err(|error| error.to_string());
                let _ = sender.send(result);
            })
        {
            self.clone_in_progress = false;
            self.service
                .report_error(format!("could not clone change set: {error}"));
        }
    }

    fn submit_clone_on_ctrl_enter(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if self.clone_target.is_none()
            || !self.view.is_active()
            || !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return false;
        }
        *self.clone_requested.borrow_mut() =
            Some(self.view.layer().child().current_value().trim().into());
        self.drain_clone_dialog(ctx);
        ctx.stop_propagation();
        true
    }

    fn drain_clone_result(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.clone_receiver.try_recv() {
            self.clone_in_progress = false;
            match result {
                Ok(cloned) => {
                    if let Err(error) = self.service.record_cloned_change_set(&cloned) {
                        self.service
                            .report_notification(tuicore::Notification::error(
                                "Clone failed",
                                error,
                            ));
                        continue;
                    }
                    let cloned_id = cloned.change_set.id.clone();
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::CloneChangeSet(cloned.change_set))
                        .expect("cloned change set is valid");
                    self.view
                        .base_mut()
                        .base_mut()
                        .show_open_change_set(&cloned_id);
                    self.service
                        .report_notification(tuicore::Notification::success(
                            "Change set cloned",
                            format!("Created {cloned_id}"),
                        ));
                    changed = true;
                }
                Err(error) => self
                    .service
                    .report_notification(tuicore::Notification::error("Clone failed", error)),
            }
        }
        changed
    }
}

fn clone_dialog(
    keys: &ComposerKeyBindings,
    requested: Rc<RefCell<Option<String>>>,
    dismissed: Rc<RefCell<bool>>,
    prefilled: &str,
) -> ChangeSetDialog {
    let submit = Rc::clone(&requested);
    let cancel = Rc::clone(&dismissed);
    let close = Rc::clone(&dismissed);
    let value = Rc::new(RefCell::new(prefilled.to_owned()));
    let submitted_value = Rc::clone(&value);
    Dialog::new()
        .top_left("Clone change set")
        .actions([
            DialogAction::new("Clone")
                .hotkey(keys.create_confirm.spec())
                .on_trigger(move || {
                    *submit.borrow_mut() = Some(submitted_value.borrow().trim().into())
                }),
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || *cancel.borrow_mut() = true),
        ])
        .close_on_unfocus_from_descendants(true)
        .on_close(move |_| *close.borrow_mut() = true)
        .host(WideTextInput::new(
            TextInput::new()
                .style(InputChrome::plain())
                .value(prefilled)
                .placeholder("Change set title")
                .focused(true)
                .on_change(move |name| *value.borrow_mut() = name),
        ))
}

fn rename_dialog(
    keys: &ComposerKeyBindings,
    requested: Rc<RefCell<Option<String>>>,
    dismissed: Rc<RefCell<bool>>,
    prefilled: &str,
) -> ChangeSetDialog {
    let submit = Rc::clone(&requested);
    let cancel = Rc::clone(&dismissed);
    let close = Rc::clone(&dismissed);
    let value = Rc::new(RefCell::new(prefilled.to_owned()));
    let submitted_value = Rc::clone(&value);
    Dialog::new()
        .top_left("Rename change set")
        .actions([
            DialogAction::new("OK")
                .hotkey(keys.create_confirm.spec())
                .on_trigger(move || {
                    *submit.borrow_mut() = Some(submitted_value.borrow().trim().into())
                }),
            DialogAction::new("Cancel")
                .hotkey(keys.dialog_cancel.spec())
                .on_trigger(move || *cancel.borrow_mut() = true),
        ])
        .close_on_unfocus_from_descendants(true)
        .on_close(move |_| *close.borrow_mut() = true)
        .host(WideTextInput::new(
            TextInput::new()
                .style(InputChrome::plain())
                .value(prefilled)
                .placeholder("Change set title")
                .focused(true)
                .on_change(move |name| *value.borrow_mut() = name),
        ))
}

impl TuiNode for ChangeSetOverview {
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
        if self.open_quick_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_archive(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_rename(event, ctx) || self.open_clone(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.submit_rename_on_ctrl_enter(event, ctx)
            || self.submit_clone_on_ctrl_enter(event, ctx)
        {
            return EventOutcome::Handled;
        }
        let outcome = self.view.event(event, ctx);
        self.drain_quick_menu(ctx);
        self.drain_outcome(ctx);
        self.drain_rename_dialog(ctx);
        self.drain_clone_dialog(ctx);
        self.drain_clone_result();
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.open_quick_menu(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_archive(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.open_rename(event, ctx) || self.open_clone(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.submit_rename_on_ctrl_enter(event, ctx)
            || self.submit_clone_on_ctrl_enter(event, ctx)
        {
            return EventOutcome::Handled;
        }
        let outcome = self.view.dispatch_event(route, event, ctx);
        self.drain_quick_menu(ctx);
        self.drain_outcome(ctx);
        self.drain_rename_dialog(ctx);
        self.drain_clone_dialog(ctx);
        self.drain_clone_result();
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let outcome = self.view.tick(dt, settings);
        if self.drain_clone_result() {
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
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.view.destroy(ctx);
    }
}
