use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use ratatui::{
    Frame,
    layout::Rect,
    text::Text,
    widgets::{Paragraph as RatatuiParagraph, Wrap},
};
use tuicore::{
    AnimationSettings, AxisProposal, CrossAlign, CrossSize, EventCtx, EventOutcome, EventRoute,
    Flex, FlexItem, FocusCtx, FocusId, FocusTarget, Image, ImageProtocol, Language, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, ScrollContainer,
    ScrollbarConfig, SyntaxHighlighter, ThemeName, TickResult, TuiEvent, TuiNode,
};

use crate::{
    service::AppService,
    store::work_items::content::{TicketContentBlock, TicketImage, ticket_content_blocks},
};

const MAX_IMAGE_WIDTH: u16 = 120;
const MAX_IMAGE_HEIGHT: u16 = 40;
const DIRECT_KITTY_SCROLL_PAUSE: Duration = Duration::from_millis(80);

pub(crate) fn image_scroll_container<C: TuiNode>(child: C) -> ScrollContainer<C> {
    let scroll = ScrollContainer::vertical(child).scrollbars(ScrollbarConfig::default());
    // Zellij's scaled-crop cache cannot safely reconcile moving Kitty images yet.
    if std::env::var_os("ZELLIJ").is_some() {
        scroll.pause_direct_kitty_while_scrolling(DIRECT_KITTY_SCROLL_PAUSE)
    } else {
        scroll
    }
}

pub(crate) struct TicketDocument {
    content: Flex<()>,
}

pub(crate) struct TicketContent {
    content: ScrollContainer<TicketDocument>,
    focused: bool,
}

struct MarkdownBlock {
    source: String,
    text: Text<'static>,
    theme: ThemeName,
}

struct InlineImage {
    metadata: TicketImage,
    state: InlineImageState,
}

enum InlineImageState {
    Loading(Receiver<Result<Image, String>>),
    Ready(Box<Image>),
    Failed(String),
}

impl TicketDocument {
    pub(crate) fn new(source: impl Into<String>, service: AppService) -> Self {
        let source = source.into();
        let mut content = Flex::column().gap(1).align(CrossAlign::Start);
        for (index, block) in ticket_content_blocks(&source).into_iter().enumerate() {
            let key = format!("block-{index}");
            content = match block {
                TicketContentBlock::Markdown(markdown) => content.child(
                    key,
                    MarkdownBlock::new(markdown),
                    FlexItem::fit_content().shrink(0),
                ),
                TicketContentBlock::Image(image) => {
                    let (width, height) = image_cell_size(&image);
                    content.child(
                        key,
                        InlineImage::new(image, service.clone()),
                        FlexItem::fixed(height)
                            .cross_size(CrossSize::Fixed(width))
                            .align_self(CrossAlign::Start),
                    )
                }
            };
        }
        Self { content }
    }
}

impl TicketContent {
    pub(crate) fn new(source: impl Into<String>, service: AppService) -> Self {
        Self {
            content: image_scroll_container(TicketDocument::new(source, service)),
            focused: false,
        }
    }
}

impl MarkdownBlock {
    fn new(source: String) -> Self {
        let theme = tuicore::theme().name();
        let text = SyntaxHighlighter::new(source.clone(), Language::Markdown).highlighted_text();
        Self {
            source,
            text,
            theme,
        }
    }

    fn refresh_theme(&mut self) -> bool {
        let theme = tuicore::theme().name();
        if self.theme == theme {
            return false;
        }
        self.theme = theme;
        self.text =
            SyntaxHighlighter::new(self.source.clone(), Language::Markdown).highlighted_text();
        true
    }
}

impl InlineImage {
    fn new(metadata: TicketImage, service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        let url = metadata.url.clone();
        thread::spawn(move || {
            let result = service.load_jira_attachment_image(&url);
            let _ = sender.send(result);
        });
        Self {
            metadata,
            state: InlineImageState::Loading(receiver),
        }
    }

    fn status(&self) -> String {
        match &self.state {
            InlineImageState::Loading(_) => format!("Loading image: {}", self.metadata.alt),
            InlineImageState::Failed(error) => {
                format!("Could not load image {}: {error}", self.metadata.alt)
            }
            InlineImageState::Ready(_) => String::new(),
        }
    }
}

