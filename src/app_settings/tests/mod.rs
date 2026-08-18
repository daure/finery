use std::collections::HashMap;

use super::{
    AppSettings, COMPOSER_ADD_CHILD_KEY_SETTING, COMPOSER_ADD_SIBLING_KEY_SETTING,
    COMPOSER_COMMIT_KEY_SETTING, COMPOSER_VIEW_KEY_SETTING, SPEED_READER_WPM_SETTING,
};

#[test]
fn persistence_only_includes_changed_settings() {
    let previous = AppSettings {
        jira_api_token: "environment-secret".into(),
        ..AppSettings::default()
    };
    let mut current = previous.clone();
    current.speed_reader.wpm = 500;

    assert_eq!(
        current.changed_values(&previous),
        vec![(SPEED_READER_WPM_SETTING, "500".into())]
    );
}

#[test]
fn composer_keys_resolve_labels_and_reject_active_duplicates() {
    let values = HashMap::from([
        (COMPOSER_ADD_SIBLING_KEY_SETTING.into(), "alt+s".into()),
        (COMPOSER_ADD_CHILD_KEY_SETTING.into(), "shift+c".into()),
        (COMPOSER_COMMIT_KEY_SETTING.into(), "ctrl+m".into()),
        (COMPOSER_VIEW_KEY_SETTING.into(), "shift+v".into()),
    ]);
    let settings = AppSettings::resolve(&values).unwrap();
    assert_eq!(settings.composer_keys.add_sibling.label(), "⌥s");
    assert_eq!(settings.composer_keys.commit.label(), "⌃m");

    let duplicate = HashMap::from([
        (COMPOSER_ADD_SIBLING_KEY_SETTING.into(), "shift+s".into()),
        (COMPOSER_COMMIT_KEY_SETTING.into(), "shift+s".into()),
    ]);
    assert!(super::ComposerKeyBindings::from_values(&duplicate).is_err());
    assert!(AppSettings::resolve(&duplicate).is_err());
}
