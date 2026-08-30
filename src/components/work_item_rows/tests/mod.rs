use ratatui::style::Modifier;

use super::{
    WorkItemKind, WorkItemRow, story_points_label, work_item_title_with_key_line_with_match,
};

#[test]
fn search_matches_are_underlined_in_the_shared_ticket_title_template() {
    tuicore::init();
    let row = WorkItemRow {
        id: "KAN-2".into(),
        key: "KAN-2".into(),
        title: "Legacy in-progress placeholder".into(),
        kind: WorkItemKind::Story,
        priority: "Low".into(),
        status: "In Progress".into(),
        done: false,
        assignee: "Marlo".into(),
        story_points: None,
        show_story_points: false,
        story_points_estimated: false,
        story_points_from_average: false,
        change_badge: None,
        submitted: false,
    };

    let line = work_item_title_with_key_line_with_match(&row, None, Some("legacy"));
    let matched = line
        .spans
        .iter()
        .find(|span| span.content == "Legacy")
        .expect("the matching title text is rendered separately");

    assert!(matched.style.add_modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn done_ticket_keys_are_struck_through_in_shared_ticket_rows() {
    tuicore::init();
    let row = WorkItemRow {
        id: "KAN-22".into(),
        key: "KAN-22".into(),
        title: "Completed ticket".into(),
        kind: WorkItemKind::Story,
        priority: "Low".into(),
        status: "Done".into(),
        done: true,
        assignee: "Marlo".into(),
        story_points: None,
        show_story_points: false,
        story_points_estimated: false,
        story_points_from_average: false,
        change_badge: None,
        submitted: false,
    };

    let line = work_item_title_with_key_line_with_match(&row, None, None);
    let key = line
        .spans
        .iter()
        .find(|span| span.content == "KAN-22")
        .expect("the ticket key is rendered separately");

    assert!(key.style.add_modifier.contains(Modifier::CROSSED_OUT));
}

#[test]
fn average_derived_story_points_show_one_decimal_place() {
    let row = WorkItemRow {
        id: "KAN-22".into(),
        key: "KAN-22".into(),
        title: "Estimated ticket".into(),
        kind: WorkItemKind::Story,
        priority: "Low".into(),
        status: "To Do".into(),
        done: false,
        assignee: "Marlo".into(),
        story_points: Some(3.932_423_532),
        show_story_points: true,
        story_points_estimated: true,
        story_points_from_average: true,
        change_badge: None,
        submitted: false,
    };

    assert_eq!(story_points_label(&row), "~3.9");
}
