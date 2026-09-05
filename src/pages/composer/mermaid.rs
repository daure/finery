use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, Image,
    ImageProtocol, Language, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    MermaidRasterOptions, MermaidRenderer, RenderCtx, SyntaxHighlighter, TickResult, TuiEvent,
    TuiNode, theme,
};

use crate::store::composer::{ComposerAction, ComposerState};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;

pub(super) struct DiagramTitle {
    state: Rc<RefCell<ComposerState>>,
    input: tuicore::TextInput<()>,
}

impl DiagramTitle {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, pending: PendingActions) -> Self {
        let input = tuicore::TextInput::new()
            .panel("Diagram title")
            .on_edit_end(move |value| {
                pending
                    .borrow_mut()
                    .push(ComposerAction::RenameSelectedMermaidDiagram(value));
            });
        Self { state, input }
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = state
            .selected_mermaid_diagram()
            .map(|diagram| diagram.title.as_str())
            .unwrap_or_default();
        let editable = state.selected_mermaid_diagram_is_editable();
        let value_changed =
            (!self.input.insert_mode() || !editable) && self.input.current_value() != value;
        if value_changed {
            self.input.set_value(value);
            self.input.move_cursor_to_end();
        }
        let disabled_changed = self.input.is_disabled() == editable;
        self.input.set_disabled(!editable);
        value_changed || disabled_changed
    }
}

