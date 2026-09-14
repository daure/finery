use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings, line_width,
};

use crate::app_settings::ComposerKeyBindings;

pub(super) const MENU_HOST_WIDTH: u16 = 69;
pub(super) const MENU_HOST_HEIGHT: u16 = 18;
const MENU_FIELD_WIDTH: u16 = 54;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ChangeSetQuickAction {
    Clone,
    Rename,
    Delete,
    Archive,
}

impl ChangeSetQuickAction {
    fn label(self) -> &'static str {
        match self {
            Self::Clone => "Clone",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
            Self::Archive => "Archive",
        }
    }
}

pub(super) struct ChangeSetQuickMenu {
    dropdown: Dropdown<ChangeSetQuickAction, ChangeSetQuickAction>,
    selected: Rc<RefCell<Option<ChangeSetQuickAction>>>,
    keys: ComposerKeyBindings,
    field_area: Rect,
}

impl ChangeSetQuickMenu {
    pub(super) fn new(keys: ComposerKeyBindings) -> Self {
        let selected = Rc::new(RefCell::new(None));
        let selection = Rc::clone(&selected);
        let labels = keys.clone();
        let dropdown = Dropdown::single_rich(
            [
                ChangeSetQuickAction::Rename,
                ChangeSetQuickAction::Delete,
                ChangeSetQuickAction::Archive,
            ],
            |action| *action,
            |action| action.label().to_owned(),
            move |action, _, _| {
                let label = action.label();
                let hotkey = match action {
                    ChangeSetQuickAction::Clone => labels.clone_change_set.label(),
                    ChangeSetQuickAction::Rename => labels.rename_change_set.label(),
                    ChangeSetQuickAction::Delete => labels.delete_change_set.label(),
                    ChangeSetQuickAction::Archive => labels.archive.label(),
                };
                let spacing = usize::from(MENU_FIELD_WIDTH)
                    .saturating_sub(line_width(&Line::from(label)))
                    .saturating_sub(line_width(&Line::from(hotkey.as_str())));
                Text::from(Line::from(vec![
                    Span::raw(label),
                    Span::raw(" ".repeat(spacing)),
                    Span::styled(hotkey, Style::default().fg(tuicore::theme().muted_fg())),
                ]))
            },
        )
        .variant(DropdownVariant::Filled)
        .label("Change set actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(16)
        .on_select(move |actions| *selection.borrow_mut() = actions.first().copied());
        Self {
            dropdown,
            selected,
            keys,
            field_area: Rect::default(),
        }
    }

    pub(super) fn open(&mut self, clone_available: bool, ctx: &mut EventCtx<()>) {
        self.selected.borrow_mut().take();
        self.dropdown.clear_selection();
        self.dropdown.set_search_query("");
        let actions = if clone_available {
            vec![
                ChangeSetQuickAction::Clone,
                ChangeSetQuickAction::Rename,
                ChangeSetQuickAction::Delete,
            ]
        } else {
            vec![
                ChangeSetQuickAction::Rename,
                ChangeSetQuickAction::Delete,
                ChangeSetQuickAction::Archive,
            ]
        };
        self.dropdown.set_rows(actions);
        self.dropdown.open_with_context(ctx);
    }

    pub(super) fn is_open(&self) -> bool {
        self.dropdown.is_open()
    }

    pub(super) fn take_action(&mut self) -> Option<ChangeSetQuickAction> {
        self.selected.borrow_mut().take()
    }

    fn handle_shortcut(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };
        let action = if self.keys.rename_change_set.matches(*key) {
            Some(ChangeSetQuickAction::Rename)
        } else if self.keys.clone_change_set.matches(*key) {
            Some(ChangeSetQuickAction::Clone)
        } else if self.keys.delete_change_set.matches(*key) {
            Some(ChangeSetQuickAction::Delete)
        } else if self.keys.archive.matches(*key) {
            Some(ChangeSetQuickAction::Archive)
        } else if keybindings().focus().unfocus_matches(*key) {
            None
        } else {
            return false;
        };
        *self.selected.borrow_mut() = action;
        self.dropdown.close();
        ctx.stop_propagation();
        ctx.request_layout();
        ctx.request_redraw();
        true
    }
}

impl TuiNode for ChangeSetQuickMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let height = <Dropdown<_, _> as TuiNode<()>>::measure(
            &self.dropdown,
            LayoutProposal::at_most(width, area.height),
        )
        .preferred
        .height
        .min(area.height);
        self.field_area = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        );
        <Dropdown<_, _> as TuiNode<()>>::layout(&mut self.dropdown, self.field_area, ctx);
        LayoutResult::new(area)
    }
    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.field_area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.handle_shortcut(event, ctx) {
            return EventOutcome::Handled;
        }
        self.dropdown.event(event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.handle_shortcut(event, ctx) {
            return EventOutcome::Handled;
        }
        self.dropdown.dispatch_event(route, event, ctx)
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<_, _> as TuiNode<()>>::tick(&mut self.dropdown, dt, settings)
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
