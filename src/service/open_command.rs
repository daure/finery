use std::process::{Command, Stdio};

use super::AppService;

impl AppService {
    pub(crate) fn run_open_command(&self, key: &str) -> bool {
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
        let service = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-open-command".into())
            .spawn(move || {
                let result =
                    run_command(&command, &key, url.as_deref().unwrap_or_default(), || {
                        service.report_notification(tuicore::Notification::info(
                            "Open command started",
                            format!("Running open command for {key}"),
                        ));
                    });
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
    url: &str,
    on_started: impl FnOnce(),
) -> Result<(), String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("FINERY_TICKET_KEY", key)
        .env("FINERY_TICKET_URL", url)
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
