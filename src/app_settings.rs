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
pub(crate) const SPEED_READER_WPM_SETTING: &str = "reader.wpm";
pub(crate) const SPEED_READER_BLOCK_DELAY_SETTING: &str = "reader.markdown_block_pause_ms";
pub(crate) const COMPOSER_ADD_SIBLING_KEY_SETTING: &str = "composer.add_sibling_key";
pub(crate) const COMPOSER_ADD_CHILD_KEY_SETTING: &str = "composer.add_child_key";
pub(crate) const COMPOSER_COMMIT_KEY_SETTING: &str = "composer.commit_key";
pub(crate) const COMPOSER_REFRESH_KEY_SETTING: &str = "composer.refresh_key";
pub(crate) const COMPOSER_VIEW_KEY_SETTING: &str = "composer.view_key";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerKeyBindings {
    pub(crate) add_sibling: ComposerKeyBinding,
    pub(crate) add_child: ComposerKeyBinding,
    pub(crate) commit: ComposerKeyBinding,
    pub(crate) refresh: ComposerKeyBinding,
    pub(crate) view: ComposerKeyBinding,
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
        let bindings = Self {
            add_sibling: binding(COMPOSER_ADD_SIBLING_KEY_SETTING, "shift+s")?,
            add_child: binding(COMPOSER_ADD_CHILD_KEY_SETTING, "shift+c")?,
            commit: binding(COMPOSER_COMMIT_KEY_SETTING, "shift+m")?,
            refresh: binding(COMPOSER_REFRESH_KEY_SETTING, "shift+r")?,
            view: binding(COMPOSER_VIEW_KEY_SETTING, "shift+v")?,
        };
        let sequences = [
            bindings.add_sibling.sequence(),
            bindings.add_child.sequence(),
            bindings.commit.sequence(),
            bindings.refresh.sequence(),
            bindings.view.sequence(),
        ];
        if sequences
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != sequences.len()
        {
            return Err("Composer action keys must be unique".into());
        }
        Ok(bindings)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppSettings {
    pub(crate) jira_base_url: String,
    pub(crate) jira_email: String,
    pub(crate) jira_api_token: String,
    pub(crate) jira_default_project: String,
    pub(crate) jira_default_board: String,
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

    pub(crate) fn values(&self) -> [(&'static str, String); 12] {
        [
            (JIRA_BASE_URL_SETTING, self.jira_base_url.clone()),
            (JIRA_EMAIL_SETTING, self.jira_email.clone()),
            (JIRA_API_TOKEN_SETTING, self.jira_api_token.clone()),
            (
                JIRA_DEFAULT_PROJECT_SETTING,
                self.jira_default_project.clone(),
            ),
            (JIRA_DEFAULT_BOARD_SETTING, self.jira_default_board.clone()),
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
        ]
    }

    pub(crate) fn changed_values(&self, previous: &Self) -> Vec<(&'static str, String)> {
        self.values()
            .into_iter()
            .zip(previous.values())
            .filter_map(|(current, old)| (current.1 != old.1).then_some(current))
            .collect()
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
    let mut chars = key.chars();
    let key = chars.next()?;
    chars
        .next()
        .is_none()
        .then(|| KeySpec::key_with_modifiers(Key::Char(key), modifier))
}

#[cfg(test)]
mod tests;
