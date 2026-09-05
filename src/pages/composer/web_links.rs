use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::Style,
};
use tuicore::{
    AnimationSettings, Column, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl,
    ListControlEvent, ListControlField, RenderCtx, TickResult, TuiEvent, TuiNode, theme,
};

use crate::{
    app_settings::ComposerSequenceBinding,
    service::{AppService, composer_service::validate_web_link},
    store::composer::{ComposerAction, ComposerState, ComposerViewMode, TicketWebLink},
};

type PendingActions = Rc<RefCell<Vec<ComposerAction>>>;
type WebLinkControl = ListControl<WebLinkRow, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WebLinkDiff {
    None,
    Added,
    Removed,
}

#[derive(Clone, PartialEq, Eq)]
struct WebLinkRow {
    row_id: String,
    link_id: String,
    title: String,
    url: String,
    diff: WebLinkDiff,
}

pub(super) struct BoundWebLinks {
    state: Rc<RefCell<ComposerState>>,
    pending: PendingActions,
    service: AppService,
    control: WebLinkControl,
    synced_rows: Vec<WebLinkRow>,
    disabled: bool,
}

impl BoundWebLinks {
    pub(super) fn new(
        state: Rc<RefCell<ComposerState>>,
        pending: PendingActions,
        service: AppService,
        hotkey: ComposerSequenceBinding,
    ) -> Self {
        let mut control = ListControl::new_fields(
            [],
            |row: &WebLinkRow| row.row_id.clone(),
            [
                ListControlField::text("Link title"),
                ListControlField::text("https://example.com"),
            ],
            |values, _| WebLinkRow {
                row_id: local_web_link_id(),
                link_id: String::new(),
                title: values[0].trim().into(),
                url: values[1].trim().into(),
                diff: WebLinkDiff::None,
            },
        )
        .editable(
            |row| vec![row.title.clone(), row.url.clone()],
            |row, values| {
                row.title = values[0].trim().into();
                row.url = values[1].trim().into();
            },
        )
        .columns([
            Column::text("title", "", Constraint::Length(0), web_link_title),
            Column::text("url", "", Constraint::Length(0), |row: &WebLinkRow| {
                row.url.clone()
            }),
        ])
        .headers(false)
        .focus_id("web-links-data-view")
        .row_height(1)
        .max_rows(3)
        .filter_controls(false)
        .action_bar(true)
        .title("Web links")
        .hotkey(hotkey.sequence())
        .empty_message("No web links");
        control.data_view_mut().set_row_style_by(|row| {
            let theme = theme();
            match row.diff {
                WebLinkDiff::None => None,
                WebLinkDiff::Added => Some(
                    Style::default()
                        .fg(theme.diff_added_fg())
                        .bg(theme.diff_added_bg()),
                ),
                WebLinkDiff::Removed => Some(
                    Style::default()
                        .fg(theme.diff_removed_fg())
                        .bg(theme.diff_removed_bg()),
                ),
            }
        });
        let mut bound = Self {
            state,
            pending,
            service,
            control,
            synced_rows: Vec::new(),
            disabled: false,
        };
        bound.sync();
        bound
    }

    fn sync(&mut self) -> bool {
        let state = self.state.borrow();
        let rows = web_link_rows(&state);
        let disabled = !state.selected_is_editable();
        let changed = rows != self.synced_rows || disabled != self.disabled;
        if rows != self.synced_rows {
            self.control.set_rows(rows.clone());
            self.synced_rows = rows;
        }
        if disabled != self.disabled {
            self.control.set_disabled(disabled);
            self.disabled = disabled;
        }
        changed
    }

    fn drain_events(&mut self) {
        for event in self.control.take_events() {
            let action = match event {
                ListControlEvent::Added { row_id } => {
                    let row = self
                        .control
                        .items()
                        .iter()
                        .find(|row| row.row_id == row_id)
                        .cloned();
                    row.and_then(|row| {
                        self.valid_values(&row.title, &row.url).map_or_else(
                            || {
                                self.control.set_rows(self.synced_rows.clone());
                                None
                            },
                            |(title, url)| {
                                Some(ComposerAction::AddWebLink {
                                    id: row_id,
                                    title,
                                    url,
                                })
                            },
                        )
                    })
                }
                ListControlEvent::Edited { row_id } => {
                    let row = self
                        .control
                        .items()
                        .iter()
                        .find(|row| row.row_id == row_id)
                        .cloned();
                    row.and_then(|row| {
                        self.valid_values(&row.title, &row.url).map_or_else(
                            || {
                                self.control.set_rows(self.synced_rows.clone());
                                None
                            },
                            |(title, url)| {
                                Some(ComposerAction::UpdateWebLink {
                                    id: row.link_id,
                                    title,
                                    url,
                                })
                            },
                        )
                    })
                }
                ListControlEvent::Removed { row_id } => self
                    .synced_rows
                    .iter()
                    .find(|row| row.row_id == row_id)
                    .map(|row| ComposerAction::RemoveWebLink(row.link_id.clone())),
                _ => None,
            };
            if let Some(action) = action {
                self.pending.borrow_mut().push(action);
            }
        }
    }

