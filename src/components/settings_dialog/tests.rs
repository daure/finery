use tuicore::EventCtx;

use super::{SettingChange, SettingsDialog};
use crate::{app_settings::COMPOSER_TITLE_KEY_SETTING, service::AppService};

#[test]
fn composer_binding_changes_wait_for_restart_before_replacing_live_shortcuts() {
    let service = AppService::for_tests();
    let settings = service.settings();
    let live_title_key = settings
        .read()
        .unwrap()
        .composer_keys
        .title
        .sequence()
        .to_owned();
    let dialog = SettingsDialog::new(settings.clone(), service.clone());
    dialog
        .changes
        .borrow_mut()
        .push(SettingChange::ComposerBinding(
            COMPOSER_TITLE_KEY_SETTING,
            "shift+g".into(),
        ));

    dialog.apply_changes(&mut EventCtx::default());
    service.flush().unwrap();

    assert_eq!(
        settings.read().unwrap().composer_keys.title.sequence(),
        live_title_key
    );
}
