use std::{collections::HashMap, env, time::Duration};

use crate::speed_reader_settings::{
    SpeedReaderSettings, parse_markdown_block_pause, parse_speed_reader_wpm,
};

pub(crate) const JIRA_BASE_URL_SETTING: &str = "jira.base_url";
pub(crate) const JIRA_EMAIL_SETTING: &str = "jira.email";
pub(crate) const JIRA_API_TOKEN_SETTING: &str = "jira.api_token";
pub(crate) const JIRA_DEFAULT_PROJECT_SETTING: &str = "jira.default_project";
pub(crate) const SPEED_READER_WPM_SETTING: &str = "reader.wpm";
pub(crate) const SPEED_READER_BLOCK_DELAY_SETTING: &str = "reader.markdown_block_pause_ms";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AppSettings {
    pub(crate) jira_base_url: String,
    pub(crate) jira_email: String,
    pub(crate) jira_api_token: String,
    pub(crate) jira_default_project: String,
    pub(crate) speed_reader: SpeedReaderSettings,
}

impl AppSettings {
    pub(crate) fn resolve(values: &HashMap<String, String>) -> Self {
        let defaults = Self::default();
        let value_or_env = |key: &str, variable: &str| {
            env::var(variable)
                .ok()
                .or_else(|| values.get(key).cloned())
                .unwrap_or_default()
        };
        Self {
            jira_base_url: value_or_env(JIRA_BASE_URL_SETTING, "JIRA_BASE_URL"),
            jira_email: value_or_env(JIRA_EMAIL_SETTING, "JIRA_EMAIL"),
            jira_api_token: value_or_env(JIRA_API_TOKEN_SETTING, "JIRA_API_TOKEN"),
            jira_default_project: value_or_env(
                JIRA_DEFAULT_PROJECT_SETTING,
                "JIRA_DEFAULT_PROJECT",
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
        }
    }

    pub(crate) fn values(&self) -> [(&'static str, String); 6] {
        [
            (JIRA_BASE_URL_SETTING, self.jira_base_url.clone()),
            (JIRA_EMAIL_SETTING, self.jira_email.clone()),
            (JIRA_API_TOKEN_SETTING, self.jira_api_token.clone()),
            (
                JIRA_DEFAULT_PROJECT_SETTING,
                self.jira_default_project.clone(),
            ),
            (SPEED_READER_WPM_SETTING, self.speed_reader.wpm.to_string()),
            (
                SPEED_READER_BLOCK_DELAY_SETTING,
                self.speed_reader
                    .markdown_block_pause
                    .as_millis()
                    .to_string(),
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

#[cfg(test)]
mod tests;
