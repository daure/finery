use super::*;
use tuicore::{Flex, FlexItem, FocusManager, HotkeyEvent, TreeDispatcher};

const AREA: Rect = Rect::new(0, 0, 100, 40);

#[test]
fn manager_lists_size_to_one_through_six_value_rows_and_shrink_after_removal() {
    tuicore::init();
    let area = Rect::new(0, 0, 180, 100);
    for count in [0usize, 1, 2, 6, 7] {
        let values = (0..count)
            .map(|index| format!("Value {index}"))
            .collect::<Vec<_>>();
        let filter = SavedBacklogFilter {
            id: 1,
            name: "Sizing".into(),
            criteria: BacklogFilterCriteria {
                issue_types: values.clone(),
                users: values.clone(),
                statuses: values.clone(),
                epics: values.clone(),
                labels: values.clone(),
                releases: values.clone(),
                ..Default::default()
            },
        };
        let options = BacklogFilterOptions {
            issue_types: values.clone(),
            users: values.clone(),
            statuses: values.clone(),
            epics: values.clone(),
            labels: values.clone(),
            releases: values,
        };
        let (sender, _) = mpsc::channel();
        let mut manager = saved_filter_dialog(
            &[filter],
            Some(1),
            &options,
            sender,
            Rc::new(Cell::new(false)),
            None,
        );
        let mut layout = LayoutCtx::new();
        manager.layout(area, &mut layout);
        for slot in [
            "issue-types",
            "users",
            "statuses",
            "epics",
            "labels",
            "releases",
        ] {
            let list = layout
                .focus_targets()
                .iter()
                .find(|target| {
                    target.id.as_str() == "data-view"
                        && target.path.keys().iter().any(|key| key.as_str() == slot)
                })
                .unwrap();
            assert_eq!(
                list.area.height,
                count.clamp(1, 6) as u16 + 1,
                "{slot}: {count} values plus search row"
            );
        }
        if count == 7 {
            let route = layout
                .focus_targets()
                .iter()
                .find(|target| {
                    target.id.as_str() == "data-view"
                        && target
                            .path
                            .keys()
                            .iter()
                            .any(|key| key.as_str() == "labels")
                })
                .unwrap()
                .path
                .clone();
            for expected_rows in [6, 5] {
                manager.dispatch_event(
                    &EventRoute::new(route.clone()),
                    &TuiEvent::Key(KeyEvent::from(Key::Char('-'))),
                    &mut EventCtx::default(),
                );
                let mut layout = LayoutCtx::new();
                manager.layout(area, &mut layout);
                let list = layout
                    .focus_targets()
                    .iter()
                    .find(|target| target.path == route && target.id.as_str() == "data-view")
                    .unwrap();
                assert_eq!(list.area.height, expected_rows + 1);
            }
        }
    }
}

fn settings() -> AnimationSettings {
    AnimationSettings {
        enabled: false,
        ..AnimationSettings::default()
    }
}

fn layout(root: &mut impl TuiNode) -> LayoutCtx {
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(AREA, |ctx| root.layout(AREA, ctx));
    layout
}

fn main_page_text(root: &impl TuiNode) -> String {
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            root.render(frame, AREA, &mut render);
            render.flush(frame);
        })
        .unwrap();
    rendered_lines(&terminal, Rect::new(0, 0, 60, AREA.height)).join("\n")
}

