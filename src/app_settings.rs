use std::{collections::HashMap, env, time::Duration};

use tuicore::{Key, KeyModifiers, KeySpec};

use crate::speed_reader_settings::{
    SpeedReaderSettings, parse_markdown_block_pause, parse_speed_reader_wpm,
};

pub(crate) const JIRA_BASE_URL_SETTING: &str = "jira.base_url";
pub(crate) const JIRA_EMAIL_SETTING: &str = "jira.email";
pub(crate) const JIRA_API_TOKEN_SETTING: &str = "jira.api_token";
pub(crate) const JIRA_DEFAULT_PROJECT_SETTING: &str = "jira.default_project";
pub(crate) const JIRA_DEFAULT_BOARD_SETTING: &str = "jira.default_board";
pub(crate) const JIRA_COMPANY_MANAGED_URLS_SETTING: &str = "jira.company_managed_urls";
pub(crate) const JIRA_STORY_POINTS_FIELD_ID_SETTING: &str = "jira.story_points_field_id";
pub(crate) const JIRA_STORY_POINTS_BOARD_ID_SETTING: &str = "jira.story_points_board_id";
pub(crate) const JIRA_STORY_POINTS_DISCOVERY_COMPLETE_SETTING: &str =
    "jira.story_points_discovery_complete";
pub(crate) const BACKLOG_USE_JIRA_VELOCITY_SETTING: &str = "backlog.use_jira_velocity";
pub(crate) const BACKLOG_JIRA_VELOCITY_SPRINTS_SETTING: &str = "backlog.jira_velocity_sprints";
pub(crate) const BACKLOG_FIXED_SPRINT_CAPACITY_SETTING: &str = "backlog.fixed_sprint_capacity";
pub(crate) const BACKLOG_USE_AVERAGE_TICKET_SIZE_SETTING: &str = "backlog.use_average_ticket_size";
pub(crate) const BACKLOG_FIXED_TICKET_SIZE_SETTING: &str = "backlog.fixed_ticket_size";
pub(crate) const BACKLOG_SPRINT_TOLERANCE_PERCENT_SETTING: &str =
    "backlog.sprint_tolerance_percent";
pub(crate) const BACKLOG_FILTERS_SETTING: &str = "backlog.filters";
pub(crate) const BACKLOG_EXCLUDED_SPRINT_NAME_FRAGMENTS_SETTING: &str =
    "backlog.excluded_sprint_name_fragments";
pub(crate) const SPEED_READER_WPM_SETTING: &str = "reader.wpm";
pub(crate) const SPEED_READER_BLOCK_DELAY_SETTING: &str = "reader.markdown_block_pause_ms";
pub(crate) const RECENT_TICKETS_LIMIT_SETTING: &str = "recent_tickets.limit";
pub(crate) const COMPOSER_ADD_SIBLING_KEY_SETTING: &str = "composer.add_sibling_key";
pub(crate) const COMPOSER_ADD_CHILD_KEY_SETTING: &str = "composer.add_child_key";
pub(crate) const COMPOSER_NEW_CHANGE_SET_KEY_SETTING: &str = "composer.new_change_set_key";
pub(crate) const COMPOSER_CHANGE_SET_FILTER_KEY_SETTING: &str = "composer.change_set_filter_key";
pub(crate) const COMPOSER_COMMIT_KEY_SETTING: &str = "composer.commit_key";
pub(crate) const COMPOSER_REFRESH_KEY_SETTING: &str = "composer.refresh_key";
pub(crate) const COMPOSER_VIEW_KEY_SETTING: &str = "composer.view_key";
pub(crate) const COMPOSER_TITLE_KEY_SETTING: &str = "composer.title_key";
pub(crate) const COMPOSER_DESCRIPTION_TAB_KEY_SETTING: &str = "composer.description_tab_key";
pub(crate) const COMPOSER_PROPERTIES_TAB_KEY_SETTING: &str = "composer.properties_tab_key";
pub(crate) const COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING: &str = "composer.description_focus_key";
pub(crate) const COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING: &str = "composer.description_editor_key";
pub(crate) const COMPOSER_DESCRIPTION_READER_KEY_SETTING: &str = "composer.description_reader_key";
pub(crate) const COMPOSER_DESCRIPTION_INLINE_KEY_SETTING: &str = "composer.description_inline_key";
pub(crate) const COMPOSER_ISSUE_TYPE_KEY_SETTING: &str = "composer.issue_type_key";
pub(crate) const COMPOSER_PARENT_KEY_SETTING: &str = "composer.parent_key";
pub(crate) const COMPOSER_STATUS_KEY_SETTING: &str = "composer.status_key";
pub(crate) const COMPOSER_PRIORITY_KEY_SETTING: &str = "composer.priority_key";
pub(crate) const COMPOSER_ASSIGNEE_KEY_SETTING: &str = "composer.assignee_key";
pub(crate) const COMPOSER_STORY_POINTS_KEY_SETTING: &str = "composer.story_points_key";
pub(crate) const COMPOSER_FIX_VERSIONS_KEY_SETTING: &str = "composer.fix_versions_key";
pub(crate) const COMPOSER_LABELS_KEY_SETTING: &str = "composer.labels_key";
pub(crate) const COMPOSER_WEB_LINKS_KEY_SETTING: &str = "composer.web_links_key";
pub(crate) const COMPOSER_CREATE_SUBMIT_KEY_SETTING: &str = "composer.create_submit_key";
pub(crate) const COMPOSER_CREATE_CONFIRM_KEY_SETTING: &str = "composer.create_confirm_key";
pub(crate) const COMPOSER_DIALOG_CANCEL_KEY_SETTING: &str = "composer.dialog_cancel_key";
pub(crate) const COMPOSER_SUBMIT_CONFIRM_KEY_SETTING: &str = "composer.submit_confirm_key";
pub(crate) const COMPOSER_REPARENT_CONFIRM_KEY_SETTING: &str = "composer.reparent_confirm_key";
pub(crate) const COMPOSER_TICKET_ACTION_KEY_SETTING: &str = "composer.ticket_action_key";
pub(crate) const COMPOSER_RESTORE_RESET_KEY_SETTING: &str = "composer.restore_reset_key";
pub(crate) const COMPOSER_DELETE_KEY_SETTING: &str = "composer.delete_key";
pub(crate) const COMPOSER_REMOVE_KEY_SETTING: &str = "composer.remove_key";
pub(crate) const COMPOSER_RESTORE_KEY_SETTING: &str = "composer.restore_key";
pub(crate) const COMPOSER_RESET_KEY_SETTING: &str = "composer.reset_key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerKeyBinding {
    sequence: String,
    spec: KeySpec,
}

