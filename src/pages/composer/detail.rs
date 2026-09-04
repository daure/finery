use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    widgets::Paragraph as RatatuiParagraph,
};
use tuicore::{
    AnimationSettings, AxisProposal, BorderKind, EventCtx, EventOutcome, EventRoute, Flex,
    FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, Image, ImageProtocol, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, Panel, PanelHost, RenderCtx,
    SeasonalEmptyState, Split, Tab, Tabs, TabsBodyBorderStyle, TabsVariant, TickResult, TuiEvent,
    TuiNode, theme,
};

use crate::{
    app_settings::ComposerKeyBindings,
    service::AppService,
    store::composer::{ChangeKind, ComposerAction, ComposerState, ComposerViewMode},
};

use super::fields::{
    BoundDescription, BoundDescriptionDiffStyle, BoundTextField, BoundViewMode, DescriptionAction,
    DescriptionEditRequest, PendingDescriptionActions,
};
use super::property_fields::PropertyFields;

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type WideDescription = PanelHost<BoundDescription, ()>;
type WideProperties = PanelHost<PropertyFields, ()>;
type WideDetails = Split<WideDescription, WideProperties>;
type TicketFields = Split<BoundTextField, ResponsiveDetails>;
type TicketDetail = Split<Flex<()>, TicketFields>;
type FileFields = Split<AttachmentFilename, Tabs<()>>;
type FileDetail = Split<Flex<()>, FileFields>;

const WIDE_BREAKPOINT: u16 = 100;

struct ResponsiveDetails {
    narrow: Tabs<()>,
    wide: WideDetails,
    is_wide: bool,
    description_focus_key: String,
    description_editor_key: String,
    description_reader_key: String,
}

struct FileContent {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    content_url: Option<String>,
    image: Option<Image>,
    loading: Option<Receiver<Result<Image, String>>>,
    error: Option<String>,
}

struct AttachmentFilename {
    state: Rc<RefCell<ComposerState>>,
    input: tuicore::TextInput<()>,
}

pub(super) struct DetailPane {
    state: Rc<RefCell<ComposerState>>,
    description_actions: PendingDescriptionActions,
    description_edit_request: DescriptionEditRequest,
    external_editor_pending: bool,
    service: AppService,
    detail: TicketDetail,
    file: FileDetail,
    empty: SeasonalEmptyState,
}

