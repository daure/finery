use super::*;

const AREA: Rect = Rect::new(0, 0, 180, 46);

fn ctrl(code: Key) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::CONTROL,
    }
}

fn render(node: &mut impl TuiNode) -> Terminal<TestBackend> {
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(AREA, |ctx| node.layout(AREA, ctx));
    let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            node.render(frame, AREA, &mut render);
            render.flush(frame);
        })
        .unwrap();
    terminal
}

fn missing_snapshot() -> BacklogSnapshot {
    let mut snapshot = snapshot();
    snapshot.sprints.clear();
    snapshot.work_items = vec![
        WorkItem {
            assignee: "Unassigned".into(),
            ..work_item("FIN-8", "Missing values")
        },
        WorkItem {
            assignee: " ".into(),
            epic_name: Some(" ".into()),
            labels: vec![" ".into()],
            fix_versions: vec![" ".into()],
            ..work_item("FIN-9", "Blank values")
        },
        WorkItem {
            assignee: "Ada".into(),
            epic_name: Some("No epic".into()),
            labels: vec!["ready".into()],
            fix_versions: vec!["v1.0".into()],
            ..work_item("FIN-10", "Selected values")
        },
        WorkItem {
            assignee: "Grace".into(),
            epic_name: Some("Platform".into()),
            labels: vec!["backend".into()],
            fix_versions: vec!["v2.0".into()],
            ..work_item("FIN-11", "Other values")
        },
    ];
    snapshot
}

#[test]
fn missing_filters_combine_with_values_and_clearing_restores_all_tickets() {
    tuicore::init();
    for (value, apply) in [
        (
            "Ada",
            BacklogTree::set_users_filter as fn(&mut BacklogTree, Vec<String>),
        ),
        ("No epic", BacklogTree::set_epics_filter),
        ("ready", BacklogTree::set_labels_filter),
        ("v1.0", BacklogTree::set_releases_filter),
    ] {
        let (sender, _) = mpsc::channel();
        let mut tree = backlog_tree(&missing_snapshot(), sender, Default::default());
        for (selection, expected) in [
            (vec![String::new()], vec!["FIN-8", "FIN-9"]),
            (
                vec![String::new(), value.into()],
                vec!["FIN-8", "FIN-9", "FIN-10"],
            ),
            (vec![value.into()], vec!["FIN-10"]),
            (Vec::new(), vec!["FIN-8", "FIN-9", "FIN-10", "FIN-11"]),
        ] {
            apply(&mut tree, selection);
            let text = rendered_lines(&render(&mut tree), AREA).join("\n");
            for key in ["FIN-8", "FIN-9", "FIN-10", "FIN-11"] {
                assert_eq!(
                    text.contains(key),
                    expected.contains(&key),
                    "{value}: {text}"
                );
            }
        }
    }
}

#[test]
fn saved_missing_filters_use_and_across_fields_and_accept_legacy_unassigned() {
    tuicore::init();
    let mut snapshot = missing_snapshot();
    snapshot.work_items[1].fix_versions = vec!["v1.0".into()];
    let mut filter = SavedBacklogFilter::new(1, "Needs triage");
    filter.criteria.users = vec!["Unassigned".into()];
    filter.criteria.epics = vec![String::new()];
    filter.criteria.releases = vec![String::new()];
    let (sender, _) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    tree.apply_saved_filter(&filter);
    let text = rendered_lines(&render(&mut tree), AREA).join("\n");
    assert!(text.contains("FIN-8"), "{text}");
    for excluded in ["FIN-9", "FIN-10", "FIN-11"] {
        assert!(!text.contains(excluded), "{text}");
    }
}

