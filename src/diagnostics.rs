use std::{
    backtrace::Backtrace,
    error::Error,
    fs,
    io::Write,
    panic,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use tracing_appender::rolling::{RollingFileAppender, Rotation};

const LOG_FILE_PREFIX: &str = "finery";
const MAX_LOG_FILES: usize = 15;
const PROCESS_MODE_ENV: &str = "FINERY_PROCESS_MODE";

struct DiagnosticLog {
    appender: Mutex<RollingFileAppender>,
    mode: String,
    pid: u32,
}

static LOG: OnceLock<Arc<DiagnosticLog>> = OnceLock::new();

pub fn install() {
    if LOG.get().is_some() {
        return;
    }

    let Some(directory) = log_directory() else {
        eprintln!("Finery diagnostics disabled: user state directory is unavailable");
        return;
    };
    if let Err(error) = create_log_directory(&directory) {
        eprintln!(
            "Finery diagnostics disabled: could not create {}: {error}",
            directory.display()
        );
        return;
    }

    let appender = match RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(&directory)
    {
        Ok(appender) => appender,
        Err(error) => {
            eprintln!(
                "Finery diagnostics disabled: could not open {}: {error}",
                directory.display()
            );
            return;
        }
    };

    let log = Arc::new(DiagnosticLog {
        appender: Mutex::new(appender),
        mode: process_mode(),
        pid: std::process::id(),
    });
    if LOG.set(Arc::clone(&log)).is_err() {
        return;
    }

    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        write_entry(
            &log,
            "PANIC",
            &format!("{panic_info}\n\n{}", Backtrace::force_capture()),
        );
        previous(panic_info);
    }));
}

pub fn record_error(context: &str, error: &dyn Error) {
    let Some(log) = LOG.get() else {
        return;
    };
    write_entry(log, "ERROR", &format!("{context}: {error}"));
}

fn write_entry(log: &DiagnosticLog, level: &str, message: &str) {
    let Ok(mut appender) = log.appender.lock() else {
        return;
    };
    let timestamp = chrono::Utc::now().to_rfc3339();
    let _ = writeln!(
        appender,
        "[{timestamp}] {level} pid={} mode={} {message}\n",
        log.pid, log.mode
    );
    let _ = appender.flush();
}

fn process_mode() -> String {
    if let Ok(mode) = std::env::var(PROCESS_MODE_ENV) {
        return mode;
    }
    match std::env::args().nth(1).as_deref() {
        None => "tui",
        Some("dev") => "dev",
        Some("mcp") => "mcp-stdio",
        Some("serve") => "mcp-http",
        Some("service") => "service-control",
        Some(_) => "cli",
    }
    .into()
}

fn log_directory() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|directory| directory.join("finery").join("logs"))
}

fn create_log_directory(directory: &Path) -> std::io::Result<()> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