fn open_manager() -> (Flex<()>, FocusRequest) {
    tuicore::init();
    let mut root = Flex::column()
        .child(
            "backlog-page",
            BacklogPage::with_snapshot_for_test(snapshot()),
            FlexItem::fill(1),
        )
        .child(
            "status",
            tuicore::Paragraph::new("status".repeat(20)),
            FlexItem::fixed(1),
        );
    let targets = layout(&mut root);
    let filter = targets
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+f"])
        .unwrap();
    let route = EventRoute::new(filter.path.clone());
    root.dispatch_focus(filter, true, &mut FocusCtx::new(settings()));
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_event(
        &mut root,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    layout(&mut root);
    dispatcher.dispatch_event(
        &mut root,
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        settings(),
    );
    let open = dispatcher.dispatch_event(
        &mut root,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    (root, open.focus_request.expect("manager requests focus"))
}

#[test]
fn manager_dock_covers_the_status_bar_through_the_screen_bottom() {
    let (mut root, _) = open_manager();
    for area in [AREA, Rect::new(0, 0, 80, 32)] {
        let mut layout = LayoutCtx::new();
        layout.with_overlay_bounds(area, |ctx| root.layout(area, ctx));
        let dialog = layout
            .focus_targets()
            .iter()
            .find(|target| target.enabled && target.id.as_str() == "dialog")
            .unwrap();
        assert_eq!(dialog.area.bottom(), area.bottom());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| {
                let mut render = RenderCtx::new();
                root.render(frame, area, &mut render);
                render.flush(frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, area.bottom() - 1)].symbol(), "s");
        let covered_footer = (dialog.area.x..area.right())
            .map(|x| buffer[(x, area.bottom() - 1)].symbol())
            .collect::<String>();
        assert!(!covered_footer.contains("status"), "{covered_footer}");
    }
}

#[test]
fn nested_manager_open_focuses_the_selector_and_enter_opens_it() {
    let (mut root, request) = open_manager();
    let targets = layout(&mut root);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(&request, targets.focus_targets())
        .expect("manager focus must resolve within the nested page");
    let selector = focus.current().unwrap().clone();
    assert_eq!(selector.id.as_str(), "field");
    assert_eq!(selector.path.keys().last().unwrap().as_str(), "selector");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut root, transition, settings());
    dispatcher.dispatch_event(
        &mut root,
        &EventRoute::new(selector.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    let targets = layout(&mut root);
    assert!(
        targets
            .focus_targets()
            .iter()
            .any(|target| target.path == selector.path && target.id.as_str() == "input")
    );
}

#[test]
fn nested_manager_new_hotkey_focuses_the_name_in_insert_mode() {
    let (mut root, _) = open_manager();
    let targets = layout(&mut root);
    let new = targets
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+n"])
        .unwrap();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: new.path.clone(),
                id: new.id.clone(),
            },
            targets.focus_targets(),
        )
        .unwrap();
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut root, transition, settings());
    let create = dispatcher.dispatch_event(
        &mut root,
        &EventRoute::new(new.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit("shift+n".into())),
        settings(),
    );
    let targets = layout(&mut root);
    let transition = focus
        .apply_request(
            create.focus_request.as_ref().unwrap(),
            targets.focus_targets(),
        )
        .expect("new filter name focus must resolve within the nested page");
    let name = focus.current().unwrap().clone();
    assert_eq!(name.id.as_str(), "input");
    assert_eq!(name.path.keys().last().unwrap().as_str(), "name");
    dispatcher.dispatch_focus(&mut root, transition, settings());
    dispatcher.dispatch_event(
        &mut root,
        &EventRoute::new(name.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('q'))),
        settings(),
    );
    layout(&mut root);
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            root.render(frame, AREA, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(name.area.x, name.area.y)].symbol(), "q");
    assert_eq!(
        buffer[(name.area.x - 1, name.area.y - 1)].fg,
        tuicore::theme().accent_fg(),
    );

    dispatcher.dispatch_event(
        &mut root,
        &EventRoute::new(name.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    layout(&mut root);
    assert!(
        main_page_text(&root).contains('q'),
        "new filter must be selected on the backlog"
    );
}

#[test]
fn saved_filter_dock_preserves_the_backdrop_styles() {
    tuicore::init();
    let (sender, _) = mpsc::channel();
    let mut manager = saved_filter_dialog(
        &[],
        None,
        &BacklogFilterOptions::default(),
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    layout(&mut manager);
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    let style = ratatui::style::Style::default()
        .fg(tuicore::theme().error_fg())
        .bg(tuicore::theme().surface_bg())
        .add_modifier(Modifier::DIM);
    terminal
        .draw(|frame| {
            frame.buffer_mut().set_string(4, 4, "Backlog", style);
            let mut render = RenderCtx::new();
            manager.render(frame, AREA, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let cell = &terminal.backend().buffer()[(4, 4)];
    assert_eq!(cell.symbol(), "B");
    assert_eq!(cell.fg, style.fg.unwrap());
    assert_eq!(cell.bg, style.bg.unwrap());
    assert!(cell.modifier.contains(Modifier::DIM));
}

#[test]
fn choosing_none_clears_the_saved_filter_and_restores_unfiltered_tickets() {
    tuicore::init();
    let service = AppService::for_tests();
    let mut filter = SavedBacklogFilter::new(1, "Hidden tickets");
    filter.criteria.users = vec!["Nobody".into()];
    filter.criteria.estimated = false;
    service.settings().write().unwrap().saved_backlog_filters = vec![filter.clone()];
    let mut page = BacklogPage::with_snapshot_and_service_for_test(snapshot(), service.clone());
    let targets = layout(&mut page);
    let dropdown = targets
        .focus_targets()
        .iter()
        .find(|target| target.hotkey_sequences == ["shift+f"])
        .unwrap();
    let route = EventRoute::new(dropdown.path.clone());
    page.dispatch_focus(dropdown, true, &mut FocusCtx::new(settings()));
    let mut dispatcher = TreeDispatcher::new();
    for key in [
        KeyEvent::from(Key::Enter),
        KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::Enter),
    ] {
        dispatcher.dispatch_event(&mut page, &route, &TuiEvent::Key(key), settings());
        layout(&mut page);
    }
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    let draw = |page: &BacklogPage, terminal: &mut Terminal<TestBackend>| {
        terminal
            .draw(|frame| {
                let mut render = RenderCtx::new();
                page.render(frame, AREA, &mut render);
                render.flush(frame);
            })
            .unwrap();
    };
    draw(&page, &mut terminal);
    let text = rendered_lines(&terminal, AREA).join("\n");
    assert!(!text.contains("Plan next sprint"), "{text}");

    dispatcher.dispatch_event(
        &mut page,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    layout(&mut page);
    draw(&page, &mut terminal);
    let lines = rendered_lines(&terminal, AREA);
    let none_y = lines
        .iter()
        .position(|line| line.contains("None"))
        .unwrap_or_else(|| panic!("None option missing:\n{}", lines.join("\n")));
    let filter_y = lines
        .iter()
        .rposition(|line| line.contains("Hidden tickets"))
        .unwrap();
    assert!(none_y < filter_y);
    let none_x = cell_position(&lines[none_y], "None").unwrap();
    assert_eq!(
        terminal.backend().buffer()[(none_x as u16, none_y as u16)].fg,
        tuicore::theme().muted_fg(),
    );

    for key in [
        KeyEvent {
            code: Key::Char('k'),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::Enter),
    ] {
        dispatcher.dispatch_event(&mut page, &route, &TuiEvent::Key(key), settings());
        layout(&mut page);
    }
    draw(&page, &mut terminal);
    let text = rendered_lines(&terminal, AREA).join("\n");
    assert!(text.contains("Plan next sprint"));
    assert!(!text.contains("Hidden tickets"));
    assert_eq!(
        service.settings().read().unwrap().saved_backlog_filters,
        [filter]
    );
}

#[test]
fn sidebar_selection_and_edits_apply_to_the_backlog() {
    tuicore::init();
    let service = AppService::for_tests();
    let all = SavedBacklogFilter::new(1, "All tickets");
    let mut unestimated = SavedBacklogFilter::new(2, "Unestimated");
    unestimated.criteria.estimated = false;
    service.settings().write().unwrap().saved_backlog_filters = vec![all, unestimated];
    let mut snapshot = snapshot();
    snapshot.work_items[0].story_points = Some(5.0);
    let mut page = BacklogPage::with_snapshot_and_service_for_test(snapshot, service);
    layout(&mut page);
    page.open_saved_filter_manager_for_test(&mut EventCtx::new(settings()));
    let targets = layout(&mut page);
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            page.render(frame, AREA, &mut render);
            render.flush(frame);
        })
        .unwrap();
    let text = rendered_lines(&terminal, AREA).join("\n");
    assert!(text.contains("Select filter"), "{text}");
    assert!(main_page_text(&page).contains("FIN-8"));
    let selector = targets
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == "selector")
        })
        .unwrap();
    let route = EventRoute::new(selector.path.clone());
    page.dispatch_focus(selector, true, &mut FocusCtx::new(settings()));
    let mut dispatcher = TreeDispatcher::new();
    for key in [
        KeyEvent::from(Key::Enter),
        KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::Enter),
    ] {
        dispatcher.dispatch_event(&mut page, &route, &TuiEvent::Key(key), settings());
        layout(&mut page);
    }
    let text = main_page_text(&page);
    assert!(text.contains("Unestimated"), "{text}");
    assert!(!text.contains("FIN-8"), "{text}");

    let targets = layout(&mut page);
    let estimated = targets
        .focus_targets()
        .iter()
        .find(|target| {
            target.enabled
                && target
                    .path
                    .keys()
                    .last()
                    .is_some_and(|key| key.as_str() == "estimated")
        })
        .unwrap();
    dispatcher.dispatch_event(
        &mut page,
        &EventRoute::new(estimated.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        settings(),
    );
    layout(&mut page);
    let text = main_page_text(&page);
    assert!(text.contains("Unestimated"), "{text}");
    assert!(text.contains("FIN-8"), "{text}");
}
