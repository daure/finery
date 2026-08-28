mod app;
mod app_settings;
mod components;
mod jira;
mod mcp;
mod pages;
mod service;
mod speed_reader_settings;
mod storage;
mod store;

use std::{net::SocketAddr, sync::mpsc::Sender, thread};

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (service, change_sets) = service::AppService::initialize()?;
    tuicore::TreeApp::new(app::root(service.clone(), change_sets)).run()?;
    service.flush()?;
    Ok(())
}

pub fn run_mcp() -> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = service::AppService::initialize()?;
    tokio::runtime::Runtime::new()?.block_on(mcp::run_stdio(service))
}

pub fn run_http(bind: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = service::AppService::initialize()?;
    tokio::runtime::Runtime::new()?.block_on(mcp::run_http(service, bind))
}

pub fn run_http_with_startup(
    bind: SocketAddr,
    startup: Sender<Result<(), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = match service::AppService::initialize() {
        Ok(service) => service,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return Err(Box::new(error));
        }
    };
    runtime.block_on(mcp::run_http_with_startup(service, bind, startup))
}

pub fn run_dev(bind: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let (service, change_sets) = service::AppService::initialize()?;
    let (startup_tx, startup_rx) = std::sync::mpsc::channel();
    let http_service = service.clone();
    thread::Builder::new()
        .name("finery-mcp-http".into())
        .spawn(move || {
            let runtime = tokio::runtime::Runtime::new();
            match runtime {
                Ok(runtime) => {
                    if let Err(error) =
                        runtime.block_on(mcp::run_http_with_startup(http_service, bind, startup_tx))
                    {
                        eprintln!("HTTP MCP stopped: {error}");
                    }
                }
                Err(error) => {
                    let _ = startup_tx.send(Err(error.to_string()));
                }
            }
        })?;
    startup_rx
        .recv()
        .map_err(|_| "HTTP MCP startup thread exited without status")?
        .map_err(|error| format!("HTTP MCP startup failed: {error}"))?;
    tuicore::TreeApp::new(app::root(service.clone(), change_sets)).run()?;
    service.flush()?;
    Ok(())
}
