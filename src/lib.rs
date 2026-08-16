mod app;
mod app_settings;
mod components;
mod jira;
mod pages;
mod service;
mod speed_reader_settings;
mod storage;
mod store;

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (service, change_sets) = service::AppService::initialize()?;
    tuicore::TreeApp::new(app::root(service.clone(), change_sets)).run()?;
    service.flush()?;
    Ok(())
}
