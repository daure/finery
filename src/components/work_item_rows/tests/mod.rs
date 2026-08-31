use ratatui::style::Modifier;

use super::{
    ChangeBadge, TicketRowDetails, WorkItemKind, WorkItemRow, story_points_label,
    ticket_menu_max_height, ticket_summary_text, work_item_title_with_key_line_with_match,
};

#[test]
fn ticket_menu_height_cap_uses_sixty_percent_of_the_viewport() {
    assert_eq!(ticket_menu_max_height(50), 30);
}

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
        labels: Vec::new(),
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
        labels: Vec::new(),
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
        labels: Vec::new(),
        story_points: Some(3.932_423_532),
        show_story_points: true,
        story_points_estimated: true,
        story_points_from_average: true,
        change_badge: None,
        submitted: false,
    };

    assert_eq!(story_points_label(&row), "~3.9");
}

#[test]
fn ticket_annotations_extend_composer_metadata_without_hiding_change_state() {
    tuicore::init();
    let row = WorkItemRow {
        id: "FIN-123".into(),
        key: "FIN-123".into(),
        title: "Move ticket".into(),
        kind: WorkItemKind::Story,
        priority: "High".into(),
        status: "In Progress".into(),
        done: false,
        assignee: "Marlo".into(),
        labels: vec!["AB".into(), "CD".into(), "Refinery".into()],
        story_points: None,
        show_story_points: false,
        story_points_estimated: false,
        story_points_from_average: false,
        change_badge: Some(ChangeBadge::Modified),
        submitted: true,
    };

    let text = ticket_summary_text(
        &row,
        None,
        None,
        TicketRowDetails {
            subtask_progress: None,
            fix_versions: &[],
            epic_name: None,
            annotation: Some("FIN-100 → FIN-200"),
        },
    );
    let metadata = text.lines[1]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(metadata.contains("M"));
    assert!(metadata.contains("AB|CD|Refinery"));
    assert!(metadata.find("AB|CD|Refinery") < metadata.find("In Progress"));
    assert!(metadata.contains("submitted"));
    assert!(metadata.contains("FIN-100 → FIN-200"));
}

#[test]
fn long_labels_use_a_tight_chip_with_an_overflow_count_before_status() {
    tuicore::init();
    let row = WorkItemRow {
        id: "FIN-123".into(),
        key: "FIN-123".into(),
        title: "Keep labels compact".into(),
        kind: WorkItemKind::Story,
        priority: "High".into(),
        status: "To Do".into(),
        done: false,
        assignee: "Marlo".into(),
        labels: vec![
            "backlog-rank-03".into(),
            "finery-runway-seed-v1".into(),
            "northstar-commerce".into(),
            "seed-backlog".into(),
        ],
        story_points: None,
        show_story_points: false,
        story_points_estimated: false,
        story_points_from_average: false,
        change_badge: None,
        submitted: false,
    };

    let text = ticket_summary_text(
        &row,
        None,
        None,
        TicketRowDetails {
            subtask_progress: None,
            fix_versions: &[],
            epic_name: None,
            annotation: None,
        },
    );
    let metadata = text.lines[1]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(metadata.contains("backlog-r…|+3"));
    assert!(metadata.find("backlog-r…|+3") < metadata.find("To Do"));
}
