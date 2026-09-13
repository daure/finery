use super::super::{ChangeSetFilter, change_set_column, rows};
use super::*;
use crate::store::composer::{ChangeSet, ComposerState};
use ratatui::{Terminal, backend::TestBackend};
use tuicore::{DataView, LayoutCtx, TuiNode};

#[test]
fn metadata_orders_change_artifact_link_and_submission_counts_on_one_unindented_line() {
    tuicore::init();
    let summary = ChangeSetSummary {
        created: 2,
        edited: 3,
        deleted: 1,
        reference: 4,
        diagrams: 2,
        uploads: 3,
        web_links: 2,
        external_links: 1,
        submitted: 0,
    };
    assert_eq!(
        summary_line(&summary).to_string(),
        " 2 ·  3 ·  1 ·  4 ·  2 ·  3 · 󰖟 2 ·  1 ·  0"
    );
    assert_eq!(
        summary_line(&ChangeSetSummary::default()).to_string(),
        " 0"
    );
}

#[test]
fn change_set_rows_render_as_two_lines_with_aligned_metadata() {
    tuicore::init();
    let state = ComposerState::from_change_sets(vec![ChangeSet {
        id: "CS-1".into(),
        name: "Improve checkout".into(),
        tickets: Vec::new(),
        selected_ticket_ids: Vec::new(),
        closed: false,
        submission_attempt: None,
    }]);
    let mut view = DataView::new(rows(&state, ChangeSetFilter::Open), |row| row.id.clone())
        .column(change_set_column())
        .headers(false)
        .row_height(2);
    let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            <DataView<_, _> as TuiNode<()>>::layout(&mut view, area, &mut LayoutCtx::new());
            view.render(frame, area);
        })
        .unwrap();
    let lines = terminal
        .backend()
        .buffer()
        .content
        .chunks(60)
        .map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let title = lines
        .iter()
        .position(|line| line.contains("CS-1 · Improve checkout"))
        .unwrap();
    assert_eq!(lines[title].find("CS-1"), lines[title + 1].find(" 0"));
}
