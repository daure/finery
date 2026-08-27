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
pub(crate) const JIRA_STORY_POINTS_FIELD_ID_SETTING: &str = "jira.story_points_field_id";
pub(crate) const JIRA_STORY_POINTS_BOARD_ID_SETTING: &str = "jira.story_points_board_id";
pub(crate) const SPEED_READER_WPM_SETTING: &str = "reader.wpm";
pub(crate) const SPEED_READER_BLOCK_DELAY_SETTING: &str = "reader.markdown_block_pause_ms";
pub(crate) const COMPOSER_ADD_SIBLING_KEY_SETTING: &str = "composer.add_sibling_key";
pub(crate) const COMPOSER_ADD_CHILD_KEY_SETTING: &str = "composer.add_child_key";
pub(crate) const COMPOSER_COMMIT_KEY_SETTING: &str = "composer.commit_key";
pub(crate) const COMPOSER_REFRESH_KEY_SETTING: &str = "composer.refresh_key";
pub(crate) const COMPOSER_VIEW_KEY_SETTING: &str = "composer.view_key";
pub(crate) const COMPOSER_TITLE_KEY_SETTING: &str = "composer.title_key";
pub(crate) const COMPOSER_DESCRIPTION_TAB_KEY_SETTING: &str = "composer.description_tab_key";
pub(crate) const COMPOSER_PROPERTIES_TAB_KEY_SETTING: &str = "composer.properties_tab_key";
pub(crate) const COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING: &str = "composer.description_focus_key";
pub(crate) const COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING: &str = "composer.description_editor_key";
pub(crate) const COMPOSER_DESCRIPTION_READER_KEY_SETTING: &str = "composer.description_reader_key";
pub(crate) const COMPOSER_ISSUE_TYPE_KEY_SETTING: &str = "composer.issue_type_key";
pub(crate) const COMPOSER_PARENT_KEY_SETTING: &str = "composer.parent_key";
pub(crate) const COMPOSER_STATUS_KEY_SETTING: &str = "composer.status_key";
pub(crate) const COMPOSER_PRIORITY_KEY_SETTING: &str = "composer.priority_key";
pub(crate) const COMPOSER_ASSIGNEE_KEY_SETTING: &str = "composer.assignee_key";
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

