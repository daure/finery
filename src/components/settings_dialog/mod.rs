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
    PasswordInput, RenderCtx, TextInput, TickResult, Toggle, TuiEvent, TuiNode,
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
    JiraCompanyManagedUrls(bool),
    JiraStoryPointsFieldId(String),
    BacklogUseJiraVelocity(bool),
    BacklogJiraVelocitySprints(String),
    BacklogFixedSprintCapacity(String),
    BacklogUseAverageTicketSize(bool),
    BacklogFixedTicketSize(String),
    BacklogSprintTolerancePercent(String),
    Wpm(String),
    MarkdownBlockPause(String),
    RecentTicketsLimit(String),
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
        let company_managed_urls_changes = Rc::clone(&changes);
        let story_points_changes = Rc::clone(&changes);
        let velocity_changes = Rc::clone(&changes);
        let velocity_sprints_changes = Rc::clone(&changes);
        let sprint_capacity_changes = Rc::clone(&changes);
        let average_ticket_size_changes = Rc::clone(&changes);
        let ticket_size_changes = Rc::clone(&changes);
        let tolerance_changes = Rc::clone(&changes);
        let wpm_changes = Rc::clone(&changes);
        let delay_changes = Rc::clone(&changes);
        let recent_tickets_limit_changes = Rc::clone(&changes);
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
                "jira-company-managed-urls",
                Toggle::new("Jira project is company-managed")
                    .checked(values.jira_company_managed_urls)
                    .on_change(move |value| {
                        company_managed_urls_changes
                            .borrow_mut()
                            .push(SettingChange::JiraCompanyManagedUrls(value));
                    }),
                FlexItem::fixed(1),
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
                "backlog-use-jira-velocity",
                Toggle::new("Backlog capacity: use Jira velocity")
                    .checked(values.backlog_runway.use_jira_velocity)
                    .on_change(move |value| {
                        velocity_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogUseJiraVelocity(value));
                    }),
                FlexItem::fixed(1),
            )
            .child(
                "backlog-jira-velocity-sprints",
                TextInput::new()
                    .value(values.backlog_runway.jira_velocity_sprints.to_string())
                    .numbers_only(true)
                    .panel("Jira velocity sprints to average")
                    .placeholder("4")
                    .on_edit_end(move |value| {
                        velocity_sprints_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogJiraVelocitySprints(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "backlog-fixed-sprint-capacity",
                TextInput::new()
                    .value(values.backlog_runway.fixed_sprint_capacity.to_string())
                    .panel("Backlog fixed / fallback sprint capacity")
                    .placeholder("20")
                    .on_edit_end(move |value| {
                        sprint_capacity_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogFixedSprintCapacity(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "backlog-use-average-ticket-size",
                Toggle::new("Backlog assumptions: average loaded estimates")
                    .checked(values.backlog_runway.use_average_ticket_size)
                    .on_change(move |value| {
                        average_ticket_size_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogUseAverageTicketSize(value));
                    }),
                FlexItem::fixed(1),
            )
            .child(
                "backlog-fixed-ticket-size",
                TextInput::new()
                    .value(values.backlog_runway.fixed_ticket_size.to_string())
                    .panel("Backlog fixed assumed ticket size")
                    .placeholder("3")
                    .on_edit_end(move |value| {
                        ticket_size_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogFixedTicketSize(value));
                    }),
                FlexItem::fixed(3),
            )
            .child(
                "backlog-sprint-tolerance-percent",
                TextInput::new()
                    .value(values.backlog_runway.sprint_tolerance_percent.to_string())
                    .numbers_only(true)
                    .panel("Sprint target tolerance (%)")
                    .placeholder("20")
                    .on_edit_end(move |value| {
                        tolerance_changes
                            .borrow_mut()
                            .push(SettingChange::BacklogSprintTolerancePercent(value));
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
            )
            .child(
                "recent-tickets-limit",
                TextInput::new()
                    .value(values.recent_tickets_limit.to_string())
                    .numbers_only(true)
                    .panel("Recent tickets to remember")
                    .placeholder("15")
                    .on_edit_end(move |value| {
                        recent_tickets_limit_changes
                            .borrow_mut()
                            .push(SettingChange::RecentTicketsLimit(value));
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
                    let value = value.trim().trim_end_matches('/').to_owned();
                    if settings.jira_base_url != value {
                        settings.jira_base_url = value;
                        settings.invalidate_discovered_story_points();
                    }
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
                    let value = value.trim().to_owned();
                    if settings.jira_default_board != value {
                        settings.jira_default_board = value;
                        settings.invalidate_discovered_story_points();
                    }
                    changed = true;
                }
                SettingChange::JiraCompanyManagedUrls(value) => {
                    settings.jira_company_managed_urls = value;
                    changed = true;
                }
                SettingChange::JiraStoryPointsFieldId(value) => {
                    settings.set_manual_story_points_field(value.trim().into());
                    changed = true;
                }
                SettingChange::BacklogUseJiraVelocity(value) => {
                    settings.backlog_runway.use_jira_velocity = value;
                    changed = true;
                }
                SettingChange::BacklogJiraVelocitySprints(value) => {
                    let Some(value) = value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|value| *value > 0)
                    else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid velocity sprint count",
                            "Enter a whole number greater than zero.",
                        ));
                        continue;
                    };
                    settings.backlog_runway.jira_velocity_sprints = value;
                    changed = true;
                }
                SettingChange::BacklogFixedSprintCapacity(value) => {
                    let Some(value) = parse_positive_number(&value) else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid sprint capacity",
                            "Enter a positive number of story points.",
                        ));
                        continue;
                    };
                    settings.backlog_runway.fixed_sprint_capacity = value;
                    changed = true;
                }
                SettingChange::BacklogUseAverageTicketSize(value) => {
                    settings.backlog_runway.use_average_ticket_size = value;
                    changed = true;
                }
                SettingChange::BacklogFixedTicketSize(value) => {
                    let Some(value) = parse_nonnegative_number(&value) else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid assumed ticket size",
                            "Enter zero or a positive number of story points.",
                        ));
                        continue;
                    };
                    settings.backlog_runway.fixed_ticket_size = value;
                    changed = true;
                }
                SettingChange::BacklogSprintTolerancePercent(value) => {
                    let Some(value) = value
                        .trim()
                        .parse::<u8>()
                        .ok()
                        .filter(|value| *value <= 100)
                    else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid sprint tolerance",
                            "Enter a whole percentage from 0 to 100.",
                        ));
                        continue;
                    };
                    settings.backlog_runway.sprint_tolerance_percent = value;
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
                SettingChange::RecentTicketsLimit(value) => {
                    let Some(value) = value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|value| (1..=100).contains(value))
                    else {
                        ctx.notify(tuicore::Notification::warning(
                            "Invalid recent ticket limit",
                            "Enter a whole number from 1 to 100.",
                        ));
                        continue;
                    };
                    settings.recent_tickets_limit = value;
                    changed = true;
                }
            }
        }
        if changed {
            self.service.save_settings(settings);
        }
    }
}

fn parse_positive_number(value: &str) -> Option<f64> {
    parse_nonnegative_number(value).filter(|value| *value > 0.0)
}

fn parse_nonnegative_number(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
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
