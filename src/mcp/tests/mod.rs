use std::sync::Arc;

use crate::{
    mcp::{run_composer, workspace_change_ticket_tree, workspace_view},
    service::composer_service::{
        ChangeKindView, ChangeSetCatalogView, ChangeSetView, JiraTicketLookup, TicketChangeView,
        TicketKindView, TicketView, Versioned, test_service,
    },
    storage::Storage,
    store::{
        composer::Ticket,
        work_items::{BacklogSnapshot, Sprint, WorkItem},
    },
};

fn change(id: &str, parent_key: Option<&str>) -> TicketChangeView {
    TicketChangeView {
        id: id.into(),
        kind: ChangeKindView::Modified,
        original: Some(TicketView {
            key: id.into(),
            project_key: "FIN".into(),
            title: format!("Title for {id}"),
            description: "Must not appear in the workspace response".into(),
            description_safe_to_overwrite: true,
            kind: TicketKindView::Story,
            status: "To Do".into(),
            priority: "Medium".into(),
            assignee: "Unassigned".into(),
            parent_key: parent_key.map(str::to_owned),
            parent_title: parent_key.map(|key| format!("Title for {key}")),
            parent_kind: None,
            has_children: false,
        }),
        updated: None,
        submitted: false,
        selected_for_commit: false,
        retry_blocked: false,
        create_attempt: false,
        submission_claimed: false,
    }
}

#[test]
fn composer_calls_complete_from_async_runtime() {
    let setup_runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let storage = setup_runtime
        .block_on(Storage::connect_for_tests())
        .unwrap();
    let jira: Arc<dyn JiraTicketLookup> =
        Arc::new(|_: &str| -> Result<Ticket, String> { Err("not used".into()) });
    let service = test_service(storage, setup_runtime, jira);
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_composer(service, |service| {
            service.change_set_catalog()
        }));

    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn workspace_ticket_tree_nests_children_without_descriptions() {
    let tickets =
        workspace_change_ticket_tree(vec![change("FIN-1", None), change("FIN-2", Some("FIN-1"))]);

    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, "FIN-1");
    assert_eq!(tickets[0].children[0].id, "FIN-2");
    assert!(tickets[0].original.as_ref().unwrap().has_children);
    assert_eq!(
        tickets[0].children[0]
            .original
            .as_ref()
            .unwrap()
            .parent
            .as_ref()
            .unwrap()
            .title,
        "Title for FIN-1"
    );
    assert!(
        !serde_json::to_string(&tickets)
            .unwrap()
            .contains("Must not appear in the workspace response")
    );
}

#[test]
fn workspace_ticket_tree_uses_submitted_draft_keys_as_parent_aliases() {
    let mut parent = change("NEW-1", None);
    parent.updated = parent.original.clone();
    parent.original = None;
    parent.updated.as_mut().unwrap().key = "FIN-1".into();
    parent.submitted = true;
    let tickets = workspace_change_ticket_tree(vec![parent, change("NEW-2", Some("FIN-1"))]);

    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].id, "NEW-1");
    assert_eq!(tickets[0].children[0].id, "NEW-2");
}

fn work_item(index: usize) -> WorkItem {
    WorkItem {
        key: format!("FIN-{index}"),
        title: format!("Title for FIN-{index}"),
        kind: "Story".into(),
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        fix_versions: Vec::new(),
        epic_name: None,
        story_points: Some(index as f64),
    }
}

#[test]
fn workspace_limits_only_unplanned_tickets_and_omits_closed_change_sets() {
    let open = ChangeSetView {
        id: "CS-open".into(),
        name: "Open".into(),
        closed: false,
        selected_ticket_ids: Vec::new(),
        tickets: vec![change("FIN-1", None)],
    };
    let closed = ChangeSetView {
        id: "CS-closed".into(),
        name: "Closed".into(),
        closed: true,
        selected_ticket_ids: Vec::new(),
        tickets: Vec::new(),
    };
    let view = workspace_view(
        BacklogSnapshot {
            board_name: "Finery".into(),
            story_points_configured: true,
            sprints: vec![Sprint {
                id: 1,
                name: "Sprint 1".into(),
                state: "active".into(),
                start_date: None,
                end_date: None,
                work_items: (0..51).map(work_item).collect(),
                capacity: None,
            }],
            work_items: (51..102).map(work_item).collect(),
            warnings: vec!["Story points are unavailable".into()],
            runway: None,
            velocity: None,
        },
        Versioned {
            revision: 7,
            value: ChangeSetCatalogView {
                change_sets: vec![
                    Versioned {
                        revision: 2,
                        value: open,
                    },
                    Versioned {
                        revision: 3,
                        value: closed,
                    },
                ],
            },
        },
    );

    assert_eq!(view.backlog.sprints[0].tickets.len(), 51);
    assert_eq!(view.backlog.unplanned_tickets.len(), 50);
    assert_eq!(view.backlog.unplanned_ticket_limit, 50);
    assert_eq!(view.backlog.unplanned_total_count, 51);
    assert!(view.backlog.unplanned_truncated);
    assert_eq!(view.backlog.warnings, ["Story points are unavailable"]);
    assert_eq!(view.backlog.sprints[0].tickets[0].story_points, Some(0.0));
    assert_eq!(view.change_sets.len(), 1);
    assert_eq!(view.change_sets[0].id, "CS-open");
}