#[test]
fn epic_dropdown_has_a_muted_missing_checkbox_and_commits_it_with_a_real_value() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items[0].epic_name = Some("Checkout".into());
    let (sender, receiver) = mpsc::channel();
    let mut tree = backlog_tree(&snapshot, sender, Default::default());
    let mut layout = LayoutCtx::new();
    layout.with_overlay_bounds(AREA, |ctx| tree.layout(AREA, ctx));
    let epic = layout
        .focus_targets()
        .iter()
        .find(|target| target.path == TreePath::from_keys([ChildKey::new("epics")]))
        .unwrap();
    tree.dispatch_focus(epic, true, &mut FocusCtx::default());
    let route = EventRoute::new(epic.path.clone());
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        &mut EventCtx::default(),
    );
    tree.dispatch_event(
        &route,
        &TuiEvent::Key(ctrl(Key::Char('j'))),
        &mut EventCtx::default(),
    );
    let terminal = render(&mut tree);
    let lines = rendered_lines(&terminal, AREA);
    let missing_y = lines
        .iter()
        .position(|line| line.contains("No epic"))
        .unwrap();
    let value_y = lines
        .iter()
        .position(|line| line.contains("Checkout"))
        .unwrap();
    assert_eq!(value_y, missing_y + 1);
    let x = cell_position(&lines[missing_y], "No epic").unwrap() as u16;
    assert_eq!(
        terminal.backend().buffer()[(x, missing_y as u16)].fg,
        tuicore::theme().muted_fg()
    );
    for key in [
        ctrl(Key::Char('k')),
        KeyEvent::from(Key::Enter),
        ctrl(Key::Char('j')),
        KeyEvent::from(Key::Enter),
        ctrl(Key::Enter),
    ] {
        tree.dispatch_event(&route, &TuiEvent::Key(key), &mut EventCtx::default());
    }
    assert!(matches!(
        receiver.try_recv(),
        Ok(super::super::components::BacklogSectionEvent::EpicsChanged(values))
            if values == ["", "Checkout"]
    ));
}

#[test]
fn saved_filter_editor_adds_and_removes_missing_values() {
    tuicore::init();
    let filter = SavedBacklogFilter::new(1, "Triage");
    let (sender, receiver) = mpsc::channel();
    let options = BacklogFilterOptions {
        epics: vec!["Checkout".into()],
        ..Default::default()
    };
    let mut manager = saved_filter_dialog(
        &[filter],
        Some(1),
        &options,
        sender,
        Rc::new(Cell::new(false)),
        None,
    );
    let mut layout = LayoutCtx::new();
    manager.layout(AREA, &mut layout);
    let epics = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && target.path.keys().iter().any(|key| key.as_str() == "epics")
        })
        .unwrap()
        .clone();
    manager.dispatch_event(
        &EventRoute::new(epics.path.clone()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('+'))),
        &mut EventCtx::default(),
    );
    let mut layout = LayoutCtx::new();
    manager.layout(AREA, &mut layout);
    let input = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "input"
                && target.path.keys().iter().any(|key| key.as_str() == "epics")
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "add-input")
        })
        .unwrap();
    let route = EventRoute::new(input.path.clone());
    manager.dispatch_focus(input, true, &mut FocusCtx::default());
    let terminal = render(&mut manager);
    let lines = rendered_lines(&terminal, AREA);
    let y = lines
        .iter()
        .position(|line| line.contains("No epic"))
        .unwrap();
    let x = cell_position(&lines[y], "No epic").unwrap() as u16;
    assert_eq!(
        terminal.backend().buffer()[(x, y as u16)].fg,
        tuicore::theme().muted_fg()
    );
    for key in [ctrl(Key::Char('j')), KeyEvent::from(Key::Enter)] {
        manager.dispatch_event(&route, &TuiEvent::Key(key), &mut EventCtx::default());
    }
    assert_eq!(
        receiver.try_recv(),
        Ok(SavedFilterManagerEvent::SetValues {
            id: 1,
            field: SavedFilterField::Epics,
            values: vec![String::new()],
        })
    );
    let text = rendered_lines(&render(&mut manager), AREA).join("\n");
    assert!(text.contains("No epic"));
    assert!(!text.contains("unavailable"));
    manager.dispatch_event(
        &EventRoute::new(epics.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('-'))),
        &mut EventCtx::default(),
    );
    assert_eq!(
        receiver.try_recv(),
        Ok(SavedFilterManagerEvent::SetValues {
            id: 1,
            field: SavedFilterField::Epics,
            values: Vec::new(),
        })
    );
}
