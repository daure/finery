use super::*;
use crate::store::composer::ArchiveOutcome;

fn press(page: &mut ComposerPage, key: KeyEvent) {
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, TEST_WIDTH, 40), &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .rev()
        .find(|target| !target.area.is_empty())
        .unwrap()
        .clone();
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    page.dispatch_event(
        &EventRoute::new(target.path),
        &TuiEvent::Key(key),
        &mut EventCtx::default(),
    );
}

fn archive_dialog(page: &mut ComposerPage) {
    let list = focus(page, "data-view");
    let mut ctx = EventCtx::default();
    assert_eq!(
        page.dispatch_event(
            &EventRoute::new(list.path),
            &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
            &mut ctx,
        ),
        EventOutcome::Handled
    );
}

fn archived_filter(page: &mut ComposerPage) {
    let target = filter_button(page);
    page.dispatch_focus(&target, true, &mut FocusCtx::default());
    let route = EventRoute::new(target.path);
    page.dispatch_event(
        &route,
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+f".into())),
        &mut EventCtx::default(),
    );
    let text = render_text(page);
    assert!(text.contains("Archived"), "{text}");
    let area = Rect::new(0, 0, TEST_WIDTH, 40);
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(area, |ctx| page.layout(area, ctx));
    let popup = layout.overlays().last().unwrap().route_path.clone();
    let route = EventRoute::new(popup);
    for key in "Archived".chars().map(Key::Char).chain([Key::Enter]) {
        page.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(key)),
            &mut EventCtx::default(),
        );
    }
}

#[test]
fn overview_archives_remaining_items_and_preserves_submissions_after_reload() {
    tuicore::init();
    for (key, outcome) in [
        ('d', ArchiveOutcome::Concluded),
        ('r', ArchiveOutcome::Cancelled),
    ] {
        for submitted_count in [0, 1] {
            let mut state = ComposerState::demo();
            state.change_sets.truncate(1);
            let original = &mut state.change_sets[0];
            for change in original.tickets.iter_mut().take(submitted_count) {
                change.submitted = Some(SubmissionSnapshot {
                    original: change.original.clone(),
                    updated: change.updated.clone(),
                    warnings: Vec::new(),
                });
            }
            let tickets = original.tickets.clone();
            let service = AppService::for_tests();
            service.save_change_set(original.clone());
            service.flush().unwrap();
            let mut page =
                ComposerPage::new(state.change_sets, service.clone(), service.settings());
            page.init(&mut LifecycleCtx::default());
            archive_dialog(&mut page);
            let dialog = render_text(&mut page);
            for label in ["Done", "Reject", "Cancel"] {
                assert!(dialog.contains(label));
            }
            press(&mut page, KeyEvent::from(Key::Char(key)));
            service.flush().unwrap();
            let saved = service.change_set_for_tests("CS-1").unwrap();
            assert!(saved.closed);
            assert_eq!(saved.archive_outcome, Some(outcome));
            assert!(saved.closed_at.is_some());
            assert_eq!(saved.tickets, tickets);
            assert!(saved.selected_ticket_ids.is_empty());
            let canonical = service.composer_service().change_set("CS-1").unwrap();
            assert_eq!(canonical.value.archive_outcome, Some(outcome.into()));
            assert_eq!(
                canonical.value.closed_at,
                saved.closed_at.map(|date| date.to_rfc3339())
            );
            assert_eq!(
                canonical
                    .value
                    .tickets
                    .iter()
                    .filter(|ticket| ticket.submitted)
                    .count(),
                submitted_count
            );
            assert!(
                service
                    .composer_service()
                    .change_set_catalog(false)
                    .unwrap()
                    .value
                    .change_sets
                    .is_empty()
            );
            assert_eq!(
                service
                    .composer_service()
                    .change_set_catalog(true)
                    .unwrap()
                    .value
                    .change_sets
                    .len(),
                1
            );
            assert!(!render_text(&mut page).contains("Checkout reliability"));

            let mut page = composer_page_with_change_sets(vec![saved]);
            archived_filter(&mut page);
            let overview = render_text(&mut page);
            assert!(overview.contains("Checkout reliability"));
            assert!(overview.contains(outcome.label()));
            open_change_set(&mut page, 0);
            let text = render_text_at(&mut page, 180);
            assert_eq!(text.matches(outcome.label()).count(), 3 - submitted_count);
            assert_eq!(text.matches("󱋭").count(), 3);
            assert_eq!(text.matches("Submitted").count(), submitted_count);
        }
    }
}

