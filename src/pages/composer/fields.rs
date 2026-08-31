use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, BorderKind, DiffStyle, DiffViewer, Dropdown, DropdownLabelPosition,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget,
    InputChrome, Language, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    Panel, PanelHost, RenderCtx, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::{ComposerKeyBinding, ComposerKeyBindings},
    store::composer::{ComposerAction, ComposerState, ComposerViewMode},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;

pub(super) type PendingDescriptionActions = Rc<RefCell<Vec<DescriptionAction>>>;
pub(super) type DescriptionEditRequest = Rc<Cell<bool>>;

pub(super) enum DescriptionAction {
    ShowChanges,
    Focus { edit: bool },
    OpenExternalEditor(String),
    OpenSpeedReader(String),
    CloseSpeedReader,
}

#[derive(Clone, Copy)]
enum TextField {
    Title,
}

impl TextField {
    fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
        }
    }

    fn value(self, state: &ComposerState) -> String {
        let Some(ticket) = state.selected_ticket() else {
            return String::new();
        };
        match self {
            Self::Title => ticket.title.clone(),
        }
    }

    fn action(self, value: String) -> ComposerAction {
        match self {
            Self::Title => ComposerAction::UpdateTitle(value),
        }
    }
}

pub(super) struct BoundTextField {
    state: Rc<RefCell<ComposerState>>,
    field: TextField,
    input: TextInput,
    diff: PanelHost<DiffViewer>,
}

impl BoundTextField {
    fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        field: TextField,
        hotkey: ComposerKeyBinding,
    ) -> Self {
        let sink = Rc::clone(&pending);
        let input = TextInput::new()
            .placeholder(field.label())
            .on_edit_end(move |value| sink.borrow_mut().push(field.action(value)));
        let input = if matches!(field, TextField::Title) {
            input.hotkey(hotkey.sequence())
        } else {
            input
        };
        let diff = DiffViewer::new("", "")
            .labels("Source", "Changes")
            .style(DiffStyle::Inline)
            .min_rows(2)
            .max_rows(2)
            .show_headers(false)
            .wrap(true);
        let mut bound = Self {
            state,
            field,
            input,
            diff: Panel::new()
                .border(BorderKind::RoundedDashed)
                .top_right("read-only")
                .host(diff),
        };
        bound.sync();
        bound
    }

    pub(super) fn title(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        hotkey: ComposerKeyBinding,
    ) -> Self {
        Self::new(state, pending, TextField::Title, hotkey)
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = self.field.value(&state);
        let editable = state.selected_is_editable();
        let read_only = matches!(
            state.view_mode,
            ComposerViewMode::Source | ComposerViewMode::Diff
        );
        let key = state
            .selected_ticket()
            .map_or_else(|| "No ticket selected".into(), |ticket| ticket.key.clone());
        let chrome = if read_only {
            InputChrome::panel(key.clone()).top_right("read-only")
        } else {
            InputChrome::panel(key.clone())
        };
        let source = state
            .selected_source()
            .map_or("", |ticket| ticket.title.as_str());
        let changes = state
            .selected_changes()
            .map_or("", |ticket| ticket.title.as_str());
        self.diff
            .child_mut()
            .set_texts(terminated_line(source), terminated_line(changes));
        self.diff.panel_mut().set_top_left(key);
        let value_changed =
            (!self.input.insert_mode() || !editable) && self.input.current_value() != value;
        let changed = value_changed || editable == self.input.is_disabled();
        if value_changed {
            self.input.set_value(value);
            self.input.move_cursor_to_end();
        }
        self.input.set_disabled(!editable);
        self.input.set_style(chrome);
        changed
    }

    fn shows_diff(&self) -> bool {
        let state = self.state.borrow();
        state.view_mode == ComposerViewMode::Diff
            && state.selected_source().map(|ticket| &ticket.title)
                != state.selected_changes().map(|ticket| &ticket.title)
    }

    pub(super) fn height(&self) -> u16 {
        if self.shows_diff() { 4 } else { 3 }
    }
}

fn terminated_line(value: &str) -> String {
    if value.ends_with('\n') {
        value.into()
    } else {
        format!("{value}\n")
    }
}

impl TuiNode for BoundTextField {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if self.shows_diff() {
            self.diff.measure(proposal)
        } else {
            self.input.measure(proposal)
        }
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        if self.shows_diff() {
            self.diff.layout(area, ctx)
        } else {
            self.input.layout(area, ctx)
        }
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        if self.shows_diff() {
            self.diff.render(frame, area, _ctx);
        } else {
            self.input.render(frame, area);
        }
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.shows_diff() {
            self.diff.event(event, ctx)
        } else {
            self.input.event(event, ctx)
        }
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.shows_diff() {
            self.diff.dispatch_event(route, event, ctx)
        } else if route.path.is_empty() {
            self.event(event, ctx)
        } else {
            EventOutcome::Ignored
        }
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        let active = if self.shows_diff() {
            self.diff.tick(dt, settings)
        } else {
            self.input.tick(dt, settings)
        };
        active.merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
        self.diff.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx);
        self.diff.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
        self.diff.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
        self.diff.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.diff.unmount(ctx);
        self.input.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.diff.destroy(ctx);
        self.input.destroy(ctx);
    }
}

pub(super) struct BoundDescription {
    state: Rc<RefCell<ComposerState>>,
    edit_request: DescriptionEditRequest,
    input: TextareaInput,
    diff: DiffViewer,
}

