use std::{collections::HashSet, rc::Rc, sync::mpsc::Sender, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
    widgets::Borders,
};
use tuicore::{
    AnimationSettings, Button, Column, CrossAlign, Dialog, DialogAction, DialogBackdrop,
    DialogHost, DialogLayer, DialogLayerPlacement, DockChrome, DockSpec, Dropdown,
    DropdownLabelPosition, DropdownVariant, EventCtx, EventOutcome, EventRoute, Flex, FlexItem,
    FocusCtx, FocusId, FocusRequest, FocusTarget, Key, KeyEvent, KeySpec, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl, ListControlField,
    ListControlKeyBindings, RenderCtx, ScrollContainer, ScrollbarConfig, TextInput, TickResult,
    Toggle, TuiEvent, TuiNode,
};

use super::filter_values::{SavedFilterField, option_id, option_value};
use crate::store::work_items::saved_filter::{BacklogFilterOptions, SavedBacklogFilter};
use crate::{
    app_settings::{BacklogKeyBindings, ComposerKeyBinding},
    components::text_entry_paths::TextEntryPaths,
};

type SavedFilterDialogBody = DialogHost<ScrollContainer<SavedFilterContent>, ()>;
type SavedFilterManagerDock = DialogLayer<Flex<()>, SavedFilterDialogBody>;

const SAVED_FILTER_DOCK_PERCENT: u16 = 40;

pub(in crate::pages::backlog) struct SavedFilterDialog {
    layer: DialogLayer<SavedFilterManagerDock, Dialog<()>>,
    text_entry_paths: TextEntryPaths,
    done_key: ComposerKeyBinding,
    sender: Sender<SavedFilterManagerEvent>,
}

impl SavedFilterDialog {
    pub(in crate::pages::backlog) fn done_key(mut self, key: ComposerKeyBinding) -> Self {
        self.done_key = key;
        self
    }

    fn done_matches(&self, event: &TuiEvent) -> bool {
        matches!(event, TuiEvent::Key(key) if self.done_key.matches(*key))
    }

    fn finish(&self, ctx: &mut EventCtx<()>) -> EventOutcome {
        let _ = self.sender.send(SavedFilterManagerEvent::Finish);
        ctx.stop_propagation();
        EventOutcome::Handled
    }
}

struct SavedFilterContent {
    content: Flex<()>,
}

impl TuiNode for SavedFilterContent {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.content.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        ctx.with_overlay_bounds(area, |ctx| self.content.layout(area, ctx))
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.content.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.content.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.content.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.content.tick(dt, settings)
    }

    fn take_pending_focus_request(&mut self) -> Option<FocusRequest> {
        self.content.take_pending_focus_request()
    }

    fn take_pending_clipboard_request(&mut self) -> Option<String> {
        self.content.take_pending_clipboard_request()
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.content.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.content.dispatch_focus(target, focused, ctx);
    }

    fn focus_reveal_area(&self, target: &FocusTarget) -> Option<Rect> {
        self.content.focus_reveal_area(target)
    }

    fn focus_reveal_centered(&self, target: &FocusTarget) -> bool {
        self.content.focus_reveal_centered(target)
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

impl DockChrome for SavedFilterDialog {
    fn set_dock_edge_borders(&mut self, borders: Borders) {
        self.layer
            .base_mut()
            .layer_mut()
            .set_dock_edge_borders(borders);
    }
}

impl TuiNode for SavedFilterDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.layer.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let path = ctx.current_path();
        let result = self.layer.layout(area, ctx);
        self.text_entry_paths.capture(ctx, &path);
        result
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.layer.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if self.done_matches(event) {
            return self.finish(ctx);
        }
        self.layer.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if self.done_matches(event) {
            if !self.layer.is_active() && self.text_entry_paths.contains(&route.path) {
                self.layer
                    .dispatch_event(route, &TuiEvent::Key(KeyEvent::from(Key::Enter)), ctx);
            }
            return self.finish(ctx);
        }
        self.layer.dispatch_event(route, event, ctx)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.layer.tick(dt, settings)
    }

    fn take_pending_focus_request(&mut self) -> Option<FocusRequest> {
        self.layer.take_pending_focus_request()
    }

    fn take_pending_clipboard_request(&mut self) -> Option<String> {
        self.layer.take_pending_clipboard_request()
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.layer.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.layer.dispatch_focus(target, focused, ctx);
    }

    fn focus_reveal_area(&self, target: &FocusTarget) -> Option<Rect> {
        self.layer.focus_reveal_area(target)
    }

    fn focus_reveal_centered(&self, target: &FocusTarget) -> bool {
        self.layer.focus_reveal_centered(target)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.layer.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.layer.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.layer.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.layer.destroy(ctx);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::pages::backlog) enum SavedFilterManagerEvent {
    Finish,
    SelectForEditing(u64),
    Create,
    RequestDelete(u64),
    ConfirmDelete(u64),
    CancelDelete,
    Rename {
        id: u64,
        name: String,
    },
    SetEstimated {
        id: u64,
        estimated: bool,
    },
    SetValues {
        id: u64,
        field: SavedFilterField,
        values: Vec<String>,
    },
}

