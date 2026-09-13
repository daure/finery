use super::{snapshot, work_item};
use crate::store::work_items::{BacklogSnapshot, RunwayCapacitySource, apply_capacity, release::*};
use chrono::NaiveDate;

fn date(value: &str) -> NaiveDate {
    parse_date(value).unwrap()
}

fn planned(points: f64) -> BacklogSnapshot {
    let mut snapshot = snapshot();
    snapshot.sprints[0].start_date = Some("2026-08-31T09:00:00Z".into());
    snapshot.sprints[0].end_date = Some("2026-09-14T09:00:00Z".into());
    snapshot.sprints[0].work_items.clear();
    let mut story = work_item("FIN-1", Some(points));
    story.fix_versions = vec!["v1.0".into()];
    story.releases = vec![ReleaseVersion {
        id: "1".into(),
        name: "v1.0".into(),
        start_date: Some(date("2026-09-14")),
        end_date: Some(date("2026-10-02")),
    }];
    snapshot.work_items = vec![story];
    apply_capacity(
        &mut snapshot,
        10.0,
        Some((3.0, false)),
        RunwayCapacitySource::JiraVelocity,
        20,
    );
    snapshot
}

fn evaluate(snapshot: &BacklogSnapshot, today: &str, complete: bool) -> ReleaseForecast {
    forecast(
        snapshot,
        &snapshot.work_items.iter().collect::<Vec<_>>(),
        "v1.0",
        complete,
        date(today),
    )
}

#[test]
fn planning_uses_fractional_workweek_capacity_and_inclusive_tolerance() {
    for (points, expected) in [
        (1.0, ReleaseStatus::Underplanned),
        (12.0, ReleaseStatus::Planned),
        (14.0, ReleaseStatus::Planned),
        (18.0, ReleaseStatus::Planned),
        (30.0, ReleaseStatus::Overplanned),
    ] {
        let snapshot = planned(points);
        let result = evaluate(&snapshot, "2026-09-12", false);
        assert_eq!(result.status, expected, "{points} points");
        assert_eq!(result.sprints, Some((points / 10.0, 1.5)));
    }
    let mut snapshot = planned(18.0);
    snapshot.runway.as_mut().unwrap().tolerance_percent = 10;
    assert_eq!(
        evaluate(&snapshot, "2026-09-12", false).status,
        ReleaseStatus::Overplanned
    );
}

#[test]
fn active_release_uses_outstanding_points_and_remaining_workdays() {
    let mut snapshot = planned(10.0);
    snapshot.work_items[0].done = true;
    let mut remaining = work_item("FIN-2", None);
    remaining.fix_versions = vec!["v1.0".into()];
    snapshot.work_items.push(remaining);
    let before_start = evaluate(&snapshot, "2026-09-13", false);
    assert_eq!(before_start.sprints, Some((1.3, 1.5)));
    assert_eq!(before_start.status, ReleaseStatus::Planned);
    let result = evaluate(&snapshot, "2026-09-28", false);
    assert_eq!(result.completed_points, 10.0);
    assert_eq!(result.total_points, 13.0);
    assert!(result.assumed);
    assert_eq!(result.sprints, Some((0.3, 0.5)));
    assert_eq!(result.status, ReleaseStatus::OnTarget);
    snapshot.work_items[1].story_points = Some(5.0);
    assert_eq!(
        evaluate(&snapshot, "2026-09-28", false).status,
        ReleaseStatus::OnTarget
    );
    snapshot.work_items[1].story_points = Some(8.0);
    assert_eq!(
        evaluate(&snapshot, "2026-09-28", false).status,
        ReleaseStatus::AtRisk
    );
    assert_eq!(
        evaluate(&snapshot, "2026-10-03", false).status,
        ReleaseStatus::MissedTarget
    );
    assert_eq!(
        evaluate(&snapshot, "2026-10-03", true).status,
        ReleaseStatus::Delivered
    );
    assert_eq!(
        evaluate(&snapshot, "2026-09-12", true).status,
        ReleaseStatus::Delivered
    );
}

#[test]
fn unavailable_inputs_never_produce_a_capacity_forecast() {
    let mut snapshot = planned(10.0);
    snapshot.work_items[0].releases[0].start_date = None;
    assert!(evaluate(&snapshot, "2026-09-12", false).sprints.is_none());
    snapshot.work_items[0].releases[0].start_date = Some(date("2026-10-03"));
    assert_eq!(
        evaluate(&snapshot, "2026-09-12", false).status,
        ReleaseStatus::Unavailable
    );
    let mut snapshot = planned(10.0);
    snapshot.sprints[0].end_date = None;
    assert_eq!(
        evaluate(&snapshot, "2026-09-12", false).status,
        ReleaseStatus::Unavailable
    );
    let mut snapshot = planned(10.0);
    snapshot.runway.as_mut().unwrap().capacity = 0.0;
    assert_eq!(
        evaluate(&snapshot, "2026-09-12", false).status,
        ReleaseStatus::Unavailable
    );
    snapshot.runway = None;
    assert_eq!(
        evaluate(&snapshot, "2026-09-14", false).status,
        ReleaseStatus::InProgress
    );
    snapshot.work_items[0].story_points = None;
    let result = evaluate(&snapshot, "2026-09-12", false);
    assert!(!result.points_known);
    assert_eq!(result.status, ReleaseStatus::Unavailable);
}

#[test]
fn ambiguous_same_name_versions_do_not_share_dates() {
    let mut snapshot = planned(10.0);
    let mut other = snapshot.work_items[0].clone();
    other.key = "OTHER-1".into();
    other.releases[0].id = "2".into();
    snapshot.work_items.push(other);
    let result = evaluate(&snapshot, "2026-09-12", false);
    assert_eq!(result.start, None);
    assert_eq!(result.status, ReleaseStatus::Unavailable);
}
