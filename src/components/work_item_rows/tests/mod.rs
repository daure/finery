use ratatui::style::Modifier;

use super::{
    ChangeBadge, TicketRowDetails, WorkItemKind, WorkItemRow, attachment_summary_text,
    mermaid_diagram_summary_text, story_points_label, ticket_menu_max_height, ticket_summary_text,
    work_item_title_with_key_line_with_match,
};

#[test]
fn ticket_menu_height_cap_uses_sixty_percent_of_the_viewport() {
    assert_eq!(ticket_menu_max_height(50), 30);
}

#[test]
fn mermaid_rows_capitalize_the_diagram_type() {
    tuicore::init();

    let text = mermaid_diagram_summary_text("Lifecycle", "state", false, false, false);

    assert_eq!(text.lines[0].spans[2].content, " ");
    assert_eq!(text.lines[0].spans.last().unwrap().content, "State");
}

#[test]
fn published_mermaid_rows_show_the_published_state() {
    tuicore::init();

    let text = mermaid_diagram_summary_text("Lifecycle", "state", false, true, true);

    assert_eq!(text.lines[0].spans.last().unwrap().content, " • published");
    assert!(
        text.lines[0]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(tuicore::theme().muted_fg()))
    );
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
fn case_insensitive_done_statuses_strike_through_ticket_keys() {
    tuicore::init();
    let row = WorkItemRow {
        id: "KAN-23".into(),
        key: "KAN-23".into(),
        title: "Lowercase done ticket".into(),
        kind: WorkItemKind::Story,
        priority: "Low".into(),
        status: "done".into(),
        done: crate::store::work_items::is_done_status("done"),
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
        .find(|span| span.content == "KAN-23")
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
            fix_versions: &["v0.5".into()],
            epic_name: Some("Ticket metadata"),
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
    assert!(metadata.find("In Progress") < metadata.find("AB|CD|Refinery"));
    assert!(metadata.find("Ticket metadata") < metadata.find("v0.5"));
    assert!(metadata.contains("submitted"));
    assert!(metadata.contains("FIN-100 → FIN-200"));
}

#[test]
fn long_labels_use_a_tight_chip_with_an_overflow_count_after_status() {
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
    assert!(metadata.find("To Do") < metadata.find("backlog-r…|+3"));
}

#[test]
fn attachment_rows_show_filename_date_and_size() {
    tuicore::init();

    let text = attachment_summary_text(
        crate::store::composer::AttachmentChangeKind::Synced,
        "image-20260904-161404.png",
        "2026-09-04T16:14:04.000+0000",
        21_504,
        false,
        false,
    );
    let rendered = text.lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(rendered.contains("S "));
    assert!(rendered.contains("image-20260904-161404.png"));
    assert!(rendered.contains("Sep 04, 2026"));
    assert!(rendered.contains("21 KB"));
}

#[test]
fn highlighted_attachment_rows_use_the_selected_foreground() {
    tuicore::init();

    let text = attachment_summary_text(
        crate::store::composer::AttachmentChangeKind::Synced,
        "design.png",
        "2026-09-04T16:14:04.000+0000",
        21_504,
        true,
        false,
    );

    assert!(
        text.lines[0]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(tuicore::theme().selected_fg()))
    );
}
