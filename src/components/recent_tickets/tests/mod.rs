use super::recent_ticket_row;
use crate::store::work_items::WorkItem;

#[test]
fn unestimated_tasks_and_stories_use_the_assumed_story_points() {
    for kind in ["Task", "Story"] {
        let row = recent_ticket_row(work_item(kind), true, 3.0, false);
        assert_eq!(row.item.story_points, Some(3.0));
        assert!(row.item.story_points_estimated);
    }
}

#[test]
fn unestimated_non_estimated_ticket_types_keep_the_dash_placeholder() {
    for kind in ["Bug", "Epic", "Sub-task"] {
        let row = recent_ticket_row(work_item(kind), true, 3.0, false);
        assert_eq!(row.item.story_points, None);
        assert!(!row.item.story_points_estimated);
    }
}

fn work_item(kind: &str) -> WorkItem {
    WorkItem {
        key: "FIN-1".into(),
        title: "Example".into(),
        kind: kind.into(),
        status: "To Do".into(),
        priority: String::new(),
        assignee: "Unassigned".into(),
        parent_key: None,
        parent_title: None,
        has_children: false,
        subtask_progress: None,
        fix_versions: Vec::new(),
        epic_name: None,
        story_points: None,
    }
}
