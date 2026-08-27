use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};

use crate::{
    jira::{SubmitBatchOutcome, TicketSubmitOutcome},
    service::AppService,
    store::composer::{ComposerAction, ComposerState, TicketChange},
};

enum SubmissionPhase {
    Idle,
    AwaitingCreateMarkers {
        change_set_id: String,
        changes: Vec<TicketChange>,
        create_attempts: Vec<String>,
        response: Receiver<Result<(), String>>,
    },
    AwaitingJira {
        request_id: u64,
        change_set_id: String,
        create_attempts: Vec<String>,
    },
    AwaitingReconciliation {
        change_set_id: String,
        restore_create_attempts: Vec<String>,
        submitted: Vec<(String, String)>,
        response: Receiver<Result<(), String>>,
    },
}

pub(super) struct SubmissionController {
    state: Rc<RefCell<ComposerState>>,
    service: AppService,
    sender: Sender<(u64, String, Result<SubmitBatchOutcome, String>)>,
    receiver: Receiver<(u64, String, Result<SubmitBatchOutcome, String>)>,
    request_id: u64,
    phase: SubmissionPhase,
}

impl SubmissionController {
    pub(super) fn new(state: Rc<RefCell<ComposerState>>, service: AppService) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state,
            service,
            sender,
            receiver,
            request_id: 0,
            phase: SubmissionPhase::Idle,
        }
    }

    pub(super) fn is_submitting(&self) -> bool {
        !matches!(self.phase, SubmissionPhase::Idle)
    }

    pub(super) fn start(&mut self, changes: Vec<TicketChange>, ctx: &mut tuicore::EventCtx<()>) {
        if self.is_submitting() || changes.is_empty() {
            return;
        }
        let Some(change_set_id) = self.state.borrow().active_change_set.clone() else {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    "Open a change set before committing",
                ));
            return;
        };
        if !self.state.borrow_mut().begin_submission(&change_set_id) {
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    "The change set is unavailable",
                ));
            return;
        }
        let create_attempts = changes
            .iter()
            .filter(|change| change.kind == crate::store::composer::ChangeKind::Added)
            .map(|change| change.id.clone())
            .collect::<Vec<_>>();
        if create_attempts.is_empty() {
            self.start_jira_submission(change_set_id, changes, create_attempts);
        } else {
            self.state
                .borrow_mut()
                .dispatch(ComposerAction::MarkCreateAttempts {
                    change_set_id: change_set_id.clone(),
                    ids: create_attempts.clone(),
                })
                .expect("submission target must exist");
            let set = self.change_set(&change_set_id);
            match self.service.save_change_set_durably(set) {
                Ok(response) => {
                    self.phase = SubmissionPhase::AwaitingCreateMarkers {
                        change_set_id,
                        changes,
                        create_attempts,
                        response,
                    };
                }
                Err(error) => {
                    self.finish_submission(&change_set_id);
                    self.report_marker_persistence_failure(&error);
                }
            }
        }
        if self.is_submitting() {
            ctx.request_tick();
        }
    }

    pub(super) fn drain_results(&mut self) -> bool {
        match &self.phase {
            SubmissionPhase::Idle => false,
            SubmissionPhase::AwaitingCreateMarkers { response, .. } => {
                let Some(result) = take_persistence_result(response) else {
                    return false;
                };
                let SubmissionPhase::AwaitingCreateMarkers {
                    change_set_id,
                    changes,
                    create_attempts,
                    ..
                } = std::mem::replace(&mut self.phase, SubmissionPhase::Idle)
                else {
                    unreachable!();
                };
                match result {
                    Ok(()) => self.start_jira_submission(change_set_id, changes, create_attempts),
                    Err(error) => {
                        self.finish_submission(&change_set_id);
                        self.report_marker_persistence_failure(&error);
                    }
                }
                true
            }
            SubmissionPhase::AwaitingJira { .. } => self.drain_jira_result(),
            SubmissionPhase::AwaitingReconciliation { response, .. } => {
                let Some(result) = take_persistence_result(response) else {
                    return false;
                };
                let SubmissionPhase::AwaitingReconciliation {
                    change_set_id,
                    restore_create_attempts,
                    submitted,
                    ..
                } = std::mem::replace(&mut self.phase, SubmissionPhase::Idle)
                else {
                    unreachable!();
                };
                self.finish_submission(&change_set_id);
                self.report_submitted(&submitted);
                if let Err(error) = result {
                    self.restore_create_attempts(&change_set_id, &restore_create_attempts);
                    self.service
                        .report_notification(tuicore::Notification::error(
                            "Commit persistence failed",
                            format!(
                                "Jira returned a result, but Finery could not save its reconciliation state: {error}"
                            ),
                        ));
                }
                true
            }
        }
    }

    fn start_jira_submission(
        &mut self,
        change_set_id: String,
        changes: Vec<TicketChange>,
        create_attempts: Vec<String>,
    ) {
        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        self.phase = SubmissionPhase::AwaitingJira {
            request_id,
            change_set_id: change_set_id.clone(),
            create_attempts: create_attempts.clone(),
        };
        let service = self.service.clone();
        let sender = self.sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("finery-jira-commit".into())
            .spawn(move || {
                let _ = sender.send((
                    request_id,
                    change_set_id,
                    Ok(service.submit_ticket_changes(&changes)),
                ));
            })
        {
            let change_set_id = match std::mem::replace(&mut self.phase, SubmissionPhase::Idle) {
                SubmissionPhase::AwaitingJira { change_set_id, .. } => change_set_id,
                _ => unreachable!(),
            };
            self.clear_create_attempts(&change_set_id, &create_attempts);
            self.enqueue_reconciliation(change_set_id, create_attempts, Vec::new());
            self.service
                .report_notification(tuicore::Notification::error(
                    "Commit failed",
                    format!("Could not start Jira commit: {error}"),
                ));
        }
    }

    fn drain_jira_result(&mut self) -> bool {
        let Ok((request_id, change_set_id, result)) = self.receiver.try_recv() else {
            return false;
        };
        let SubmissionPhase::AwaitingJira {
            request_id: active_request_id,
            change_set_id: active_change_set_id,
            create_attempts,
        } = std::mem::replace(&mut self.phase, SubmissionPhase::Idle)
        else {
            unreachable!();
        };
        if (active_request_id, active_change_set_id.as_str())
            != (request_id, change_set_id.as_str())
        {
            self.phase = SubmissionPhase::AwaitingJira {
                request_id: active_request_id,
                change_set_id: active_change_set_id,
                create_attempts,
            };
            return false;
        }
        match result {
            Err(error) => {
                self.clear_create_attempts(&change_set_id, &create_attempts);
                self.enqueue_reconciliation(change_set_id, create_attempts, Vec::new());
                self.service
                    .report_notification(tuicore::Notification::error("Commit failed", error));
            }
            Ok(SubmitBatchOutcome::PreflightError(error)) => {
                self.clear_create_attempts(&change_set_id, &create_attempts);
                self.enqueue_reconciliation(change_set_id, create_attempts, Vec::new());
                self.service
                    .report_notification(tuicore::Notification::error("Commit failed", error));
            }
            Ok(SubmitBatchOutcome::Conflict(keys)) => {
                self.clear_create_attempts(&change_set_id, &create_attempts);
                self.enqueue_reconciliation(change_set_id, create_attempts, Vec::new());
                self.service.report_notification(tuicore::Notification::warning(
                    "Commit cancelled",
                    format!(
                        "Jira changed since this change set was composed: {}. Refresh those tickets before retrying.",
                        keys.join(", ")
                    ),
                ));
            }
            Ok(SubmitBatchOutcome::Completed(outcomes)) => {
                let (submitted, restore_create_attempts) =
                    self.apply_outcomes(&change_set_id, outcomes, &create_attempts);
                self.enqueue_reconciliation(change_set_id, restore_create_attempts, submitted);
            }
        }
        true
    }

    fn apply_outcomes(
        &mut self,
        change_set_id: &str,
        outcomes: Vec<TicketSubmitOutcome>,
        create_attempts: &[String],
    ) -> (Vec<(String, String)>, Vec<String>) {
        let mut submitted = Vec::new();
        let mut restore_create_attempts = Vec::new();
        for outcome in outcomes {
            if !self.state.borrow().has_change(change_set_id, &outcome.id) {
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Commit result lost",
                        format!(
                            "Jira returned a result for {} but its change-set target is unavailable",
                            outcome.id
                        ),
                    ));
                continue;
            }
            match outcome.result {
                Ok(snapshot) => {
                    if let Some(ticket) = snapshot.updated.as_ref().or(snapshot.original.as_ref()) {
                        submitted.push((ticket.key.clone(), ticket.title.clone()));
                    }
                    self.state
                        .borrow_mut()
                        .dispatch(ComposerAction::CompleteSubmission {
                            change_set_id: change_set_id.into(),
                            id: outcome.id,
                            snapshot,
                        })
                        .expect("checked submission target must exist");
                }
                Err(failure) => {
                    if failure.retry_blocked {
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::BlockTicketRetry {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                            })
                            .expect("checked submission target must exist");
                    } else if failure.refresh.is_none() {
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::ResolveCreateAttempt {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                            })
                            .expect("checked submission target must exist");
                        if create_attempts.contains(&outcome.id) {
                            restore_create_attempts.push(outcome.id.clone());
                        }
                    }
                    if let Some(refresh) = failure.refresh {
                        let (original, updated) = *refresh;
                        self.state
                            .borrow_mut()
                            .dispatch(ComposerAction::RefreshAfterFailedSubmission {
                                change_set_id: change_set_id.into(),
                                id: outcome.id.clone(),
                                original,
                                updated,
                            })
                            .expect("checked submission target must exist");
                    }
                    self.service
                        .report_notification(tuicore::Notification::error(
                            format!("{} failed", outcome.id),
                            failure.message,
                        ));
                }
            }
        }
        (submitted, restore_create_attempts)
    }

    fn clear_create_attempts(&mut self, change_set_id: &str, create_attempts: &[String]) {
        for id in create_attempts {
            self.state
                .borrow_mut()
                .dispatch(ComposerAction::ResolveCreateAttempt {
                    change_set_id: change_set_id.into(),
                    id: id.clone(),
                })
                .expect("submission target must exist");
        }
    }

    fn enqueue_reconciliation(
        &mut self,
        change_set_id: String,
        restore_create_attempts: Vec<String>,
        submitted: Vec<(String, String)>,
    ) {
        match self
            .service
            .save_change_set_durably(self.change_set(&change_set_id))
        {
            Ok(response) => {
                self.phase = SubmissionPhase::AwaitingReconciliation {
                    change_set_id,
                    restore_create_attempts,
                    submitted,
                    response,
                };
            }
            Err(error) => {
                self.restore_create_attempts(&change_set_id, &restore_create_attempts);
                self.finish_submission(&change_set_id);
                self.report_submitted(&submitted);
                self.service
                    .report_notification(tuicore::Notification::error(
                        "Commit persistence failed",
                        format!(
                            "Jira returned a result, but Finery could not save its reconciliation state: {error}"
                        ),
                    ));
            }
        }
    }

    fn change_set(&self, change_set_id: &str) -> crate::store::composer::ChangeSet {
        self.state
            .borrow()
            .change_sets
            .iter()
            .find(|set| set.id == change_set_id)
            .cloned()
            .expect("submission target must exist")
    }

    fn restore_create_attempts(&mut self, change_set_id: &str, create_attempts: &[String]) {
        if create_attempts.is_empty() {
            return;
        }
        self.state
            .borrow_mut()
            .dispatch(ComposerAction::MarkCreateAttempts {
                change_set_id: change_set_id.into(),
                ids: create_attempts.to_vec(),
            })
            .expect("submission target must exist");
    }

    fn finish_submission(&mut self, change_set_id: &str) {
        self.phase = SubmissionPhase::Idle;
        self.state.borrow_mut().end_submission(change_set_id);
    }

    fn report_marker_persistence_failure(&self, error: &str) {
        self.service
            .report_notification(tuicore::Notification::error(
                "Commit marker persistence failed",
                format!("Could not persist the Jira create-attempt marker; Jira was not contacted: {error}"),
            ));
    }

    fn report_submitted(&self, submitted: &[(String, String)]) {
        match submitted {
            [] => {}
            [(key, title)] => self
                .service
                .report_notification(tuicore::Notification::success(
                    "Ticket committed",
                    format!("{key} · {title}"),
                )),
            submitted => self
                .service
                .report_notification(tuicore::Notification::success(
                    "Tickets committed",
                    format!("{} tickets committed", submitted.len()),
                )),
        }
    }
}

fn take_persistence_result(response: &Receiver<Result<(), String>>) -> Option<Result<(), String>> {
    match response.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err(
            "persistence worker stopped before saving Composer state".into(),
        )),
    }
}
