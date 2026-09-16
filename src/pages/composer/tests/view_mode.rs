use super::*;

fn switch_mode(
    page: &mut ComposerPage,
    current: tuicore::FocusTarget,
    hotkey: &str,
    width: u16,
) -> tuicore::FocusTarget {
    let buttons = target_at(page, "button-group", width);
    let mut event = EventCtx::default();
    page.dispatch_event(
        &EventRoute::new(buttons.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(hotkey.into())),
        &mut event,
    );
    let mut layout = LayoutCtx::new();
    page.layout(Rect::new(0, 0, width, 40), &mut layout);
    let mut manager = FocusManager::new();
    manager.apply_request(
        &FocusRequest::TargetAt {
            path: current.path.clone(),
            id: current.id.clone(),
        },
        std::slice::from_ref(&current),
    );
    let request = event.focus_request().expect("hotkey must preserve focus");
    if let Some(transition) = manager.apply_request(request, layout.focus_targets()) {
        if let Some(previous) = transition.previous {
            page.dispatch_focus(&previous, false, &mut FocusCtx::default());
        }
        if let Some(current) = transition.current {
            page.dispatch_focus(&current, true, &mut FocusCtx::default());
        }
    }
    let focused = manager.current().unwrap().clone();
    assert!(layout.focus_targets().iter().any(|target| {
        target.path == focused.path && target.id == focused.id && target.enabled
    }));
    focused
}

#[test]
fn view_mode_hotkeys_keep_unchanged_controls_focused_including_the_selected_mode() {
    tuicore::init();
    for id in ["data-view", "button-group", "input", "tabs"] {
        let mut page = composer_page();
        open_change_set(&mut page, 1);
        let tickets = target(&mut page, "data-view");
        page.dispatch_focus(&tickets, false, &mut FocusCtx::default());
        let mut current = focus(&mut page, id);
        let original = current.clone();
        for hotkey in ["shift+e", "shift+s", "shift+s", "shift+e"] {
            current = switch_mode(&mut page, current, hotkey, TEST_WIDTH);
            assert_eq!(current.path, original.path, "{id}: {hotkey}");
            assert_eq!(current.id, original.id, "{id}: {hotkey}");
        }
    }
}

#[test]
fn view_mode_hotkeys_follow_description_and_title_between_fields_and_diffs() {
    tuicore::init();
    for width in [TEST_WIDTH, 120] {
        for id in ["textarea", "input"] {
            let mut change_sets = ComposerState::demo().change_sets;
            let change = &mut change_sets[0].tickets[0];
            let mut updated = change.original.clone().unwrap();
            updated.title = "Changed title".into();
            updated.description = "Changed description".into();
            change.updated = Some(updated);
            change.kind = ChangeKind::Modified;
            let mut page = composer_page_with_change_sets(change_sets);
            open_change_set(&mut page, 1);
            let tickets = target_at(&mut page, "data-view", width);
            page.dispatch_focus(&tickets, false, &mut FocusCtx::default());
            let mut current = target_at(&mut page, id, width);
            page.dispatch_focus(&current, true, &mut FocusCtx::default());
            let field_path = current.path.clone();

            for (hotkey, expected_id) in [
                ("shift+s", id),
                ("shift+f", "diff-viewer"),
                ("shift+f", "diff-viewer"),
                ("shift+e", id),
                ("shift+f", "diff-viewer"),
                ("shift+s", id),
            ] {
                current = switch_mode(&mut page, current, hotkey, width);
                assert_eq!(
                    current.id,
                    FocusId::new(expected_id),
                    "{width}: {id}: {hotkey}"
                );
                let expected_path = if id == "input" && expected_id == "diff-viewer" {
                    field_path.child(ChildKey::body())
                } else {
                    field_path.clone()
                };
                assert_eq!(current.path, expected_path, "{width}: {id}: {hotkey}");
            }
        }
    }
}
