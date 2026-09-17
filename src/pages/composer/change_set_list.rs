use std::{cell::RefCell, rc::Rc, time::Duration};

#[path = "change_set_summary.rs"]
mod summary;

use crate::store::composer::summary::ChangeSetSummary;

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};
use tuicore::{
    ActivationMode, AnimationSettings, Button, CellContext, ChildKey, Column, Dialog, DialogAction,
    DialogBackdrop, DialogLayer, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId,
    FocusRequest, FocusTarget, HotkeyEvent, HotkeyLabelMode, InputChrome, Key, KeyModifiers,
    KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl,
    ListControlEvent, ListControlKeyBindings, MenuButton, MenuItem, RenderCtx, TextInput,
    TickResult, TuiEvent, TuiNode, keybindings, line_width,
};

use crate::{
    app_settings::ComposerKeyBindings,
    service::AppService,
    store::composer::{
        ArchiveOutcome, ChangeKind, ChangeSet, ComposerAction, ComposerState, Ticket, TicketChange,
        TicketKind,
    },
};

#[derive(Clone)]
struct ChangeSetRow {
    id: String,
    name: String,
    status: ChangeSetStatus,
    summary: ChangeSetSummary,
}

#[derive(Clone, Copy)]
enum ChangeSetStatus {
    Open,
    Rejected,
    Done,
}

impl ChangeSetStatus {
    fn from_change_set(change_set: &ChangeSet) -> Self {
        match change_set.archive_outcome {
            Some(ArchiveOutcome::Cancelled) => Self::Rejected,
            Some(ArchiveOutcome::Concluded) => Self::Done,
            None if !change_set.tickets.is_empty()
                && change_set.submitted_count() == change_set.tickets.len() =>
            {
                Self::Done
            }
            None => Self::Open,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Open => "",
            Self::Rejected | Self::Done => "",
        }
    }

    fn style(self) -> Style {
        let theme = tuicore::theme();
        match self {
            Self::Open => Style::default().fg(theme.accent_fg()),
            Self::Rejected | Self::Done => Style::default().fg(theme.text_fg()),
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum ChangeSetFilter {
    All,
    Open,
    Archived,
}

impl ChangeSetFilter {
    const OPTIONS: [Self; 3] = [Self::All, Self::Open, Self::Archived];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Open => "Open",
            Self::Archived => "Archived",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Open => "",
            Self::Archived => "",
        }
    }

    fn menu_label(self) -> String {
        format!("{} {}", self.icon(), self.label())
    }

    fn contains(self, change_set: &ChangeSet) -> bool {
        match self {
            Self::All => true,
            Self::Open => !change_set.closed,
            Self::Archived => change_set.closed,
        }
    }
}

pub(super) struct ChangeSetListView {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    view: ChangeSetView,
    filter: ChangeSetFilter,
    new_change_set_requested: Rc<RefCell<Option<String>>>,
    dialog_close_requested: Rc<RefCell<bool>>,
    delete_key: crate::app_settings::ComposerKeyBinding,
}

type ChangeSetControl = ListControl<ChangeSetRow, String>;
pub(super) type ChangeSetDialog = tuicore::DialogHost<WideTextInput, ()>;
type ChangeSetView = DialogLayer<ChangeSetContent, ChangeSetDialog>;

const CHANGE_SET_DIALOG_WIDTH: u16 = 48;

pub(super) struct WideTextInput {
    input: TextInput<()>,
    visible: bool,
    input_visible: bool,
    prose: Option<String>,
    prose_area: Rect,
    input_area: Rect,
}

impl WideTextInput {
    pub(super) fn new(input: TextInput<()>) -> Self {
        Self {
            input,
            visible: true,
            input_visible: true,
            prose: None,
            prose_area: Rect::default(),
            input_area: Rect::default(),
        }
    }

