use super::{
    BacklogSnapshot, RunwayCapacitySource, Sprint, SprintCapacityState, WorkItem, apply_capacity,
    loaded_story_point_average,
};

fn work_item(key: &str, story_points: Option<f64>) -> WorkItem {
    WorkItem {
        key: key.into(),
        title: key.into(),
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
        story_points,
    }
}

fn snapshot() -> BacklogSnapshot {
    BacklogSnapshot {
        board_name: "Finery".into(),
        story_points_configured: true,
        sprints: vec![Sprint {
            id: 1,
            name: "Sprint 1".into(),
            state: "future".into(),
            start_date: None,
            end_date: None,
            work_items: vec![work_item("FIN-10", Some(26.0))],
            capacity: None,
        }],
        work_items: vec![
            work_item("FIN-1", Some(8.0)),
            work_item("FIN-2", Some(5.0)),
            work_item("FIN-3", Some(4.0)),
            work_item("FIN-4", None),
            work_item("FIN-5", Some(8.0)),
        ],
        warnings: Vec::new(),
        runway: None,
        velocity: None,
    }
}

#[test]
fn capacity_assigns_ranked_tickets_to_virtual_sprints_without_creating_rows() {
    let mut snapshot = snapshot();

    apply_capacity(
        &mut snapshot,
        20.0,
        Some((6.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );

    let runway = snapshot.runway.unwrap();
    assert_eq!(runway.tickets.len(), 5);
    assert_eq!(
        runway
            .tickets
            .iter()
            .map(|ticket| ticket.virtual_sprint)
            .collect::<Vec<_>>(),
        [1, 1, 1, 1, 2]
    );
    assert!(runway.tickets[3].assumed);
    assert!(!runway.tickets[3].assumed_from_average);
    assert_eq!(runway.estimated_points, 25.0);
    assert_eq!(runway.assumed_points, 6.0);
}

#[test]
fn capacity_starts_a_new_virtual_sprint_before_exceeding_its_tolerance() {
    let mut snapshot = snapshot();
    snapshot.work_items = vec![
        work_item("FIN-1", Some(3.0)),
        work_item("FIN-2", Some(5.0)),
        work_item("FIN-3", Some(8.0)),
    ];

    apply_capacity(
        &mut snapshot,
        9.1,
        Some((3.0, false)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );

    assert_eq!(
        snapshot
            .runway
            .unwrap()
            .tickets
            .iter()
            .map(|ticket| ticket.virtual_sprint)
            .collect::<Vec<_>>(),
        [1, 1, 2]
    );
}

#[test]
fn loaded_story_point_average_uses_backlog_and_sprint_tickets() {
    let snapshot = snapshot();

    assert_eq!(loaded_story_point_average(&snapshot), Some(51.0 / 5.0));
}

#[test]
fn sprint_health_marks_more_than_twenty_percent_over_capacity() {
    let mut snapshot = snapshot();

    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );

    assert_eq!(
        snapshot.sprints[0].capacity.as_ref().unwrap().state,
        SprintCapacityState::OverCommitted
    );
}

#[test]
fn sprint_health_uses_the_configured_tolerance_range() {
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items[0].story_points = Some(16.0);

    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );
    assert_eq!(
        snapshot.sprints[0].capacity.as_ref().unwrap().state,
        SprintCapacityState::OnTarget
    );

    snapshot.sprints[0].work_items[0].story_points = Some(15.0);
    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, false)),
        RunwayCapacitySource::Fixed,
        20,
    );
    assert_eq!(
        snapshot.sprints[0].capacity.as_ref().unwrap().state,
        SprintCapacityState::UnderCommitted
    );
}

#[test]
fn capacity_uses_pointed_bugs_but_never_assumes_unestimated_bugs() {
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items = vec![
        work_item("FIN-10", Some(5.0)),
        WorkItem {
            kind: "BUG".into(),
            story_points: None,
            ..work_item("FIN-11", None)
        },
        WorkItem {
            kind: "bug".into(),
            story_points: Some(2.0),
            ..work_item("FIN-12", None)
        },
    ];
    snapshot.work_items = vec![WorkItem {
        kind: "Bug".into(),
        story_points: None,
        ..work_item("FIN-1", None)
    }];

    apply_capacity(
        &mut snapshot,
        20.0,
        Some((3.0, true)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );

    let capacity = snapshot.sprints[0].capacity.as_ref().unwrap();
    assert_eq!(capacity.source, RunwayCapacitySource::JiraVelocity);
    assert_eq!(capacity.effective_points, 7.0);
    assert_eq!(capacity.assumed_points, 0.0);
    let runway_ticket = snapshot.runway.as_ref().unwrap().tickets.first().unwrap();
    assert_eq!(runway_ticket.effective_points, 0.0);
    assert!(!runway_ticket.assumed);
    assert!(!runway_ticket.assumed_from_average);
}

#[test]
fn capacity_without_an_assumption_keeps_all_bug_loads() {
    let mut snapshot = snapshot();
    snapshot.sprints[0].work_items = vec![WorkItem {
        kind: "Bug".into(),
        story_points: None,
        ..work_item("FIN-10", None)
    }];
    snapshot.work_items = vec![WorkItem {
        kind: "BUG".into(),
        story_points: None,
        ..work_item("FIN-1", None)
    }];

    apply_capacity(&mut snapshot, 20.0, None, RunwayCapacitySource::Fixed, 20);

    assert_eq!(
        snapshot.sprints[0]
            .capacity
            .as_ref()
            .unwrap()
            .effective_points,
        0.0
    );
    let runway = snapshot.runway.as_ref().unwrap();
    assert_eq!(runway.tickets[0].effective_points, 0.0);
    assert!(!runway.tickets[0].assumed);
}
