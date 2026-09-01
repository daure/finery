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

#[test]
fn company_managed_url_changes_apply_to_live_settings() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let dialog = SettingsDialog::new(settings.clone(), service);
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::JiraCompanyManagedUrls(true));

    dialog.apply_changes(&mut EventCtx::default());

    assert!(settings.read().unwrap().jira_company_managed_urls);
}

#[test]
fn backlog_runway_changes_apply_to_live_settings() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let dialog = SettingsDialog::new(settings.clone(), service.clone());
    dialog.changes.borrow_mut().extend([
        SettingChange::BacklogUseJiraVelocity(true),
        SettingChange::BacklogJiraVelocitySprints("4".into()),
        SettingChange::BacklogFixedSprintCapacity("18.5".into()),
        SettingChange::BacklogUseAverageTicketSize(true),
        SettingChange::BacklogFixedTicketSize("2.5".into()),
        SettingChange::BacklogSprintTolerancePercent("15".into()),
    ]);

    dialog.apply_changes(&mut EventCtx::default());

    let settings = settings.read().unwrap();
    assert!(settings.backlog_runway.use_jira_velocity);
    assert_eq!(settings.backlog_runway.jira_velocity_sprints, 4);
    assert_eq!(settings.backlog_runway.fixed_sprint_capacity, 18.5);
    assert!(settings.backlog_runway.use_average_ticket_size);
    assert_eq!(settings.backlog_runway.fixed_ticket_size, 2.5);
    assert_eq!(settings.backlog_runway.sprint_tolerance_percent, 15);
}

#[test]
fn excluded_sprint_name_fragments_apply_to_live_settings() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let dialog = SettingsDialog::new(settings.clone(), service);
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::BacklogExcludedSprintNameFragments(
            " abc, Archive,ABC ".into(),
        ));

    dialog.apply_changes(&mut EventCtx::default());

    let settings = settings.read().unwrap();
    assert_eq!(settings.excluded_sprint_name_fragments, ["abc", "Archive"]);
}