impl DetailPane {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        description_actions: PendingDescriptionActions,
        service: AppService,
        keys: ComposerKeyBindings,
    ) -> Self {
        let title =
            BoundTextField::title(Rc::clone(&state), Rc::clone(&pending), keys.title.clone());
        let description_edit_request = Rc::new(std::cell::Cell::new(false));
        let narrow_description = BoundDescription::new(
            Rc::clone(&state),
            Rc::clone(&pending),
            Rc::clone(&description_edit_request),
            &keys,
            Rc::clone(&description_actions),
        );
        let tabs = Tabs::new(vec![
            Tab::new("Description", narrow_description).hotkey(keys.description_tab.sequence()),
            Tab::new(
                "Properties",
                PropertyFields::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    service.clone(),
                    keys.clone(),
                ),
            )
            .hotkey(keys.properties_tab.sequence()),
        ])
        .action_hotkey(
            keys.description_focus.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Focus,
            ),
        )
        .action_hotkey(
            keys.description_editor.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::Editor,
            ),
        )
        .action_hotkey(
            keys.description_reader.sequence(),
            description_tab_action(
                Rc::clone(&state),
                Rc::clone(&description_actions),
                DescriptionTabAction::SpeedReader,
            ),
        )
        .variant(TabsVariant::Underline)
        .bordered(true);
        let wide_description = Panel::new()
            .top_left("Description")
            .action_hotkey(
                keys.description_focus.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::Focus,
                ),
            )
            .action_hotkey(
                keys.description_editor.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::Editor,
                ),
            )
            .action_hotkey(
                keys.description_reader.sequence(),
                panel_description_action(
                    Rc::clone(&state),
                    Rc::clone(&description_actions),
                    DescriptionTabAction::SpeedReader,
                ),
            )
            .host(BoundDescription::new(
                Rc::clone(&state),
                Rc::clone(&pending),
                Rc::clone(&description_edit_request),
                &keys,
                Rc::clone(&description_actions),
            ));
        let wide_properties = Panel::new()
            .top_left("Properties")
            .hotkey(keys.properties_tab.sequence())
            .host(PropertyFields::new(
                Rc::clone(&state),
                Rc::clone(&pending),
                service.clone(),
                keys.clone(),
            ));
        let details = ResponsiveDetails {
            narrow: tabs,
            wide: Split::horizontal(wide_description, wide_properties).ratio(70, 30),
            is_wide: false,
            description_focus_key: keys.description_focus.sequence().into(),
            description_editor_key: keys.description_editor.sequence().into(),
            description_reader_key: keys.description_reader.sequence().into(),
        };
        let fields =
            Split::vertical(title, details).constraints(Constraint::Length(3), Constraint::Fill(1));
        let mode = Flex::row()
            .gap(2)
            .child(
                "mode",
                BoundViewMode::new(Rc::clone(&state), Rc::clone(&pending), keys.view.clone()),
                FlexItem::fit_content(),
            )
            .child(
                "description-diff-style",
                BoundDescriptionDiffStyle::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    keys.description_inline.clone(),
                ),
                FlexItem::fit_content(),
            );
        let file_tabs = Tabs::new(vec![Tab::new(
            "File",
            FileContent::new(Rc::clone(&state), service.clone()),
        )])
        .variant(TabsVariant::Underline)
        .bordered(true);
        let file_mode = Flex::row()
            .gap(2)
            .child(
                "mode",
                BoundViewMode::new(Rc::clone(&state), Rc::clone(&pending), keys.view.clone()),
                FlexItem::fit_content(),
            )
            .child(
                "description-diff-style",
                BoundDescriptionDiffStyle::new(
                    Rc::clone(&state),
                    Rc::clone(&pending),
                    keys.description_inline.clone(),
                ),
                FlexItem::fit_content(),
            );
        let file_fields = Split::vertical(
            AttachmentFilename::new(Rc::clone(&state), Rc::clone(&pending)),
            file_tabs,
        )
        .constraints(Constraint::Length(3), Constraint::Fill(1));
        let file = Split::vertical(file_mode, file_fields)
            .constraints(Constraint::Length(1), Constraint::Fill(1));
        Self {
            state,
            description_actions,
            description_edit_request,
            external_editor_pending: false,
            service,
            detail: Split::vertical(mode, fields)
                .constraints(Constraint::Length(1), Constraint::Fill(1)),
            file,
            empty: SeasonalEmptyState::new("No tickets added"),
        }
    }

    fn active(&self) -> &dyn TuiNode<()> {
        let state = self.state.borrow();
        if state.selected_attachment().is_some() {
            &self.file
        } else if state.selected_ticket.is_some() {
            &self.detail
        } else {
            &self.empty
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        let selected_attachment = self.state.borrow().selected_attachment().is_some();
        if selected_attachment {
            &mut self.file
        } else if self.state.borrow().selected_ticket.is_some() {
            &mut self.detail
        } else {
            &mut self.empty
        }
    }

    fn process_description_actions(&mut self, ctx: &mut EventCtx<()>) {
        let actions = self
            .description_actions
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        let mut deferred = Vec::new();
        for action in actions {
            let details = self.detail.second_mut().second_mut();
            match action {
                DescriptionAction::ShowChanges => {
                    let _ = self
                        .state
                        .borrow_mut()
                        .dispatch(ComposerAction::SetViewMode(ComposerViewMode::Changes));
                }
                DescriptionAction::Focus { edit } => {
                    self.description_edit_request.set(edit);
                    details.focus_description();
                    ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
                    ctx.request_layout();
                    ctx.request_redraw();
                }
                DescriptionAction::FocusDiff => {
                    details.focus_description();
                    ctx.request_layout();
                    ctx.request_redraw();
                }
                DescriptionAction::OpenExternalEditor(description) => {
                    self.external_editor_pending = true;
                    ctx.request_external_editor_with_extension(description, 1, 1, "md");
                }
                action => deferred.push(action),
            }
        }
        self.description_actions.borrow_mut().extend(deferred);
    }

    pub(super) fn focus_description(&mut self, edit: bool, ctx: &mut EventCtx<()>) {
        self.description_edit_request.set(edit);
        self.detail.second_mut().second_mut().focus_description();
        ctx.focus(FocusRequest::Target(FocusId::new("textarea")));
        ctx.request_layout();
        ctx.request_redraw();
    }

    pub(super) fn focus_diff(&mut self, ctx: &mut EventCtx<()>) {
        self.detail.second_mut().second_mut().focus_description();
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn sync(&mut self) {
        if self.state.borrow().selected_attachment().is_some() {
            return;
        }
        let fields = self.detail.second_mut();
        let title_height = fields.first().height();
        fields.set_constraints(Constraint::Length(title_height), Constraint::Fill(1));
        let state = self.state.borrow();
        let editable = state.selected_is_editable();
        let deleted = state
            .selected_change()
            .is_some_and(|change| change.kind == ChangeKind::Deleted);
        let details = self.detail.second_mut().second_mut();
        let (focus, editor, reader) = match state.view_mode {
            ComposerViewMode::Changes => (!deleted, editable, true),
            ComposerViewMode::Source => (true, false, true),
            ComposerViewMode::Diff => (true, false, false),
        };
        details.set_action_hotkeys(focus, editor, reader);
        let submitted = state
            .selected_change()
            .is_some_and(|change| change.is_submitted());
        details.set_dashed(
            deleted
                || submitted
                || state.view_mode == ComposerViewMode::Source
                || state.view_mode == ComposerViewMode::Diff,
        );
    }

    fn handle_external_editor(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> Option<EventOutcome> {
        let TuiEvent::ExternalEditor(response) = event else {
            return None;
        };
        if !self.external_editor_pending {
            return None;
        }
        self.external_editor_pending = false;
        if let Err(error) = self
            .state
            .borrow_mut()
            .dispatch(ComposerAction::UpdateDescription(response.value.clone()))
        {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Change blocked",
                    error.to_string(),
                ));
        }
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    #[cfg(test)]
    pub(super) fn detail_panel_areas(&self) -> (Rect, Rect) {
        self.detail.second().second().wide_areas()
    }

    #[cfg(test)]
    pub(super) fn narrow_border_style(&self) -> TabsBodyBorderStyle {
        self.detail.second().second().narrow_border_style()
    }
}

impl ResponsiveDetails {
    fn active(&self) -> &dyn TuiNode<()> {
        if self.is_wide {
            &self.wide
        } else {
            &self.narrow
        }
    }

    fn active_mut(&mut self) -> &mut dyn TuiNode<()> {
        if self.is_wide {
            &mut self.wide
        } else {
            &mut self.narrow
        }
    }

    fn focus_description(&mut self) {
        if !self.is_wide {
            self.narrow.select_index(0);
        }
    }

    fn set_action_hotkeys(&mut self, description: bool, editor: bool, reader: bool) {
        self.narrow
            .set_action_hotkey_enabled(&self.description_focus_key, description);
        self.narrow
            .set_action_hotkey_enabled(&self.description_editor_key, editor);
        self.narrow
            .set_action_hotkey_enabled(&self.description_reader_key, reader);
        let description_panel = self.wide.first_mut().panel_mut();
        description_panel.set_action_hotkey_enabled(&self.description_focus_key, description);
        description_panel.set_action_hotkey_enabled(&self.description_editor_key, editor);
        description_panel.set_action_hotkey_enabled(&self.description_reader_key, reader);
    }

    fn set_dashed(&mut self, dashed: bool) {
        self.narrow.set_body_border_style(if dashed {
            TabsBodyBorderStyle::Dashed
        } else {
            TabsBodyBorderStyle::Solid
        });
        let border = if dashed {
            BorderKind::AsciiDashed
        } else {
            tuicore::preset().border()
        };
        self.wide.first_mut().panel_mut().set_border(border);
        self.wide.second_mut().panel_mut().set_border(border);
    }

    #[cfg(test)]
    fn wide_areas(&self) -> (Rect, Rect) {
        self.wide.child_areas()
    }

    #[cfg(test)]
    fn narrow_border_style(&self) -> TabsBodyBorderStyle {
        self.narrow.current_body_border_style()
    }
}

impl AttachmentFilename {
    fn new(state: Rc<RefCell<ComposerState>>, pending: PendingActions) -> Self {
        let input = tuicore::TextInput::new()
            .panel("File name")
            .on_edit_end(move |value| {
                pending
                    .borrow_mut()
                    .push(ComposerAction::RenameSelectedAttachment(value));
            });
        Self { state, input }
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let value = state
            .selected_attachment()
            .map(|attachment| attachment.filename.clone())
            .unwrap_or_default();
        let editable = state.selected_attachment_is_editable();
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

impl TuiNode for AttachmentFilename {
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

impl FileContent {
    fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        Self {
            state,
            service,
            content_url: None,
            image: None,
            loading: None,
            error: None,
        }
    }

    fn sync(&mut self) {
        let state = self.state.borrow();
        let Some(attachment) = state.selected_attachment() else {
            return;
        };
        let is_image = is_image_file(&attachment.filename);
        if is_image && let Some(data) = attachment.local_data.as_ref() {
            let identity = format!(
                "local:{}:{}:{}",
                attachment.id,
                attachment.filename,
                data.len()
            );
            if self.content_url.as_deref() != Some(&identity) {
                self.content_url = Some(identity);
                self.loading = None;
                match Image::from_bytes(data) {
                    Ok(image) => {
                        self.image = Some(image.protocol(ImageProtocol::Kitty));
                        self.error = None;
                    }
                    Err(error) => {
                        self.image = None;
                        self.error = Some(error.to_string());
                    }
                }
            }
            return;
        }
        let Some(url) = attachment.content_url.as_deref().filter(|_| is_image) else {
            self.content_url = None;
            self.image = None;
            self.loading = None;
            self.error = None;
            return;
        };
        if self.content_url.as_deref() == Some(url) {
            return;
        }
        self.content_url = Some(url.to_owned());
        self.image = None;
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        let service = self.service.clone();
        let url = url.to_owned();
        thread::spawn(move || {
            let _ = sender.send(service.load_jira_attachment_image(&url));
        });
        self.loading = Some(receiver);
    }

    fn text(&self) -> String {
        let state = self.state.borrow();
        let Some(attachment) = state.selected_attachment() else {
            return "File unavailable".into();
        };
        if is_image_file(&attachment.filename) {
            if let Some(error) = self.error.as_deref() {
                return format!("Could not load image: {error}");
            }
            if self.loading.is_some() {
                return "Loading image...".into();
            }
        }
        format!("{}\n{} bytes", attachment.filename, attachment.size)
    }
}

impl TuiNode for FileContent {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(32, 16).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        if let Some(image) = self.image.as_mut() {
            <Image as TuiNode<()>>::layout(image, area, ctx);
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if let Some(image) = self.image.as_ref() {
            <Image as TuiNode<()>>::render(image, frame, area, ctx);
        } else {
            frame.render_widget(
                RatatuiParagraph::new(self.text()).style(Style::default().fg(theme().text_fg())),
                area,
            );
        }
    }

    fn tick(&mut self, _dt: Duration, _settings: AnimationSettings) -> TickResult {
        self.sync();
        if let Some(receiver) = self.loading.as_ref() {
            match receiver.try_recv() {
                Ok(Ok(image)) => {
                    self.image = Some(image.protocol(ImageProtocol::Kitty));
                    self.loading = None;
                    return TickResult {
                        changed: true,
                        layout: true,
                        active: true,
                        next_tick: None,
                    };
                }
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.loading = None;
                    return TickResult::CHANGED;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("image download stopped".into());
                    self.loading = None;
                    return TickResult::CHANGED;
                }
                Err(mpsc::TryRecvError::Empty) => return TickResult::ACTIVE,
            }
        }
        self.image
            .as_mut()
            .map(Image::tick)
            .unwrap_or(TickResult::IDLE)
    }
}

fn is_image_file(filename: &str) -> bool {
    filename.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "apng" | "avif" | "bmp" | "gif" | "ico" | "jpeg" | "jpg" | "png" | "webp"
        )
    })
}

