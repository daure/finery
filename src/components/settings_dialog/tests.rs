use tuicore::EventCtx;

use super::{SettingChange, SettingsDialog};
use crate::service::AppService;

#[test]
fn story_points_field_changes_apply_to_live_settings() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let dialog = SettingsDialog::new(settings.clone(), service.clone());
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::JiraStoryPointsFieldId(
            " customfield_10016 ".into(),
        ));

    dialog.apply_changes(&mut EventCtx::default());
    service.flush().unwrap();

    assert_eq!(
        settings.read().unwrap().jira_story_points_field_id,
        "customfield_10016"
    );
}

#[test]
fn board_changes_preserve_manual_story_points_field() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let dialog = SettingsDialog::new(settings.clone(), service.clone());
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::JiraStoryPointsFieldId(
            "customfield_manual".into(),
        ));
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::JiraDefaultBoard("17".into()));

    dialog.apply_changes(&mut EventCtx::default());

    assert_eq!(
        settings.read().unwrap().jira_story_points_field_id,
        "customfield_manual"
    );
}