    pub(super) fn prose_only(prose: impl Into<String>) -> Self {
        Self {
            input: TextInput::new(),
            visible: true,
            input_visible: false,
            prose: Some(prose.into()),
            prose_area: Rect::default(),
            input_area: Rect::default(),
        }
    }

    pub(super) fn current_value(&self) -> &str {
        self.input.current_value()
    }

    pub(super) fn enter_insert_mode(&mut self) {
        self.input.set_insert_mode(true);
    }

    fn reset(&mut self) {
        self.input.set_value("");
        self.input.set_insert_mode(true);
    }
}

impl TuiNode for WideTextInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if !self.visible {
            return LayoutSizeHint::content(0, 0).normalized(proposal);
        }
        let input = self.input_visible.then(|| self.input.measure(proposal));
        let width = input
            .as_ref()
            .map(|input| input.preferred.width)
            .unwrap_or_default()
            .max(CHANGE_SET_DIALOG_WIDTH);
        LayoutSizeHint::content(
            width,
            input
                .as_ref()
                .map(|input| input.preferred.height)
                .unwrap_or_default()
                .saturating_add(self.prose_height(width)),
        )
        .normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if self.visible {
            let prose_height = self.prose_height(area.width).min(area.height);
            self.prose_area = Rect::new(area.x, area.y, area.width, prose_height);
            self.input_area = Rect::new(
                area.x,
                area.y.saturating_add(prose_height),
                area.width,
                area.height.saturating_sub(prose_height),
            );
            if self.input_visible {
                self.input.layout(self.input_area, ctx)
            } else {
                LayoutResult::new(area)
            }
        } else {
            LayoutResult::new(area)
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, _ctx: &mut RenderCtx<'a>) {
        if self.visible {
            if let Some(prose) = &self.prose {
                frame.render_widget(
                    Paragraph::new(prose.as_str())
                        .style(Style::default().fg(tuicore::theme().text_fg()))
                        .wrap(Wrap { trim: true }),
                    self.prose_area,
                );
            }
            if self.input_visible {
                self.input.render(frame, self.input_area);
            }
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.input_visible {
            self.input.event(event, ctx)
        } else {
            EventOutcome::Ignored
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.input_visible {
            self.input.dispatch_event(route, event, ctx)
        } else {
            EventOutcome::Ignored
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        if self.input_visible {
            self.input.tick(dt, settings)
        } else {
            TickResult::IDLE
        }
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.input_visible {
            self.input.focus(target, focused, ctx);
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if self.input_visible {
            self.input.dispatch_focus(target, focused, ctx);
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        if self.input_visible {
            self.input.init(ctx);
        }
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        if self.input_visible {
            self.input.mount(ctx);
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        if self.input_visible {
            self.input.unmount(ctx);
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        if self.input_visible {
            self.input.destroy(ctx);
        }
    }
}

impl WideTextInput {
    fn prose_height(&self, width: u16) -> u16 {
        let Some(prose) = &self.prose else {
            return 0;
        };
        let width = usize::from(width.max(1));
        let mut lines = 1usize;
        let mut current_width = 0usize;
        for word in prose.split_whitespace() {
            let word_width = line_width(&Line::from(word));
            let separator = usize::from(current_width > 0);
            if current_width > 0 && current_width + separator + word_width > width {
                lines = lines.saturating_add(1);
                current_width = word_width;
            } else {
                current_width += separator + word_width;
            }
        }
        lines as u16
    }
}

struct ChangeSetContent {
    new_button: Button<()>,
    filter_menu: MenuButton<ChangeSetFilter, ()>,
    control: ChangeSetControl,
    button_area: Rect,
    filter_area: Rect,
    control_area: Rect,
    archive_sequence: String,
    actions_sequence: String,
}

impl ChangeSetContent {
    fn new(
        new_button: Button<()>,
        filter_menu: MenuButton<ChangeSetFilter, ()>,
        control: ChangeSetControl,
        archive_sequence: String,
        actions_sequence: String,
    ) -> Self {
        Self {
            new_button,
            filter_menu,
            control,
            button_area: Rect::default(),
            filter_area: Rect::default(),
            control_area: Rect::default(),
            archive_sequence,
            actions_sequence,
        }
    }

    fn take_filter(&mut self) -> Option<ChangeSetFilter> {
        self.filter_menu.take_activated().into_iter().last()
    }
}

impl TuiNode for ChangeSetContent {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let button = self.new_button.measure(proposal);
        let filter = self.filter_menu.measure(proposal);
        let control = self.control.measure(proposal);
        LayoutSizeHint::content(
            button
                .preferred
                .width
                .saturating_add(filter.preferred.width)
                .max(control.preferred.width),
            button
                .preferred
                .height
                .max(filter.preferred.height)
                .saturating_add(control.preferred.height),
        )
        .normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let button_height = self
            .new_button
            .measure(LayoutProposal::unbounded())
            .preferred
            .height
            .max(
                self.filter_menu
                    .measure(LayoutProposal::unbounded())
                    .preferred
                    .height,
            )
            .min(area.height);
        let filter_width = self
            .filter_menu
            .measure(LayoutProposal::unbounded())
            .preferred
            .width
            .min(area.width);
        let button_width = self
            .new_button
            .measure(LayoutProposal::unbounded())
            .preferred
            .width
            .min(area.width.saturating_sub(filter_width));
        self.button_area = Rect::new(area.x, area.y, button_width, button_height);
        self.filter_area = Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(filter_width)),
            area.y,
            filter_width,
            button_height,
        );
        self.control_area = Rect::new(
            area.x,
            area.y.saturating_add(button_height),
            area.width,
            area.height.saturating_sub(button_height),
        );
        ctx.with_focus_fallback_hotkey_sequences_status(
            FocusId::new("data-view"),
            self.control_area,
            [
                "ys".to_owned(),
                "yp".to_owned(),
                self.archive_sequence.clone(),
                self.actions_sequence.clone(),
            ],
            |ctx| {
                ctx.push_slot(ChildKey::new("change-sets"), self.control_area, |ctx| {
                    self.control.layout(self.control_area, ctx);
                });
            },
        );
        ctx.push_slot(
            ChildKey::new("change-set-filter"),
            self.filter_area,
            |ctx| {
                self.filter_menu.layout(self.filter_area, ctx);
            },
        );
        ctx.push_slot(ChildKey::new("new-change-set"), self.button_area, |ctx| {
            self.new_button.layout(self.button_area, ctx);
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.new_button.render(frame, self.button_area);
        self.filter_menu.render(frame, self.filter_area, ctx);
        self.control.render(frame, self.control_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.new_button.event(event, ctx);
        if outcome == EventOutcome::Handled {
            outcome
        } else {
            let outcome = self.filter_menu.event(event, ctx);
            if outcome == EventOutcome::Handled {
                outcome
            } else {
                self.control.event(event, ctx)
            }
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let Some(path) = route
            .path
            .without_first_if(&ChildKey::new("new-change-set"))
        {
            let outcome = self
                .new_button
                .dispatch_event(&EventRoute::new(path), event, ctx);
            if outcome == EventOutcome::Ignored
                && matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key))
            {
                ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
            return outcome;
        }
        if let Some(path) = route.path.without_first_if(&ChildKey::new("change-sets")) {
            if !self.control.is_confirming_remove()
                && path.keys().len() == 1
                && !self.control.data_view().is_searching()
                && matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key))
            {
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
            let outcome = self
                .control
                .dispatch_event(&EventRoute::new(path), event, ctx);
            return if outcome == EventOutcome::Ignored {
                let outcome = self.new_button.event(event, ctx);
                if outcome == EventOutcome::Ignored {
                    self.filter_menu.event(event, ctx)
                } else {
                    outcome
                }
            } else {
                outcome
            };
        }
        if let Some(path) = route
            .path
            .without_first_if(&ChildKey::new("change-set-filter"))
        {
            let outcome = self
                .filter_menu
                .dispatch_event(&EventRoute::new(path), event, ctx);
            if matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key)) {
                ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
            return outcome;
        }
        EventOutcome::Ignored
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.new_button
            .tick(dt, settings)
            .merge(self.filter_menu.tick(dt, settings))
            .merge(self.control.tick(dt, settings))
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.new_button.focus(target, focused, ctx);
        self.filter_menu.focus(target, focused, ctx);
        self.control.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        if let Some(target) = target.for_child(&ChildKey::new("new-change-set")) {
            self.new_button.dispatch_focus(&target, focused, ctx);
        }
        if let Some(target) = target.for_child(&ChildKey::new("change-sets")) {
            self.control.dispatch_focus(&target, focused, ctx);
        }
        if let Some(target) = target.for_child(&ChildKey::new("change-set-filter")) {
            self.filter_menu.dispatch_focus(&target, focused, ctx);
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.new_button.init(ctx);
        self.filter_menu.init(ctx);
        self.control.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.new_button.mount(ctx);
        self.filter_menu.mount(ctx);
        self.control.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.new_button.unmount(ctx);
        self.filter_menu.unmount(ctx);
        self.control.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.new_button.destroy(ctx);
        self.filter_menu.destroy(ctx);
        self.control.destroy(ctx);
    }
}

impl ChangeSetListView {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        service: AppService,
        keys: ComposerKeyBindings,
    ) -> Self {
        let filter = ChangeSetFilter::Open;
        let rows = rows(&state.borrow(), filter);
        let delete_state = Rc::clone(&state);
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
                    status: ChangeSetStatus::Open,
                    summary: ChangeSetSummary::default(),
                }
            },
        )
        .column(change_set_column())
        .copy_with(|row| format!("Finery {} \"{}\"", row.id, escape_reference(&row.name)))
        .row_height(2)
        .panel_visible(false)
        .action_bar(true)
        .filter_controls(false)
        .keybindings(ListControlKeyBindings {
            add: Vec::new(),
            remove: vec![keys.delete_change_set.spec()],
            ..ListControlKeyBindings::default()
        })
        .activation_mode(ActivationMode::OnActivateKey)
        .confirm_remove("Delete change set?", move |row| {
            let description = format!(
                "Delete {} · {}? This removes its local ticket snapshots.",
                row.id, row.name
            );
            if delete_state
                .borrow()
                .change_set_has_submission_attempt(&row.id)
            {
                format!(
                    "{description}\n\nWARNING: This deletes Jira submission recovery data. Jira may contain tickets that Finery cannot reconcile."
                )
            } else {
                description
            }
        });
        let new_change_set_requested = Rc::new(RefCell::new(None));
        let request_new_change_set = Rc::clone(&new_change_set_requested);
        let dialog_close_requested = Rc::new(RefCell::new(false));
        let cancel_dialog = Rc::clone(&dialog_close_requested);
        let close_dialog = Rc::clone(&dialog_close_requested);
        let input_value = Rc::new(RefCell::new(String::new()));
        let reset_input_value = Rc::clone(&input_value);
        let submit_value = Rc::clone(&input_value);
        let submit_new_change_set = Rc::clone(&new_change_set_requested);
        let dialog = Dialog::new()
            .top_left("New change set")
            .actions([
                DialogAction::new("OK")
                    .hotkey(keys.create_confirm.spec())
                    .on_trigger(move || {
                        *submit_new_change_set.borrow_mut() =
                            Some(submit_value.borrow().trim().into())
                    }),
                DialogAction::new("Cancel")
                    .hotkey(keys.dialog_cancel.spec())
                    .on_trigger(move || *cancel_dialog.borrow_mut() = true),
            ])
            .close_on_unfocus_from_descendants(true)
            .on_close(move |_| *close_dialog.borrow_mut() = true)
            .host(WideTextInput::new(
                TextInput::new()
                    .style(InputChrome::plain())
                    .placeholder("Change set title")
                    .focused(true)
                    .on_change(move |value| *input_value.borrow_mut() = value),
            ));
        let view = DialogLayer::new(
            ChangeSetContent::new(
                Button::new("New change set")
                    .hotkey(keys.new_change_set.sequence())
                    .on_press(move || {
                        *reset_input_value.borrow_mut() = String::new();
                        *request_new_change_set.borrow_mut() = Some(String::new());
                    }),
                change_set_filter_menu(filter, &keys.change_set_filter),
                control,
                keys.archive.sequence().to_owned(),
                keys.change_set_actions.sequence().to_owned(),
            ),
            dialog,
        )
        .active(false)
        .fit_content()
        .child_overlays_use_base_bounds(true)
        .backdrop(DialogBackdrop::dim().amount(0.55));
        Self {
            state,
            service,
            view,
            filter,
            new_change_set_requested,
            dialog_close_requested,
            delete_key: keys.delete_change_set,
        }
    }

    pub(super) fn sync(&mut self) {
        self.view
            .base_mut()
            .control
            .data_view_mut()
            .set_rows(rows(&self.state.borrow(), self.filter));
    }

    pub(super) fn reset_to_home(&mut self, ctx: &mut EventCtx<()>) {
        self.view.set_active_with_context(false, ctx);
        self.filter = ChangeSetFilter::Open;
        let content = self.view.base_mut();
        content.filter_menu.close();
        content
            .filter_menu
            .set_label(ChangeSetFilter::Open.menu_label());
        content.control.data_view_mut().clear_search();
        self.sync();
    }

    fn create_change_set(&mut self, name: String, ctx: &mut EventCtx<()>) {
        if name.is_empty() {
            return;
        }
        let next = self
            .state
            .borrow()
            .change_sets
            .iter()
            .filter_map(|set| set.id.strip_prefix("CS-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("CS-{next}");
        let _ = self
            .state
            .borrow_mut()
            .dispatch(ComposerAction::CreateChangeSet {
                id: id.clone(),
                name,
            });
        if let Some(set) = self
            .state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == id)
            .cloned()
        {
            self.service.save_change_set(set);
        }
        self.sync();
        self.view
            .base_mut()
            .control
            .data_view_mut()
            .highlight_id(&id);
        let _ = self
            .state
            .borrow_mut()
            .dispatch(ComposerAction::OpenChangeSet(id));
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn drain_events(&mut self, ctx: &mut EventCtx<()>) {
        if let Some(filter) = self.view.base_mut().take_filter() {
            self.filter = filter;
            self.view
                .base_mut()
                .filter_menu
                .set_label(filter.menu_label());
            self.sync();
            ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
            ctx.request_layout();
            ctx.request_redraw();
        }
        let new_change_set = self.new_change_set_requested.borrow_mut().take();
        if let Some(name) = new_change_set {
            if name.is_empty() {
                self.view.layer_mut().child_mut().reset();
                self.view.set_active_with_context(true, ctx);
            } else {
                self.view.set_active_with_context(false, ctx);
                self.create_change_set(name, ctx);
            }
        }
        if self.dialog_close_requested.replace(false) {
            self.view.set_active_with_context(false, ctx);
        }
        for event in self.view.base_mut().control.take_events() {
            if let ListControlEvent::Removed { row_id } = event {
                let deleted = {
                    let mut state = self.state.borrow_mut();
                    let _ = state.dispatch(ComposerAction::DeleteChangeSet(row_id.clone()));
                    !state.change_sets.iter().any(|set| set.id == row_id)
                };
                if !deleted {
                    self.sync();
                    ctx.request_layout();
                    ctx.request_redraw();
                    continue;
                }
                self.service.delete_change_set(row_id);
            }
        }
        for event in self.view.base_mut().control.data_view_mut().drain_events() {
            if let tuicore::DataViewTypedEvent::Activated { row_id } = event {
                let _ = self
                    .state
                    .borrow_mut()
                    .dispatch(ComposerAction::OpenChangeSet(row_id));
                ctx.request_layout();
                ctx.request_redraw();
            }
        }
    }

    fn submit_on_ctrl_enter(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !self.view.is_active()
            || !matches!(event, TuiEvent::Key(key) if KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key))
        {
            return false;
        }
        let name = self.view.layer().child().current_value().trim().into();
        *self.new_change_set_requested.borrow_mut() = Some(name);
        self.drain_events(ctx);
        ctx.stop_propagation();
        true
    }

    fn yank_share(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) if sequence == "ys") {
            return false;
        }
        let Some(id) = self.view.base().control.data_view().highlighted_id() else {
            return false;
        };
        let state = self.state.borrow();
        let Some(change_set) = state.change_sets.iter().find(|set| set.id == id) else {
            return false;
        };
        let base_url = self
            .service
            .settings()
            .read()
            .ok()
            .map(|settings| {
                settings
                    .jira_base_url
                    .trim()
                    .trim_end_matches('/')
                    .to_owned()
            })
            .filter(|url| !url.is_empty());
        ctx.copy_to_clipboard(change_set_share_text(change_set, base_url.as_deref()));
        ctx.stop_propagation();
        true
    }

    fn yank_prepare(&self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        if !matches!(event, TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) if sequence == "yp")
            || !self.prepare_yank_available()
        {
            return false;
        }
        let Some(id) = self.view.base().control.data_view().highlighted_id() else {
            return false;
        };
        let state = self.state.borrow();
        let Some(change_set) = state.change_sets.iter().find(|set| set.id == id) else {
            return false;
        };
        ctx.copy_to_clipboard(change_set_prepare_text(change_set));
        ctx.stop_propagation();
        true
    }

