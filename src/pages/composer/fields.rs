use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId,
    FocusTarget, InputChrome, Language, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TextInput, TextareaInput, TickResult, Toggle, TuiEvent, TuiNode,
};

use crate::store::composer::{ComposerAction, ComposerState, TicketKind};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;

pub(super) type PendingDescriptionActions = Rc<RefCell<Vec<DescriptionAction>>>;
pub(super) type DescriptionEditRequest = Rc<Cell<bool>>;

pub(super) enum DescriptionAction {
    Focus { edit: bool },
    OpenExternalEditor(String),
    OpenSpeedReader(String),
    CloseSpeedReader,
}

#[derive(Clone, Copy)]
enum TextField {
    Title,
    Kind,
    Status,
    Priority,
    Assignee,
}

impl TextField {
    fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Kind => "Issue type",
            Self::Status => "Status",
            Self::Priority => "Priority",
            Self::Assignee => "Assignee",
        }
    }

    fn value(self, state: &ComposerState) -> String {
        let Some(ticket) = state.selected_ticket() else {
            return String::new();
        };
        match self {
            Self::Title => ticket.title.clone(),
            Self::Kind => format!("{:?}", ticket.kind),
            Self::Status => ticket.status.clone(),
            Self::Priority => ticket.priority.clone(),
            Self::Assignee => ticket.assignee.clone(),
        }
    }

    fn action(self, value: String) -> ComposerAction {
        match self {
            Self::Title => ComposerAction::UpdateTitle(value),
            Self::Kind => ComposerAction::UpdateKind(match value.to_ascii_lowercase().as_str() {
                "epic" => TicketKind::Epic,
                "story" => TicketKind::Story,
                "bug" => TicketKind::Bug,
                "subtask" | "sub-task" => TicketKind::Subtask,
                _ => TicketKind::Task,
            }),
            Self::Status => ComposerAction::UpdateStatus(value),
            Self::Priority => ComposerAction::UpdatePriority(value),
            Self::Assignee => ComposerAction::UpdateAssignee(value),
        }
    }
}

pub(super) struct BoundTextField {
    state: Rc<RefCell<ComposerState>>,
    field: TextField,
    input: TextInput,
}

impl BoundTextField {
    fn new(state: Rc<RefCell<ComposerState>>, pending: PendingActions, field: TextField) -> Self {
        let sink = Rc::clone(&pending);
        let input = TextInput::new()
            .placeholder(field.label())
            .on_edit_end(move |value| sink.borrow_mut().push(field.action(value)));
        let input = if matches!(field, TextField::Title) {
            input.hotkey("shift+t")
        } else {
            input
        };
        let mut bound = Self {
            state,
            field,
            input,
        };
        bound.sync();
        bound
    }

    pub(super) fn title(state: Rc<RefCell<ComposerState>>, pending: PendingActions) -> Self {
        Self::new(state, pending, TextField::Title)
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = self.field.value(&state);
        let editable = state.selected_is_editable();
        let mode = if state.show_updated {
            "Updated"
        } else {
            "Original · read-only"
        };
        let key = state
            .selected_ticket()
            .map_or_else(|| "No ticket selected".into(), |ticket| ticket.key.clone());
        let chrome = if matches!(self.field, TextField::Title) {
            InputChrome::panel(key).top_right(mode)
        } else {
            InputChrome::panel(self.field.label()).top_right(mode)
        };
        let value_changed = !self.input.insert_mode() && self.input.current_value() != value;
        let changed = value_changed || editable == self.input.is_disabled();
        if value_changed {
            self.input.set_value(value);
            self.input.move_cursor_to_end();
        }
        self.input.set_disabled(!editable);
        self.input.set_style(chrome);
        changed
    }
}

impl TuiNode for BoundTextField {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
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
        if route.path.is_empty() {
            self.event(event, ctx)
        } else {
            EventOutcome::Ignored
        }
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        self.input.tick(dt, settings).merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
    }
}

pub(super) struct BoundDescription {
    state: Rc<RefCell<ComposerState>>,
    edit_request: DescriptionEditRequest,
    input: TextareaInput,
}

impl BoundDescription {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        edit_request: DescriptionEditRequest,
    ) -> Self {
        let sink = Rc::clone(&pending);
        let input = TextareaInput::new()
            .language(Language::Markdown)
            .on_edit_end(move |value| {
                let adf = crate::store::composer::jira_adf::markdown_to_adf(&value);
                let markdown = crate::store::composer::jira_adf::adf_to_markdown(&adf);
                sink.borrow_mut()
                    .push(ComposerAction::UpdateDescription(markdown));
            });
        let mut bound = Self {
            state,
            edit_request,
            input,
        };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = state
            .selected_ticket()
            .map_or("", |ticket| ticket.description.as_str());
        let editable = state.selected_is_editable();
        let value_changed = !self.input.insert_mode() && self.input.current_value() != value;
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
        self.input.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        if self.edit_request.replace(false) && !self.input.is_disabled() {
            self.input.move_cursor_to_end();
            self.input.set_insert_mode(true);
        }
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
        let changed = self.sync();
        self.input.tick(dt, settings).merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.input.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
    }
}

pub(super) fn properties_form(
    state: Rc<RefCell<ComposerState>>,
    pending: PendingActions,
) -> Flex<()> {
    Flex::column()
        .child(
            "kind",
            BoundTextField::new(Rc::clone(&state), Rc::clone(&pending), TextField::Kind),
            FlexItem::fixed(3),
        )
        .child(
            "status",
            BoundTextField::new(Rc::clone(&state), Rc::clone(&pending), TextField::Status),
            FlexItem::fixed(3),
        )
        .child(
            "priority",
            BoundTextField::new(Rc::clone(&state), Rc::clone(&pending), TextField::Priority),
            FlexItem::fixed(3),
        )
        .child(
            "assignee",
            BoundTextField::new(state, pending, TextField::Assignee),
            FlexItem::fixed(3),
        )
}

pub(super) struct BoundVersionToggle {
    state: Rc<RefCell<ComposerState>>,
    toggle: Toggle,
}

impl BoundVersionToggle {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, pending: PendingActions) -> Self {
        let sink = Rc::clone(&pending);
        let toggle = Toggle::new("Original / Updated")
            .hotkey("v")
            .on_change(move |value| sink.borrow_mut().push(ComposerAction::ShowUpdated(value)));
        let mut bound = Self { state, toggle };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let value = self.state.borrow().show_updated;
        let changed = value != self.toggle.is_checked();
        self.toggle.set_value(value);
        self.toggle.set_label(if value {
            "Original / [Updated]"
        } else {
            "[Original] / Updated"
        });
        changed
    }
}

impl TuiNode for BoundVersionToggle {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.toggle.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.toggle.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        self.toggle.render(frame, area);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.toggle.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.toggle.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        self.toggle.tick(dt, settings).merge(if changed {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.toggle.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.toggle.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.toggle.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.toggle.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.toggle.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.toggle.destroy(ctx);
    }
}