    fn valid_values(&self, title: &str, url: &str) -> Option<(String, String)> {
        let values = validated_web_link(title, url);
        if values.is_none() {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Invalid web link",
                    "Enter a title and a valid web address.",
                ));
        }
        values
    }
}

fn validated_web_link(title: &str, url: &str) -> Option<(String, String)> {
    validate_web_link(title.into(), url.into()).ok()
}

fn web_link_rows(state: &ComposerState) -> Vec<WebLinkRow> {
    if state.view_mode != ComposerViewMode::Diff {
        return state
            .selected_ticket()
            .into_iter()
            .flat_map(|ticket| &ticket.web_links)
            .map(|link| row(link, link.id.clone(), WebLinkDiff::None))
            .collect();
    }
    let source = state
        .selected_source()
        .map(|ticket| &ticket.web_links)
        .cloned()
        .unwrap_or_default();
    let changes = state
        .selected_changes()
        .map(|ticket| &ticket.web_links)
        .cloned()
        .unwrap_or_default();
    let source_by_id = source
        .iter()
        .map(|link| (link.id.as_str(), link))
        .collect::<HashMap<_, _>>();
    let changes_by_id = changes
        .iter()
        .map(|link| (link.id.as_str(), link))
        .collect::<HashMap<_, _>>();
    let mut rows = Vec::new();
    for link in &source {
        match changes_by_id.get(link.id.as_str()) {
            Some(changed) if changed.title == link.title && changed.url == link.url => {
                rows.push(row(link, link.id.clone(), WebLinkDiff::None));
            }
            Some(changed) => {
                rows.push(row(
                    link,
                    format!("diff-removed:{}", link.id),
                    WebLinkDiff::Removed,
                ));
                rows.push(row(
                    changed,
                    format!("diff-added:{}", link.id),
                    WebLinkDiff::Added,
                ));
            }
            None => rows.push(row(
                link,
                format!("diff-removed:{}", link.id),
                WebLinkDiff::Removed,
            )),
        }
    }
    rows.extend(
        changes
            .iter()
            .filter(|link| !source_by_id.contains_key(link.id.as_str()))
            .map(|link| row(link, format!("diff-added:{}", link.id), WebLinkDiff::Added)),
    );
    rows
}

fn row(link: &TicketWebLink, row_id: String, diff: WebLinkDiff) -> WebLinkRow {
    WebLinkRow {
        row_id,
        link_id: link.id.clone(),
        title: link.title.clone(),
        url: link.url.clone(),
        diff,
    }
}

fn web_link_title(row: &WebLinkRow) -> String {
    let marker = match row.diff {
        WebLinkDiff::None => "",
        WebLinkDiff::Added => "+ ",
        WebLinkDiff::Removed => "- ",
    };
    format!("{marker}{}", row.title)
}

fn local_web_link_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("local-{timestamp}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

impl TuiNode for BoundWebLinks {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.control.measure(proposal)
    }
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync();
        self.control.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.control.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.control.event(event, ctx);
        self.drain_events();
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.control.dispatch_event(route, event, ctx);
        self.drain_events();
        outcome
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.control.tick(dt, settings).merge(if self.sync() {
            TickResult::CHANGED
        } else {
            TickResult::IDLE
        })
    }
    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.focus(target, focused, ctx);
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.control.dispatch_focus(target, focused, ctx);
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.control.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::validated_web_link;

    #[test]
    fn web_link_validation_rejects_malformed_and_unsupported_urls() {
        assert!(validated_web_link("Docs", "not a url").is_none());
        assert!(validated_web_link("Docs", "localhost").is_none());
        assert!(validated_web_link("Docs", "https://intranet").is_none());
        assert!(validated_web_link("Docs", "www..example.com").is_none());
        assert!(validated_web_link("Docs", "-bad.example.com").is_none());
        assert!(validated_web_link("Docs", "ftp://example.com/docs").is_none());
        assert!(validated_web_link("", "https://example.com/docs").is_none());
        assert_eq!(
            validated_web_link(" Docs ", " https://example.com/docs "),
            Some(("Docs".into(), "https://example.com/docs".into()))
        );
        assert_eq!(
            validated_web_link("Google", "www.google.com"),
            Some(("Google".into(), "https://www.google.com".into()))
        );
    }
}