    pub(super) fn highlighted_change_set(&self) -> Option<String> {
        self.view.base().control.data_view().highlighted_id()
    }

    pub(super) fn highlight_change_set(&mut self, id: &str) {
        self.view
            .base_mut()
            .control
            .data_view_mut()
            .highlight_id(&id.to_owned());
    }

    pub(super) fn show_open_change_set(&mut self, id: &str) {
        self.filter = ChangeSetFilter::Open;
        self.view
            .base_mut()
            .filter_menu
            .set_label(ChangeSetFilter::Open.menu_label());
        self.sync();
        self.highlight_change_set(id);
    }

    pub(super) fn request_delete(&mut self, id: &str, ctx: &mut EventCtx<()>) {
        let control = &mut self.view.base_mut().control;
        if !control.data_view().rows().iter().any(|row| row.id == id) {
            return;
        }
        control.data_view_mut().highlight_id(&id.to_owned());
        control.event(&TuiEvent::Key(self.delete_key.event()), ctx);
        self.drain_events(ctx);
    }

    pub(super) fn archive_available(&self) -> bool {
        let content = self.view.base();
        !self.view.is_active()
            && !content.filter_menu.is_open()
            && !content.control.is_confirming_remove()
            && !content.control.is_adding()
            && !content.control.is_editing()
            && !content.control.is_reordering()
            && !content.control.data_view().is_searching()
    }

