use std::{cell::RefCell, rc::Rc};

use chrono::{DateTime, FixedOffset, Local};
use ratatui::{Frame, layout::Rect};
use time::OffsetDateTime;
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RelativeDate, RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::store::composer::{ComposerState, JiraTicketMetadata};

pub(super) struct JiraMetadata {
    fields: Flex<()>,
}

impl JiraMetadata {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>) -> Self {
        let fields = Flex::column()
            .child(
                "reporter",
                ReadonlyMetadataText::new(Rc::clone(&state), MetadataField::Reporter),
                FlexItem::fixed(3),
            )
            .child(
                "created",
                ReadonlyMetadataText::new(Rc::clone(&state), MetadataField::Created),
                FlexItem::fixed(3),
            )
            .child(
                "updated",
                ReadonlyMetadataText::new(state, MetadataField::Updated),
                FlexItem::fixed(3),
            );
        Self { fields }
    }
}

#[derive(Clone, Copy)]
enum MetadataField {
    Reporter,
    Created,
    Updated,
}

impl MetadataField {
    fn label(self) -> &'static str {
        match self {
            Self::Reporter => "Reporter",
            Self::Created => "Created",
            Self::Updated => "Last updated",
        }
    }

    fn value(self, metadata: &JiraTicketMetadata) -> String {
        match self {
            Self::Reporter => value_or_unavailable(&metadata.reporter).into(),
            Self::Created => format_timestamp(&metadata.created),
            Self::Updated => format_timestamp(&metadata.updated),
        }
    }
}

struct ReadonlyMetadataText {
    state: Rc<RefCell<ComposerState>>,
    field: MetadataField,
    input: TextInput,
    relative_target: Option<OffsetDateTime>,
    relative_date: Option<RelativeDate>,
}

impl ReadonlyMetadataText {
    fn new(state: Rc<RefCell<ComposerState>>, field: MetadataField) -> Self {
        let value = metadata_value(&state.borrow(), field);
        let relative_target = metadata_timestamp(&state.borrow(), field);
        let relative_date = relative_target.map(RelativeDate::new);
        let mut input = TextInput::new()
            .panel(field.label())
            .value(value)
            .disabled(true);
        if let Some(relative_date) = &relative_date {
            input.set_bottom_left(relative_date.text());
        }
        Self {
            state,
            field,
            input,
            relative_target,
            relative_date,
        }
    }

    fn sync(&mut self) -> bool {
        let value = metadata_value(&self.state.borrow(), self.field);
        let relative_target = metadata_timestamp(&self.state.borrow(), self.field);
        let mut changed = false;
        if self.input.current_value() != value {
            self.input.set_value(value);
            changed = true;
        }
        if self.relative_target != relative_target {
            self.relative_target = relative_target;
            self.relative_date = relative_target.map(RelativeDate::new);
            if let Some(relative_date) = &self.relative_date {
                self.input.set_bottom_left(relative_date.text());
            } else {
                self.input.clear_bottom_left();
            }
            changed = true;
        }
        changed
    }
}

fn metadata_value(state: &ComposerState, field: MetadataField) -> String {
    let ticket = state.selected_source().or_else(|| state.selected_changes());
    let Some(ticket) = ticket.filter(|ticket| !ticket.key.starts_with("NEW-")) else {
        return "Available after Jira creates this ticket.".into();
    };
    let Some(metadata) = ticket.jira_metadata.as_ref() else {
        return "Unavailable. Refresh this ticket to load it.".into();
    };
    field.value(metadata)
}

fn metadata_timestamp(state: &ComposerState, field: MetadataField) -> Option<OffsetDateTime> {
    let ticket = state
        .selected_source()
        .or_else(|| state.selected_changes())?;
    let metadata = (!ticket.key.starts_with("NEW-"))
        .then_some(ticket.jira_metadata.as_ref())
        .flatten()?;
    let value = match field {
        MetadataField::Reporter => return None,
        MetadataField::Created => &metadata.created,
        MetadataField::Updated => &metadata.updated,
    };
    let timestamp = parse_timestamp(value).ok()?;
    OffsetDateTime::from_unix_timestamp_nanos(timestamp.timestamp_nanos_opt()? as i128).ok()
}

fn value_or_unavailable(value: &str) -> &str {
    if value.trim().is_empty() {
        "Unavailable"
    } else {
        value
    }
}

fn format_timestamp(value: &str) -> String {
    parse_timestamp(value)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| value_or_unavailable(value).into())
}

fn parse_timestamp(value: &str) -> Result<DateTime<FixedOffset>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::<FixedOffset>::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f%z"))
}

impl TuiNode for JiraMetadata {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.fields.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.fields.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.fields.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.fields.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.fields.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: std::time::Duration, settings: AnimationSettings) -> TickResult {
        self.fields.tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.fields.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.fields.dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.fields.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.fields.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.fields.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.fields.destroy(ctx);
    }
}

impl TuiNode for ReadonlyMetadataText {
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

    fn tick(&mut self, dt: std::time::Duration, settings: AnimationSettings) -> TickResult {
        let changed = self.sync();
        let relative_tick = self
            .relative_date
            .as_mut()
            .map_or(TickResult::IDLE, |relative_date| {
                <RelativeDate as TuiNode<()>>::tick(relative_date, dt, settings)
            });
        if relative_tick.changed
            && let Some(relative_date) = &self.relative_date
        {
            self.input.set_bottom_left(relative_date.text());
        }
        self.input
            .tick(dt, settings)
            .merge(relative_tick)
            .merge(if changed {
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
        if self.relative_date.is_some() {
            ctx.request_tick();
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.input.destroy(ctx);
    }
}