pub(crate) const COMPOSER_BINDING_SETTINGS: [(&str, &str); 27] = [
    (COMPOSER_ADD_SIBLING_KEY_SETTING, "Add sibling"),
    (COMPOSER_ADD_CHILD_KEY_SETTING, "Add child"),
    (COMPOSER_COMMIT_KEY_SETTING, "Commit"),
    (COMPOSER_REFRESH_KEY_SETTING, "Refresh"),
    (COMPOSER_VIEW_KEY_SETTING, "View"),
    (COMPOSER_TITLE_KEY_SETTING, "Title"),
    (COMPOSER_DESCRIPTION_TAB_KEY_SETTING, "Description tab"),
    (COMPOSER_PROPERTIES_TAB_KEY_SETTING, "Properties tab"),
    (COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING, "Description focus"),
    (
        COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING,
        "Description editor",
    ),
    (
        COMPOSER_DESCRIPTION_READER_KEY_SETTING,
        "Description reader",
    ),
    (COMPOSER_ISSUE_TYPE_KEY_SETTING, "Issue type"),
    (COMPOSER_PARENT_KEY_SETTING, "Parent"),
    (COMPOSER_STATUS_KEY_SETTING, "Status"),
    (COMPOSER_PRIORITY_KEY_SETTING, "Priority"),
    (COMPOSER_ASSIGNEE_KEY_SETTING, "Assignee"),
    (COMPOSER_CREATE_SUBMIT_KEY_SETTING, "Create submit"),
    (COMPOSER_CREATE_CONFIRM_KEY_SETTING, "Create confirm"),
    (COMPOSER_DIALOG_CANCEL_KEY_SETTING, "Dialog cancel"),
    (COMPOSER_SUBMIT_CONFIRM_KEY_SETTING, "Submit confirm"),
    (COMPOSER_REPARENT_CONFIRM_KEY_SETTING, "Reparent confirm"),
    (COMPOSER_TICKET_ACTION_KEY_SETTING, "Ticket action"),
    (COMPOSER_RESTORE_RESET_KEY_SETTING, "Restore or reset"),
    (COMPOSER_DELETE_KEY_SETTING, "Delete"),
    (COMPOSER_REMOVE_KEY_SETTING, "Remove"),
    (COMPOSER_RESTORE_KEY_SETTING, "Restore"),
    (COMPOSER_RESET_KEY_SETTING, "Reset"),
];

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
    pub(crate) commit: ComposerKeyBinding,
    pub(crate) refresh: ComposerKeyBinding,
    pub(crate) view: ComposerKeyBinding,
    pub(crate) title: ComposerKeyBinding,
    pub(crate) description_tab: ComposerKeyBinding,
    pub(crate) properties_tab: ComposerKeyBinding,
    pub(crate) description_focus: ComposerSequenceBinding,
    pub(crate) description_editor: ComposerSequenceBinding,
    pub(crate) description_reader: ComposerSequenceBinding,
    pub(crate) issue_type: ComposerSequenceBinding,
    pub(crate) parent: ComposerSequenceBinding,
    pub(crate) status: ComposerSequenceBinding,
    pub(crate) priority: ComposerSequenceBinding,
    pub(crate) assignee: ComposerSequenceBinding,
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
            add_sibling: binding(COMPOSER_ADD_SIBLING_KEY_SETTING, "shift+s")?,
            add_child: binding(COMPOSER_ADD_CHILD_KEY_SETTING, "shift+c")?,
            commit: binding(COMPOSER_COMMIT_KEY_SETTING, "shift+m")?,
            refresh: binding(COMPOSER_REFRESH_KEY_SETTING, "shift+r")?,
            view: binding(COMPOSER_VIEW_KEY_SETTING, "shift+v")?,
            title: binding(COMPOSER_TITLE_KEY_SETTING, "shift+t")?,
            description_tab: binding(COMPOSER_DESCRIPTION_TAB_KEY_SETTING, "shift+d")?,
            properties_tab: binding(COMPOSER_PROPERTIES_TAB_KEY_SETTING, "shift+p")?,
            description_focus: sequence_binding(COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING, "dd")?,
            description_editor: sequence_binding(COMPOSER_DESCRIPTION_EDITOR_KEY_SETTING, "do")?,
            description_reader: sequence_binding(COMPOSER_DESCRIPTION_READER_KEY_SETTING, "ds")?,
            issue_type: sequence_binding(COMPOSER_ISSUE_TYPE_KEY_SETTING, "it")?,
            parent: sequence_binding(COMPOSER_PARENT_KEY_SETTING, "pa")?,
            status: sequence_binding(COMPOSER_STATUS_KEY_SETTING, "st")?,
            priority: sequence_binding(COMPOSER_PRIORITY_KEY_SETTING, "pr")?,
            assignee: sequence_binding(COMPOSER_ASSIGNEE_KEY_SETTING, "ee")?,
            create_submit: binding(COMPOSER_CREATE_SUBMIT_KEY_SETTING, "ctrl+enter")?,
            create_confirm: binding(COMPOSER_CREATE_CONFIRM_KEY_SETTING, "o")?,
            dialog_cancel: binding(COMPOSER_DIALOG_CANCEL_KEY_SETTING, "c")?,
            submit_confirm: binding(COMPOSER_SUBMIT_CONFIRM_KEY_SETTING, "s")?,
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
            bindings.issue_type.sequence(),
            bindings.parent.sequence(),
            bindings.status.sequence(),
            bindings.priority.sequence(),
            bindings.assignee.sequence(),
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppSettings {
    pub(crate) jira_base_url: String,
    pub(crate) jira_email: String,
    pub(crate) jira_api_token: String,
    pub(crate) jira_default_project: String,
    pub(crate) jira_default_board: String,
    pub(crate) jira_story_points_field_id: String,
    pub(crate) jira_story_points_board_id: String,
    pub(crate) speed_reader: SpeedReaderSettings,
    pub(crate) composer_keys: ComposerKeyBindings,
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
            jira_story_points_field_id: values
                .get(JIRA_STORY_POINTS_FIELD_ID_SETTING)
                .cloned()
                .unwrap_or_default(),
            jira_story_points_board_id: values
                .get(JIRA_STORY_POINTS_BOARD_ID_SETTING)
                .cloned()
                .unwrap_or_default(),
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
                JIRA_STORY_POINTS_FIELD_ID_SETTING,
                self.jira_story_points_field_id.clone(),
            ),
            (
                JIRA_STORY_POINTS_BOARD_ID_SETTING,
                self.jira_story_points_board_id.clone(),
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
                COMPOSER_ADD_SIBLING_KEY_SETTING,
                self.composer_keys.add_sibling.sequence.clone(),
            ),
            (
                COMPOSER_ADD_CHILD_KEY_SETTING,
                self.composer_keys.add_child.sequence.clone(),
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

    pub(crate) fn composer_binding_value(&self, setting: &str) -> String {
        self.values()
            .into_iter()
            .find_map(|(key, value)| (key == setting).then_some(value))
            .unwrap_or_default()
    }

    pub(crate) fn update_composer_binding(
        &mut self,
        setting: &str,
        value: String,
    ) -> Result<(), String> {
        if !COMPOSER_BINDING_SETTINGS
            .iter()
            .any(|(candidate, _)| *candidate == setting)
        {
            return Err(format!("Unknown Composer binding setting `{setting}`"));
        }
        let mut values = self
            .values()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<HashMap<_, _>>();
        values.insert(setting.to_owned(), value);
        self.composer_keys = ComposerKeyBindings::from_values(&values)?;
        Ok(())
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

    pub(crate) fn block_delay(&self) -> Duration {
        self.speed_reader.markdown_block_pause
    }
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