#[derive(Clone)]
struct FilterValueRow {
    value: String,
    available: bool,
}

struct FilterValueList {
    list: ListControl<FilterValueRow, String>,
    filter_id: Option<u64>,
    field: SavedFilterField,
    available: Vec<String>,
    sender: Sender<SavedFilterManagerEvent>,
}

impl FilterValueList {
    fn new(
        title: &str,
        filter_id: Option<u64>,
        values: Vec<String>,
        available: Vec<String>,
        field: SavedFilterField,
        sender: Sender<SavedFilterManagerEvent>,
    ) -> Self {
        let values = field.selection(values);
        let available = field.options(available);
        let rows = value_rows(&values, &available);
        let available_for_creator = available.clone();
        let mut list = ListControl::new_fields(
            rows,
            |row: &FilterValueRow| row.value.clone(),
            [ListControlField::dropdown_options(
                "Add value",
                available
                    .iter()
                    .map(|value| (option_id(value), field.label(value))),
            )],
            move |values, _| {
                let value = option_value(&values[0]).to_owned();
                FilterValueRow {
                    available: available_for_creator
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&value)),
                    value,
                }
            },
        )
        .column(Column::text(
            "value",
            "",
            Constraint::Percentage(100),
            move |row: &FilterValueRow| {
                if row.available {
                    field.label(&row.value)
                } else {
                    format!("{} · unavailable", row.value)
                }
            },
        ))
        .headers(false)
        .filter_controls(false)
        .action_bar(true)
        .row_height(1)
        .max_rows(6)
        .keybindings(ListControlKeyBindings::default().remove([KeySpec::plain('-')]))
        .title(title)
        .hotkey(field.hotkey())
        .empty_message("No values selected")
        .disabled(filter_id.is_none());
        list.data_view_mut().set_row_style_by(move |row| {
            if row.available {
                field.style(&row.value)
            } else {
                Some(Style::default().fg(tuicore::theme().error_fg()))
            }
        });
        list.set_dropdown_row_style_by(0, move |id, _| field.style(option_value(id)));
        let mut control = Self {
            list,
            filter_id,
            field,
            available,
            sender,
        };
        control.refresh_dropdown_options();
        control
    }

    fn sync(&mut self) {
        if self.list.take_events().is_empty() {
            return;
        }
        let values = unique_values(
            self.list
                .items()
                .iter()
                .map(|row| row.value.clone())
                .collect(),
        );
        self.list.set_rows(value_rows(&values, &self.available));
        self.refresh_dropdown_options();
        if let Some(id) = self.filter_id {
            let _ = self.sender.send(SavedFilterManagerEvent::SetValues {
                id,
                field: self.field,
                values,
            });
        }
    }

    fn refresh_dropdown_options(&mut self) {
        let field = self.field;
        let selected = self
            .list
            .items()
            .iter()
            .map(|row| row.value.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        self.list.set_dropdown_rows(
            0,
            self.available
                .iter()
                .filter(|value| !selected.contains(&value.to_ascii_lowercase()))
                .map(|value| (option_id(value), field.label(value))),
        );
        self.list.set_dropdown_row_height(0, 1);
    }
}

