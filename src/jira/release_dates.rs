use std::collections::{BTreeSet, HashMap};

use super::{BacklogSnapshot, Client, JiraFixVersion, response_json};
use crate::store::work_items::release::parse_date;

pub(super) fn project_versions(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    project: &str,
) -> Result<Vec<JiraFixVersion>, String> {
    let response = client
        .get(format!("{base_url}/rest/api/3/project/{project}/versions"))
        .basic_auth(email, Some(token))
        .send()
        .map_err(|error| error.to_string())?;
    response_json(response)
}

pub(super) fn hydrate(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    snapshot: &mut BacklogSnapshot,
) {
    let projects = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .filter(|item| !item.releases.is_empty())
        .filter_map(|item| {
            item.key
                .rsplit_once('-')
                .map(|(project, _)| project.to_owned())
        })
        .collect::<BTreeSet<_>>();
    // Jira issue payloads omit version startDate; the project versions endpoint carries it.
    for project in projects {
        let versions = match project_versions(client, base_url, email, token, &project) {
            Ok(versions) => versions,
            Err(error) => {
                snapshot.warnings.push(format!(
                    "Could not load release dates for {project}: {error}"
                ));
                continue;
            }
        };
        let dates = versions
            .into_iter()
            .map(|version| {
                (
                    version.id,
                    (
                        version.start_date.as_deref().and_then(parse_date),
                        version.release_date.as_deref().and_then(parse_date),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        for item in snapshot
            .sprints
            .iter_mut()
            .flat_map(|sprint| &mut sprint.work_items)
            .chain(&mut snapshot.work_items)
            .filter(|item| {
                item.key
                    .rsplit_once('-')
                    .is_some_and(|(key, _)| key == project)
            })
        {
            for version in &mut item.releases {
                if let Some(&(start, end)) = dates.get(&version.id) {
                    version.start_date = start;
                    version.end_date = end;
                }
            }
        }
    }
}