    fn prepare_yank_available(&self) -> bool {
        self.archive_available() && self.view.base().control.data_view().is_focused()
    }
}

fn rows(state: &ComposerState, filter: ChangeSetFilter) -> Vec<ChangeSetRow> {
    let mut sets: Vec<_> = state
        .change_sets
        .iter()
        .rev()
        .filter(|set| filter.contains(set))
        .collect();
    sets.sort_by_key(|set| {
        (
            set.closed,
            std::cmp::Reverse(if set.closed { set.closed_at } else { None }),
        )
    });
    sets.into_iter()
        .map(|set| ChangeSetRow {
            id: set.id.clone(),
            name: set.name.clone(),
            status: ChangeSetStatus::from_change_set(set),
            summary: ChangeSetSummary::new(set),
        })
        .collect()
}

fn change_set_filter_menu(
    filter: ChangeSetFilter,
    key: &crate::app_settings::ComposerKeyBinding,
) -> MenuButton<ChangeSetFilter, ()> {
    MenuButton::new(
        filter.menu_label(),
        ChangeSetFilter::OPTIONS.map(|option| MenuItem::new(option, option.menu_label())),
    )
    .visible_items(ChangeSetFilter::OPTIONS.len() as u16)
    .min_popup_width(12)
    .hotkey(key.sequence())
    .hotkey_label_mode(HotkeyLabelMode::Inline)
}