impl TuiNode for FilterValueList {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.list.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.list.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.list.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.list.event(event, ctx);
        self.sync();
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.list.dispatch_event(route, event, ctx);
        self.sync();
        outcome
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.list.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.list.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.list.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.list.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.list.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.list.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.list.destroy(ctx);
    }
}

pub(in crate::pages::backlog) fn saved_filter_dialog(
    filters: &[SavedBacklogFilter],
    selected_id: Option<u64>,
    options: &BacklogFilterOptions,
    sender: Sender<SavedFilterManagerEvent>,
    close_requested: Rc<std::cell::Cell<bool>>,
    pending_delete: Option<(u64, String)>,
) -> SavedFilterDialog {
    let selected = selected_id.and_then(|id| filters.iter().find(|filter| filter.id == id));
    let selector_sender = sender.clone();
    let mut selector = Dropdown::single(
        filters.to_vec(),
        |filter: &SavedBacklogFilter| filter.id,
        |filter| {
            if filter.name.trim().is_empty() {
                "New filter".into()
            } else {
                filter.name.clone()
            }
        },
    )
    .label_position(DropdownLabelPosition::Inline)
    .variant(DropdownVariant::Filled)
    .placeholder("Select filter")
    .hotkey("shift+f")
    .on_select(move |ids| {
        if let Some(id) = ids.first().copied() {
            let _ = selector_sender.send(SavedFilterManagerEvent::SelectForEditing(id));
        }
    });
    if let Some(filter) = selected {
        selector.set_selected_one(filter.id);
    }

    let create_sender = sender.clone();
    let create = Button::new("New").hotkey("shift+n").on_press(move || {
        let _ = create_sender.send(SavedFilterManagerEvent::Create);
    });
    let delete_sender = sender.clone();
    let delete_id = selected.map(|filter| filter.id);
    let delete = Button::new("Delete")
        .hotkey("shift+d")
        .disabled(delete_id.is_none())
        .on_press(move || {
            if let Some(id) = delete_id {
                let _ = delete_sender.send(SavedFilterManagerEvent::RequestDelete(id));
            }
        });

    let rename_sender = sender.clone();
    let rename_id = selected.map(|filter| filter.id);
    let mut name = TextInput::new()
        .value(
            selected
                .map(|filter| filter.name.as_str())
                .unwrap_or_default(),
        )
        .placeholder("Filter name")
        .panel("Name")
        .disabled(rename_id.is_none())
        .on_edit_end(move |name| {
            if let Some(id) = rename_id {
                let _ = rename_sender.send(SavedFilterManagerEvent::Rename { id, name });
            }
        });
    if selected.is_some_and(|filter| filter.name.is_empty()) {
        name.set_insert_mode(true);
    }

    let estimated_sender = sender.clone();
    let estimated_id = selected.map(|filter| filter.id);
    let estimated = Toggle::new("Include estimated tickets")
        .checked(selected.is_none_or(|filter| filter.criteria.estimated))
        .disabled(estimated_id.is_none())
        .on_change(move |estimated| {
            if let Some(id) = estimated_id {
                let _ =
                    estimated_sender.send(SavedFilterManagerEvent::SetEstimated { id, estimated });
            }
        });

    let values = |field| filter_values(selected, field);
    let mut content = Flex::column()
        .child(
            "toolbar",
            Flex::row()
                .align(CrossAlign::Center)
                .child("selector", selector, FlexItem::fill(1))
                .child("new", create, FlexItem::fit_content())
                .child("delete", delete, FlexItem::fit_content()),
            FlexItem::fixed(1),
        )
        .child("name", name, FlexItem::fixed(3))
        .child("estimated", estimated, FlexItem::fixed(1));

    for (key, title, field, selected_values, available) in [
        (
            "issue-types",
            "Types",
            SavedFilterField::IssueTypes,
            values(SavedFilterField::IssueTypes),
            options.issue_types.clone(),
        ),
        (
            "users",
            "Users",
            SavedFilterField::Users,
            values(SavedFilterField::Users),
            options.users.clone(),
        ),
        (
            "statuses",
            "Statuses",
            SavedFilterField::Statuses,
            values(SavedFilterField::Statuses),
            options.statuses.clone(),
        ),
        (
            "epics",
            "Epics",
            SavedFilterField::Epics,
            values(SavedFilterField::Epics),
            options.epics.clone(),
        ),
        (
            "labels",
            "Labels",
            SavedFilterField::Labels,
            values(SavedFilterField::Labels),
            options.labels.clone(),
        ),
        (
            "releases",
            "Releases",
            SavedFilterField::Releases,
            values(SavedFilterField::Releases),
            options.releases.clone(),
        ),
    ] {
        content = content.child(
            key,
            FilterValueList::new(
                title,
                selected.map(|filter| filter.id),
                selected_values,
                available,
                field,
                sender.clone(),
            ),
            FlexItem::fit_content(),
        );
    }

    let manager = Dialog::new()
        .edge_borders(Borders::ALL)
        .dock_padding(0)
        .focused(true)
        .on_close(move |_| close_requested.set(true))
        .host(
            ScrollContainer::vertical(SavedFilterContent { content })
                .scrollbars(ScrollbarConfig::default())
                .focus_reveal(true),
        );
    let confirmation_open = pending_delete.is_some();
    let confirmation = pending_delete.map_or_else(Dialog::new, |(id, name)| {
        delete_saved_filter_dialog(id, &name, sender.clone())
    });
    let manager = DialogLayer::new(Flex::column(), manager)
        .active(true)
        .docked(DockSpec::right(SAVED_FILTER_DOCK_PERCENT));
    SavedFilterDialog {
        text_entry_paths: TextEntryPaths::default(),
        done_key: BacklogKeyBindings::default().saved_filter_done,
        sender,
        layer: DialogLayer::new(manager, confirmation)
            .active(confirmation_open)
            .base_overlays_visible(true)
            .fit_content()
            .fit_content_max(64, 8)
            .placement(DialogLayerPlacement::Center)
            .backdrop(DialogBackdrop::dim().amount(0.45)),
    }
}