fn panel_description_action(
    state: Rc<RefCell<ComposerState>>,
    actions: PendingDescriptionActions,
    action: DescriptionTabAction,
) -> impl Fn() + 'static {
    let trigger = description_tab_action(state, actions, action);
    move || trigger(0)
}

impl TuiNode for ResponsiveDetails {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let wide = match proposal.width {
            AxisProposal::Exact(width) | AxisProposal::AtMost(width) => width >= WIDE_BREAKPOINT,
            AxisProposal::Unbounded => true,
        };
        if wide {
            self.wide.measure(proposal)
        } else {
            self.narrow.measure(proposal)
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.is_wide = area.width >= WIDE_BREAKPOINT;
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.active().render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.active_mut().event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.active_mut().dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.active_mut().tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.narrow.init(ctx);
        self.wide.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.narrow.mount(ctx);
        self.wide.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.wide.unmount(ctx);
        self.narrow.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.wide.destroy(ctx);
        self.narrow.destroy(ctx);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DescriptionTabAction {
    Focus,
    Editor,
    SpeedReader,
}

fn description_tab_action(
    state: Rc<RefCell<ComposerState>>,
    actions: PendingDescriptionActions,
    action: DescriptionTabAction,
) -> impl Fn(usize) + 'static {
    move |selected| {
        let state = state.borrow();
        let view_mode = state.view_mode;
        if view_mode == ComposerViewMode::Changes
            && matches!(
                action,
                DescriptionTabAction::Focus | DescriptionTabAction::SpeedReader
            )
        {
            actions.borrow_mut().push(DescriptionAction::ShowChanges);
        }
        if selected != 0 {
            if action != DescriptionTabAction::Editor || state.selected_is_editable() {
                actions
                    .borrow_mut()
                    .push(description_focus_action(view_mode, false));
            }
            return;
        }
        let description = match view_mode {
            ComposerViewMode::Source => state.selected_source(),
            ComposerViewMode::Changes | ComposerViewMode::Diff => state.selected_changes(),
        }
        .map(|ticket| ticket.description.clone())
        .unwrap_or_default();
        match action {
            DescriptionTabAction::Focus => {
                actions.borrow_mut().push(description_focus_action(
                    view_mode,
                    state.selected_is_editable(),
                ));
            }
            DescriptionTabAction::Editor
                if view_mode == ComposerViewMode::Changes && state.selected_is_editable() =>
            {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenExternalEditor(description));
            }
            DescriptionTabAction::SpeedReader if view_mode != ComposerViewMode::Diff => {
                actions
                    .borrow_mut()
                    .push(DescriptionAction::OpenSpeedReader(description));
            }
            DescriptionTabAction::SpeedReader => {}
            DescriptionTabAction::Editor => {}
        }
    }
}

