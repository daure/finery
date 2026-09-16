use std::collections::{BTreeSet, HashMap};

use super::{BacklogSnapshot, Client, request_bulk_fetch};
use crate::store::composer::jira_adf::adf_to_markdown;

pub(super) fn hydrate(
    client: &Client,
    base_url: &str,
    email: &str,
    token: &str,
    snapshot: &mut BacklogSnapshot,
) {
    let keys = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .filter(|item| !item.description.is_empty())
        .map(|item| item.key.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    // Agile descriptions use wiki markup; REST v3 retains ADF and code-block languages.
    let mut descriptions = HashMap::new();
    for batch in keys.chunks(100) {
        let response =
            match request_bulk_fetch(client, base_url, email, token, batch, &["description"]) {
                Ok(response) => response,
                Err(error) => {
                    snapshot
                        .warnings
                        .push(format!("Could not load formatted descriptions: {error}"));
                    continue;
                }
            };
        for issue in response.issues {
            if let Some(description) = issue.fields.get("description") {
                descriptions.insert(issue.key, adf_to_markdown(description));
            }
        }
        let missing = batch
            .iter()
            .filter(|key| !descriptions.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            snapshot.warnings.push(format!(
                "Could not load formatted descriptions for {}",
                missing.join(", ")
            ));
        }
    }

    for item in snapshot
        .sprints
        .iter_mut()
        .flat_map(|sprint| &mut sprint.work_items)
        .chain(&mut snapshot.work_items)
    {
        if let Some(description) = descriptions.get(&item.key) {
            item.description.clone_from(description);
        }
    }
}
