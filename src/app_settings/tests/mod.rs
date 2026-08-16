use super::{AppSettings, SPEED_READER_WPM_SETTING};

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
