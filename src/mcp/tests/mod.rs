use std::sync::Arc;

use crate::{
    mcp::{
        JiraPosition, final_order, placement_rank_plan, run_composer,
        workspace_capacity_guidance_view, workspace_view,
    },
    service::composer_service::{
        ChangeSetCatalogView, ChangeSetView, JiraTicketLookup, Versioned, test_service,
    },
    storage::Storage,
    store::{
        composer::Ticket,
        work_items::{
            BacklogRunway, BacklogSnapshot, RunwayCapacitySource, RunwayTicket, Sprint,
            VelocityReport, VelocitySprint, WorkItem,
        },
    },
};

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

fn work_item(index: usize) -> WorkItem {
    WorkItem {
        key: format!("FIN-{index}"),
        title: format!("Title for FIN-{index}"),
        kind: "Story".into(),
        status: "To Do".into(),
        done: false,
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
fn workspace_compacts_backlog_and_change_set_payloads() {
    let open = ChangeSetView {
        id: "CS-open".into(),
        name: "Open".into(),
        closed: false,
        selected_ticket_ids: Vec::new(),
        tickets: Vec::new(),
    };
    let closed = ChangeSetView {
        id: "CS-closed".into(),
        name: "Closed".into(),
        closed: true,
        selected_ticket_ids: Vec::new(),
        tickets: Vec::new(),
    };
    let mut sprint_tickets = (0..51).map(work_item).collect::<Vec<_>>();
    sprint_tickets[0].done = true;
    sprint_tickets[0].story_points = None;
    let view = workspace_view(
        BacklogSnapshot {
            board_name: "Finery".into(),
            story_points_configured: true,
            sprints: vec![Sprint {
                id: 1,
                name: "Sprint 1".into(),
                state: "active".into(),
                goal: Some("Ship workspace planning".into()),
                start_date: Some("2026-08-01T09:00:00.000Z".into()),
                end_date: Some("2026-08-14T17:00:00.000Z".into()),
                work_items: sprint_tickets,
                capacity: None,
            }],
            work_items: (51..102).map(work_item).collect(),
            warnings: vec!["Story points are unavailable".into()],
            runway: Some(BacklogRunway {
                capacity: 20.5,
                source: RunwayCapacitySource::JiraVelocity,
                estimated_points: 100.0,
                assumed_points: 12.0,
                tickets: (51..102)
                    .map(|index| RunwayTicket {
                        key: format!("FIN-{index}"),
                        virtual_sprint: 1,
                        effective_points: index as f64,
                        assumed: true,
                        assumed_from_average: true,
                    })
                    .collect(),
            }),
            velocity: Some(VelocityReport {
                configured_sprints: 2,
                dynamic_capacity: Some(20.5),
                sprints: vec![
                    VelocitySprint {
                        id: 10,
                        name: "Sprint 0".into(),
                        completed: 22.0,
                        goal: Some("Finish prior work".into()),
                    },
                    VelocitySprint {
                        id: 9,
                        name: "Sprint -1".into(),
                        completed: 20.0,
                        goal: None,
                    },
                    VelocitySprint {
                        id: 8,
                        name: "Sprint -2".into(),
                        completed: 18.0,
                        goal: None,
                    },
                ],
            }),
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
    )
    .unwrap();

    assert_eq!(view.backlog.sprints[0].tickets.len(), 51);
    assert_eq!(view.backlog.unplanned.tickets.len(), 50);
    assert_eq!(view.backlog.unplanned.total_count, 51);
    assert_eq!(view.backlog.warnings, ["Story points are unavailable"]);
    assert_eq!(
        view.backlog
            .velocity
            .as_ref()
            .unwrap()
            .average_completed_points,
        Some(20.5)
    );
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[0].completed_points,
        22.0
    );
    assert_eq!(
        view.backlog.capacity_guidance.as_ref().unwrap().source,
        "jira_velocity"
    );
    let guidance = view.backlog.capacity_guidance.as_ref().unwrap();
    assert_eq!(guidance.first_unplanned_capacity_band.len(), 50);
    assert_eq!(guidance.first_unplanned_capacity_band[0].key, "FIN-51");
    assert_eq!(
        guidance.first_unplanned_capacity_band[0].effective_points,
        51.0
    );
    assert_eq!(view.backlog.velocity.as_ref().unwrap().sprints.len(), 2);
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[1].name,
        "Sprint -1"
    );
    assert_eq!(
        view.backlog.sprints[0].end_date.as_deref(),
        Some("2026-08-14T17:00:00.000Z")
    );
    assert_eq!(
        view.backlog.sprints[0].goal.as_deref(),
        Some("Ship workspace planning")
    );
    assert_eq!(
        view.backlog.velocity.as_ref().unwrap().sprints[0]
            .goal
            .as_deref(),
        Some("Finish prior work")
    );
    assert_eq!(view.change_sets.len(), 1);
    assert_eq!(view.change_sets[0].id, "CS-open");
    assert_eq!(view.change_sets[0].revision, 2);

    let json = serde_json::to_value(view).unwrap();
    assert!(json.get("change_set_catalog_revision").is_none());
    assert!(json["change_sets"][0].get("tickets").is_none());
    assert!(json["change_sets"][0].get("closed").is_none());
    assert!(json["backlog"].get("unplanned_tickets").is_none());
    assert!(json["backlog"].get("runway").is_none());
    let sparse_ticket = &json["backlog"]["sprints"][0]["tickets"][0];
    assert_eq!(sparse_ticket["done"], true);
    assert!(sparse_ticket.get("parent").is_none());
    assert!(sparse_ticket.get("has_children").is_none());
    assert!(sparse_ticket.get("story_points").is_none());
}

#[test]
fn workspace_caps_velocity_projection_at_ten_sprints() {
    let view = workspace_view(
        BacklogSnapshot {
            board_name: "Finery".into(),
            story_points_configured: true,
            sprints: Vec::new(),
            work_items: Vec::new(),
            warnings: Vec::new(),
            runway: None,
            velocity: Some(VelocityReport {
                configured_sprints: 11,
                dynamic_capacity: Some(20.5),
                sprints: (0..11)
                    .map(|index| VelocitySprint {
                        id: index,
                        name: format!("Sprint {index}"),
                        completed: index as f64,
                        goal: None,
                    })
                    .collect(),
            }),
        },
        Versioned {
            revision: 1,
            value: ChangeSetCatalogView {
                change_sets: Vec::new(),
            },
        },
    )
    .unwrap();

    let velocity_sprints = serde_json::to_value(view).unwrap()["backlog"]["velocity"]["sprints"]
        .as_array()
        .unwrap();
    assert_eq!(velocity_sprints.len(), 10);
    assert_eq!(velocity_sprints.last().unwrap()["name"], "Sprint 9");
}

#[test]
fn capacity_guidance_uses_key_references_and_points_sources() {
    let mut fixed = work_item(2);
    fixed.story_points = None;
    let mut bug = work_item(3);
    bug.kind = "Bug".into();
    bug.story_points = None;
    let mut average = work_item(4);
    average.story_points = None;
    let guidance = workspace_capacity_guidance_view(
        BacklogRunway {
            capacity: 20.0,
            source: RunwayCapacitySource::Fixed,
            estimated_points: 3.0,
            assumed_points: 0.0,
            tickets: vec![
                RunwayTicket {
                    key: "FIN-1".into(),
                    virtual_sprint: 1,
                    effective_points: 1.0,
                    assumed: false,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-2".into(),
                    virtual_sprint: 1,
                    effective_points: 2.0,
                    assumed: true,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-3".into(),
                    virtual_sprint: 1,
                    effective_points: 0.0,
                    assumed: false,
                    assumed_from_average: false,
                },
                RunwayTicket {
                    key: "FIN-4".into(),
                    virtual_sprint: 1,
                    effective_points: 2.0,
                    assumed: true,
                    assumed_from_average: true,
                },
            ],
        },
        &[work_item(1), fixed, bug, average],
    )
    .unwrap();

    assert_eq!(
        guidance
            .first_unplanned_capacity_band
            .iter()
            .map(|candidate| (candidate.key.as_str(), candidate.effective_points))
            .collect::<Vec<_>>(),
        [
            ("FIN-1", 1.0),
            ("FIN-2", 2.0),
            ("FIN-3", 0.0),
            ("FIN-4", 2.0)
        ]
    );
    let json = serde_json::to_value(guidance).unwrap();
    let band = json["first_unplanned_capacity_band"].as_array().unwrap();
    assert_eq!(band[0].as_object().unwrap().len(), 3);
    assert!(band[0].get("title").is_none());
    assert_eq!(
        band.iter()
            .map(|candidate| candidate["points_source"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "story_points",
            "fixed_assumption",
            "unestimated_bug",
            "average_assumption",
        ]
    );
}

#[test]
fn placement_order_keeps_multiple_moved_tickets_in_requested_order() {
    let order = final_order(
        &[
            "FIN-1".into(),
            "FIN-2".into(),
            "FIN-3".into(),
            "FIN-4".into(),
        ],
        &["FIN-4".into(), "FIN-2".into()],
        &JiraPosition::After {
            issue_key: "FIN-1".into(),
        },
    )
    .unwrap();

    assert_eq!(order, ["FIN-1", "FIN-4", "FIN-2", "FIN-3"]);
}

#[test]
fn placement_plan_can_order_an_entire_destination() {
    let plan = placement_rank_plan(
        &["FIN-3".into(), "FIN-2".into(), "FIN-1".into()],
        &["FIN-3".into(), "FIN-2".into(), "FIN-1".into()],
    )
    .unwrap()
    .unwrap();

    assert_eq!(plan.issues, ["FIN-2", "FIN-1"]);
    assert_eq!(plan.rank_after_issue.as_deref(), Some("FIN-3"));
    assert_eq!(plan.rank_before_issue, None);
}
