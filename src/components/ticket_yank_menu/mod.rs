use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Text},
};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget,
    HotkeyEvent, HotkeyLabelMode, Key, KeyModifiers, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent, TuiNode, hotkey_label_spans,
    hotkey_underline_style,
};

use crate::store::work_items::content::ticket_content_markdown;

const MENU_ANCHOR_WIDTH: u16 = 1;
const MENU_HEIGHT: u16 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TicketYankAction {
    Url,
    Title,
    Description,
    Key,
    Full,
    Slack,
}

impl TicketYankAction {
    const ALL: [Self; 6] = [
        Self::Url,
        Self::Title,
        Self::Description,
        Self::Key,
        Self::Full,
        Self::Slack,
    ];

    fn hotkey(self) -> &'static str {
        match self {
            Self::Url => "u",
            Self::Title => "t",
            Self::Description => "d",
            Self::Key => "k",
            Self::Full => "f",
            Self::Slack => "s",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Url => "URL",
            Self::Title => "Title",
            Self::Description => "Description",
            Self::Key => "Key",
            Self::Full => "Full",
            Self::Slack => "Slack",
        }
    }

    pub(crate) fn text(self, target: &TicketYankTarget) -> Option<String> {
        match self {
            Self::Url => None,
            Self::Title => Some(target.title.clone()),
            Self::Description => Some(ticket_content_markdown(&target.description)),
            Self::Key => Some(target.key.clone()),
            Self::Full => Some(format!("{} - {}", target.key, target.title)),
            Self::Slack => Some(format!(":ticket: {} - {}", target.key, target.title)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TicketYankTarget {
    pub key: String,
    pub title: String,
    pub description: String,
}

pub(crate) struct TicketYankMenu {
    dropdown: Dropdown<TicketYankAction, TicketYankAction>,
    selected: Rc<RefCell<Option<TicketYankAction>>>,
    target: Option<TicketYankTarget>,
    field_area: Rect,
}

impl TicketYankMenu {
    pub(crate) fn new() -> Self {
        let selected = Rc::new(RefCell::new(None));
        let selection = Rc::clone(&selected);
        let dropdown = Dropdown::single_rich(
            TicketYankAction::ALL,
            |action| *action,
            |action| action.label().to_owned(),
            |action, _, _| action_text(*action),
        )
        .variant(DropdownVariant::Filled)
        .label("Yank ticket")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::None)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .show_field_when_open(false)
        .backdrop_amount(0.55)
        .tab_stop(false)
        .max_popup_height(MENU_HEIGHT)
        .max_popup_width(u16::MAX)
        .on_select(move |actions| *selection.borrow_mut() = actions.first().copied());
        Self {
            dropdown,
            selected,
            target: None,
            field_area: Rect::default(),
        }
    }

    pub(crate) fn open(&mut self, target: TicketYankTarget, ctx: &mut EventCtx<()>) {
        self.selected.borrow_mut().take();
        self.target = Some(target);
        self.dropdown.clear_selection();
        self.dropdown.open_with_context(ctx);
    }

    pub(crate) fn is_open(&self) -> bool {
        self.dropdown.is_open()
    }

    pub(crate) fn take_selection(&mut self) -> Option<(TicketYankAction, TicketYankTarget)> {
        let action = self.selected.borrow_mut().take()?;
        Some((action, self.target.clone()?))
    }

    fn handle_shortcut(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> bool {
        let action = match event {
            TuiEvent::Key(key) if key.modifiers == KeyModifiers::NONE => match key.code {
                Key::Char(character) => action_for_hotkey(character),
                _ => None,
            },
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) => sequence
                .strip_prefix('y')
                .filter(|suffix| suffix.chars().count() == 1)
                .and_then(|suffix| suffix.chars().next())
                .and_then(action_for_hotkey),
            _ => None,
        };
        let Some(action) = action else {
            return false;
        };
        *self.selected.borrow_mut() = Some(action);
        self.dropdown.close();
        ctx.stop_propagation();
        ctx.request_layout();
        ctx.request_redraw();
        true
    }
}

fn action_for_hotkey(hotkey: char) -> Option<TicketYankAction> {
    TicketYankAction::ALL
        .into_iter()
        .find(|action| action.hotkey() == hotkey.to_string())
}

fn action_text(action: TicketYankAction) -> Text<'static> {
    let base = Style::default().fg(tuicore::theme().text_fg());
    Text::from(Line::from(hotkey_label_spans(
        action.label(),
        Some(action.hotkey()),
        HotkeyLabelMode::PreferMnemonic,
        None,
        base,
        hotkey_underline_style(base),
    )))
}

impl TuiNode for TicketYankMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_ANCHOR_WIDTH, MENU_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let width = MENU_ANCHOR_WIDTH.min(area.width);
        self.field_area = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y.saturating_add(area.height / 2),
            width,
            u16::from(!area.is_empty()),
        );
        <Dropdown<_, _> as TuiNode<()>>::layout(&mut self.dropdown, self.field_area, ctx);
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.field_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.handle_shortcut(event, ctx) {
            EventOutcome::Handled
        } else {
            self.dropdown.event(event, ctx)
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.handle_shortcut(event, ctx) {
            EventOutcome::Handled
        } else {
            self.dropdown.dispatch_event(route, event, ctx)
        }
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

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