impl TuiNode for DiagramTitle {
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
        self.input.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings).merge(if self.sync() {
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

pub(super) struct DiagramContent {
    state: Rc<RefCell<ComposerState>>,
    cache_updates: PendingActions,
    images: HashMap<String, DiagramImage>,
    renders: HashMap<String, DiagramRender>,
    render_errors: HashMap<String, String>,
    preview_size: (u16, u16),
}

enum DiagramImage {
    Loading(Receiver<Result<Image, String>>),
    Ready(Image),
    Failed(String),
}

struct DiagramRender {
    ticket_id: String,
    diagram_id: String,
    markup: String,
    theme: String,
    receiver: Receiver<Result<Vec<u8>, String>>,
}

impl DiagramContent {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, cache_updates: PendingActions) -> Self {
        Self {
            state,
            cache_updates,
            images: HashMap::new(),
            renders: HashMap::new(),
            render_errors: HashMap::new(),
            preview_size: (32, 16),
        }
    }

    fn sync_cache(&mut self) -> bool {
        let mut desired = HashSet::new();
        let mut images_to_load = Vec::new();
        let mut renders_to_start = Vec::new();
        let active_theme = theme();
        let theme_id = active_theme.name().id().to_owned();
        {
            let state = self.state.borrow();
            if let Some(set) = state.active_set() {
                for change in &set.tickets {
                    let Some(ticket) = state.changes_for_change(change) else {
                        continue;
                    };
                    for diagram in &ticket.mermaid_diagrams {
                        let key = diagram_image_key(diagram, &theme_id);
                        let current =
                            !diagram.rendered_png.is_empty() && diagram.rendered_theme == theme_id;
                        if desired.insert(key.clone()) && !self.images.contains_key(&key) {
                            if current {
                                images_to_load.push((key, diagram.rendered_png.clone()));
                            } else if !self.renders.contains_key(&key) {
                                renders_to_start.push((
                                    key,
                                    change.id.clone(),
                                    diagram.id.clone(),
                                    diagram.markup.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        let removed = self.images.keys().any(|key| !desired.contains(key));
        let added = !images_to_load.is_empty() || !renders_to_start.is_empty();
        self.images.retain(|key, _| desired.contains(key));
        self.renders.retain(|key, _| desired.contains(key));
        self.render_errors.retain(|key, _| desired.contains(key));
        for (key, png) in images_to_load {
            self.images
                .insert(key, DiagramImage::Loading(self.load(png)));
        }
        for (key, ticket_id, diagram_id, markup) in renders_to_start {
            self.render_errors.remove(&key);
            self.renders.insert(
                key,
                DiagramRender {
                    ticket_id,
                    diagram_id,
                    markup: markup.clone(),
                    theme: theme_id.clone(),
                    receiver: self.render(markup, active_theme.clone()),
                },
            );
        }
        removed || added
    }

    fn load(&self, png: Vec<u8>) -> Receiver<Result<Image, String>> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = Image::from_bytes(png).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        receiver
    }

    fn render(
        &self,
        markup: String,
        active_theme: tuicore::Theme,
    ) -> Receiver<Result<Vec<u8>, String>> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = MermaidRenderer::new()
                .render_png_with_theme(&markup, &MermaidRasterOptions::default(), &active_theme)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        receiver
    }

    fn selected_image_key(&self) -> Option<String> {
        let theme_id = theme().name().id();
        self.state
            .borrow()
            .selected_mermaid_diagram()
            .filter(|diagram| {
                !diagram.rendered_png.is_empty() && diagram.rendered_theme == theme_id
            })
            .map(|diagram| diagram_image_key(diagram, theme_id))
    }

    fn text(&self) -> String {
        let theme_id = theme().name().id();
        let selected_diagram_key = self
            .state
            .borrow()
            .selected_mermaid_diagram()
            .map(|diagram| diagram_image_key(diagram, theme_id));
        if let Some(key) = selected_diagram_key.as_ref()
            && let Some(error) = self.render_errors.get(key)
        {
            return format!("Could not render diagram: {error}");
        }
        if selected_diagram_key
            .as_ref()
            .is_some_and(|key| self.renders.contains_key(key))
        {
            return "Rendering diagram...".into();
        }
        let selected = self.selected_image_key();
        match selected.as_ref().and_then(|key| self.images.get(key)) {
            Some(DiagramImage::Loading(_)) | None => "Loading diagram...".into(),
            Some(DiagramImage::Failed(error)) => format!("Could not load diagram: {error}"),
            Some(DiagramImage::Ready(_)) => String::new(),
        }
    }
}

impl TuiNode for DiagramContent {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(32, 16).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_cache();
        self.preview_size = (area.width, area.height);
        let selected = self.selected_image_key();
        if let Some(DiagramImage::Ready(image)) =
            selected.as_ref().and_then(|key| self.images.get_mut(key))
        {
            image.preload(area.width, area.height);
            <Image as TuiNode<()>>::layout(image, area, ctx);
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        let selected = self.selected_image_key();
        if let Some(DiagramImage::Ready(image)) =
            selected.as_ref().and_then(|key| self.images.get(key))
        {
            <Image as TuiNode<()>>::render(image, frame, area, ctx);
        } else {
            frame.render_widget(
                Paragraph::new(self.text()).style(Style::default().fg(theme().text_fg())),
                area,
            );
        }
    }

    fn tick(&mut self, _dt: Duration, _settings: AnimationSettings) -> TickResult {
        let mut result = if self.sync_cache() {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        };
        let completed = self
            .renders
            .iter()
            .filter_map(|(key, render)| match render.receiver.try_recv() {
                Ok(result) => Some((key.clone(), result)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((key.clone(), Err("diagram rendering stopped".into())))
                }
                Err(mpsc::TryRecvError::Empty) => {
                    result = result.merge(TickResult::ACTIVE);
                    None
                }
            })
            .collect::<Vec<_>>();
        for (key, rendered) in completed {
            let render = self
                .renders
                .remove(&key)
                .expect("completed diagram render must be cached");
            match rendered {
                Ok(rendered_png) => {
                    self.cache_updates
                        .borrow_mut()
                        .push(ComposerAction::CacheMermaidDiagram {
                            ticket_id: render.ticket_id,
                            diagram_id: render.diagram_id,
                            markup: render.markup,
                            rendered_png,
                            rendered_theme: render.theme,
                        })
                }
                Err(error) => {
                    self.render_errors.insert(key, error);
                }
            }
            result = result.merge(TickResult::CHANGED);
        }
        let selected = self.selected_image_key();
        for (key, cached) in &mut self.images {
            let next = match cached {
                DiagramImage::Loading(receiver) => match receiver.try_recv() {
                    Ok(Ok(mut rendered)) => {
                        rendered.preload(self.preview_size.0, self.preview_size.1);
                        Some(DiagramImage::Ready(rendered.protocol(ImageProtocol::Kitty)))
                    }
                    Ok(Err(error)) => Some(DiagramImage::Failed(error)),
                    Err(mpsc::TryRecvError::Empty) => {
                        result = result.merge(TickResult::ACTIVE);
                        None
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(DiagramImage::Failed("diagram decoding stopped".into()))
                    }
                },
                DiagramImage::Ready(image) => {
                    result = result.merge(image.tick());
                    None
                }
                DiagramImage::Failed(_) => None,
            };
            if let Some(mut next) = next {
                if let DiagramImage::Ready(image) = &mut next {
                    image.preload(self.preview_size.0, self.preview_size.1);
                    result = result.merge(TickResult::ACTIVE);
                }
                *cached = next;
                if selected.as_ref() == Some(key) {
                    result = result.merge(TickResult {
                        changed: true,
                        layout: true,
                        active: true,
                        next_tick: None,
                    });
                }
            }
        }
        result
    }
}

fn diagram_image_key(diagram: &crate::store::composer::MermaidDiagram, theme: &str) -> String {
    format!("{}:{}:{theme}", diagram.id, diagram.markup)
}

pub(super) struct DiagramMarkup {
    state: Rc<RefCell<ComposerState>>,
    markup: String,
    highlighter: SyntaxHighlighter,
}

impl DiagramMarkup {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>) -> Self {
        Self {
            state,
            markup: String::new(),
            highlighter: SyntaxHighlighter::new("", Language::Markdown),
        }
    }

    fn sync(&mut self) -> bool {
        let markup = self
            .state
            .borrow()
            .selected_mermaid_diagram()
            .map(|diagram| diagram.markup.clone())
            .unwrap_or_default();
        if markup == self.markup {
            return false;
        }
        self.markup = markup.clone();
        self.highlighter.set_code(markup);
        true
    }
}

impl TuiNode for DiagramMarkup {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <SyntaxHighlighter as TuiNode<()>>::measure(&self.highlighter, proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        <SyntaxHighlighter as TuiNode<()>>::layout(&mut self.highlighter, area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <SyntaxHighlighter as TuiNode<()>>::render(&self.highlighter, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        <SyntaxHighlighter as TuiNode<()>>::event(&mut self.highlighter, event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        <SyntaxHighlighter as TuiNode<()>>::dispatch_event(&mut self.highlighter, route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <SyntaxHighlighter as TuiNode<()>>::tick(&mut self.highlighter, dt, settings).merge(
            if self.sync() {
                TickResult::CHANGED
            } else {
                TickResult::IDLE
            },
        )
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::focus(&mut self.highlighter, target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::dispatch_focus(
            &mut self.highlighter,
            target,
            focused,
            ctx,
        );
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::init(&mut self.highlighter, ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::mount(&mut self.highlighter, ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::unmount(&mut self.highlighter, ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        <SyntaxHighlighter as TuiNode<()>>::destroy(&mut self.highlighter, ctx);
    }
}
