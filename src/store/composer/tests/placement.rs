use super::super::{
    ChangeKind, ChangeSet, ComposerAction, ComposerState, PlacementError, PlacementTarget, Ticket,
    TicketChange, TicketKind,
};

fn ticket(id: &str, kind: TicketKind) -> Ticket {
    Ticket {
        key: id.into(),
        project_key: "FIN".into(),
        title: id.into(),
        description: String::new(),
        description_safe_to_overwrite: true,
        description_overwrite_warning: None,
        kind,
        status: "To Do".into(),
        priority: "Medium".into(),
        assignee: "Unassigned".into(),
        assignee_account_id: String::new(),
        story_points: None,
        fix_versions: Vec::new(),
        labels: Vec::new(),
        parent_key: None,
        parent_title: None,
        parent_kind: None,
        has_children: false,
        attachments: Vec::new(),
        mermaid_diagrams: Vec::new(),
        web_links: Vec::new(),
    }
}

fn state() -> ComposerState {
    let mut state = ComposerState::from_change_sets(Vec::new());
    state.dispatch(ComposerAction::CreateChangeSet {
        id: "CS-1".into(),
        name: "Placement".into(),
    });
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state
}

#[test]
fn placement_builds_ordered_forest_and_keeps_external_parent_key() {
    let mut state = state();
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-1", TicketKind::Epic),
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-2", TicketKind::Story),
        placement: PlacementTarget::ChildOf("FIN-1".into()),
    });
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-3", TicketKind::Subtask),
        placement: PlacementTarget::ChildOf("FIN-2".into()),
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Independent task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Task,
        placement: PlacementTarget::Root,
    });

    assert_eq!(
        state
            .ordered_changes()
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["FIN-1", "FIN-2", "FIN-3", "NEW-1"]
    );
    let subtask = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-3")
        .unwrap();
    assert_eq!(
        subtask.updated.as_ref().unwrap().parent_key.as_deref(),
        Some("FIN-2")
    );
    assert_eq!(subtask.sibling_order, 0);
}

#[test]
fn opening_a_change_set_selects_the_first_visible_ticket() {
    let mut child = ticket("FIN-2", TicketKind::Subtask);
    child.parent_key = Some("FIN-1".into());
    child.parent_kind = Some(TicketKind::Story);
    let parent = ticket("FIN-1", TicketKind::Story);
    let mut state = ComposerState::from_change_sets(vec![ChangeSet {
        id: "CS-1".into(),
        name: "Placement".into(),
        tickets: vec![
            TicketChange {
                id: child.key.clone(),
                original: Some(child),
                updated: None,
                kind: ChangeKind::Synced,
                submitted: None,
                retry_blocked: false,
                create_attempt: false,
                sibling_order: 0,
            },
            TicketChange {
                id: parent.key.clone(),
                original: Some(parent),
                updated: None,
                kind: ChangeKind::Synced,
                submitted: None,
                retry_blocked: false,
                create_attempt: false,
                sibling_order: 0,
            },
        ],
        selected_ticket_ids: Vec::new(),
        closed: false,
        submission_attempt: None,
    }]);

    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));

    assert_eq!(state.selected_ticket.as_deref(), Some("FIN-1"));
}

#[test]
fn invalid_placement_and_kind_changes_leave_tree_unchanged() {
    let mut state = state();
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-1", TicketKind::Epic),
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-2", TicketKind::Story),
        placement: PlacementTarget::ChildOf("FIN-1".into()),
    });

    assert_eq!(
        state.validate_placement("FIN-1", &PlacementTarget::ChildOf("FIN-1".into())),
        Err(PlacementError::Cycle)
    );
    assert_eq!(
        state.dispatch(ComposerAction::ReparentTicket {
            id: "FIN-1".into(),
            placement: PlacementTarget::ChildOf("FIN-2".into()),
        }),
        Err(PlacementError::Cycle)
    );
    state.dispatch(ComposerAction::SelectTicket(Some("FIN-1".into())));
    state.dispatch(ComposerAction::UpdateKind(TicketKind::Task));

    let epic = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-1")
        .unwrap();
    assert_eq!(
        state.changes_for_change(epic).unwrap().kind,
        TicketKind::Epic
    );
    assert_eq!(state.changes_for_change(epic).unwrap().parent_key, None);
    assert_eq!(
        state
            .parent_candidates(TicketKind::Subtask)
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["FIN-2"]
    );
}

