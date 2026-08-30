#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkItem {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub done: bool,
    pub priority: String,
    pub assignee: String,
    pub parent_key: Option<String>,
    pub parent_title: Option<String>,
    pub has_children: bool,
    pub subtask_progress: Option<SubtaskProgress>,
    pub fix_versions: Vec<String>,
    pub epic_name: Option<String>,
    pub story_points: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubtaskProgress {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sprint {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub goal: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub work_items: Vec<WorkItem>,
    pub capacity: Option<SprintCapacity>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BacklogSnapshot {
    pub board_name: String,
    pub story_points_configured: bool,
    pub sprints: Vec<Sprint>,
    pub work_items: Vec<WorkItem>,
    pub warnings: Vec<String>,
    pub runway: Option<BacklogRunway>,
    pub velocity: Option<VelocityReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VelocityReport {
    pub sprints: Vec<VelocitySprint>,
    pub dynamic_capacity: Option<f64>,
    pub configured_sprints: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VelocitySprint {
    pub id: u64,
    pub name: String,
    pub completed: f64,
    pub goal: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BacklogRunway {
    pub capacity: f64,
    pub source: RunwayCapacitySource,
    pub estimated_points: f64,
    pub assumed_points: f64,
    pub tickets: Vec<RunwayTicket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunwayCapacitySource {
    Fixed,
    JiraVelocity,
    FixedFallback,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RunwayTicket {
    pub key: String,
    pub virtual_sprint: usize,
    pub effective_points: f64,
    pub assumed: bool,
    pub assumed_from_average: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SprintCapacity {
    pub capacity: f64,
    pub source: RunwayCapacitySource,
    pub effective_points: f64,
    pub assumed_points: f64,
    pub assumed_ticket_size: f64,
    pub assumed_ticket_size_from_average: bool,
    pub state: SprintCapacityState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SprintCapacityState {
    UnderCommitted,
    OnTarget,
    OverCommitted,
}

pub(crate) fn loaded_story_point_average(snapshot: &BacklogSnapshot) -> Option<f64> {
    let points = snapshot
        .work_items
        .iter()
        .chain(
            snapshot
                .sprints
                .iter()
                .flat_map(|sprint| &sprint.work_items),
        )
        .filter_map(|item| item.story_points)
        .filter(|points| points.is_finite() && *points >= 0.0)
        .collect::<Vec<_>>();
    (!points.is_empty()).then(|| points.iter().sum::<f64>() / points.len() as f64)
}

pub(crate) fn apply_capacity(
    snapshot: &mut BacklogSnapshot,
    capacity: f64,
    assumed_ticket_size: Option<(f64, bool)>,
    source: RunwayCapacitySource,
    tolerance_percent: u8,
) {
    let assumed_ticket_size =
        assumed_ticket_size.filter(|(value, _)| value.is_finite() && *value >= 0.0);
    let requires_assumption = snapshot
        .work_items
        .iter()
        .chain(
            snapshot
                .sprints
                .iter()
                .flat_map(|sprint| &sprint.work_items),
        )
        .any(needs_assumption);
    if requires_assumption && assumed_ticket_size.is_none() {
        snapshot.runway = None;
        for sprint in &mut snapshot.sprints {
            sprint.capacity = None;
        }
        return;
    }
    if !capacity.is_finite() || capacity <= 0.0 {
        snapshot.runway = None;
        return;
    }
    let (assumed_ticket_size, assumed_from_average) = assumed_ticket_size.unwrap_or((0.0, false));

    let mut used_capacity = 0.0;
    let mut virtual_sprint = 1;
    let upper_limit = capacity * (1.0 + f64::from(tolerance_percent) / 100.0);
    let mut estimated_points = 0.0;
    let mut assumed_points = 0.0;
    let tickets = snapshot
        .work_items
        .iter()
        .map(|item| {
            let (effective_points, assumed) = effective_points(item, assumed_ticket_size);
            if used_capacity > 0.0 && used_capacity + effective_points > upper_limit {
                virtual_sprint += 1;
                used_capacity = 0.0;
            }
            used_capacity += effective_points;
            if assumed {
                assumed_points += effective_points;
            } else {
                estimated_points += effective_points;
            }
            RunwayTicket {
                key: item.key.clone(),
                virtual_sprint,
                effective_points,
                assumed,
                assumed_from_average: assumed && assumed_from_average,
            }
        })
        .collect();
    snapshot.runway = Some(BacklogRunway {
        capacity,
        source,
        estimated_points,
        assumed_points,
        tickets,
    });
    for sprint in &mut snapshot.sprints {
        let (effective_points, assumed_points) =
            sprint
                .work_items
                .iter()
                .fold((0.0, 0.0), |(effective_total, assumed_total), item| {
                    let (points, assumed) = effective_points(item, assumed_ticket_size);
                    (
                        effective_total + points,
                        assumed_total + if assumed { points } else { 0.0 },
                    )
                });
        let tolerance = f64::from(tolerance_percent) / 100.0;
        let lower_limit = capacity * (1.0 - tolerance);
        let upper_limit = capacity * (1.0 + tolerance);
        let state = if effective_points < lower_limit {
            SprintCapacityState::UnderCommitted
        } else if effective_points > upper_limit {
            SprintCapacityState::OverCommitted
        } else {
            SprintCapacityState::OnTarget
        };
        sprint.capacity = Some(SprintCapacity {
            capacity,
            source,
            effective_points,
            assumed_points,
            assumed_ticket_size,
            assumed_ticket_size_from_average: assumed_from_average,
            state,
        });
    }
}

fn effective_points(item: &WorkItem, assumed_ticket_size: f64) -> (f64, bool) {
    item.story_points
        .filter(|points| points.is_finite() && *points >= 0.0)
        .map(|points| (points, false))
        .unwrap_or_else(|| {
            if item.kind.eq_ignore_ascii_case("bug") {
                (0.0, false)
            } else {
                (assumed_ticket_size, true)
            }
        })
}

fn needs_assumption(item: &WorkItem) -> bool {
    !item.kind.eq_ignore_ascii_case("bug")
        && item
            .story_points
            .filter(|points| points.is_finite() && *points >= 0.0)
            .is_none()
}

#[cfg(test)]
mod tests;

pub(crate) const MAX_RANK_ISSUES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RankPlan {
    pub issues: Vec<String>,
    pub rank_before_issue: Option<String>,
    pub rank_after_issue: Option<String>,
}

pub(crate) fn rank_plan(
    issues: Vec<String>,
    final_order: &[String],
) -> Result<Option<RankPlan>, String> {
    if issues.len() > MAX_RANK_ISSUES {
        return Err(format!(
            "Jira can rank at most {MAX_RANK_ISSUES} issues at once"
        ));
    }
    if issues.is_empty() {
        return Ok(None);
    }

    let moved = issues.iter().collect::<std::collections::HashSet<_>>();
    let first = final_order.iter().position(|key| moved.contains(key));
    let last = final_order.iter().rposition(|key| moved.contains(key));
    let (Some(first), Some(last)) = (first, last) else {
        return Ok(None);
    };
    let rank_before_issue = final_order[last.saturating_add(1)..]
        .iter()
        .find(|key| !moved.contains(key))
        .cloned();
    let rank_after_issue = if rank_before_issue.is_none() {
        final_order[..first]
            .iter()
            .rev()
            .find(|key| !moved.contains(key))
            .cloned()
    } else {
        None
    };
    if rank_before_issue.is_none() && rank_after_issue.is_none() {
        return Ok(None);
    }
    Ok(Some(RankPlan {
        issues,
        rank_before_issue,
        rank_after_issue,
    }))
}