fn change_set_column() -> Column<ChangeSetRow, String> {
    Column::multiline(
        "change_set",
        "",
        Constraint::Percentage(100),
        |row: &ChangeSetRow, _: &CellContext<String>| {
            let theme = tuicore::theme();
            let mut summary = summary::summary_line(&row.summary);
            summary.spans.insert(0, Span::raw("  "));
            Text::from(vec![
                Line::from(vec![
                    Span::styled(format!("{} ", row.status.icon()), row.status.style()),
                    Span::styled(
                        format!("{} ", row.id),
                        Style::default().fg(theme.muted_fg()),
                    ),
                    Span::styled(row.name.clone(), Style::default().fg(theme.text_fg())),
                ]),
                summary,
            ])
        },
    )
    .search_key(|row| format!("{} {}", row.id, row.name))
}

impl TuiNode for ChangeSetListView {
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
        if self.submit_on_ctrl_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.yank_share(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.yank_prepare(event, ctx) {
            return EventOutcome::Handled;
        }
        let outcome = self.view.event(event, ctx);
        self.drain_events(ctx);
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.submit_on_ctrl_enter(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.yank_share(event, ctx) {
            return EventOutcome::Handled;
        }
        if self.yank_prepare(event, ctx) {
            return EventOutcome::Handled;
        }
        let outcome = self.view.dispatch_event(route, event, ctx);
        self.drain_events(ctx);
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.view.tick(dt, settings)
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

fn escape_reference(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn change_set_share_text(change_set: &ChangeSet, base_url: Option<&str>) -> String {
    let includes_non_subtask_change = change_set.tickets.iter().any(|change| {
        change.kind != ChangeKind::Synced
            && ticket_for_change(change).is_some_and(|ticket| ticket.kind != TicketKind::Subtask)
    });
    let changes = change_set
        .tickets
        .iter()
        .filter(|change| {
            ticket_for_change(change).is_some_and(|ticket| {
                !includes_non_subtask_change || ticket.kind != TicketKind::Subtask
            })
        })
        .collect::<Vec<_>>();
    let aliases = changes
        .iter()
        .filter_map(|change| {
            ticket_for_change(change).map(|ticket| (ticket.key.as_str(), change.id.as_str()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut children = std::collections::HashMap::<Option<&str>, Vec<&TicketChange>>::new();
    for change in &changes {
        let parent = ticket_for_change(change)
            .and_then(|ticket| ticket.parent_key.as_deref())
            .and_then(|parent| aliases.get(parent).copied());
        children.entry(parent).or_default().push(change);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|change| (change.sibling_order, change.id.as_str()));
    }
    let mut lines = Vec::new();
    if children.get(&None).is_some_and(|roots| roots.len() > 1) {
        lines.push(change_set.name.clone());
    }
    let mut visited = std::collections::HashSet::new();
    append_share_children(
        None,
        "",
        &children,
        &mut visited,
        &mut lines,
        change_set,
        base_url,
    );
    for change in changes {
        if visited.insert(change.id.as_str()) {
            append_share_line(change, "", &mut lines, change_set, base_url);
        }
    }
    lines.join("\n")
}

fn change_set_prepare_text(change_set: &ChangeSet) -> String {
    format!(
        "finery prepare {} \"{}\"",
        change_set.id,
        escape_reference(&change_set.name)
    )
}

fn append_share_children<'a>(
    parent: Option<&'a str>,
    prefix: &str,
    children: &std::collections::HashMap<Option<&'a str>, Vec<&'a TicketChange>>,
    visited: &mut std::collections::HashSet<&'a str>,
    lines: &mut Vec<String>,
    change_set: &ChangeSet,
    base_url: Option<&str>,
) {
    let Some(siblings) = children.get(&parent) else {
        return;
    };
    for (index, change) in siblings.iter().enumerate() {
        if !visited.insert(change.id.as_str()) {
            continue;
        }
        let last = index + 1 == siblings.len();
        let branch = parent
            .map(|_| if last { "└─ " } else { "├─ " })
            .unwrap_or("");
        let line_prefix = format!("{prefix}{branch}");
        if share_omits_story_heading(change, children) {
            let child = children
                .get(&Some(change.id.as_str()))
                .and_then(|children| children.first())
                .expect("story heading is omitted only when it has one child");
            if visited.insert(child.id.as_str()) {
                append_share_line(child, &line_prefix, lines, change_set, base_url);
                let next_prefix = match parent {
                    Some(_) if last => format!("{prefix}   "),
                    Some(_) => format!("{prefix}│  "),
                    None => String::new(),
                };
                append_share_children(
                    Some(child.id.as_str()),
                    &next_prefix,
                    children,
                    visited,
                    lines,
                    change_set,
                    base_url,
                );
            }
            continue;
        }
        append_share_line(change, &line_prefix, lines, change_set, base_url);
        let next_prefix = match parent {
            Some(_) if last => format!("{prefix}   "),
            Some(_) => format!("{prefix}│  "),
            None => String::new(),
        };
        append_share_children(
            Some(change.id.as_str()),
            &next_prefix,
            children,
            visited,
            lines,
            change_set,
            base_url,
        );
    }
}

fn share_omits_story_heading(
    change: &TicketChange,
    children: &std::collections::HashMap<Option<&str>, Vec<&TicketChange>>,
) -> bool {
    ticket_for_change(change).is_some_and(|ticket| ticket.kind == TicketKind::Story)
        && children
            .get(&Some(change.id.as_str()))
            .is_some_and(|children| children.len() == 1)
}

fn append_share_line(
    change: &TicketChange,
    prefix: &str,
    lines: &mut Vec<String>,
    change_set: &ChangeSet,
    base_url: Option<&str>,
) {
    let Some(ticket) = ticket_for_change(change) else {
        return;
    };
    let reference = (!ticket.key.starts_with("NEW-"))
        .then(|| base_url.map(|url| format!("{url}/browse/{}", ticket.key)))
        .flatten()
        .unwrap_or_else(|| format!("Draft {}/{}", change_set.id, change.id));
    lines.push(format!(
        "{prefix}{} ({}) {} - {reference}",
        share_ticket_type_marker(ticket.kind),
        share_change_kind_label(change.kind),
        ticket.title
    ));
}

fn share_ticket_type_marker(kind: TicketKind) -> &'static str {
    match kind {
        TicketKind::Epic => "[E]",
        TicketKind::Story => "[S]",
        TicketKind::Task => "[T]",
        TicketKind::Bug => "[B]",
        TicketKind::Subtask => "[ST]",
    }
}

fn share_change_kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "Added",
        ChangeKind::Modified => "Modified",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Synced => "Synced",
    }
}

fn ticket_for_change(change: &TicketChange) -> Option<&Ticket> {
    change
        .submitted
        .as_ref()
        .and_then(|snapshot| snapshot.updated.as_ref().or(snapshot.original.as_ref()))
        .or(change.updated.as_ref())
        .or(change.original.as_ref())
}
