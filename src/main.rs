#[cfg(debug_assertions)]
use std::{
    backtrace::Backtrace,
    fs::File,
    io::Write,
    panic,
    sync::{Arc, Mutex},
};

#[cfg(debug_assertions)]
const DEVELOPMENT_CRASH_LOG: &str = ".finery-dev.log";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    install_development_crash_log();
    finery::cli::run()
}

#[cfg(debug_assertions)]
fn install_development_crash_log() {
    let Ok(log) = File::create(DEVELOPMENT_CRASH_LOG) else {
        return;
    };
    let log = Arc::new(Mutex::new(log));
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        if let Ok(mut log) = log.lock() {
            let _ = writeln!(log, "{panic_info}\n\n{}", Backtrace::force_capture());
        }
        previous(panic_info);
    }));
}
