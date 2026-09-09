use crate::{service::AppService, store::work_items::VelocitySprint};

use super::page::velocity_share_report;

pub(super) fn copy_report(service: &AppService, mut sprint: VelocitySprint) {
    let worker_service = service.clone();
    service.copy_in_background(move || {
        let (base_url, tickets) = worker_service.jira_velocity_tickets(&[sprint.id])?;
        sprint.work_items = Some(
            tickets
                .into_iter()
                .find(|(id, _)| *id == sprint.id)
                .ok_or_else(|| "Jira sprint report response is missing".to_string())?
                .1?,
        );
        Ok(velocity_share_report(&sprint, None, Some(&base_url)))
    });
}

#[cfg(test)]
#[path = "tests/velocity_reports.rs"]
mod tests;
