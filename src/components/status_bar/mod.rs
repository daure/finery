use std::{cell::Cell, rc::Rc};

use tuicore::{StatusBar, StatusBarMenuItem, WeatherProviderConfig};

const SETTINGS_MENU_ID: &str = "settings";
const STATUS_BAR_MENU_ITEMS: [StatusBarMenuItem; 3] = [
    StatusBarMenuItem::Custom {
        id: SETTINGS_MENU_ID,
        label: " Settings",
    },
    StatusBarMenuItem::Theme,
    StatusBarMenuItem::WeatherForecast,
];

pub(crate) fn status_bar(open_settings: Rc<Cell<bool>>) -> StatusBar<()> {
    StatusBar::new()
        .ai_enabled(false)
        .menu_items(STATUS_BAR_MENU_ITEMS)
        .weather_provider(WeatherProviderConfig::new().enabled(true))
        .on_custom_menu_item(move |id| {
            if id == SETTINGS_MENU_ID {
                open_settings.set(true);
            }
        })
}
