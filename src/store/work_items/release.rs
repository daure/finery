use chrono::{Datelike, NaiveDate};

use super::{BacklogSnapshot, WorkItem, is_done_status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseVersion {
    pub id: String,
    pub name: String,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseStatus {
    Underplanned,
    Planned,
    Overplanned,
    InProgress,
    OnTarget,
    AtRisk,
    Delivered,
    MissedTarget,
    Unavailable,
}

pub(crate) struct ReleaseForecast {
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub completed_points: f64,
    pub total_points: f64,
    pub assumed: bool,
    pub points_known: bool,
    pub status: ReleaseStatus,
    pub sprints: Option<(f64, f64)>,
}

pub(crate) fn forecast(
    snapshot: &BacklogSnapshot,
    items: &[&WorkItem],
    label: &str,
    complete: bool,
    today: NaiveDate,
) -> ReleaseForecast {
    let version = release_version(snapshot, label);
    let mut forecast = ReleaseForecast {
        start: version.and_then(|version| version.start_date),
        end: version.and_then(|version| version.end_date),
        completed_points: 0.0,
        total_points: 0.0,
        assumed: false,
        points_known: true,
        status: ReleaseStatus::Unavailable,
        sprints: None,
    };
    let assumed_size = snapshot
        .runway
        .as_ref()
        .and_then(|runway| runway.assumed_ticket_size);
    for item in items
        .iter()
        .filter(|item| matches!(item.kind.to_ascii_lowercase().as_str(), "story" | "task"))
    {
        let estimated = item
            .story_points
            .filter(|points| points.is_finite() && *points >= 0.0);
        let Some(points) = estimated.or(assumed_size) else {
            forecast.points_known = false;
            continue;
        };
        forecast.assumed |= estimated.is_none();
        forecast.total_points += points;
        if item.done || is_done_status(&item.status) {
            forecast.completed_points += points;
        }
    }
    if forecast.start.is_some_and(|start| today >= start)
        && forecast.end.is_some_and(|end| today <= end)
    {
        forecast.status = ReleaseStatus::InProgress;
    }
    if let (Some(start), Some(end), Some(runway), Some(sprint_days)) = (
        forecast.start,
        forecast.end,
        snapshot.runway.as_ref(),
        sprint_workdays(snapshot),
    ) && start <= end
        && forecast.points_known
        && runway.capacity.is_finite()
        && runway.capacity > 0.0
    {
        let work_points = if today < start {
            forecast.total_points
        } else {
            forecast.total_points - forecast.completed_points
        };
        let needed = work_points / runway.capacity;
        let available = workdays(today.max(start), end) as f64 / sprint_days;
        forecast.sprints = Some((needed, available));
        let tolerance = f64::from(runway.tolerance_percent) / 100.0;
        // Tolerance endpoints are inclusive despite fractional-sprint rounding noise.
        let epsilon = f64::EPSILON * needed.max(available).max(1.0) * 4.0;
        let under = needed < available * (1.0 - tolerance) - epsilon;
        let over = needed > available * (1.0 + tolerance) + epsilon;
        forecast.status = match (today < start, under, over) {
            (true, true, _) => ReleaseStatus::Underplanned,
            (true, _, true) => ReleaseStatus::Overplanned,
            (true, _, _) => ReleaseStatus::Planned,
            (false, _, true) => ReleaseStatus::AtRisk,
            (false, _, _) => ReleaseStatus::OnTarget,
        };
    }
    if complete {
        forecast.status = ReleaseStatus::Delivered;
    } else if forecast.end.is_some_and(|end| today > end) {
        forecast.status = ReleaseStatus::MissedTarget;
    }
    forecast
}

pub(crate) fn scheduled_dates(
    snapshot: &BacklogSnapshot,
    label: &str,
) -> Option<(NaiveDate, NaiveDate)> {
    let version = release_version(snapshot, label)?;
    Some((version.start_date?, version.end_date?))
}

fn release_version<'a>(snapshot: &'a BacklogSnapshot, label: &str) -> Option<&'a ReleaseVersion> {
    let mut versions = snapshot
        .sprints
        .iter()
        .flat_map(|sprint| &sprint.work_items)
        .chain(&snapshot.work_items)
        .filter(|item| item.fix_versions.iter().any(|name| name.trim() == label))
        .flat_map(|item| &item.releases)
        .filter(|version| version.name == label);
    let first = versions.next()?;
    // Same-name versions from different projects cannot define one forecast window.
    versions.all(|version| version == first).then_some(first)
}

pub(crate) fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok()
}

fn sprint_workdays(snapshot: &BacklogSnapshot) -> Option<f64> {
    let mut durations = snapshot
        .sprints
        .iter()
        .filter_map(|sprint| {
            let start = parse_date(sprint.start_date.as_deref()?)?;
            let mut end = parse_date(sprint.end_date.as_deref()?)?;
            // Same-weekday boundaries represent whole weeks (e.g. Monday to Monday).
            if end > start && end.weekday() == start.weekday() {
                end = end.pred_opt()?;
            }
            let days = workdays(start, end);
            (days > 0).then_some(days)
        })
        .collect::<Vec<_>>();
    durations.sort_unstable();
    let middle = durations.len() / 2;
    let upper = *durations.get(middle)? as f64;
    Some(if durations.len().is_multiple_of(2) {
        (durations[middle - 1] as f64 + upper) / 2.0
    } else {
        upper
    })
}

// Release date ranges include both endpoints; weekends carry no capacity.
fn workdays(start: NaiveDate, end: NaiveDate) -> i64 {
    let days = (end - start).num_days() + 1;
    if days <= 0 {
        return 0;
    }
    let whole_weeks = days / 7;
    let remaining = days % 7;
    whole_weeks * 5
        + (0..remaining)
            .filter(|offset| (i64::from(start.weekday().num_days_from_monday()) + offset) % 7 < 5)
            .count() as i64
}
