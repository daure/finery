#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkItem {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub priority: String,
    pub assignee: String,
    pub parent_key: Option<String>,
    pub parent_title: Option<String>,
    pub has_children: bool,
    pub story_points: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sprint {
    pub id: u64,
    pub name: String,
    pub state: String,
    pub work_items: Vec<WorkItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BacklogSnapshot {
    pub board_name: String,
    pub sprints: Vec<Sprint>,
    pub work_items: Vec<WorkItem>,
    pub warnings: Vec<String>,
}

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
