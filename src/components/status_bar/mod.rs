use tuicore::{StatusBar, WeatherProviderConfig};

pub(crate) fn status_bar() -> StatusBar<()> {
    StatusBar::new()
        .ai_enabled(false)
        .weather_provider(WeatherProviderConfig::new().enabled(true))
}
