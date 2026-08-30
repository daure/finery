use ratatui::style::Modifier;

use super::{WorkItemKind, WorkItemRow, work_item_title_with_key_line_with_match};

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
