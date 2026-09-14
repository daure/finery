use super::*;
use crate::store::composer::{ComposerAction, SubmissionAttempt, SubmissionAttemptPhase};

fn archive(state: &mut ComposerState) -> Result<(), PlacementError> {
    state.dispatch(ComposerAction::ArchiveChangeSet {
        id: "CS-1".into(),
        outcome: ArchiveOutcome::Concluded,
    })
}

#[test]
fn archived_sets_lock_edits_selection_and_submission() {
    let mut state = ComposerState::demo();
    state
        .dispatch(ComposerAction::OpenChangeSet("CS-1".into()))
        .unwrap();
    archive(&mut state).unwrap();
    let archived = state.active_set().unwrap().clone();
    assert!(archived.closed_at.is_some());
    assert!(!state.selected_is_editable());
    assert!(!state.selected_can_add_attachment());
    assert!(!state.remote_queries_allowed());
    assert!(!state.begin_submission("CS-1"));
    assert!(state.submission_plan(&["FIN-142".into()]).is_err());
    for action in [
        ComposerAction::UpdateTitle("New title".into()),
        ComposerAction::SetSelectedTickets(vec!["FIN-142".into()]),
        ComposerAction::RemoveTicket("FIN-142".into()),
        ComposerAction::AddAttachment {
            filename: "notes.txt".into(),
            mime_type: None,
            data: vec![1],
        },
        ComposerAction::ClaimSubmission {
            change_set_id: "CS-1".into(),
            ids: vec!["FIN-142".into()],
            owner_id: "owner".into(),
        },
    ] {
        assert_eq!(state.dispatch(action), Err(PlacementError::ClosedChangeSet));
    }
    state
        .dispatch(ComposerAction::RenameChangeSetById {
            id: "CS-1".into(),
            name: "New name".into(),
        })
        .unwrap();
    assert_eq!(state.active_set().unwrap().name, "New name");
    let renamed = state.active_set().unwrap().clone();
    assert!(archive(&mut state).is_err());
    assert_eq!(state.active_set(), Some(&renamed));
}

#[test]
fn submission_recovery_and_in_flight_work_block_archiving() {
    for condition in 0..4 {
        let mut state = ComposerState::demo();
        match condition {
            0 => {
                assert!(state.begin_submission("CS-1"));
            }
            1 => {
                state.change_sets[0].submission_attempt = Some(SubmissionAttempt {
                    owner_id: "owner".into(),
                    ticket_ids: vec!["FIN-142".into()],
                    phase: SubmissionAttemptPhase::Claimed,
                })
            }
            2 => state.change_sets[0].tickets[0].create_attempt = true,
            _ => state.change_sets[0].tickets[0].retry_blocked = true,
        }
        let before = state.change_sets.clone();
        assert!(archive(&mut state).is_err());
        assert_eq!(state.change_sets, before);
    }
}

#[test]
fn empty_sets_can_be_archived_and_legacy_sets_have_no_manual_outcome() {
    let mut state = ComposerState::demo();
    state.change_sets[0].tickets.clear();
    let mut legacy = serde_json::to_value(&state.change_sets[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("archive_outcome");
    legacy.as_object_mut().unwrap().remove("closed_at");
    let restored: ChangeSet = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.archive_outcome, None);
    assert_eq!(restored.closed_at, None);
    archive(&mut state).unwrap();
    assert!(state.change_sets[0].closed);
    assert!(state.change_sets[0].closed_at.is_some());
    assert_eq!(
        state.change_sets[0].archive_outcome,
        Some(ArchiveOutcome::Concluded)
    );
}

#[test]
fn final_submission_records_closure_time_and_replay_preserves_it() {
    use crate::store::composer::SubmissionSnapshot;

    let mut state = ComposerState::demo();
    let tickets = state.change_sets[0].tickets.clone();
    let actions = tickets
        .into_iter()
        .map(|change| ComposerAction::CompleteSubmission {
            change_set_id: "CS-1".into(),
            id: change.id,
            snapshot: SubmissionSnapshot {
                original: change.original,
                updated: change.updated,
                warnings: Vec::new(),
            },
        })
        .collect::<Vec<_>>();
    for action in &actions[..actions.len() - 1] {
        state.dispatch(action.clone()).unwrap();
        assert!(!state.change_sets[0].closed);
        assert_eq!(state.change_sets[0].closed_at, None);
    }
    let before = chrono::Utc::now();
    let final_action = actions.last().unwrap().clone();
    state.dispatch(final_action.clone()).unwrap();
    let recorded = state.change_sets[0].closed_at.unwrap();
    assert!(state.change_sets[0].closed);
    assert!(recorded >= before && recorded <= chrono::Utc::now());
    let fixed_time = "2026-09-14T10:00:00Z".parse().unwrap();
    state.change_sets[0].closed_at = Some(fixed_time);
    state.dispatch(final_action).unwrap();
    assert_eq!(state.change_sets[0].closed_at, Some(fixed_time));
}
