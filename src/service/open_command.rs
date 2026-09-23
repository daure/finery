use std::process::{Command, Stdio};

use super::AppService;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCommandRequest {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) values: Vec<String>,
}

impl AppService {
    pub(crate) fn open_command(&self, key: &str, title: &str) -> bool {
        let values = match self.settings.read() {
            Ok(settings)
                if !settings.open_command.trim().is_empty()
                    && !key.is_empty()
                    && !key.starts_with("NEW-") =>
            {
                settings.open_command_enum.clone()
            }
            Ok(_) => return false,
            Err(_) => {
                self.report_error(
                    "Could not run open command: settings lock is unavailable".into(),
                );
                return false;
            }
        };
        if values.is_empty() {
            return self.run_open_command(key, title);
        }
        match self.pending_open_command.lock() {
            Ok(mut request) => {
                *request = Some(OpenCommandRequest {
                    key: key.to_owned(),
                    title: title.to_owned(),
                    values,
                });
                true
            }
            Err(_) => {
                self.report_error(
                    "Could not open command menu: pending request lock is unavailable".into(),
                );
                false
            }
        }
    }

    pub(crate) fn take_open_command_request(&self) -> Option<OpenCommandRequest> {
        self.pending_open_command.lock().ok()?.take()
    }

    pub(crate) fn run_open_command(&self, key: &str, title: &str) -> bool {
        self.run_open_command_with_value(key, title, "")
    }

    pub(crate) fn run_open_command_with_value(&self, key: &str, title: &str, value: &str) -> bool {
        let (command, url) = match self.settings.read() {
            Ok(settings) => (settings.open_command.clone(), settings.jira_issue_url(key)),
            Err(_) => {
                self.report_error(
                    "Could not run open command: settings lock is unavailable".into(),
                );
                return false;
            }
        };
        if command.trim().is_empty() || key.is_empty() || key.starts_with("NEW-") {
            return false;
        }
        let key = key.to_owned();
        let title = title.to_owned();
        let value = value.to_owned();
        let service = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-open-command".into())
            .spawn(move || {
                let result = run_command(
                    &command,
                    &key,
                    &title,
                    url.as_deref().unwrap_or_default(),
                    &value,
                    || {
                        service.report_notification(tuicore::Notification::info(
                            "Open command started",
                            format!("Running open command for {key}"),
                        ));
                    },
                );
                if let Err(error) = result {
                    service.report_error(error);
                }
            })
        {
            self.report_error(format!("Could not start open command: {error}"));
            return false;
        }
        true
    }
}

fn run_command(
    command: &str,
    key: &str,
    title: &str,
    url: &str,
    value: &str,
    on_started: impl FnOnce(),
) -> Result<(), String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("FINERY_TICKET_KEY", key)
        .env("FINERY_TICKET_TITLE", title)
        .env("FINERY_TICKET_URL", url)
        .env("FINERY_CMD_VALUE", value)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not run open command: {error}"))?;
    on_started();
    let status = child
        .wait()
        .map_err(|error| format!("Could not wait for open command: {error}"))?;
    if !status.success() {
        return Err(format!("Open command exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/open_command.rs"]
pub(crate) mod tests;