#[test]
fn sibling_order_defaults_for_legacy_ticket_changes() {
    let change: TicketChange =
        serde_json::from_str(r#"{"id":"FIN-1","original":null,"updated":null,"kind":"Synced"}"#)
            .unwrap();

    assert_eq!(change.kind, ChangeKind::Synced);
    assert_eq!(change.sibling_order, 0);
}

#[test]
fn root_include_persists_remote_parent_and_rejects_unknown_external_move() {
    let mut state = state();
    let mut remote = ticket("FIN-2", TicketKind::Subtask);
    remote.parent_key = Some("FIN-1".into());
    remote.parent_kind = Some(TicketKind::Story);

    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: remote.clone(),
        placement: PlacementTarget::Root,
    });
    let included = state.selected_change().unwrap();
    assert_eq!(state.source_for_change(included), Some(&remote));
    assert!(included.updated.is_none());

    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: remote.clone(),
        placement: PlacementTarget::ChildOf("FIN-9".into()),
    });
    let moved = state.selected_change().unwrap();
    assert_eq!(state.source_for_change(moved), Some(&remote));
    assert!(moved.updated.is_none());
}

#[test]
fn including_under_a_different_parent_preserves_remote_parent_as_original() {
    let mut state = state();
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-1", TicketKind::Epic),
        placement: PlacementTarget::Root,
    });
    let mut remote = ticket("FIN-2", TicketKind::Story);
    remote.parent_key = Some("FIN-9".into());
    remote.parent_kind = Some(TicketKind::Epic);

    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: remote.clone(),
        placement: PlacementTarget::ChildOf("FIN-1".into()),
    });

    let change = state.selected_change().unwrap();
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(change.original.as_ref(), Some(&remote));
    assert_eq!(
        change.updated.as_ref().unwrap().parent_key.as_deref(),
        Some("FIN-1")
    );
}

#[test]
fn refreshing_parent_relation_reorders_active_tree_without_overwriting_local_move() {
    let mut state = state();
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-1", TicketKind::Story),
        placement: PlacementTarget::Root,
    });
    let mut child = ticket("FIN-2", TicketKind::Subtask);
    child.parent_key = Some("FIN-1".into());
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: child,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-9", TicketKind::Story),
        placement: PlacementTarget::Root,
    });
    assert_eq!(
        state
            .ordered_changes()
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["FIN-1", "FIN-2", "FIN-9"]
    );
    state.dispatch(ComposerAction::ReparentTicket {
        id: "FIN-2".into(),
        placement: PlacementTarget::ChildOf("FIN-9".into()),
    });
    let mut refreshed = ticket("FIN-2", TicketKind::Subtask);
    refreshed.parent_key = Some("FIN-1".into());
    state.dispatch(ComposerAction::SetSource {
        change_set_id: "CS-1".into(),
        id: "FIN-2".into(),
        ticket: refreshed,
    });

    let child = state
        .active_set()
        .unwrap()
        .tickets
        .iter()
        .find(|change| change.id == "FIN-2")
        .unwrap();
    assert_eq!(
        state
            .changes_for_change(child)
            .unwrap()
            .parent_key
            .as_deref(),
        Some("FIN-9")
    );
    assert_eq!(
        state
            .ordered_changes()
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["FIN-1", "FIN-9", "FIN-2"]
    );
}

#[test]
fn committing_selected_local_child_includes_unsent_local_ancestor() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent story".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child sub-task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });

    let changes = state.commit_changes(&["NEW-2".into()]).unwrap();

    assert_eq!(
        changes
            .iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW-1", "NEW-2"]
    );
}

#[test]
fn submitted_local_parent_keeps_unsent_child_attached_by_resolved_key() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent story".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child sub-task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });
    state.dispatch(ComposerAction::CompleteSubmission {
        change_set_id: "CS-1".into(),
        id: "NEW-1".into(),
        snapshot: super::super::SubmissionSnapshot {
            original: None,
            updated: Some(ticket("FIN-101", TicketKind::Story)),
        },
    });

    assert_eq!(
        state
            .ordered_changes()
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW-1", "NEW-2"]
    );
    assert_eq!(
        state.commit_changes(&["NEW-2".into()]).unwrap()[0].id,
        "NEW-2"
    );
    assert_eq!(
        state
            .parent_candidates(TicketKind::Subtask)
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW-1"]
    );
}

