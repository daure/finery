use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Text},
    Frame,
};
use tuicore::{
    keybindings, ActivationMode, AnimationSettings, Button, CellContext, ChildKey, Column, Dialog,
    DialogAction, DialogBackdrop, DialogLayer, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusId, FocusRequest, FocusTarget, HotkeyEvent, HotkeyLabelMode, InputChrome, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, ListControlKeyBindings, MenuButton, MenuItem, RenderCtx,
    TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::ComposerKeyBindings,
    service::AppService,
    store::composer::{ChangeKind, ChangeSet, ComposerAction, ComposerState, Ticket, TicketChange},
};

#[derive(Clone)]
struct ChangeSetRow {
    id: String,
    name: String,
    subtitle: String,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum ChangeSetFilter {
    All,
    Open,
    Closed,
}

impl ChangeSetFilter {
    const OPTIONS: [Self; 3] = [Self::All, Self::Open, Self::Closed];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Open => "Open",
            Self::Closed => "Closed",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Open => "",
            Self::Closed => "",
        }
    }

    fn menu_label(self) -> String {
        format!("{} {}", self.icon(), self.label())
    }

    fn contains(self, change_set: &ChangeSet) -> bool {
        match self {
            Self::All => true,
            Self::Open => !change_set.closed,
            Self::Closed => change_set.closed,
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
}

type ChangeSetControl = ListControl<ChangeSetRow, String>;
type ChangeSetDialog = tuicore::DialogHost<WideTextInput, ()>;
type ChangeSetView = DialogLayer<ChangeSetContent, ChangeSetDialog>;

const CHANGE_SET_DIALOG_WIDTH: u16 = 48;

struct WideTextInput {
    input: TextInput<()>,
}

impl WideTextInput {
    fn new(input: TextInput<()>) -> Self {
        Self { input }
    }

    fn current_value(&self) -> &str {
        self.input.current_value()
    }

    fn reset(&mut self) {
        self.input.set_value("");
        self.input.set_insert_mode(true);
    }
}

impl TuiNode for WideTextInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let input = self.input.measure(proposal);
        LayoutSizeHint::content(
            input.preferred.width.max(CHANGE_SET_DIALOG_WIDTH),
            input.preferred.height,
        )
        .normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        self.input.render(frame, area);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.input.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.input.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx)
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx)
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx)
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx)
    }
}

struct ChangeSetContent {
    new_button: Button<()>,
    filter_menu: MenuButton<ChangeSetFilter, ()>,
    control: ChangeSetControl,
    button_area: Rect,
    filter_area: Rect,
    control_area: Rect,
}

impl ChangeSetContent {
    fn new(
        new_button: Button<()>,
        filter_menu: MenuButton<ChangeSetFilter, ()>,
        control: ChangeSetControl,
    ) -> Self {
        Self {
            new_button,
            filter_menu,
            control,
            button_area: Rect::default(),
            filter_area: Rect::default(),
            control_area: Rect::default(),
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
            ["ys".to_owned()],
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
            if path.keys().len() == 1
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
            if matches!(event, TuiEvent::Key(key) if keybindings().focus().unfocus_matches(*key))
            {
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
        .copy_with(|row| format!("Finery {} \"{}\"", row.id, escape_reference(&row.name)))
        .row_height(2)
        .panel_visible(false)
        .action_bar(true)
        .filter_controls(false)
        .keybindings(ListControlKeyBindings {
            add: Vec::new(),
            ..ListControlKeyBindings::default()
        })
        .activation_mode(ActivationMode::OnActivateKey)
        .confirm_remove("Delete change set?", |row| {
            format!(
                "Delete {} · {}? This removes its local ticket snapshots.",
                row.id, row.name
            )
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
        }
    }

    pub(super) fn sync(&mut self) {
        self.view
            .base_mut()
            .control
            .data_view_mut()
            .set_rows(rows(&self.state.borrow(), self.filter));
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
            match event {
                ListControlEvent::Removed { row_id } => {
                    if self.state.borrow().change_set_is_submitting(&row_id) {
                        self.service
                            .report_notification(tuicore::Notification::error(
                                "Delete blocked",
                                "Cannot delete a change set with an unresolved Jira submission attempt",
                            ));
                        self.sync();
                        ctx.request_layout();
                        ctx.request_redraw();
                        continue;
                    }
                    let _ = self
                        .state
                        .borrow_mut()
                        .dispatch(ComposerAction::DeleteChangeSet(row_id.clone()));
                    self.service.delete_change_set(row_id);
                }
                _ => {}
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

    #[cfg(test)]
    pub(super) fn highlighted_change_set(&self) -> Option<String> {
        self.view.base().control.data_view().highlighted_id()
    }
}

fn rows(state: &ComposerState, filter: ChangeSetFilter) -> Vec<ChangeSetRow> {
    let mut rows: Vec<_> = state
        .change_sets
        .iter()
        .filter(|set| filter.contains(set))
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

fn change_set_share_text(change_set: &ChangeSet, base_url: Option<&str>) -> String {
    let aliases = change_set
        .tickets
        .iter()
        .filter_map(|change| {
            ticket_for_change(change).map(|ticket| (ticket.key.as_str(), change.id.as_str()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut children = std::collections::HashMap::<Option<&str>, Vec<&TicketChange>>::new();
    for change in &change_set.tickets {
        let parent = ticket_for_change(change)
            .and_then(|ticket| ticket.parent_key.as_deref())
            .and_then(|parent| aliases.get(parent).copied());
        children.entry(parent).or_default().push(change);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|change| (change.sibling_order, change.id.as_str()));
    }
    let mut lines = vec![change_set.name.clone()];
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
    for change in &change_set.tickets {
        if visited.insert(change.id.as_str()) {
            append_share_line(change, "", &mut lines, change_set, base_url);
        }
    }
    lines.join("\n")
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
        append_share_line(
            change,
            &format!("{prefix}{branch}"),
            lines,
            change_set,
            base_url,
        );
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
    let action = match change.kind {
        ChangeKind::Added => "Created",
        ChangeKind::Modified => "Updated",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Synced => "Unchanged",
    };
    let reference = (!ticket.key.starts_with("NEW-"))
        .then(|| base_url.map(|url| format!("{url}/browse/{}", ticket.key)))
        .flatten()
        .unwrap_or_else(|| format!("Draft {}/{}", change_set.id, change.id));
    lines.push(format!("{prefix}{action} - {reference} - {}", ticket.title));
}

fn ticket_for_change(change: &TicketChange) -> Option<&Ticket> {
    change
        .submitted
        .as_ref()
        .and_then(|snapshot| snapshot.updated.as_ref().or(snapshot.original.as_ref()))
        .or(change.updated.as_ref())
        .or(change.original.as_ref())
}
