use std::collections::HashMap;

use super::{
    AppSettings, COMPOSER_ADD_CHILD_KEY_SETTING, COMPOSER_ADD_SIBLING_KEY_SETTING,
    COMPOSER_COMMIT_KEY_SETTING, COMPOSER_CREATE_SUBMIT_KEY_SETTING,
    COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING, COMPOSER_DESCRIPTION_READER_KEY_SETTING,
    COMPOSER_ISSUE_TYPE_KEY_SETTING, COMPOSER_VIEW_KEY_SETTING, JIRA_STORY_POINTS_BOARD_ID_SETTING,
    JIRA_STORY_POINTS_FIELD_ID_SETTING, SPEED_READER_WPM_SETTING,
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
fn story_points_field_id_round_trips_through_settings_values() {
    let settings = AppSettings::resolve(&HashMap::from([
        (
            JIRA_STORY_POINTS_FIELD_ID_SETTING.into(),
            "customfield_10016".into(),
        ),
        (JIRA_STORY_POINTS_BOARD_ID_SETTING.into(), "42".into()),
    ]))
    .unwrap();

    assert_eq!(settings.jira_story_points_field_id, "customfield_10016");
    assert_eq!(settings.jira_story_points_board_id, "42");
    assert!(settings.values().contains(&(
        JIRA_STORY_POINTS_FIELD_ID_SETTING,
        "customfield_10016".into(),
    )));
    assert!(
        settings
            .values()
            .contains(&(JIRA_STORY_POINTS_BOARD_ID_SETTING, "42".into()))
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
    assert_eq!(settings.composer_keys.create_submit.label(), "⌃Enter");
    assert_eq!(settings.composer_keys.description_focus.label(), "dd");

    let duplicate = HashMap::from([
        (COMPOSER_ADD_SIBLING_KEY_SETTING.into(), "shift+s".into()),
        (COMPOSER_COMMIT_KEY_SETTING.into(), "shift+s".into()),
    ]);
    assert!(super::ComposerKeyBindings::from_values(&duplicate).is_err());
    assert!(AppSettings::resolve(&duplicate).is_err());

    let duplicate_sequence = HashMap::from([
        (COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING.into(), "dd".into()),
        (COMPOSER_DESCRIPTION_READER_KEY_SETTING.into(), "dd".into()),
    ]);
    assert!(AppSettings::resolve(&duplicate_sequence).is_err());

    let duplicate_create_dialog =
        HashMap::from([(COMPOSER_CREATE_SUBMIT_KEY_SETTING.into(), "o".into())]);
    assert!(AppSettings::resolve(&duplicate_create_dialog).is_err());

    let prefix_across_workspace = HashMap::from([
        (COMPOSER_ADD_SIBLING_KEY_SETTING.into(), "d".into()),
        (COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING.into(), "dd".into()),
    ]);
    assert!(AppSettings::resolve(&prefix_across_workspace).is_err());

    let exact_across_workspace = HashMap::from([
        (COMPOSER_ADD_SIBLING_KEY_SETTING.into(), "d".into()),
        (COMPOSER_ISSUE_TYPE_KEY_SETTING.into(), "d".into()),
    ]);
    assert!(AppSettings::resolve(&exact_across_workspace).is_err());
}

#[test]
fn composer_bindings_can_be_updated_through_app_settings() {
    let mut settings = AppSettings::default();

    settings
        .update_composer_binding(COMPOSER_ADD_SIBLING_KEY_SETTING, "alt+s".into())
        .unwrap();

    assert_eq!(settings.composer_keys.add_sibling.sequence(), "alt+s");
    assert!(
        settings
            .update_composer_binding(COMPOSER_COMMIT_KEY_SETTING, "alt+s".into())
            .is_err()
    );
}