fn image_cell_size(image: &TicketImage) -> (u16, u16) {
    let mut width = u32::from(image.width.max(1).div_ceil(10));
    let mut height = u32::from(image.height.max(1).div_ceil(20));
    if width > u32::from(MAX_IMAGE_WIDTH) {
        height = height
            .saturating_mul(u32::from(MAX_IMAGE_WIDTH))
            .div_ceil(width)
            .max(1);
        width = u32::from(MAX_IMAGE_WIDTH);
    }
    if height > u32::from(MAX_IMAGE_HEIGHT) {
        width = width
            .saturating_mul(u32::from(MAX_IMAGE_HEIGHT))
            .div_ceil(height)
            .max(1);
        height = u32::from(MAX_IMAGE_HEIGHT);
    }
    (width as u16, height as u16)
}

impl TuiNode for TicketDocument {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.content.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.content.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.content.render(frame, area, ctx);
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.content.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.content.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.content.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.destroy(ctx);
    }
}

impl TuiNode for TicketContent {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.content.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        ctx.register_focusable(FocusId::new("ticket-content"), area, true);
        self.content.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.content.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.content
            .dispatch_event(&EventRoute::new(tuicore::TreePath::new()), event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.content.dispatch_event(route, event, ctx)
    }

    fn focus(&mut self, _target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.focused = focused;
        self.content.set_focused(focused);
        ctx.request_redraw();
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.focus(Some(&target.id), focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.content.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.content.destroy(ctx);
    }
}

impl TuiNode for MarkdownBlock {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let natural_width = self.text.width().min(u16::MAX as usize) as u16;
        let width = match proposal.width {
            AxisProposal::Unbounded => natural_width,
            AxisProposal::AtMost(max) => natural_width.min(max),
            AxisProposal::Exact(exact) => exact,
        };
        let height = RatatuiParagraph::new(self.text.clone())
            .wrap(Wrap { trim: false })
            .line_count(width.max(1))
            .min(u16::MAX as usize) as u16;
        LayoutSizeHint::content(width, height).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        frame.render_widget(
            RatatuiParagraph::new(self.text.clone()).wrap(Wrap { trim: false }),
            area,
        );
    }

    fn tick(&mut self, _dt: Duration, _settings: AnimationSettings) -> TickResult {
        if self.refresh_theme() {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        }
    }
}

impl TuiNode for InlineImage {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let (width, height) = image_cell_size(&self.metadata);
        LayoutSizeHint::content(width, height).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if let InlineImageState::Ready(image) = &mut self.state {
            <Image as TuiNode<()>>::layout(image, area, ctx);
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if let InlineImageState::Ready(image) = &self.state {
            <Image as TuiNode<()>>::render(image, frame, area, ctx);
        } else {
            frame.render_widget(RatatuiParagraph::new(self.status()), area);
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        match &mut self.state {
            InlineImageState::Ready(image) => image.event(event, ctx),
            _ => EventOutcome::Ignored,
        }
    }

    fn tick(&mut self, _dt: Duration, _settings: AnimationSettings) -> TickResult {
        let next = match &mut self.state {
            InlineImageState::Loading(receiver) => match receiver.try_recv() {
                Ok(Ok(image)) => {
                    let (width, height) = image_cell_size(&self.metadata);
                    let mut image = image.protocol(ImageProtocol::Kitty).size(width, height);
                    image.preload(width, height);
                    Some(InlineImageState::Ready(Box::new(image)))
                }
                Ok(Err(error)) => Some(InlineImageState::Failed(error)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(InlineImageState::Failed("image download stopped".into()))
                }
                Err(mpsc::TryRecvError::Empty) => return TickResult::ACTIVE,
            },
            InlineImageState::Ready(image) => return image.as_mut().tick(),
            InlineImageState::Failed(_) => return TickResult::IDLE,
        };
        self.state = next.expect("loading image should resolve to a terminal state");
        TickResult {
            changed: true,
            layout: true,
            active: matches!(self.state, InlineImageState::Ready(_)),
            next_tick: None,
        }
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