#[test]
fn new_child_uses_committed_parent_key_instead_of_local_alias() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent story".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::IncludeTicketAt {
        ticket: ticket("FIN-9", TicketKind::Task),
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CompleteSubmission {
        change_set_id: "CS-1".into(),
        id: "NEW-1".into(),
        snapshot: super::super::SubmissionSnapshot {
            original: None,
            updated: Some(ticket("FIN-101", TicketKind::Story)),
        },
    });

    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child sub-task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });

    assert_eq!(
        state.selected_changes().unwrap().parent_key.as_deref(),
        Some("FIN-101")
    );
    assert!(state.commit_changes(&["NEW-2".into()]).is_ok());
}

#[test]
fn unknown_external_parent_cannot_accept_new_children() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Unknown parent child".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("EXT-1".into()),
    });

    assert!(state.active_set().unwrap().tickets.is_empty());
}

#[test]
fn removing_parent_removes_local_subtree_and_repairs_selection() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent story".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child sub-task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });
    state.dispatch(ComposerAction::CreateTicket {
        title: "Remaining task".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::RemoveTicket("NEW-1".into()));

    assert_eq!(
        state
            .active_set()
            .unwrap()
            .tickets
            .iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["NEW-3"]
    );
    assert_eq!(state.selected_ticket.as_deref(), Some("NEW-3"));
}

#[test]
fn submitted_descendant_blocks_local_subtree_removal() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Parent story".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Story,
        placement: PlacementTarget::Root,
    });
    state.dispatch(ComposerAction::CreateTicketAt {
        title: "Child sub-task".into(),
        project_key: "FIN".into(),
        kind: TicketKind::Subtask,
        placement: PlacementTarget::ChildOf("NEW-1".into()),
    });
    let mut submitted_child = ticket("FIN-202", TicketKind::Subtask);
    submitted_child.parent_key = Some("NEW-1".into());
    submitted_child.parent_kind = Some(TicketKind::Story);
    state.dispatch(ComposerAction::CompleteSubmission {
        change_set_id: "CS-1".into(),
        id: "NEW-2".into(),
        snapshot: super::super::SubmissionSnapshot {
            original: None,
            updated: Some(submitted_child),
        },
    });

    let error = state.removal_preview("NEW-1").unwrap_err();
    state.dispatch(ComposerAction::RemoveTicket("NEW-1".into()));

    assert!(error.contains("NEW-2 was already submitted"));
    assert_eq!(state.active_set().unwrap().tickets.len(), 2);
}

#[test]
fn removing_an_unselected_ticket_preserves_the_current_selection() {
    let mut state = state();
    state.dispatch(ComposerAction::CreateTicket {
        title: "First task".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::CreateTicket {
        title: "Second task".into(),
        project_key: "FIN".into(),
    });
    state.dispatch(ComposerAction::SelectTicket(Some("NEW-2".into())));

    state.dispatch(ComposerAction::RemoveTicket("NEW-1".into()));

    assert_eq!(state.selected_ticket.as_deref(), Some("NEW-2"));
}

#[test]
fn tasks_allow_subtask_children() {
    let mut state = ComposerState::from_change_sets(vec![ChangeSet {
        id: "CS-1".into(),
        name: "Task subtasks".into(),
        closed: false,
        tickets: vec![TicketChange {
            id: "TASK-1".into(),
            original: Some(Ticket {
                key: "TASK-1".into(),
                project_key: "FIN".into(),
                title: "Parent task".into(),
                description: String::new(),
                description_safe_to_overwrite: true,
                description_overwrite_warning: None,
                kind: TicketKind::Task,
                status: "To Do".into(),
                priority: "Medium".into(),
                assignee: "Unassigned".into(),
                assignee_account_id: String::new(),
                story_points: None,
                fix_versions: Vec::new(),
                labels: Vec::new(),
                parent_key: None,
                parent_title: None,
                parent_kind: None,
                has_children: false,
                attachments: Vec::new(),
                mermaid_diagrams: Vec::new(),
                web_links: Vec::new(),
            }),
            updated: None,
            kind: ChangeKind::Synced,
            submitted: None,
            retry_blocked: false,
            create_attempt: false,
            sibling_order: 0,
        }],
        selected_ticket_ids: Vec::new(),
        submission_attempt: None,
    }]);
    state.dispatch(ComposerAction::OpenChangeSet("CS-1".into()));
    state.dispatch(ComposerAction::SelectTicket(Some("TASK-1".into())));

    assert_eq!(
        state.legal_child_kinds(Some("TASK-1")),
        vec![TicketKind::Subtask]
    );
    assert_eq!(
        state
            .parent_candidates(TicketKind::Subtask)
            .into_iter()
            .map(|change| change.id.as_str())
            .collect::<Vec<_>>(),
        vec!["TASK-1"]
    );
}
