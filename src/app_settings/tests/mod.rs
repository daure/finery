use std::collections::HashMap;

use super::{
    AppSettings, BACKLOG_FIXED_SPRINT_CAPACITY_SETTING, BACKLOG_FIXED_TICKET_SIZE_SETTING,
    BACKLOG_SPRINT_TOLERANCE_PERCENT_SETTING, BACKLOG_USE_AVERAGE_TICKET_SIZE_SETTING,
    BACKLOG_USE_JIRA_VELOCITY_SETTING, COMPOSER_ADD_CHILD_KEY_SETTING,
    COMPOSER_ADD_SIBLING_KEY_SETTING, COMPOSER_COMMIT_KEY_SETTING,
    COMPOSER_CREATE_SUBMIT_KEY_SETTING, COMPOSER_DESCRIPTION_FOCUS_KEY_SETTING,
    COMPOSER_DESCRIPTION_READER_KEY_SETTING, COMPOSER_ISSUE_TYPE_KEY_SETTING,
    COMPOSER_VIEW_KEY_SETTING, JIRA_STORY_POINTS_BOARD_ID_SETTING,
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
fn backlog_runway_settings_round_trip_and_reject_invalid_values() {
    let settings = AppSettings::resolve(&HashMap::from([
        (BACKLOG_USE_JIRA_VELOCITY_SETTING.into(), "true".into()),
        (BACKLOG_FIXED_SPRINT_CAPACITY_SETTING.into(), "18.5".into()),
        (
            BACKLOG_USE_AVERAGE_TICKET_SIZE_SETTING.into(),
            "true".into(),
        ),
        (BACKLOG_FIXED_TICKET_SIZE_SETTING.into(), "2.5".into()),
        (BACKLOG_SPRINT_TOLERANCE_PERCENT_SETTING.into(), "15".into()),
    ]))
    .unwrap();

    assert!(settings.backlog_runway.use_jira_velocity);
    assert_eq!(settings.backlog_runway.fixed_sprint_capacity, 18.5);
    assert!(settings.backlog_runway.use_average_ticket_size);
    assert_eq!(settings.backlog_runway.fixed_ticket_size, 2.5);
    assert_eq!(settings.backlog_runway.sprint_tolerance_percent, 15);
    assert!(
        settings
            .values()
            .contains(&(BACKLOG_FIXED_SPRINT_CAPACITY_SETTING, "18.5".into()))
    );

    let defaults = AppSettings::resolve(&HashMap::from([(
        BACKLOG_FIXED_SPRINT_CAPACITY_SETTING.into(),
        "0".into(),
    )]))
    .unwrap();
    assert_eq!(defaults.backlog_runway.fixed_sprint_capacity, 20.0);
    assert_eq!(defaults.backlog_runway.sprint_tolerance_percent, 20);
}

#[test]
fn manual_story_points_field_clears_discovery_provenance() {
    let mut settings = AppSettings {
        jira_story_points_board_id: "42".into(),
        jira_story_points_discovery_complete: true,
        ..AppSettings::default()
    };

    settings.set_manual_story_points_field("customfield_10016".into());

    assert_eq!(settings.jira_story_points_field_id, "customfield_10016");
    assert!(settings.jira_story_points_board_id.is_empty());
    assert!(!settings.jira_story_points_discovery_complete);
}

#[test]
fn board_changes_invalidate_discovery_without_clearing_manual_story_points() {
    let mut discovered = AppSettings {
        jira_story_points_field_id: "customfield_discovered".into(),
        jira_story_points_board_id: "42".into(),
        jira_story_points_discovery_complete: true,
        ..AppSettings::default()
    };
    discovered.invalidate_discovered_story_points();
    assert!(discovered.jira_story_points_field_id.is_empty());
    assert!(discovered.jira_story_points_board_id.is_empty());

    let mut manual = AppSettings {
        jira_story_points_field_id: "customfield_manual".into(),
        ..AppSettings::default()
    };
    manual.invalidate_discovered_story_points();
    assert_eq!(manual.jira_story_points_field_id, "customfield_manual");
    assert!(manual.story_points_field_is_manual());
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