impl BoundDescription {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        edit_request: DescriptionEditRequest,
        keys: &ComposerKeyBindings,
        description_actions: PendingDescriptionActions,
    ) -> Self {
        let sink = Rc::clone(&pending);
        let focus_actions = Rc::clone(&description_actions);
        let reader_actions = Rc::clone(&description_actions);
        let input = TextareaInput::new()
            .language(Language::Markdown)
            .external_editor_file_extension("md")
            .editor_hotkey(keys.description_editor.sequence())
            .action_hotkey(keys.description_focus.sequence(), move |_| {
                focus_actions.borrow_mut().extend([
                    DescriptionAction::ShowChanges,
                    DescriptionAction::Focus { edit: true },
                ]);
            })
            .action_hotkey(keys.description_reader.sequence(), move |description| {
                reader_actions.borrow_mut().extend([
                    DescriptionAction::ShowChanges,
                    DescriptionAction::OpenSpeedReader(description),
                ]);
            })
            .on_edit_end(move |value| {
                sink.borrow_mut()
                    .push(ComposerAction::UpdateDescription(value));
            });
        let mut bound = Self {
            state,
            edit_request,
            input,
            diff: DiffViewer::new("", "")
                .labels("Source", "Changes")
                .style(DiffStyle::Word)
                .show_headers(false)
                .wrap(true),
        };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = state
            .selected_ticket()
            .map_or("", |ticket| ticket.description.as_str());
        let (source, changes) = state.selected_change().map_or(("", ""), |change| {
            if let Some(snapshot) = change.submitted.as_ref() {
                (
                    snapshot
                        .original
                        .as_ref()
                        .map_or("", |ticket| ticket.description.as_str()),
                    snapshot
                        .updated
                        .as_ref()
                        .map_or("", |ticket| ticket.description.as_str()),
                )
            } else {
                (
                    state
                        .selected_source()
                        .map_or("", |ticket| ticket.description.as_str()),
                    state
                        .selected_changes()
                        .map_or("", |ticket| ticket.description.as_str()),
                )
            }
        });
        self.diff.set_texts(source, changes);
        let editable = state.selected_is_editable();
        let value_changed =
            (!self.input.insert_mode() || !editable) && self.input.current_value() != value;
        let changed = value_changed || editable == self.input.is_disabled();
        if value_changed {
            self.input.set_value(value);
        }
        self.input.set_disabled(!editable);
        changed
    }
}

impl TuiNode for BoundDescription {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if self.state.borrow().view_mode == ComposerViewMode::Diff {
            <DiffViewer as TuiNode<()>>::measure(&self.diff, proposal)
        } else {
            self.input.measure(proposal)
        }
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        if self.edit_request.replace(false) && !self.input.is_disabled() {
            self.input.move_cursor_to_end();
            self.input.set_insert_mode(true);
        }
        if self.state.borrow().view_mode == ComposerViewMode::Diff {
            <DiffViewer as TuiNode<()>>::layout(&mut self.diff, area, ctx)
        } else {
            self.input.layout(area, ctx)
        }
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        if self.state.borrow().view_mode == ComposerViewMode::Diff {
            self.diff.render(frame, area);
        } else {
            self.input.render(frame, area);
        }
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.state.borrow().view_mode == ComposerViewMode::Diff {
            self.diff.event(event, ctx)
        } else {
            self.input.event(event, ctx)
        }
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.state.borrow().view_mode == ComposerViewMode::Diff {
            self.diff.dispatch_event(route, event, ctx)
        } else {
            self.input.dispatch_event(route, event, ctx)
        }
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        let active = if self.state.borrow().view_mode == ComposerViewMode::Diff {
            <DiffViewer as TuiNode<()>>::tick(&mut self.diff, dt, settings)
        } else {
            self.input.tick(dt, settings)
        };
        active.merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
        self.diff.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx);
        self.diff.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
        self.diff.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
        self.diff.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.diff.unmount(ctx);
        self.input.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.diff.destroy(ctx);
        self.input.destroy(ctx);
    }
}

pub(super) struct BoundViewMode {
    state: Rc<RefCell<ComposerState>>,
    dropdown: Dropdown<ComposerViewMode, ComposerViewMode>,
}

impl BoundViewMode {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        key: ComposerKeyBinding,
    ) -> Self {
        let sink = Rc::clone(&pending);
        let dropdown = Dropdown::single(
            [
                ComposerViewMode::Source,
                ComposerViewMode::Changes,
                ComposerViewMode::Diff,
            ],
            |mode| *mode,
            |mode| match mode {
                ComposerViewMode::Source => "Source".into(),
                ComposerViewMode::Changes => "Changes".into(),
                ComposerViewMode::Diff => "Diff".into(),
            },
        )
        .selected_one(ComposerViewMode::Changes)
        .variant(DropdownVariant::Filled)
        .label("View")
        .label_position(DropdownLabelPosition::Inline)
        .hotkey(key.sequence())
        .on_select(move |modes| {
            if let Some(mode) = modes.first() {
                sink.borrow_mut().push(ComposerAction::SetViewMode(*mode));
            }
        });
        let mut bound = Self { state, dropdown };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let value = self.state.borrow().view_mode;
        let changed = self.dropdown.selected_id() != Some(value);
        if changed {
            self.dropdown.set_selected_one(value);
        }
        changed
    }
}

impl TuiNode for BoundViewMode {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <Dropdown<ComposerViewMode, ComposerViewMode> as TuiNode<()>>::measure(
            &self.dropdown,
            proposal,
        )
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        <Dropdown<ComposerViewMode, ComposerViewMode> as TuiNode<()>>::layout(
            &mut self.dropdown,
            area,
            ctx,
        )
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, area, _ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.dropdown.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.dropdown.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        <Dropdown<ComposerViewMode, ComposerViewMode> as TuiNode<()>>::tick(
            &mut self.dropdown,
            dt,
            settings,
        )
        .merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.dropdown.destroy(ctx);
    }
}