fn description_focus_action(view_mode: ComposerViewMode, edit: bool) -> DescriptionAction {
    match view_mode {
        ComposerViewMode::Diff => DescriptionAction::FocusDiff,
        ComposerViewMode::Source | ComposerViewMode::Changes => DescriptionAction::Focus { edit },
    }
}

impl TuiNode for DetailPane {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.active().measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.active_mut().layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if self.state.borrow().selected_ticket.is_none() {
            let offset = area.height / 6;
            let shifted = Rect::new(
                area.x,
                area.y.saturating_sub(offset),
                area.width,
                area.height,
            );
            <SeasonalEmptyState as TuiNode<()>>::render(&self.empty, frame, shifted, ctx);
        } else {
            self.active().render(frame, area, ctx);
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if let Some(outcome) = self.handle_external_editor(event, ctx) {
            return outcome;
        }
        let outcome = self.active_mut().event(event, ctx);
        self.process_description_actions(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if let Some(outcome) = self.handle_external_editor(event, ctx) {
            return outcome;
        }
        let outcome = self.active_mut().dispatch_event(route, event, ctx);
        self.process_description_actions(ctx);
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.sync();
        if self.state.borrow().selected_attachment().is_some() {
            self.file.tick(dt, settings)
        } else {
            self.active_mut()
                .tick(dt, settings)
                .merge(self.file.tick(dt, settings))
        }
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.active_mut().dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.detail.init(ctx);
        self.file.init(ctx);
        self.empty.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.detail.mount(ctx);
        self.file.mount(ctx);
        self.empty.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.empty.unmount(ctx);
        self.file.unmount(ctx);
        self.detail.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.empty.destroy(ctx);
        self.file.destroy(ctx);
        self.detail.destroy(ctx);
    }
}

#[cfg(test)]
mod file_content_tests {
    use super::*;
    use crate::store::composer::{
        AttachmentChangeKind, ChangeKind, ChangeSet, Ticket, TicketAttachment, TicketChange,
        TicketKind,
    };