fn delete_saved_filter_dialog(
    id: u64,
    name: &str,
    sender: Sender<SavedFilterManagerEvent>,
) -> Dialog<()> {
    let confirm_sender = sender.clone();
    let cancel_sender = sender.clone();
    Dialog::new()
        .top_left("Delete saved filter?")
        .edge_borders(Borders::ALL)
        .content([format!("Delete “{name}”? This cannot be undone.")])
        .actions([
            DialogAction::new("Delete")
                .hotkey(KeySpec::plain('d'))
                .on_trigger(move || {
                    let _ = confirm_sender.send(SavedFilterManagerEvent::ConfirmDelete(id));
                }),
            DialogAction::new("Cancel")
                .hotkey(KeySpec::plain('c'))
                .on_trigger(move || {
                    let _ = cancel_sender.send(SavedFilterManagerEvent::CancelDelete);
                }),
        ])
        .on_close(move |_| {
            let _ = sender.send(SavedFilterManagerEvent::CancelDelete);
        })
}

fn filter_values(filter: Option<&SavedBacklogFilter>, field: SavedFilterField) -> Vec<String> {
    let Some(filter) = filter else {
        return Vec::new();
    };
    match field {
        SavedFilterField::IssueTypes => filter.criteria.issue_types.clone(),
        SavedFilterField::Users => filter.criteria.users.clone(),
        SavedFilterField::Statuses => filter.criteria.statuses.clone(),
        SavedFilterField::Epics => filter.criteria.epics.clone(),
        SavedFilterField::Labels => filter.criteria.labels.clone(),
        SavedFilterField::Releases => filter.criteria.releases.clone(),
    }
}

fn value_rows(values: &[String], available: &[String]) -> Vec<FilterValueRow> {
    values
        .iter()
        .map(|value| FilterValueRow {
            value: value.clone(),
            available: available
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(value)),
        })
        .collect()
}

fn unique_values(values: Vec<String>) -> Vec<String> {
    values.into_iter().fold(Vec::new(), |mut unique, value| {
        if !unique
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            unique.push(value);
        }
        unique
    })
}
