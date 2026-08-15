mod app;
mod components;
mod pages;

pub fn run() -> tuicore::Result<()> {
    tuicore::TreeApp::new(app::root()).run()
}
