mod app;
mod components;
mod pages;
mod speed_reader_settings;
mod store;

pub fn run() -> tuicore::Result<()> {
    tuicore::TreeApp::new(app::root()).run()
}
