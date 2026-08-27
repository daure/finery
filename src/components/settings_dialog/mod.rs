use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    PasswordInput, RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app_settings::AppSettings,
    service::AppService,
    speed_reader_settings::{
        MAX_MARKDOWN_BLOCK_PAUSE_MS, MAX_SPEED_READER_WPM, MIN_SPEED_READER_WPM,
        parse_markdown_block_pause, parse_speed_reader_wpm,
    },
};

enum SettingChange {
    JiraBaseUrl(String),
    JiraEmail(String),
    JiraApiToken(String),
    JiraDefaultProject(String),
    JiraDefaultBoard(String),
    JiraStoryPointsFieldId(String),
    Wpm(String),
    MarkdownBlockPause(String),
}

pub(crate) struct SettingsDialog {
    root: Flex<()>,
    changes: Rc<RefCell<Vec<SettingChange>>>,
    settings: Arc<RwLock<AppSettings>>,
    service: AppService,
}

impl SettingsDialog {
    pub(crate) fn new(settings: Arc<RwLock<AppSettings>>, service: AppService) -> Self {
        let values = settings.read().expect("settings lock poisoned").clone();
        let changes = Rc::new(RefCell::new(Vec::new()));
        let url_changes = Rc::clone(&changes);
        let email_changes = Rc::clone(&changes);
        let token_changes = Rc::clone(&changes);
        let project_changes = Rc::clone(&changes);
        let board_changes = Rc::clone(&changes);
        let story_points_changes = Rc::clone(&changes);
        let wpm_changes = Rc::clone(&changes);
        let delay_changes = Rc::clone(&changes);
        let root = Flex::column()
            .child(
                "jira-base-url",
                TextInput::new()
                    .value(values.jira_base_url.clone())
                    .panel("Jira URL")
                    .placeholder("https://example.atlassian.net")
                    .focused(true)
                    .on_edit_end(move |value| {
                        url_changes
                            .borrow_mut()
                            .push(SettingChange::JiraBaseUrl(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "jira-email",
                TextInput::new()
                    .value(values.jira_email.clone())
                    .panel("Jira email")
                    .on_edit_end(move |value| {
                        email_changes
                            .borrow_mut()
                            .push(SettingChange::JiraEmail(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "jira-api-token",
                PasswordInput::new()
                    .value(values.jira_api_token.clone())
                    .panel("Jira API token")
                    .on_edit_end(move |value| {
                        token_changes
                            .borrow_mut()
                            .push(SettingChange::JiraApiToken(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "jira-default-project",
                TextInput::new()
                    .value(values.jira_default_project.clone())
                    .panel("Default Jira project")
                    .placeholder("FIN")
                    .on_edit_end(move |value| {
                        project_changes
                            .borrow_mut()
                            .push(SettingChange::JiraDefaultProject(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "jira-default-board",
                TextInput::new()
                    .value(values.jira_default_board.clone())
                    .numbers_only(true)
                    .panel("Default Jira board ID")
                    .placeholder("Auto-select from project")
                    .on_edit_end(move |value| {
                        board_changes
                            .borrow_mut()
                            .push(SettingChange::JiraDefaultBoard(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "jira-story-points-field-id",
                TextInput::new()
                    .value(values.jira_story_points_field_id.clone())
                    .panel("Jira story-points custom-field ID")
                    .placeholder("Custom field ID")
                    .on_edit_end(move |value| {
                        story_points_changes
                            .borrow_mut()
                            .push(SettingChange::JiraStoryPointsFieldId(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "speed-reader-wpm",
                TextInput::new()
                    .value(values.speed_reader.wpm.to_string())
                    .numbers_only(true)
                    .panel("Reader WPM")
                    .on_edit_end(move |value| {
                        wpm_changes.borrow_mut().push(SettingChange::Wpm(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "markdown-block-pause",
                TextInput::new()
                    .value(values.block_delay().as_millis().to_string())
                    .numbers_only(true)
                    .panel("Reader block delay (ms)")
                    .on_edit_end(move |value| {
                        delay_changes
                            .borrow_mut()
                            .push(SettingChange::MarkdownBlockPause(value));
                    }),
                FlexItem::fixed(3),
            );
        Self {
            root,
            changes,
            settings,
            service,
        }
    }

    fn apply_changes(&self, ctx: &mut EventCtx<()>) {
        let mut settings = self
            .settings
            .read()
            .expect("settings lock poisoned")
            .clone();
        let mut changed = false;
        for change in self.changes.borrow_mut().drain(..) {
            match change {
                SettingChange::JiraBaseUrl(value) => {
                    settings.jira_base_url = value.trim().trim_end_matches('/').into();
                    changed = true;
                }
                SettingChange::JiraEmail(value) => {
                    settings.jira_email = value.trim().into();
                    changed = true;
                }
                SettingChange::JiraApiToken(value) => {
                    settings.jira_api_token = value.trim().into();
                    changed = true;
                }
                SettingChange::JiraDefaultProject(value) => {
                    settings.jira_default_project = value.trim().to_ascii_uppercase();
                    changed = true;
                }
                SettingChange::JiraDefaultBoard(value) => {
                    settings.jira_default_board = value.trim().into();
                    changed = true;
                }
                SettingChange::JiraStoryPointsFieldId(value) => {
                    settings.jira_story_points_field_id = value.trim().into();
                    changed = true;
                }
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
                    settings.speed_reader.wpm = wpm;
                    changed = true;
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
                    settings.speed_reader.markdown_block_pause = markdown_block_pause;
                    changed = true;
                }
            }
        }
        if changed {
            self.service.save_settings(settings);
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

#[cfg(test)]
mod tests;
