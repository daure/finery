use super::super::{ChangeSetFilter, ChangeSetStatus, change_set_column, rows};
use super::*;
use crate::store::composer::{
    ArchiveOutcome, ChangeKind, ChangeSet, ComposerState, SubmissionSnapshot, TicketChange,
};
use ratatui::{Terminal, backend::TestBackend};
use tuicore::{DataView, LayoutCtx, TuiNode};

#[test]
fn metadata_orders_submission_change_artifact_and_link_counts_on_one_unindented_line() {
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
        cancelled: 0,
        concluded: 0,
    };
    assert_eq!(
        summary_line(&summary).to_string(),
        "󰅙 0/10 · S 4 · A 2 · M 3 · D 1 ·  2 ·  3 · 󰖟 2 ·  1"
    );
    for span in summary_line(&summary)
        .spans
        .iter()
        .filter(|span| [" 2", " 3", "󰖟 2", " 1"].contains(&span.content.as_ref()))
    {
        assert_eq!(span.style.fg, Some(tuicore::theme().text_fg()));
    }
    assert_eq!(
        summary_line(&ChangeSetSummary::default()).to_string(),
        "󰅙 0/0"
    );
}

#[test]
fn submission_progress_distinguishes_none_partial_and_all_tickets() {
    tuicore::init();
    for (submitted, expected) in [(0, "󰅙 0/4"), (1, " 1/4"), (4, " 4/4")] {
        let summary = ChangeSetSummary {
            created: 1,
            edited: 1,
            deleted: 1,
            reference: 1,
            submitted,
            ..ChangeSetSummary::default()
        };
        assert_eq!(
            summary_line(&summary).to_string(),
            format!("{expected} · S 1 · A 1 · M 1 · D 1")
        );
    }
}

#[test]
fn change_set_rows_indent_metadata_below_the_change_set_title() {
    tuicore::init();
    let state = ComposerState::from_change_sets(vec![ChangeSet {
        id: "CS-1".into(),
        name: "Improve checkout".into(),
        tickets: Vec::new(),
        selected_ticket_ids: Vec::new(),
        closed: false,
        archive_outcome: None,
        closed_at: None,
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
        .position(|line| line.contains(" CS-1 Improve checkout"))
        .unwrap();
    assert_eq!(
        lines[title].find("").unwrap() + 2,
        lines[title + 1].find("󰅙 0/0").unwrap()
    );

    let cells = &terminal.backend().buffer().content[title * 60..(title + 1) * 60];
    assert_eq!(
        cells.iter().find(|cell| cell.symbol() == "").unwrap().fg,
        tuicore::theme().accent_fg()
    );
    assert_eq!(
        cells.iter().find(|cell| cell.symbol() == "C").unwrap().fg,
        tuicore::theme().muted_fg()
    );
}

#[test]
fn change_set_status_marks_archived_change_sets_with_a_plain_archive_icon() {
    tuicore::init();
    let mut change_set = ChangeSet {
        id: "CS-1".into(),
        name: "Improve checkout".into(),
        tickets: Vec::new(),
        selected_ticket_ids: Vec::new(),
        closed: false,
        archive_outcome: None,
        closed_at: None,
        submission_attempt: None,
    };
    assert_eq!(ChangeSetStatus::from_change_set(&change_set).icon(), "");

    change_set.archive_outcome = Some(ArchiveOutcome::Cancelled);
    assert_eq!(ChangeSetStatus::from_change_set(&change_set).icon(), "");
    assert_eq!(
        ChangeSetStatus::from_change_set(&change_set).style().fg,
        Some(tuicore::theme().text_fg())
    );

    change_set.archive_outcome = Some(ArchiveOutcome::Concluded);
    assert_eq!(ChangeSetStatus::from_change_set(&change_set).icon(), "");
    assert_eq!(
        ChangeSetStatus::from_change_set(&change_set).style().fg,
        Some(tuicore::theme().text_fg())
    );

    change_set.archive_outcome = None;
    change_set.tickets = vec![TicketChange {
        id: "FIN-1".into(),
        original: None,
        updated: None,
        kind: ChangeKind::Modified,
        submitted: Some(SubmissionSnapshot {
            original: None,
            updated: None,
            warnings: Vec::new(),
        }),
        retry_blocked: false,
        create_attempt: false,
        sibling_order: 0,
    }];
    assert_eq!(ChangeSetStatus::from_change_set(&change_set).icon(), "");
}

#[test]
fn change_sets_group_open_first_and_order_archives_by_latest_closure() {
    let base = ComposerState::demo().change_sets.remove(1);
    let set = |id: &str, closed, closed_at: Option<&str>, archive_outcome| ChangeSet {
        id: id.into(),
        closed,
        closed_at: closed_at.map(|date| date.parse().unwrap()),
        archive_outcome,
        ..base.clone()
    };
    let state = ComposerState::from_change_sets(vec![
        set("open-first", false, None, None),
        set("submitted", true, Some("2026-09-14T09:00:00Z"), None),
        set(
            "cancelled",
            true,
            Some("2026-09-14T08:00:00Z"),
            Some(ArchiveOutcome::Cancelled),
        ),
        set(
            "concluded",
            true,
            Some("2026-09-14T10:00:00Z"),
            Some(ArchiveOutcome::Concluded),
        ),
        set("legacy", true, None, None),
        set("open-last", false, None, None),
        set(
            "same-instant",
            true,
            Some("2026-09-14T12:00:00+02:00"),
            None,
        ),
    ]);
    let ids = |filter| {
        rows(&state, filter)
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(ChangeSetFilter::All),
        [
            "open-last",
            "open-first",
            "same-instant",
            "concluded",
            "submitted",
            "cancelled",
            "legacy",
        ]
    );
    assert_eq!(
        ids(ChangeSetFilter::Archived),
        [
            "same-instant",
            "concluded",
            "submitted",
            "cancelled",
            "legacy",
        ]
    );
    assert_eq!(ids(ChangeSetFilter::Open), ["open-last", "open-first"]);
}