    const TEST_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAAAwCAIAAABhdOiYAAAAf0lEQVR42u3RMRGAQBADwBdBTU1NjTDkvAhcffMSMJFrbjYTA9mM47wjvd4v0j2fSFcoAxAgQIAAAQIECBAgQIAKgLoOSx0PCBAgQIAAAQIECBAgQBVAXYeljgcECBAgQIAAAQIECBCgCqCuw1LHAwIECBAgQIAAAQIECFAB0A/Lrzglvf/PRwAAAABJRU5ErkJggg==";

    #[test]
    fn downloaded_image_requests_layout_before_rendering() {
        let attachment = TicketAttachment {
            id: "10000".into(),
            filename: "design.png".into(),
            created: String::new(),
            size: 1,
            content_url: Some("https://jira.example/design.png".into()),
            change: AttachmentChangeKind::Synced,
            local_data: None,
        };
        let ticket = Ticket {
            key: "FIN-1".into(),
            project_key: "FIN".into(),
            title: "Image ticket".into(),
            description: String::new(),
            description_safe_to_overwrite: true,
            description_overwrite_warning: None,
            kind: TicketKind::Task,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: String::new(),
            assignee_account_id: String::new(),
            story_points: None,
            fix_versions: Vec::new(),
            labels: Vec::new(),
            parent_key: None,
            parent_title: None,
            parent_kind: None,
            has_children: false,
            attachments: vec![attachment],
        };
        let mut state = ComposerState::from_change_sets(vec![ChangeSet {
            id: "CS-1".into(),
            name: "Images".into(),
            tickets: vec![TicketChange {
                id: "FIN-1".into(),
                original: Some(ticket.clone()),
                updated: Some(ticket),
                kind: ChangeKind::Synced,
                submitted: None,
                retry_blocked: false,
                create_attempt: false,
                sibling_order: 0,
            }],
            selected_ticket_ids: Vec::new(),
            closed: false,
            submission_attempt: None,
        }]);
        state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
        state.dispatch(ComposerAction::SelectTicket(Some(
            "FIN-1:attachment:0".into(),
        )));
        let mut content = FileContent::new(Rc::new(RefCell::new(state)), AppService::for_tests());
        content.content_url = Some("https://jira.example/design.png".into());
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Ok(Image::from_base64(TEST_PNG).unwrap()))
            .unwrap();
        content.loading = Some(receiver);

        let result = content.tick(Duration::ZERO, AnimationSettings::default());

        assert!(content.image.is_some());
        assert_eq!(
            content.image.as_ref().map(Image::graphics_protocol),
            Some(ImageProtocol::Kitty),
            "downloaded Composer previews use direct Kitty placements"
        );
        assert!(
            result.layout,
            "an image loaded after layout must request a new layout"
        );
        content
            .state
            .borrow_mut()
            .dispatch(ComposerAction::SelectTicket(Some("FIN-1".into())));
        content.tick(Duration::ZERO, AnimationSettings::default());

        assert!(
            content.image.is_some(),
            "leaving the attachment should keep its image cached"
        );
        assert_eq!(
            content.content_url.as_deref(),
            Some("https://jira.example/design.png")
        );
    }
}