#[test]
fn cancelling_or_dismissing_archive_dialog_keeps_the_set_open() {
    tuicore::init();
    for key in [Key::Char('c'), Key::Esc] {
        let mut page = composer_page();
        archive_dialog(&mut page);
        assert!(render_text(&mut page).contains("Complete change set?"));
        let dialog = render_text(&mut page);
        assert!(dialog.contains("Choose Done to complete or Reject to close."));
        press(&mut page, KeyEvent::from(key));
        let text = render_text(&mut page);
        assert!(!text.contains("Complete change set?"));
        assert!(text.contains("Customer notifications"));
    }
}

#[test]
fn archived_filter_includes_fully_submitted_sets() {
    tuicore::init();
    let mut state = ComposerState::demo();
    state
        .dispatch(ComposerAction::OpenChangeSet("CS-1".into()))
        .unwrap();
    for change in state.active_set().unwrap().tickets.clone() {
        state
            .dispatch(ComposerAction::CompleteSubmission {
                change_set_id: "CS-1".into(),
                id: change.id,
                snapshot: SubmissionSnapshot {
                    original: change.original,
                    updated: change.updated,
                    warnings: Vec::new(),
                },
            })
            .unwrap();
    }
    let mut page = composer_page_with_change_sets(state.change_sets);
    assert!(!render_text(&mut page).contains("Checkout reliability"));
    archived_filter(&mut page);
    let text = render_text(&mut page);
    assert!(text.contains("Checkout reliability"));
    assert!(!text.contains("Customer notifications"));
}

#[test]
fn archive_shortcuts_round_trip_and_use_configured_keys() {
    tuicore::init();
    let values = std::collections::HashMap::from([
        ("composer.archive_key".into(), "ctrl+e".into()),
        ("composer.archive_done_key".into(), "x".into()),
    ]);
    let configured = AppSettings::resolve(&values).unwrap();
    let persisted = configured
        .values()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    let restored = AppSettings::resolve(&persisted).unwrap();
    assert_eq!(restored.composer_keys, configured.composer_keys);
    let service = AppService::for_tests();
    *service.settings().write().unwrap() = restored;
    let set = ComposerState::demo().change_sets.remove(0);
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let mut page = ComposerPage::new(vec![set], service.clone(), service.settings());
    page.init(&mut LifecycleCtx::default());
    let list = focus(&mut page, "data-view");
    page.dispatch_event(
        &EventRoute::new(list.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("ctrl+e".into())),
        &mut EventCtx::default(),
    );
    assert!(render_text(&mut page).contains("Done (x)"));
    press(&mut page, KeyEvent::from(Key::Char('x')));
    service.flush().unwrap();
    assert_eq!(
        service
            .change_set_for_tests("CS-1")
            .unwrap()
            .archive_outcome,
        Some(ArchiveOutcome::Concluded)
    );

    let mut conflicting = values;
    conflicting.insert("composer.archive_done_key".into(), "c".into());
    assert!(AppSettings::resolve(&conflicting).is_err());
}

#[test]
fn archive_confirmation_cannot_overwrite_an_external_edit() {
    tuicore::init();
    let service = AppService::for_tests();
    let set = ComposerState::demo().change_sets.remove(0);
    service.save_change_set(set.clone());
    service.flush().unwrap();
    let mut page = ComposerPage::new(vec![set], service.clone(), service.settings());
    page.init(&mut LifecycleCtx::default());
    archive_dialog(&mut page);
    service
        .composer_service()
        .apply_change_set_patch(
            "CS-1",
            1,
            vec![ChangeSetPatchOperation::UpdateTitle {
                ticket_id: "FIN-142".into(),
                title: "Preserve basket on retry".into(),
            }],
        )
        .unwrap();
    press(&mut page, KeyEvent::from(Key::Char('d')));
    assert!(service.flush().is_err());
    let saved = service.change_set_for_tests("CS-1").unwrap();
    assert!(!saved.closed);
    assert_eq!(saved.archive_outcome, None);
    assert_eq!(
        saved.tickets[0].updated.as_ref().unwrap().title,
        "Preserve basket on retry"
    );
    for _ in 0..20 {
        page.tick(Duration::from_millis(500), AnimationSettings::default());
        if render_text(&mut page).contains("Checkout reliability") {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("conflicting archive must reload the open change set");
}
