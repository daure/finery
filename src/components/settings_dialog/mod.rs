use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::speed_reader_settings::{
    MAX_MARKDOWN_BLOCK_PAUSE_MS, MAX_SPEED_READER_WPM, MIN_SPEED_READER_WPM, SpeedReaderSettings,
    parse_markdown_block_pause, parse_speed_reader_wpm,
};

enum SettingChange {
    Wpm(String),
    MarkdownBlockPause(String),
}

pub(crate) struct SettingsDialog {
    root: Flex<()>,
    changes: Rc<RefCell<Vec<SettingChange>>>,
    settings: Rc<Cell<SpeedReaderSettings>>,
}

impl SettingsDialog {
    pub(crate) fn new(settings: Rc<Cell<SpeedReaderSettings>>) -> Self {
        let values = settings.get();
        let changes = Rc::new(RefCell::new(Vec::new()));
        let wpm_changes = Rc::clone(&changes);
        let delay_changes = Rc::clone(&changes);
        let root = Flex::row()
            .gap(1)
            .child(
                "speed-reader-wpm",
                TextInput::new()
                    .value(values.wpm.to_string())
                    .numbers_only(true)
                    .panel("Reader WPM")
                    .focused(true)
                    .on_edit_end(move |value| {
                        wpm_changes.borrow_mut().push(SettingChange::Wpm(value));
                    }),
                FlexItem::fill(1),
            )
            .child(
                "markdown-block-pause",
                TextInput::new()
                    .value(values.markdown_block_pause.as_millis().to_string())
                    .numbers_only(true)
                    .panel("Reader block delay (ms)")
                    .on_edit_end(move |value| {
                        delay_changes
                            .borrow_mut()
                            .push(SettingChange::MarkdownBlockPause(value));
                    }),
                FlexItem::fill(1),
            );
        Self {
            root,
            changes,
            settings,
        }
    }

    fn apply_changes(&self, ctx: &mut EventCtx<()>) {
        for change in self.changes.borrow_mut().drain(..) {
            match change {
                SettingChange::Wpm(value) => {
                    let Some(wpm) = parse_speed_reader_wpm(&value) else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid speed reader WPM",
                            format!(
                                "Enter a whole number from {MIN_SPEED_READER_WPM} to {MAX_SPEED_READER_WPM}."
                            ),
                        ));
                        continue;
                    };
                    self.settings.set(SpeedReaderSettings {
                        wpm,
                        ..self.settings.get()
                    });
                }
                SettingChange::MarkdownBlockPause(value) => {
                    let Some(markdown_block_pause) = parse_markdown_block_pause(&value) else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid block delay",
                            format!(
                                "Enter a whole number from 0 to {MAX_MARKDOWN_BLOCK_PAUSE_MS} ms."
                            ),
                        ));
                        continue;
                    };
                    self.settings.set(SpeedReaderSettings {
                        markdown_block_pause,
                        ..self.settings.get()
                    });
                }
            }
        }
    }
}

impl TuiNode for SettingsDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.root.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.root.event(event, ctx);
        self.apply_changes(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.root.dispatch_event(route, event, ctx);
        self.apply_changes(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}