impl ComposerKeyBinding {
    fn parse(value: String, setting: &str) -> Result<Self, String> {
        let sequence = value.trim().to_ascii_lowercase();
        let spec = parse_composer_key(&sequence).ok_or_else(|| {
            format!("setting `{setting}` must be one key or ctrl+/alt+/shift+ plus one key")
        })?;
        Ok(Self { sequence, spec })
    }

    pub(crate) fn matches(&self, key: tuicore::KeyEvent) -> bool {
        self.spec.matches(key)
    }

    pub(crate) fn sequence(&self) -> &str {
        &self.sequence
    }

    pub(crate) fn label(&self) -> String {
        self.spec.label()
    }

    pub(crate) fn spec(&self) -> KeySpec {
        self.spec
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerSequenceBinding {
    sequence: String,
    specs: Vec<KeySpec>,
}

impl ComposerSequenceBinding {
    fn parse(value: String, setting: &str) -> Result<Self, String> {
        let sequence = value.trim().to_ascii_lowercase();
        let specs = sequence
            .chars()
            .map(|key| {
                key.is_ascii_alphanumeric()
                    .then(|| KeySpec::plain(key))
                    .ok_or_else(|| {
                        format!("setting `{setting}` must contain only letters or digits")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if specs.is_empty() {
            return Err(format!("setting `{setting}` must not be empty"));
        }
        Ok(Self { sequence, specs })
    }

    pub(crate) fn sequence(&self) -> &str {
        &self.sequence
    }

    #[cfg(test)]
    pub(crate) fn label(&self) -> String {
        self.specs.iter().map(|spec| spec.label()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerKeyBindings {
    pub(crate) add_sibling: ComposerKeyBinding,
    pub(crate) add_child: ComposerKeyBinding,
    pub(crate) new_change_set: ComposerKeyBinding,
    pub(crate) change_set_filter: ComposerKeyBinding,
    pub(crate) commit: ComposerKeyBinding,
    pub(crate) refresh: ComposerKeyBinding,
    pub(crate) view: ComposerKeyBinding,
    pub(crate) title: ComposerKeyBinding,
    pub(crate) description_tab: ComposerKeyBinding,
    pub(crate) properties_tab: ComposerKeyBinding,
    pub(crate) description_focus: ComposerSequenceBinding,
    pub(crate) description_editor: ComposerSequenceBinding,
    pub(crate) description_reader: ComposerSequenceBinding,
    pub(crate) description_inline: ComposerKeyBinding,
    pub(crate) issue_type: ComposerSequenceBinding,
    pub(crate) parent: ComposerSequenceBinding,
    pub(crate) status: ComposerSequenceBinding,
    pub(crate) priority: ComposerSequenceBinding,
    pub(crate) assignee: ComposerSequenceBinding,
    pub(crate) story_points: ComposerSequenceBinding,
    pub(crate) fix_versions: ComposerSequenceBinding,
    pub(crate) labels: ComposerSequenceBinding,
    pub(crate) web_links: ComposerSequenceBinding,
    pub(crate) create_submit: ComposerKeyBinding,
    pub(crate) create_confirm: ComposerKeyBinding,
    pub(crate) dialog_cancel: ComposerKeyBinding,
    pub(crate) submit_confirm: ComposerKeyBinding,
    pub(crate) reparent_confirm: ComposerKeyBinding,
    pub(crate) ticket_action: ComposerKeyBinding,
    pub(crate) restore_reset: ComposerKeyBinding,
    pub(crate) delete: ComposerKeyBinding,
    pub(crate) remove: ComposerKeyBinding,
    pub(crate) restore: ComposerKeyBinding,
    pub(crate) reset: ComposerKeyBinding,
}

impl Default for ComposerKeyBindings {
    fn default() -> Self {
        Self::from_values(&HashMap::new()).expect("built-in Composer keys must be valid")
    }
}

impl ComposerKeyBindings {
    fn from_values(values: &HashMap<String, String>) -> Result<Self, String> {
        let binding = |setting: &str, default: &str| {
            ComposerKeyBinding::parse(
                values
                    .get(setting)
                    .cloned()
                    .unwrap_or_else(|| default.into()),
                setting,
            )
        };
        let sequence_binding = |setting: &str, default: &str| {
            ComposerSequenceBinding::parse(
                values
                    .get(setting)
                    .cloned()
                    .unwrap_or_else(|| default.into()),
                setting,
            )
        };
        let bindings = Self {
            add_sibling: binding(COMPOSER_ADD_SIBLING_KEY_SETTING, "shift+a")?,
            add_child: binding(COMPOSER_ADD_CHILD_KEY_SETTING, "shift+c")?,
            new_change_set: binding(COMPOSER_NEW_CHANGE_SET_KEY_SETTING, "shift+n")?,
            change_set_filter: binding(COMPOSER_CHANGE_SET_FILTER_KEY_SETTING, "shift+f")?,
            commit: binding(COMPOSER_COMMIT_KEY_SETTING, "shift+m")?,
            refresh: binding(COMPOSER_REFRESH_KEY_SETTING, "shift+r")?,
            view: binding(COMPOSER_VIEW_KEY_SETTING, "shift+v")?,
            title: binding(COMPOSER_TITLE_KEY_SETTING, "shift+t")?,
            description_tab: binding(COMPOSER_DESCRIPTION_TAB_KEY_SETTING, "shift+d")?,
            properties_tab: binding(COMPOSER_PROPERTIES_TAB_KEY_SETTING, "shift+p")?,
            description_focus: sequence_binding(COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING, "dd")?,
            description_editor: sequence_binding(COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING, "do")?,
            description_reader: sequence_binding(COMPOSER_DESCRIPTION_READER_KEY_SETTING, "ds")?,
            description_inline: binding(COMPOSER_DESCRIPTION_INLINE_KEY_SETTING, "shift+i")?,
            issue_type: sequence_binding(COMPOSER_ISSUE_TYPE_KEY_SETTING, "it")?,
            parent: sequence_binding(COMPOSER_PARENT_KEY_SETTING, "pa")?,
            status: sequence_binding(COMPOSER_STATUS_KEY_SETTING, "st")?,
            priority: sequence_binding(COMPOSER_PRIORITY_KEY_SETTING, "pr")?,
            assignee: sequence_binding(COMPOSER_ASSIGNEE_KEY_SETTING, "ee")?,
            story_points: sequence_binding(COMPOSER_STORY_POINTS_KEY_SETTING, "sp")?,
            fix_versions: sequence_binding(COMPOSER_FIX_VERSIONS_KEY_SETTING, "fv")?,
            labels: sequence_binding(COMPOSER_LABELS_KEY_SETTING, "be")?,
            web_links: sequence_binding(COMPOSER_WEB_LINKS_KEY_SETTING, "uu")?,
            create_submit: binding(COMPOSER_CREATE_SUBMIT_KEY_SETTING, "ctrl+enter")?,
            create_confirm: binding(COMPOSER_CREATE_CONFIRM_KEY_SETTING, "o")?,
            dialog_cancel: binding(COMPOSER_DIALOG_CANCEL_KEY_SETTING, "c")?,
            submit_confirm: binding(COMPOSER_SUBMIT_CONFIRM_KEY_SETTING, "m")?,
            reparent_confirm: binding(COMPOSER_REPARENT_CONFIRM_KEY_SETTING, "m")?,
            ticket_action: binding(COMPOSER_TICKET_ACTION_KEY_SETTING, "ctrl+x")?,
            restore_reset: binding(COMPOSER_RESTORE_RESET_KEY_SETTING, "ctrl+r")?,
            delete: binding(COMPOSER_DELETE_KEY_SETTING, "d")?,
            remove: binding(COMPOSER_REMOVE_KEY_SETTING, "r")?,
            restore: binding(COMPOSER_RESTORE_KEY_SETTING, "r")?,
            reset: binding(COMPOSER_RESET_KEY_SETTING, "s")?,
        };
        ensure_unambiguous(&[
            bindings.add_sibling.sequence(),
            bindings.add_child.sequence(),
            bindings.new_change_set.sequence(),
            bindings.change_set_filter.sequence(),
            bindings.commit.sequence(),
            bindings.refresh.sequence(),
            bindings.view.sequence(),
            bindings.title.sequence(),
            bindings.description_tab.sequence(),
            bindings.properties_tab.sequence(),
            bindings.ticket_action.sequence(),
            bindings.restore_reset.sequence(),
            bindings.description_focus.sequence(),
            bindings.description_editor.sequence(),
            bindings.description_reader.sequence(),
            bindings.description_inline.sequence(),
            bindings.issue_type.sequence(),
            bindings.parent.sequence(),
            bindings.status.sequence(),
            bindings.priority.sequence(),
            bindings.assignee.sequence(),
            bindings.story_points.sequence(),
            bindings.fix_versions.sequence(),
            bindings.labels.sequence(),
            bindings.web_links.sequence(),
        ])?;
        ensure_unambiguous(&[
            bindings.create_submit.sequence(),
            bindings.create_confirm.sequence(),
            bindings.dialog_cancel.sequence(),
        ])?;
        ensure_unambiguous(&[
            bindings.submit_confirm.sequence(),
            bindings.dialog_cancel.sequence(),
        ])?;
        ensure_unambiguous(&[
            bindings.reparent_confirm.sequence(),
            bindings.dialog_cancel.sequence(),
        ])?;
        ensure_unambiguous(&[
            bindings.delete.sequence(),
            bindings.remove.sequence(),
            bindings.dialog_cancel.sequence(),
        ])?;
        ensure_unambiguous(&[
            bindings.restore.sequence(),
            bindings.reset.sequence(),
            bindings.dialog_cancel.sequence(),
        ])?;
        Ok(bindings)
    }
}

fn ensure_unambiguous(sequences: &[&str]) -> Result<(), String> {
    sequences
        .iter()
        .enumerate()
        .all(|(index, sequence)| {
            sequences
                .iter()
                .skip(index + 1)
                .all(|candidate| !sequences_conflict(sequence, candidate))
        })
        .then_some(())
        .ok_or_else(|| {
            "Composer action keys must be unique and non-prefix within their active context".into()
        })
}

fn sequences_conflict(sequence: &str, candidate: &str) -> bool {
    match (parse_composer_key(sequence), parse_composer_key(candidate)) {
        (Some(sequence), Some(candidate)) => sequence == candidate,
        _ => sequence.starts_with(candidate) || candidate.starts_with(sequence),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AppSettings {
    pub(crate) jira_base_url: String,
    pub(crate) jira_email: String,
    pub(crate) jira_api_token: String,
    pub(crate) jira_default_project: String,
    pub(crate) jira_default_board: String,
    pub(crate) jira_company_managed_urls: bool,
    pub(crate) jira_story_points_field_id: String,
    pub(crate) jira_story_points_board_id: String,
    pub(crate) jira_story_points_discovery_complete: bool,
    pub(crate) backlog_runway: BacklogRunwaySettings,
    pub(crate) backlog_filters: BacklogFilterSettings,
    pub(crate) excluded_sprint_name_fragments: Vec<String>,
    pub(crate) speed_reader: SpeedReaderSettings,
    pub(crate) recent_tickets_limit: usize,
    pub(crate) composer_keys: ComposerKeyBindings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            jira_base_url: String::new(),
            jira_email: String::new(),
            jira_api_token: String::new(),
            jira_default_project: String::new(),
            jira_default_board: String::new(),
            jira_company_managed_urls: false,
            jira_story_points_field_id: String::new(),
            jira_story_points_board_id: String::new(),
            jira_story_points_discovery_complete: false,
            backlog_runway: BacklogRunwaySettings::default(),
            backlog_filters: BacklogFilterSettings::default(),
            excluded_sprint_name_fragments: Vec::new(),
            speed_reader: SpeedReaderSettings::default(),
            recent_tickets_limit: 15,
            composer_keys: ComposerKeyBindings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BacklogRunwaySettings {
    pub(crate) use_jira_velocity: bool,
    pub(crate) jira_velocity_sprints: usize,
    pub(crate) fixed_sprint_capacity: f64,
    pub(crate) use_average_ticket_size: bool,
    pub(crate) fixed_ticket_size: f64,
    pub(crate) sprint_tolerance_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BacklogFilter {
    Done,
    Open,
    Pointed,
    Unpointed,
}

impl BacklogFilter {
    pub(crate) const ALL: [Self; 4] = [Self::Done, Self::Open, Self::Pointed, Self::Unpointed];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::Open => "Open",
            Self::Pointed => "Pointed",
            Self::Unpointed => "Unpointed",
        }
    }

    fn setting_value(self) -> &'static str {
        match self {
            Self::Done => "hide_done",
            Self::Open => "hide_not_done",
            Self::Pointed => "hide_estimated",
            Self::Unpointed => "hide_unestimated",
        }
    }

    fn from_setting_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|filter| filter.setting_value() == value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BacklogFilterSettings {
    selected: Vec<BacklogFilter>,
}

impl BacklogFilterSettings {
    pub(crate) fn selected(&self) -> &[BacklogFilter] {
        &self.selected
    }

    pub(crate) fn set_selected(&mut self, selected: Vec<BacklogFilter>) {
        self.selected = BacklogFilter::ALL
            .into_iter()
            .filter(|filter| selected.contains(filter))
            .collect();
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.selected.is_empty()
    }

    fn from_setting_value(value: Option<&String>) -> Self {
        let selected = value
            .into_iter()
            .flat_map(|value| value.split(','))
            .filter_map(|value| BacklogFilter::from_setting_value(value.trim()))
            .collect();
        let mut settings = Self::default();
        settings.set_selected(selected);
        settings
    }

    fn setting_value(&self) -> String {
        self.selected
            .iter()
            .map(|filter| filter.setting_value())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Default for BacklogRunwaySettings {
    fn default() -> Self {
        Self {
            use_jira_velocity: false,
            jira_velocity_sprints: 4,
            fixed_sprint_capacity: 20.0,
            use_average_ticket_size: false,
            fixed_ticket_size: 3.0,
            sprint_tolerance_percent: 20,
        }
    }
}

impl AppSettings {
    pub(crate) fn resolve(values: &HashMap<String, String>) -> Result<Self, String> {
        let defaults = Self::default();
        let value_or_env = |key: &str, variable: &str| {
            env::var(variable)
                .ok()
                .or_else(|| values.get(key).cloned())
                .unwrap_or_default()
        };
        Ok(Self {
            jira_base_url: value_or_env(JIRA_BASE_URL_SETTING, "JIRA_BASE_URL"),
            jira_email: value_or_env(JIRA_EMAIL_SETTING, "JIRA_EMAIL"),
            jira_api_token: value_or_env(JIRA_API_TOKEN_SETTING, "JIRA_API_TOKEN"),
            jira_default_project: value_or_env(
                JIRA_DEFAULT_PROJECT_SETTING,
                "JIRA_DEFAULT_PROJECT",
            ),
            jira_default_board: value_or_env(JIRA_DEFAULT_BOARD_SETTING, "JIRA_DEFAULT_BOARD"),
            jira_company_managed_urls: values
                .get(JIRA_COMPANY_MANAGED_URLS_SETTING)
                .is_some_and(|value| value == "true"),
            jira_story_points_field_id: values
                .get(JIRA_STORY_POINTS_FIELD_ID_SETTING)
                .cloned()
                .unwrap_or_default(),
            jira_story_points_board_id: values
                .get(JIRA_STORY_POINTS_BOARD_ID_SETTING)
                .cloned()
                .unwrap_or_default(),
            jira_story_points_discovery_complete: values
                .get(JIRA_STORY_POINTS_DISCOVERY_COMPLETE_SETTING)
                .is_some_and(|value| value == "true"),
            backlog_runway: BacklogRunwaySettings {
                use_jira_velocity: values
                    .get(BACKLOG_USE_JIRA_VELOCITY_SETTING)
                    .is_some_and(|value| value == "true"),
                jira_velocity_sprints: values
                    .get(BACKLOG_JIRA_VELOCITY_SPRINTS_SETTING)
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(defaults.backlog_runway.jira_velocity_sprints),
                fixed_sprint_capacity: setting_number(
                    values,
                    BACKLOG_FIXED_SPRINT_CAPACITY_SETTING,
                    defaults.backlog_runway.fixed_sprint_capacity,
                    false,
                ),
                use_average_ticket_size: values
                    .get(BACKLOG_USE_AVERAGE_TICKET_SIZE_SETTING)
                    .is_some_and(|value| value == "true"),
                fixed_ticket_size: setting_number(
                    values,
                    BACKLOG_FIXED_TICKET_SIZE_SETTING,
                    defaults.backlog_runway.fixed_ticket_size,
                    true,
                ),
                sprint_tolerance_percent: values
                    .get(BACKLOG_SPRINT_TOLERANCE_PERCENT_SETTING)
                    .and_then(|value| value.parse::<u8>().ok())
                    .filter(|value| *value <= 100)
                    .unwrap_or(defaults.backlog_runway.sprint_tolerance_percent),
            },
            backlog_filters: BacklogFilterSettings::from_setting_value(
                values.get(BACKLOG_FILTERS_SETTING),
            ),
            excluded_sprint_name_fragments: sprint_name_fragments(
                values.get(BACKLOG_EXCLUDED_SPRINT_NAME_FRAGMENTS_SETTING),
            ),
            speed_reader: SpeedReaderSettings {
                wpm: values
                    .get(SPEED_READER_WPM_SETTING)
                    .and_then(|value| parse_speed_reader_wpm(value))
                    .unwrap_or(defaults.speed_reader.wpm),
                markdown_block_pause: values
                    .get(SPEED_READER_BLOCK_DELAY_SETTING)
                    .and_then(|value| parse_markdown_block_pause(value))
                    .unwrap_or(defaults.speed_reader.markdown_block_pause),
            },
            recent_tickets_limit: values
                .get(RECENT_TICKETS_LIMIT_SETTING)
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (1..=100).contains(value))
                .unwrap_or(15),
            composer_keys: ComposerKeyBindings::from_values(values)?,
        })
    }

    pub(crate) fn values(&self) -> Vec<(&'static str, String)> {
        vec![
            (JIRA_BASE_URL_SETTING, self.jira_base_url.clone()),
            (JIRA_EMAIL_SETTING, self.jira_email.clone()),
            (JIRA_API_TOKEN_SETTING, self.jira_api_token.clone()),
            (
                JIRA_DEFAULT_PROJECT_SETTING,
                self.jira_default_project.clone(),
            ),
            (JIRA_DEFAULT_BOARD_SETTING, self.jira_default_board.clone()),
            (
                JIRA_COMPANY_MANAGED_URLS_SETTING,
                self.jira_company_managed_urls.to_string(),
            ),
            (
                JIRA_STORY_POINTS_FIELD_ID_SETTING,
                self.jira_story_points_field_id.clone(),
            ),
            (
                JIRA_STORY_POINTS_BOARD_ID_SETTING,
                self.jira_story_points_board_id.clone(),
            ),
            (
                JIRA_STORY_POINTS_DISCOVERY_COMPLETE_SETTING,
                self.jira_story_points_discovery_complete.to_string(),
            ),
            (
                BACKLOG_USE_JIRA_VELOCITY_SETTING,
                self.backlog_runway.use_jira_velocity.to_string(),
            ),
            (
                BACKLOG_JIRA_VELOCITY_SPRINTS_SETTING,
                self.backlog_runway.jira_velocity_sprints.to_string(),
            ),
            (
                BACKLOG_FIXED_SPRINT_CAPACITY_SETTING,
                self.backlog_runway.fixed_sprint_capacity.to_string(),
            ),
            (
                BACKLOG_USE_AVERAGE_TICKET_SIZE_SETTING,
                self.backlog_runway.use_average_ticket_size.to_string(),
            ),
            (
                BACKLOG_FIXED_TICKET_SIZE_SETTING,
                self.backlog_runway.fixed_ticket_size.to_string(),
            ),
            (
                BACKLOG_SPRINT_TOLERANCE_PERCENT_SETTING,
                self.backlog_runway.sprint_tolerance_percent.to_string(),
            ),
            (
                BACKLOG_FILTERS_SETTING,
                self.backlog_filters.setting_value(),
            ),
            (
                BACKLOG_EXCLUDED_SPRINT_NAME_FRAGMENTS_SETTING,
                self.excluded_sprint_name_fragments.join(","),
            ),
            (SPEED_READER_WPM_SETTING, self.speed_reader.wpm.to_string()),
            (
                SPEED_READER_BLOCK_DELAY_SETTING,
                self.speed_reader
                    .markdown_block_pause
                    .as_millis()
                    .to_string(),
            ),
            (
                RECENT_TICKETS_LIMIT_SETTING,
                self.recent_tickets_limit.to_string(),
            ),
            (
                COMPOSER_ADD_SIBLING_KEY_SETTING,
                self.composer_keys.add_sibling.sequence.clone(),
            ),
            (
                COMPOSER_ADD_CHILD_KEY_SETTING,
                self.composer_keys.add_child.sequence.clone(),
            ),
            (
                COMPOSER_NEW_CHANGE_SET_KEY_SETTING,
                self.composer_keys.new_change_set.sequence.clone(),
            ),
            (
                COMPOSER_CHANGE_SET_FILTER_KEY_SETTING,
                self.composer_keys.change_set_filter.sequence.clone(),
            ),
            (
                COMPOSER_COMMIT_KEY_SETTING,
                self.composer_keys.commit.sequence.clone(),
            ),
            (
                COMPOSER_REFRESH_KEY_SETTING,
                self.composer_keys.refresh.sequence.clone(),
            ),
            (
                COMPOSER_VIEW_KEY_SETTING,
                self.composer_keys.view.sequence.clone(),
            ),
            (
                COMPOSER_TITLE_KEY_SETTING,
                self.composer_keys.title.sequence.clone(),
            ),
            (
                COMPOSER_DESCRIPTION_TAB_KEY_SETTING,
                self.composer_keys.description_tab.sequence.clone(),
            ),
            (
                COMPOSER_PROPERTIES_TAB_KEY_SETTING,
                self.composer_keys.properties_tab.sequence.clone(),
            ),
            (
                COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING,
                self.composer_keys.description_focus.sequence.clone(),
            ),
            (
                COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING,
                self.composer_keys.description_editor.sequence.clone(),
            ),
            (
                COMPOSER_DESCRIPTION_READER_KEY_SETTING,
                self.composer_keys.description_reader.sequence.clone(),
            ),
            (
                COMPOSER_DESCRIPTION_INLINE_KEY_SETTING,
                self.composer_keys.description_inline.sequence.clone(),
            ),
            (
                COMPOSER_ISSUE_TYPE_KEY_SETTING,
                self.composer_keys.issue_type.sequence.clone(),
            ),
            (
                COMPOSER_PARENT_KEY_SETTING,
                self.composer_keys.parent.sequence.clone(),
            ),
            (
                COMPOSER_STATUS_KEY_SETTING,
                self.composer_keys.status.sequence.clone(),
            ),
            (
                COMPOSER_PRIORITY_KEY_SETTING,
                self.composer_keys.priority.sequence.clone(),
            ),
            (
                COMPOSER_ASSIGNEE_KEY_SETTING,
                self.composer_keys.assignee.sequence.clone(),
            ),
            (
                COMPOSER_STORY_POINTS_KEY_SETTING,
                self.composer_keys.story_points.sequence.clone(),
            ),
            (
                COMPOSER_FIX_VERSIONS_KEY_SETTING,
                self.composer_keys.fix_versions.sequence.clone(),
            ),
            (
                COMPOSER_LABELS_KEY_SETTING,
                self.composer_keys.labels.sequence.clone(),
            ),
            (
                COMPOSER_WEB_LINKS_KEY_SETTING,
                self.composer_keys.web_links.sequence.clone(),
            ),
            (
                COMPOSER_CREATE_SUBMIT_KEY_SETTING,
                self.composer_keys.create_submit.sequence.clone(),
            ),
            (
                COMPOSER_CREATE_CONFIRM_KEY_SETTING,
                self.composer_keys.create_confirm.sequence.clone(),
            ),
            (
                COMPOSER_DIALOG_CANCEL_KEY_SETTING,
                self.composer_keys.dialog_cancel.sequence.clone(),
            ),
            (
                COMPOSER_SUBMIT_CONFIRM_KEY_SETTING,
                self.composer_keys.submit_confirm.sequence.clone(),
            ),
            (
                COMPOSER_REPARENT_CONFIRM_KEY_SETTING,
                self.composer_keys.reparent_confirm.sequence.clone(),
            ),
            (
                COMPOSER_TICKET_ACTION_KEY_SETTING,
                self.composer_keys.ticket_action.sequence.clone(),
            ),
            (
                COMPOSER_RESTORE_RESET_KEY_SETTING,
                self.composer_keys.restore_reset.sequence.clone(),
            ),
            (
                COMPOSER_DELETE_KEY_SETTING,
                self.composer_keys.delete.sequence.clone(),
            ),
            (
                COMPOSER_REMOVE_KEY_SETTING,
                self.composer_keys.remove.sequence.clone(),
            ),
            (
                COMPOSER_RESTORE_KEY_SETTING,
                self.composer_keys.restore.sequence.clone(),
            ),
            (
                COMPOSER_RESET_KEY_SETTING,
                self.composer_keys.reset.sequence.clone(),
            ),
        ]
    }

    pub(crate) fn changed_values(&self, previous: &Self) -> Vec<(&'static str, String)> {
        self.values()
            .into_iter()
            .zip(previous.values())
            .filter_map(|(current, old)| (current.1 != old.1).then_some(current))
            .collect()
    }

    pub(crate) fn set_manual_story_points_field(&mut self, field_id: String) {
        self.jira_story_points_field_id = field_id;
        self.jira_story_points_board_id.clear();
        self.jira_story_points_discovery_complete = false;
    }

    pub(crate) fn invalidate_discovered_story_points(&mut self) {
        if self.jira_story_points_board_id.is_empty() {
            return;
        }
        self.jira_story_points_field_id.clear();
        self.jira_story_points_board_id.clear();
        self.jira_story_points_discovery_complete = false;
    }

    pub(crate) fn story_points_field_is_manual(&self) -> bool {
        !self.jira_story_points_field_id.trim().is_empty()
            && self.jira_story_points_board_id.trim().is_empty()
    }

    pub(crate) fn configured_jira(&self) -> Option<(&str, &str, &str)> {
        let base_url = self.jira_base_url.trim();
        let email = self.jira_email.trim();
        let token = self.jira_api_token.trim();
        (!base_url.is_empty() && !email.is_empty() && !token.is_empty()).then_some((
            base_url.trim_end_matches('/'),
            email,
            token,
        ))
    }

    pub(crate) fn excludes_sprint(&self, sprint_name: &str) -> bool {
        let sprint_name = sprint_name.to_ascii_lowercase();
        self.excluded_sprint_name_fragments
            .iter()
            .any(|fragment| sprint_name.contains(&fragment.to_ascii_lowercase()))
    }

    pub(crate) fn jira_issue_url(&self, key: &str) -> Option<String> {
        let base_url = self.jira_base_url.trim().trim_end_matches('/');
        (!base_url.is_empty() && !key.trim().is_empty()).then(|| format!("{base_url}/browse/{key}"))
    }

    pub(crate) fn jira_board_url(&self, page: Option<&str>) -> Option<String> {
        let base_url = self.jira_base_url.trim().trim_end_matches('/');
        let project = self.jira_default_project.trim();
        let board = self.jira_default_board.trim();
        (!base_url.is_empty() && !project.is_empty() && !board.is_empty()).then(|| {
            let route = if self.jira_company_managed_urls {
                "c/projects"
            } else {
                "projects"
            };
            let url = format!("{base_url}/jira/software/{route}/{project}/boards/{board}");
            page.filter(|page| !page.is_empty())
                .map(|page| format!("{url}/{page}"))
                .unwrap_or(url)
        })
    }

    pub(crate) fn jira_releases_url(&self) -> Option<String> {
        let base_url = self.jira_base_url.trim().trim_end_matches('/');
        let project = self.jira_default_project.trim();
        (!base_url.is_empty() && !project.is_empty()).then(|| {
            format!(
                "{base_url}/projects/{project}?selectedItem=com.atlassian.jira.jira-projects-plugin%3Arelease-page"
            )
        })
    }

    pub(crate) fn block_delay(&self) -> Duration {
        self.speed_reader.markdown_block_pause
    }
}

fn setting_number(
    values: &HashMap<String, String>,
    key: &str,
    default: f64,
    permits_zero: bool,
) -> f64 {
    values
        .get(key)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (permits_zero || *value > 0.0) && *value >= 0.0)
        .unwrap_or(default)
}

fn sprint_name_fragments(value: Option<&String>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|fragment| !fragment.is_empty())
        .fold(Vec::new(), |mut fragments, fragment| {
            if !fragments
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(fragment))
            {
                fragments.push(fragment.to_owned());
            }
            fragments
        })
}

fn parse_composer_key(value: &str) -> Option<KeySpec> {
    let (modifier, key) = if let Some(key) = value.strip_prefix("ctrl+") {
        (KeyModifiers::CONTROL, key)
    } else if let Some(key) = value.strip_prefix("alt+") {
        (KeyModifiers::ALT, key)
    } else if let Some(key) = value.strip_prefix("shift+") {
        (KeyModifiers::SHIFT, key)
    } else {
        (KeyModifiers::NONE, value)
    };
    let code = match key {
        "enter" => Key::Enter,
        key if key.chars().count() == 1 => Key::Char(key.chars().next()?),
        _ => return None,
    };
    Some(KeySpec::key_with_modifiers(code, modifier))
}

#[cfg(test)]
mod tests;
