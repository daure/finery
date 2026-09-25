use super::*;

#[test]
fn bulk_expansion_keeps_subtasks_collapsed_in_every_backlog_grouping() {
    tuicore::init();
    let mut snapshot = snapshot();
    snapshot.work_items[0].title = "Plan next sprint Zebra".into();
    snapshot.sprints[0].work_items.push(WorkItem {
        kind: "Sub-task".into(),
        parent_key: Some("FIN-7".into()),
        ..work_item("FIN-10", "Sprint child work")
    });
    snapshot.work_items.push(WorkItem {
        kind: "SUBTASK".into(),
        parent_key: Some("FIN-8".into()),
        ..work_item("FIN-11", "Backlog child work")
    });

    let groupings: [Option<fn(&mut BacklogTree)>; 4] = [
        None,
        Some(BacklogTree::group_by_user_for_test),
        Some(BacklogTree::group_by_epic_for_test),
        Some(BacklogTree::group_by_release_for_test),
    ];
    let route = EventRoute::new(TreePath::from_keys([ChildKey::new("data")]));
    for grouping in groupings {
        let (sender, _) = mpsc::channel();
        let mut tree = backlog_tree(&snapshot, sender, Default::default());
        if let Some(grouping) = grouping {
            grouping(&mut tree);
        }
        let area = Rect::new(0, 0, 160, 30);
        tree.layout(area, &mut LayoutCtx::new());
        tree.dispatch_focus(
            &data_focus_target(),
            true,
            &mut FocusCtx::new(AnimationSettings::default()),
        );
        let mut ctx = EventCtx::new(AnimationSettings::default());
        let toggle = TuiEvent::Key(KeyEvent::from(Key::Char('z')));
        assert_eq!(
            tree.dispatch_event(&route, &toggle, &mut ctx),
            EventOutcome::Handled
        );

        let expanded = tree.expansion_snapshot_for_test();
        assert!(!expanded.is_empty());
        assert!(expanded.iter().all(|id| !id.starts_with("ticket:")));
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        let text = render_tree(&mut tree, &mut terminal, area);
        assert!(text.contains("Ship sprint work"));
        assert!(text.contains("Plan next sprint"));
        assert!(!text.contains("Sprint child work"));
        assert!(!text.contains("Backlog child work"));

        tree.clear_selection_and_highlight_ticket("FIN-8");
        let parent = tree.highlighted_id_for_test().unwrap();
        tree.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Right)), &mut ctx);
        assert!(tree.expansion_snapshot_for_test().contains(&parent));
        assert!(render_tree(&mut tree, &mut terminal, area).contains("Backlog child work"));
        tree.clear_selection_and_highlight_ticket("FIN-11");

        tree.dispatch_event(&route, &toggle, &mut ctx);
        assert!(tree.expansion_snapshot_for_test().is_empty());
        assert!(expanded.contains(&tree.highlighted_id_for_test().unwrap()));
        let text = render_tree(&mut tree, &mut terminal, area);
        assert!(!text.contains("Plan next sprint"));
        assert!(!text.contains("Backlog child work"));

        tree.dispatch_event(&route, &toggle, &mut ctx);
        assert_eq!(tree.expansion_snapshot_for_test(), expanded);
        let text = render_tree(&mut tree, &mut terminal, area);
        assert!(text.contains("Plan next sprint"));
        assert!(!text.contains("Backlog child work"));

        tree.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('/'))),
            &mut ctx,
        );
        tree.dispatch_event(&route, &toggle, &mut ctx);
        let text = render_tree(&mut tree, &mut terminal, area);
        assert!(text.contains("Plan next sprint Zebra"));
        assert!(!text.contains("Ship sprint work"));
    }
}

fn render_tree(tree: &mut BacklogTree, terminal: &mut Terminal<TestBackend>, area: Rect) -> String {
    tree.layout(area, &mut LayoutCtx::new());
    terminal
        .draw(|frame| {
            let mut render = RenderCtx::new();
            tree.render(frame, area, &mut render);
            render.flush(frame);
        })
        .unwrap();
    rendered_lines(terminal, area).concat()
}
